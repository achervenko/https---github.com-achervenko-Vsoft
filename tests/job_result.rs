use std::{
    collections::HashMap,
    fs,
    path::PathBuf,
    sync::atomic::{AtomicUsize, Ordering},
};

use serde_json::json;
use sha2::{Digest, Sha256};
use vsoft::job_result::{classify, JobResult, RunReceipt};

static NEXT_DIRECTORY: AtomicUsize = AtomicUsize::new(0);
const KEY: &str = "current-job";

struct Package(PathBuf, RunReceipt);

impl Package {
    fn new(report: &[u8], preview: bool) -> Self {
        let path = std::env::temp_dir().join(format!(
            "vsoft-result-{}-{}",
            std::process::id(),
            NEXT_DIRECTORY.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        let mut checksums = HashMap::new();
        for (name, bytes) in [
            ("drawing.dwg", b"test drawing".as_slice()),
            ("report.json", report),
            ("preview.pdf", b"test preview".as_slice()),
        ] {
            if name == "preview.pdf" && !preview {
                continue;
            }
            fs::write(path.join(name), bytes).unwrap();
            checksums.insert(name.to_owned(), format!("{:x}", Sha256::digest(bytes)));
        }
        fs::write(
            path.join("complete.json"),
            json!({
                "key": KEY,
                "sheets": [85, 2],
                "total": 103,
                "sha256": checksums,
                "preview_printed": preview,
            })
            .to_string(),
        )
        .unwrap();
        Self(
            path,
            RunReceipt {
                key: KEY.into(),
                sha256: checksums,
                preview_printed: preview,
            },
        )
    }

    fn result(&self) -> JobResult {
        classify(0, &self.0, &self.1)
    }
}

impl Drop for Package {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

const CLEAN: &[u8] = br#"[{"sheet":85,"review":[]},{"sheet":2,"review":[]}]"#;

#[cfg(windows)]
#[test]
fn structured_conversion_rejects_result_while_python_holds_package_lock() {
    use std::{
        io::{BufRead, BufReader},
        process::{Command, Stdio},
    };

    let package = Package::new(CLEAN, true);
    let work = package.0.join("_work");
    fs::create_dir(&work).unwrap();
    let lock_path = work.join("package.lock");
    fs::write(&lock_path, b"x").unwrap();
    let engine = package.0.join("mock engine.py");
    fs::write(
        &engine,
        r#"import os, shutil, sys
from pathlib import Path
shutil.copyfile(Path(sys.argv[3]) / 'complete.json', os.environ['VSOFT_RESULT_RECEIPT'])
"#,
    )
    .unwrap();

    let python = std::env::var_os("VSOFT_PYTHON").unwrap_or_else(|| "python".into());
    let mut holder = Command::new(&python)
        .args([
            "-c",
            r#"import msvcrt, runpy, sys
from pathlib import Path
from unittest.mock import patch

lock_path = Path(sys.argv[1])
destination = lock_path.parent.parent
existing_file = str(destination / 'drawing.dwg')
sys.argv = ['convert.py', existing_file, existing_file, str(destination), '--acad', sys.executable]
real_lock = msvcrt.locking

def hold_lock(*args):
    real_lock(*args)
    print('locked', flush=True)
    sys.stdin.read()
    raise SystemExit(0)

with patch('pymupdf.open', return_value=[None]), patch('msvcrt.locking', side_effect=hold_lock):
    runpy.run_path('convert.py', run_name='__main__')
"#,
        ])
        .arg(&lock_path)
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .env_remove("VSOFT_PAGE_SELECTOR")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .expect("start Python lock holder");
    let mut ready = String::new();
    BufReader::new(holder.stdout.take().unwrap())
        .read_line(&mut ready)
        .unwrap();
    assert_eq!(ready.trim(), "locked");

    let mut vsoft = Command::new(env!("CARGO_BIN_EXE_vsoft"));
    vsoft
        .args(["convert-result", "source.pdf", "reference.dxf"])
        .arg(&package.0)
        .env("VSOFT_PYTHON", &python)
        .env("VSOFT_ENGINE", &engine);
    let locked = vsoft.output();
    drop(holder.stdin.take());
    assert!(holder.wait().unwrap().success());

    let locked = locked.unwrap();
    assert_eq!(locked.status.code(), Some(1));
    let result: serde_json::Value = serde_json::from_slice(&locked.stdout).unwrap();
    assert_eq!(result["status"], "result_error");
    assert!(result["message"].as_str().unwrap().contains("package.lock"));

    let unlocked = vsoft.output().unwrap();
    assert!(unlocked.status.success(), "{unlocked:?}");
    let result: serde_json::Value = serde_json::from_slice(&unlocked.stdout).unwrap();
    assert_eq!(result["status"], "ready");
}

#[test]
fn process_error_takes_precedence_over_old_results() {
    let package = Package::new(CLEAN, false);
    assert_eq!(
        classify(256, &package.0, &package.1),
        JobResult::Error { exit_code: 256 }
    );
}

#[test]
fn clean_report_returns_dwg_path() {
    let package = Package::new(CLEAN, false);
    assert_eq!(
        package.result(),
        JobResult::Ready {
            dwg: package.0.join("drawing.dwg")
        }
    );
}

#[test]
fn review_result_contains_only_flagged_sheets_and_their_reasons() {
    let package = Package::new(
        br#"[{"sheet":85,"review":[]},{"sheet":2,"review":["dash_review","plot_text_review"]}]"#,
        true,
    );
    assert_eq!(
        serde_json::to_value(package.result()).unwrap(),
        json!({
            "status": "needs_review",
            "dwg": package.0.join("drawing.dwg"),
            "sheets": [{"sheet": 2, "review": ["dash_review", "plot_text_review"]}],
        })
    );
}

#[test]
fn missing_or_changed_artifacts_are_result_errors() {
    let package = Package::new(CLEAN, true);
    fs::remove_file(package.0.join("preview.pdf")).unwrap();
    assert!(
        matches!(package.result(), JobResult::ResultError { message } if message.contains("preview.pdf"))
    );

    let package = Package::new(CLEAN, false);
    fs::remove_file(package.0.join("report.json")).unwrap();
    assert!(
        matches!(package.result(), JobResult::ResultError { message } if message.contains("report.json"))
    );

    let package = Package::new(CLEAN, false);
    fs::write(package.0.join("report.json"), b"[]").unwrap();
    assert!(
        matches!(package.result(), JobResult::ResultError { message } if message.contains("SHA-256 mismatch"))
    );
}

#[test]
fn old_marker_and_invalid_reports_are_result_errors() {
    let mut package = Package::new(CLEAN, false);
    package.1.key = "another-job".into();
    assert!(
        matches!(package.result(), JobResult::ResultError { message } if message.contains("key does not match"))
    );

    for report in [b"not JSON".as_slice(), br#"[{"sheet":2,"review":[]}]"#] {
        let package = Package::new(report, false);
        assert!(
            matches!(package.result(), JobResult::ResultError { message } if message.contains("report.json"))
        );
    }
}

#[test]
fn replacement_package_with_same_key_but_another_report_is_rejected() {
    let current = Package::new(CLEAN, false);
    let replacement = Package::new(
        br#"[{"sheet":85,"review":["dash_review"]},{"sheet":2,"review":[]}]"#,
        false,
    );
    assert!(matches!(
        replacement.result(),
        JobResult::NeedsReview { .. }
    ));
    for name in ["report.json", "complete.json"] {
        fs::copy(replacement.0.join(name), current.0.join(name)).unwrap();
    }
    assert!(
        matches!(current.result(), JobResult::ResultError { message } if message.contains("SHA-256 for report.json does not match"))
    );
}

fn replace_artifact_and_marker_hash(package: &Package, name: &str) {
    let marker_path = package.0.join("complete.json");
    let mut marker: serde_json::Value =
        serde_json::from_slice(&fs::read(&marker_path).unwrap()).unwrap();
    let replacement = b"bytes produced by another run";
    fs::write(package.0.join(name), replacement).unwrap();
    marker["sha256"][name] = json!(format!("{:x}", Sha256::digest(replacement)));
    fs::write(marker_path, marker.to_string()).unwrap();
}

#[test]
fn replacement_dwg_with_matching_marker_and_unchanged_report_is_rejected() {
    let package = Package::new(CLEAN, false);
    replace_artifact_and_marker_hash(&package, "drawing.dwg");
    assert!(
        matches!(package.result(), JobResult::ResultError { message } if message.contains("SHA-256 for drawing.dwg does not match"))
    );
}

#[test]
fn replacement_preview_with_matching_marker_and_unchanged_report_is_rejected() {
    let package = Package::new(CLEAN, true);
    replace_artifact_and_marker_hash(&package, "preview.pdf");
    assert!(
        matches!(package.result(), JobResult::ResultError { message } if message.contains("SHA-256 for preview.pdf does not match"))
    );
}

#[test]
fn full_run_rejects_marker_claiming_preview_was_not_printed() {
    let mut package = Package::new(CLEAN, false);
    package.1.preview_printed = true;
    assert!(
        matches!(package.result(), JobResult::ResultError { message } if message.contains("preview mode does not match"))
    );
}
