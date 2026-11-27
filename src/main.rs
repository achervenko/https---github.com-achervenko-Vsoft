use std::{
    env,
    ffi::{OsStr, OsString},
    fs,
    io::{self, Read, Write},
    path::PathBuf,
    process::{Command, ExitCode, Stdio},
    time::{SystemTime, UNIX_EPOCH},
};

use vsoft::{
    input_pages::select_pages,
    job_result::{classify, JobResult, RunReceipt},
    pdf_subset,
    universal_layout::{plan_sheets, SheetInput},
};

mod dashes;
mod dxf_builder;
mod package_lock;
mod pdf_extract;
use package_lock::PackageLock;

const SELECT_PAGES: &str = "_select-pages";
const PLAN_LAYOUT: &str = "_plan-layout";
const USAGE: &str = "Usage: vsoft <convert|convert-result> <pdf> <reference-dxf|none> <destination> [--pages <range>] [--acad <path>] [--skip-preview] [--profile <eplan|universal>] [--compare-layout] [--builder <python|rust>] [--no-restore-dashes]\n\
convert-result writes one JSON result to stdout. Put the three paths before the options.\n\
Run from the repository folder or set VSOFT_ENGINE to the path to convert.py.\n\
Set VSOFT_PYTHON to select the Python executable (default: python).";

fn engine_path() -> Result<PathBuf, String> {
    if let Some(path) = env::var_os("VSOFT_ENGINE") {
        let path = PathBuf::from(path);
        if path.is_file() {
            return Ok(path);
        }
        return Err(format!(
            "VSOFT_ENGINE does not point to a file: {}",
            path.display()
        ));
    }

    if let Ok(directory) = env::current_dir() {
        let script = directory.join("convert.py");
        if script.is_file() {
            return Ok(script);
        }
    }

    let executable = env::current_exe()
        .map_err(|error| format!("Could not locate the vsoft executable: {error}"))?;
    executable
        .ancestors()
        .skip(1)
        .filter(|directory| directory.join("Cargo.toml").is_file())
        .map(|directory| directory.join("convert.py"))
        .find(|path| path.is_file())
        .ok_or_else(|| "convert.py not found. Set VSOFT_ENGINE to its path.".to_owned())
}

fn page_selection(mut args: impl Iterator<Item = OsString>) -> ExitCode {
    let mut next_number = || args.next()?.to_str()?.parse::<usize>().ok();
    let (Some(total), Some(max_digits)) = (next_number(), next_number()) else {
        eprintln!("vsoft: expected a page count and a digit limit");
        return ExitCode::FAILURE;
    };
    if args.next().is_some() {
        eprintln!("vsoft: unexpected page selection arguments");
        return ExitCode::FAILURE;
    }

    let mut selection = String::new();
    if let Err(error) = io::stdin().read_to_string(&mut selection) {
        eprintln!("vsoft: could not read page selection: {error}");
        return ExitCode::FAILURE;
    }
    match select_pages(Some(&selection), total, max_digits) {
        Ok(pages) => {
            let numbers = pages.iter().map(usize::to_string).collect::<Vec<_>>();
            println!("[{}]", numbers.join(","));
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("{error}");
            ExitCode::from(2)
        }
    }
}

fn layout_plan(mut args: impl Iterator<Item = OsString>) -> Result<(), String> {
    if args.next().is_some() {
        return Err("unexpected layout arguments".into());
    }
    let sheets: Vec<SheetInput> = serde_json::from_reader(io::stdin().lock())
        .map_err(|error| format!("could not read sheet dimensions: {error}"))?;
    let plan = plan_sheets(&sheets).map_err(|error| error.to_string())?;
    serde_json::to_writer(io::stdout().lock(), &plan)
        .map_err(|error| format!("could not write layout plan: {error}"))
}

fn conversion_command(args: impl IntoIterator<Item = OsString>) -> Result<Command, String> {
    let script = engine_path()?;
    let python = env::var_os("VSOFT_PYTHON").unwrap_or_else(|| "python".into());
    let executable = env::current_exe()
        .map_err(|error| format!("Could not locate the vsoft executable: {error}"))?;
    let mut command = Command::new(python);
    command
        .arg(script)
        .args(args)
        .env("VSOFT_PAGE_SELECTOR", executable);
    Ok(command)
}

struct ReceiptDirectory(PathBuf);

impl ReceiptDirectory {
    fn create() -> io::Result<Self> {
        let time = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let base = env::temp_dir();
        for attempt in 0..16 {
            let path = base.join(format!(
                "vsoft-result-{}-{time}-{attempt}",
                std::process::id()
            ));
            match fs::create_dir(&path) {
                Ok(()) => return Ok(Self(path)),
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(error) => return Err(error),
            }
        }
        Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            "could not reserve a unique receipt directory",
        ))
    }

    fn receipt_path(&self) -> PathBuf {
        self.0.join("receipt.json")
    }
}

impl Drop for ReceiptDirectory {
    fn drop(&mut self) {
        let _ = fs::remove_file(self.receipt_path());
        let _ = fs::remove_dir(&self.0);
    }
}

fn structured_conversion(args: Vec<OsString>) -> JobResult {
    if args.len() < 3
        || args[..3]
            .iter()
            .any(|path| path.is_empty() || path.to_string_lossy().starts_with('-'))
    {
        eprintln!("vsoft: convert-result requires PDF, reference and destination paths first");
        return JobResult::Error { exit_code: 2 };
    }
    let destination = PathBuf::from(&args[2]);
    let mut command = match conversion_command(args) {
        Ok(command) => command,
        Err(message) => {
            eprintln!("vsoft: {message}");
            return JobResult::Error { exit_code: 1 };
        }
    };
    let scratch = match ReceiptDirectory::create() {
        Ok(directory) => directory,
        Err(error) => {
            return JobResult::ResultError {
                message: format!("Could not create a receipt directory: {error}"),
            };
        }
    };
    let receipt_path = scratch.receipt_path();
    let output = match command
        .env("VSOFT_RESULT_RECEIPT", &receipt_path)
        .stdin(Stdio::inherit())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .output()
    {
        Ok(output) => output,
        Err(error) => {
            eprintln!(
                "vsoft: Could not run {}: {error}",
                command.get_program().to_string_lossy()
            );
            return JobResult::Error { exit_code: 1 };
        }
    };
    let _ = io::stderr().write_all(&output.stdout);
    let exit_code = output.status.code().unwrap_or(1);
    if exit_code != 0 {
        return JobResult::Error { exit_code };
    }

    let receipt = fs::read(&receipt_path)
        .map_err(|error| error.to_string())
        .and_then(|bytes| {
            serde_json::from_slice::<RunReceipt>(&bytes).map_err(|error| error.to_string())
        });
    match receipt {
        Ok(receipt) => match PackageLock::acquire(&destination) {
            Ok(_lock) => classify(exit_code, &destination, &receipt),
            Err(error) => JobResult::ResultError {
                message: format!("Could not lock _work/package.lock to verify the result: {error}"),
            },
        },
        Err(error) => JobResult::ResultError {
            message: format!("Could not read this run's receipt: {error}"),
        },
    }
}

fn write_job_result(result: JobResult) -> ExitCode {
    let mut stdout = io::stdout().lock();
    if serde_json::to_writer(&mut stdout, &result).is_err()
        || writeln!(stdout).is_err()
        || stdout.flush().is_err()
    {
        return ExitCode::FAILURE;
    }
    match result {
        JobResult::Ready { .. } | JobResult::NeedsReview { .. } => ExitCode::SUCCESS,
        JobResult::ResultError { .. } => ExitCode::FAILURE,
        JobResult::Error { exit_code } => std::process::exit(exit_code),
    }
}

fn main() -> ExitCode {
    let mut args = env::args_os().skip(1);
    let Some(action) = args.next() else {
        println!("{USAGE}");
        return ExitCode::SUCCESS;
    };
    if action == "--help" || action == "-h" {
        println!("{USAGE}");
        return ExitCode::SUCCESS;
    }
    if action == SELECT_PAGES {
        return page_selection(args);
    }
    if action == "_prepare-pdf" {
        let args: Vec<_> = args.collect();
        let result = (|| -> Result<(), Box<dyn std::error::Error>> {
            let (input, output, selection) = match args.as_slice() {
                [input, output] => (input, output, None),
                [input, output, pages] => (
                    input,
                    output,
                    Some(pages.to_str().ok_or("invalid UTF-8 page selection")?),
                ),
                _ => return Err("expected input PDF, output PDF and optional pages".into()),
            };
            let result =
                pdf_subset::prepare(&PathBuf::from(input), &PathBuf::from(output), selection)?;
            serde_json::to_writer(io::stdout().lock(), &result)?;
            Ok(())
        })();
        return match result {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) => {
                eprintln!("vsoft: {error}");
                ExitCode::FAILURE
            }
        };
    }
    if action == "_extract-pdf" {
        return match pdf_extract::run(args) {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) => {
                eprintln!("vsoft: PDF extraction failed: {error}");
                ExitCode::FAILURE
            }
        };
    }
    if action == "_build-dxf" {
        return match dxf_builder::run(args) {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) => {
                eprintln!("vsoft: {error}");
                ExitCode::FAILURE
            }
        };
    }
    if action == PLAN_LAYOUT {
        return match layout_plan(args) {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) => {
                eprintln!("vsoft: {error}");
                ExitCode::FAILURE
            }
        };
    }
    if action == "convert-result" {
        let args: Vec<_> = args.collect();
        if args.len() == 1 && (args[0] == "--help" || args[0] == "-h") {
            println!("{USAGE}");
            return ExitCode::SUCCESS;
        }
        return write_job_result(structured_conversion(args));
    }
    if action != OsStr::new("convert") {
        eprintln!("Unknown command: {}\n{USAGE}", action.to_string_lossy());
        return ExitCode::from(2);
    }

    let result = conversion_command(args).and_then(|mut command| {
        command.status().map_err(|err| {
            format!(
                "Could not start {}: {err}",
                command.get_program().to_string_lossy()
            )
        })
    });
    match result {
        Ok(status) => std::process::exit(status.code().unwrap_or(1)),
        Err(message) => {
            eprintln!("vsoft: {message}");
            ExitCode::FAILURE
        }
    }
}
