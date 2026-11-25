use std::{
    env,
    ffi::OsStr,
    path::PathBuf,
    process::{Command, ExitCode},
};

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
    if action != OsStr::new("convert") {
        eprintln!("Unknown command: {}\n{USAGE}", action.to_string_lossy());
        return ExitCode::from(2);
    }

    let result = engine_path().and_then(|script| {
        let python = env::var_os("VSOFT_PYTHON").unwrap_or_else(|| "python".into());
        Command::new(&python)
            .arg(script)
            .args(args)
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
