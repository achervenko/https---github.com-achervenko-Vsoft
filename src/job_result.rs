use std::{
    collections::{HashMap, HashSet},
    fs::{self, File},
    io,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

#[derive(Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum JobResult {
    Ready {
        dwg: PathBuf,
    },
    NeedsReview {
        dwg: PathBuf,
        sheets: Vec<SheetReview>,
    },
    Error {
        exit_code: i32,
    },
    ResultError {
        message: String,
    },
}

#[derive(Debug, PartialEq, Eq, Deserialize, Serialize)]
pub struct SheetReview {
    pub sheet: usize,
    pub review: Vec<String>,
}

#[derive(Debug, PartialEq, Eq, Deserialize)]
pub struct RunReceipt {
    pub key: String,
    pub sha256: HashMap<String, String>,
    pub preview_printed: bool,
}

#[derive(Deserialize)]
struct Completion {
    key: String,
    sheets: Vec<usize>,
    total: usize,
    sha256: HashMap<String, String>,
    preview_printed: bool,
}

/// Classify a conversion using the receipt produced while that invocation
/// held the destination lock, including on a verified cache hit.
pub fn classify(exit_code: i32, destination: &Path, receipt: &RunReceipt) -> JobResult {
    if exit_code != 0 {
        return JobResult::Error { exit_code };
    }

    match read_result(destination, receipt) {
        Ok(sheets) => {
            let dwg = destination.join("drawing.dwg");
            if sheets.is_empty() {
                JobResult::Ready { dwg }
            } else {
                JobResult::NeedsReview { dwg, sheets }
            }
        }
        Err(message) => JobResult::ResultError { message },
    }
}

fn read_result(destination: &Path, receipt: &RunReceipt) -> Result<Vec<SheetReview>, String> {
    let marker =
        fs::read(destination.join("complete.json")).map_err(|e| format!("complete.json: {e}"))?;
    let marker: Completion =
        serde_json::from_slice(&marker).map_err(|e| format!("complete.json: {e}"))?;
    if marker.key != receipt.key {
        return Err("complete.json: key does not match this conversion".into());
    }
    if marker.preview_printed != receipt.preview_printed {
        return Err("complete.json: preview mode does not match this conversion".into());
    }
    for (name, expected) in &receipt.sha256 {
        if marker.sha256.get(name) != Some(expected) {
            return Err(format!(
                "complete.json: SHA-256 for {name} does not match this conversion"
            ));
        }
    }
    let expected_count = if receipt.preview_printed { 3 } else { 2 };
    if marker.sha256.len() != receipt.sha256.len() || receipt.sha256.len() != expected_count {
        return Err("complete.json: checksum inventory does not match this conversion".into());
    }

    let report =
        fs::read(destination.join("report.json")).map_err(|e| format!("report.json: {e}"))?;
    check_hash(receipt, "report.json", Sha256::digest(&report))?;
    let rows: Vec<SheetReview> =
        serde_json::from_slice(&report).map_err(|e| format!("report.json: {e}"))?;
    let mut seen = HashSet::new();
    if marker.sheets.is_empty()
        || marker
            .sheets
            .iter()
            .any(|&sheet| sheet == 0 || sheet > marker.total || !seen.insert(sheet))
        || !rows
            .iter()
            .map(|row| row.sheet)
            .eq(marker.sheets.iter().copied())
    {
        return Err("report.json: sheet inventory does not match a valid complete.json".into());
    }

    verify_artifact(destination, receipt, "drawing.dwg")?;
    if receipt.preview_printed {
        verify_artifact(destination, receipt, "preview.pdf")?;
    }
    Ok(rows
        .into_iter()
        .filter(|row| !row.review.is_empty())
        .collect())
}

fn verify_artifact(destination: &Path, receipt: &RunReceipt, name: &str) -> Result<(), String> {
    let mut file = File::open(destination.join(name)).map_err(|e| format!("{name}: {e}"))?;
    let mut hasher = Sha256::new();
    io::copy(&mut file, &mut hasher).map_err(|e| format!("{name}: {e}"))?;
    check_hash(receipt, name, hasher.finalize())
}

fn check_hash(
    receipt: &RunReceipt,
    name: &str,
    hash: impl std::fmt::LowerHex,
) -> Result<(), String> {
    match receipt.sha256.get(name) {
        Some(expected) if *expected == format!("{hash:x}") => Ok(()),
        Some(_) => Err(format!("{name}: SHA-256 mismatch")),
        None => Err(format!("run receipt: missing SHA-256 for {name}")),
    }
}
