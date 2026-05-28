use std::{env, path::PathBuf, process::ExitCode};

use ekza_bevy_sdk::{
    BUILTIN_MODEL_MANIFEST, EkzaCharacter, GlbValidationRules, ModelEntry, ModelSource,
    bevy::cache_remote_glb, builtin_model_entry, validate_glb_file,
};

fn main() -> ExitCode {
    let config = match parse_config() {
        Ok(config) => config,
        Err(error) => {
            eprintln!("{error}");
            print_usage();
            return ExitCode::from(64);
        }
    };

    let mut failed = false;
    for entry in config.entries() {
        if let Err(error) = verify_entry(entry, &config.asset_root) {
            eprintln!("{}: FAIL: {error}", entry.display_name);
            failed = true;
        }
    }

    if failed {
        ExitCode::from(2)
    } else {
        ExitCode::SUCCESS
    }
}

struct Config {
    asset_root: PathBuf,
    character: Option<EkzaCharacter>,
}

impl Config {
    fn entries(&self) -> Vec<ModelEntry> {
        if let Some(character) = self.character {
            vec![*builtin_model_entry(character)]
        } else {
            BUILTIN_MODEL_MANIFEST.to_vec()
        }
    }
}

fn parse_config() -> Result<Config, String> {
    let mut args = env::args().skip(1);
    let mut asset_root = PathBuf::from("assets");
    let mut character = None;

    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--asset-root" => {
                let Some(raw) = args.next() else {
                    return Err("--asset-root requires a path".to_string());
                };
                asset_root = PathBuf::from(raw);
            }
            "--character" => {
                let Some(raw) = args.next() else {
                    return Err("--character requires ipfs, toka, wang, or cube".to_string());
                };
                character = Some(parse_character(&raw)?);
            }
            "--all" => character = None,
            "--help" | "-h" => {
                print_usage();
                std::process::exit(0);
            }
            other => return Err(format!("unknown argument: {other}")),
        }
    }

    Ok(Config {
        asset_root,
        character,
    })
}

fn parse_character(raw: &str) -> Result<EkzaCharacter, String> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "ipfs" => Ok(EkzaCharacter::Ipfs),
        "toka" => Ok(EkzaCharacter::Toka),
        "wang" => Ok(EkzaCharacter::Wang),
        "cube" => Ok(EkzaCharacter::Cube),
        _ => Err(format!("unknown character: {raw}")),
    }
}

fn verify_entry(entry: ModelEntry, asset_root: &PathBuf) -> Result<(), String> {
    match entry.source {
        ModelSource::LocalGlb { path, .. } => {
            let final_path = asset_root.join(path);
            validate_glb(&final_path)?;
            println!(
                "{}: PASS local {}",
                entry.display_name,
                final_path.display()
            );
            Ok(())
        }
        ModelSource::RemoteGlb {
            url, cache_path, ..
        } => {
            let Some(relative_path) = cache_remote_glb(url, asset_root, cache_path) else {
                return Err(format!("failed to cache remote GLB from {url}"));
            };
            let final_path = asset_root.join(&relative_path);
            validate_glb(&final_path)?;
            println!(
                "{}: PASS remote {} -> {}",
                entry.display_name,
                url,
                final_path.display()
            );
            Ok(())
        }
        ModelSource::PrimitiveFallback => {
            println!("{}: PASS primitive fallback", entry.display_name);
            Ok(())
        }
    }
}

fn validate_glb(path: &PathBuf) -> Result<(), String> {
    let report = validate_glb_file(path, &GlbValidationRules::default())
        .map_err(|error| format!("failed to read {}: {error}", path.display()))?;
    if report.is_valid() {
        Ok(())
    } else {
        let issues = report
            .issues()
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join("; ");
        Err(format!(
            "{} failed GLB validation: {issues}",
            path.display()
        ))
    }
}

fn print_usage() {
    eprintln!(
        "Usage:
  cargo run --example model_cache -- --asset-root PATH [--all]
  cargo run --example model_cache -- --asset-root PATH --character ipfs|toka|wang|cube"
    );
}
