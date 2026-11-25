use std::{
    env,
    ffi::{OsStr, OsString},
    io::{self, Read},
    path::PathBuf,
    process::{Command, ExitCode},
};

use vsoft::input_pages::select_pages;

const SELECT_PAGES: &str = "_select-pages";
const USAGE: &str = "Usage: vsoft convert <pdf> <reference-dxf> <destination> [--pages <range>] [--acad <path>] [--skip-preview]\n\
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
    if action != OsStr::new("convert") {
        eprintln!("Unknown command: {}\n{USAGE}", action.to_string_lossy());
        return ExitCode::from(2);
    }

    let result = engine_path().and_then(|script| {
        let python = env::var_os("VSOFT_PYTHON").unwrap_or_else(|| "python".into());
        let executable = env::current_exe()
            .map_err(|error| format!("Could not locate the vsoft executable: {error}"))?;
        Command::new(&python)
            .arg(script)
            .args(args)
            .env("VSOFT_PAGE_SELECTOR", executable)
            .status()
            .map_err(|err| format!("Could not start {}: {err}", python.to_string_lossy()))
    });
    match result {
        Ok(status) => std::process::exit(status.code().unwrap_or(1)),
        Err(message) => {
            eprintln!("vsoft: {message}");
            ExitCode::FAILURE
        }
    }
}
