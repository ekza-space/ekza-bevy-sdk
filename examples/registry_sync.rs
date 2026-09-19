//! Pull every Ekza avatar into a game's asset root.
//!
//! ```bash
//! # Everything public: approved registry templates + the free library.
//! cargo run --example registry_sync --no-default-features --features http -- \
//!     --asset-root ../omoba-bevy/client/assets --project omoba \
//!     --platform desktop --profile humanoid-glb-v1
//!
//! Registry/passport templates are products: by default they are staged only
//! when approved for `--project` and carry a `passport` boundary in the
//! manifest, so the game server still demands a consumed ticket. Use
//! `--protected free` only for previews without an entitlement check.
//!
//! # Only avatars an operator approved for Omoba.
//! cargo run --example registry_sync --no-default-features --features http -- \
//!     --asset-root assets --project omoba --approved-only
//!
//! # Local development against a registry checkout on port 8080.
//! cargo run --example registry_sync --no-default-features --features http -- \
//!     --registry http://127.0.0.1:8080 --asset-root /tmp/assets --library-max 20
//! ```
//!
//! The manifest written is `avatars/manifest.json`, models are staged as
//! `avatars/<slug>.glb`, thumbnails as `avatars/<slug>.png|jpg`. Consumers that
//! bake their own animation clips (Omoba's `scripts/retarget_animations.py`)
//! run that step after this one.

use std::{env, path::PathBuf, process::ExitCode};

use ekza_bevy_sdk::{
    catalog::{EkzaAvatar, merge_avatars},
    registry::{DEFAULT_PASSPORT_URL, DEFAULT_REGISTRY_URL, PassportCatalogClient, RegistryClient},
    roster::sync::{ProtectedPolicy, SyncOptions, sync_roster},
};

struct Args {
    registry: String,
    passport: Option<String>,
    library: bool,
    library_max: Option<u64>,
    offline_catalog: Option<PathBuf>,
    options: SyncOptions,
    verbose: bool,
}

fn usage() {
    eprintln!(
        "usage: registry_sync --asset-root DIR [--manifest FILE] [--registry URL] \\\n\
         \x20   [--passport URL|--no-passport] [--no-library] [--library-max N] \\\n\
         \x20   [--offline-catalog FILE] [--project ID] [--platform P --profile Q] \\\n\
         \x20   [--approved-only] [--protected passport|skip|free] [--no-thumbnails] \\\n\
         \x20   [--prune] [--dry-run] [--verbose]"
    );
}

fn parse() -> Result<Args, String> {
    let mut argv = env::args().skip(1);
    let mut asset_root: Option<PathBuf> = None;
    let mut manifest = None;
    let mut registry = DEFAULT_REGISTRY_URL.to_string();
    let mut passport = Some(DEFAULT_PASSPORT_URL.to_string());
    let mut library = true;
    let mut library_max = None;
    let mut offline_catalog = None;
    let mut project = None;
    let mut platform = "desktop".to_string();
    let mut profile = "humanoid-glb-v1".to_string();
    let mut approved_only = false;
    let mut protected = ProtectedPolicy::Passport;
    let mut thumbnails = true;
    let mut prune = false;
    let mut dry_run = false;
    let mut verbose = false;

    let value = |flag: &str, argv: &mut dyn Iterator<Item = String>| {
        argv.next()
            .ok_or_else(|| format!("{flag} requires a value"))
    };
    while let Some(arg) = argv.next() {
        match arg.as_str() {
            "--asset-root" => asset_root = Some(PathBuf::from(value(&arg, &mut argv)?)),
            "--manifest" => manifest = Some(PathBuf::from(value(&arg, &mut argv)?)),
            "--registry" => registry = value(&arg, &mut argv)?,
            "--passport" => passport = Some(value(&arg, &mut argv)?),
            "--no-passport" => passport = None,
            "--no-library" => library = false,
            "--library-max" => {
                library_max = Some(
                    value(&arg, &mut argv)?
                        .parse()
                        .map_err(|_| "--library-max expects a number".to_string())?,
                )
            }
            "--offline-catalog" => offline_catalog = Some(PathBuf::from(value(&arg, &mut argv)?)),
            "--project" => project = Some(value(&arg, &mut argv)?),
            "--platform" => platform = value(&arg, &mut argv)?,
            "--profile" => profile = value(&arg, &mut argv)?,
            "--approved-only" => approved_only = true,
            "--protected" => {
                protected = match value(&arg, &mut argv)?.as_str() {
                    "passport" => ProtectedPolicy::Passport,
                    "skip" => ProtectedPolicy::Skip,
                    "free" => ProtectedPolicy::Free,
                    other => return Err(format!("unknown --protected policy {other}")),
                }
            }
            "--no-thumbnails" => thumbnails = false,
            "--prune" => prune = true,
            "--dry-run" => dry_run = true,
            "--verbose" | "-v" => verbose = true,
            "--help" | "-h" => {
                usage();
                std::process::exit(0);
            }
            other => return Err(format!("unknown argument {other}")),
        }
    }
    let asset_root = asset_root.ok_or("--asset-root is required")?;
    if approved_only && project.is_none() {
        return Err("--approved-only needs --project".into());
    }
    let mut options = SyncOptions::new(asset_root);
    options.manifest = manifest;
    options.project_id = project;
    options.platform = platform;
    options.profile = profile;
    options.approved_only = approved_only;
    options.protected = protected;
    options.thumbnails = thumbnails;
    options.prune = prune;
    options.dry_run = dry_run;
    Ok(Args {
        registry,
        passport,
        library,
        library_max,
        offline_catalog,
        options,
        verbose,
    })
}

fn main() -> ExitCode {
    let args = match parse() {
        Ok(args) => args,
        Err(error) => {
            eprintln!("{error}");
            usage();
            return ExitCode::from(64);
        }
    };

    let mut feeds: Vec<Vec<EkzaAvatar>> = Vec::new();

    if let Some(path) = &args.offline_catalog {
        match std::fs::read_to_string(path)
            .map_err(|e| e.to_string())
            .and_then(|raw| {
                serde_json::from_str::<ekza_bevy_sdk::catalog::RegistryCatalogDocument>(&raw)
                    .map_err(|e| e.to_string())
            }) {
            Ok(document) => {
                let avatars: Vec<EkzaAvatar> = document
                    .items
                    .into_iter()
                    .map(|avatar| avatar.into_avatar(&args.registry))
                    .collect();
                eprintln!(
                    "offline catalogue {}: {} avatars",
                    path.display(),
                    avatars.len()
                );
                feeds.push(avatars);
            }
            Err(error) => {
                eprintln!("cannot read offline catalogue {}: {error}", path.display());
                return ExitCode::from(65);
            }
        }
    } else {
        let client = match RegistryClient::new(&args.registry) {
            Ok(client) => client,
            Err(error) => {
                eprintln!("{error}");
                return ExitCode::from(65);
            }
        };
        match client.catalog(None) {
            Ok(avatars) => {
                eprintln!(
                    "registry {}/v1/avatars: {} avatars",
                    client.base_url(),
                    avatars.len()
                );
                feeds.push(avatars);
            }
            Err(error) => eprintln!("registry catalogue unavailable: {error}"),
        }
        if args.library {
            match client.library(args.library_max) {
                Ok(avatars) => {
                    eprintln!(
                        "registry {}/library/avatars: {} avatars",
                        client.base_url(),
                        avatars.len()
                    );
                    feeds.push(avatars);
                }
                Err(error) => eprintln!("free library unavailable: {error}"),
            }
        }
    }

    if let Some(passport) = &args.passport {
        match PassportCatalogClient::new(passport).and_then(|client| client.catalog()) {
            Ok(avatars) => {
                eprintln!("passport {passport}/catalog: {} avatars", avatars.len());
                feeds.push(avatars);
            }
            Err(error) => eprintln!("passport catalogue unavailable: {error}"),
        }
    }

    if feeds.is_empty() {
        eprintln!("no avatar feed could be read");
        return ExitCode::from(69);
    }
    let avatars = merge_avatars(feeds);
    eprintln!("{} unique avatars after merge", avatars.len());

    let report = match sync_roster(&avatars, &args.options) {
        Ok(report) => report,
        Err(error) => {
            eprintln!("sync failed: {error}");
            return ExitCode::from(70);
        }
    };
    if args.verbose {
        for slug in &report.staged {
            eprintln!("  staged  {slug}");
        }
        for (name, reason) in &report.skipped {
            eprintln!("  skipped {name}: {reason}");
        }
    }
    for (name, detail) in &report.failed {
        eprintln!("  FAILED  {name}: {detail}");
    }
    eprintln!(
        "staged {} ({} reused from cache), skipped {}, failed {}{}",
        report.staged.len(),
        report.reused,
        report.skipped.len(),
        report.failed.len(),
        report
            .manifest
            .as_ref()
            .map(|path| format!(" -> {}", path.display()))
            .unwrap_or_else(|| " (dry run)".into())
    );
    if report.staged.is_empty() && !report.failed.is_empty() {
        ExitCode::from(1)
    } else {
        ExitCode::SUCCESS
    }
}
