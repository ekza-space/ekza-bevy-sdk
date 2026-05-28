use std::{env, path::PathBuf, process::ExitCode};

use ekza_bevy_sdk::{GlbValidationRules, validate_glb_file};

fn main() -> ExitCode {
    let args = env::args().skip(1).collect::<Vec<_>>();
    if args
        .iter()
        .any(|arg| matches!(arg.as_str(), "--help" | "-h"))
    {
        print_usage();
        return ExitCode::SUCCESS;
    }

    let mut path = None;
    let mut rules = GlbValidationRules::default();
    let mut args = args.into_iter();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "path" | "--path" => {
                let Some(raw) = args.next() else {
                    eprintln!("{arg} requires a GLB file path");
                    return ExitCode::from(64);
                };
                if path.replace(PathBuf::from(raw)).is_some() {
                    eprintln!("model path was provided more than once");
                    return ExitCode::from(64);
                }
            }
            "--strict" => rules.allow_trailing_bytes = false,
            "--max-bytes" => {
                let Some(raw) = args.next() else {
                    eprintln!("--max-bytes requires a number");
                    return ExitCode::from(64);
                };
                match raw.parse::<usize>() {
                    Ok(value) => rules.max_bytes = Some(value),
                    Err(error) => {
                        eprintln!("Invalid --max-bytes value {raw:?}: {error}");
                        return ExitCode::from(64);
                    }
                }
            }
            value if !value.starts_with('-') && path.is_none() => path = Some(PathBuf::from(value)),
            other => {
                eprintln!("Unknown argument: {other}");
                return ExitCode::from(64);
            }
        }
    }

    let Some(path) = path else {
        print_usage();
        return ExitCode::from(64);
    };

    let report = match validate_glb_file(&path, &rules) {
        Ok(report) => report,
        Err(error) => {
            eprintln!("Failed to read {path:?}: {error}");
            return ExitCode::from(66);
        }
    };

    println!("file: {}", path.display());
    println!("format: {:?}", report.format);
    println!("bytes: {}", report.byte_len);
    println!("declared_len: {:?}", report.declared_len);
    if report.is_valid() {
        println!("result: PASS");
        ExitCode::SUCCESS
    } else {
        println!("result: FAIL");
        for issue in report.issues() {
            println!("- {issue}");
        }
        ExitCode::from(2)
    }
}

fn print_usage() {
    eprintln!(
        "Usage:
  cargo run --example model_check -- <model.glb> [--strict] [--max-bytes N]
  cargo run --example model_check -- path <model.glb> [--strict]
  cargo run --example model_check -- --path <model.glb> [--strict]"
    );
}
