//! Pair a wallet, list what it owns for a game, optionally install it.
//!
//! The smallest end-to-end consumer of the SDK: the same calls a game makes,
//! without an engine.
//!
//! ```text
//! cargo run --example passport_pair --no-default-features --features http -- \
//!     --passport https://avatar.ekza.io/api/passport --project omoba \
//!     --platform desktop --profile humanoid-glb-v1 \
//!     [--registry https://registry.ekza.io --install-dir ./ekza-store] [--wait 600]
//! ```
use std::{
    env,
    process::ExitCode,
    time::{Duration, Instant},
};

use ekza_bevy_sdk::{
    passport::{
        SupportSelector,
        client::PassportClient,
        pairing::{PairingFlow, PairingState, open_in_browser},
        protected_slug,
    },
    registry::{DEFAULT_PASSPORT_URL, DEFAULT_REGISTRY_URL},
    store::AvatarStore,
};

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("error: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<(), String> {
    let mut passport = DEFAULT_PASSPORT_URL.to_string();
    let mut registry = DEFAULT_REGISTRY_URL.to_string();
    let mut project = "omoba".to_string();
    let mut platform = "desktop".to_string();
    let mut profile = "humanoid-glb-v1".to_string();
    let mut install_dir = None;
    let mut wait = 600u64;
    let mut open = true;
    let mut argv = env::args().skip(1);
    while let Some(flag) = argv.next() {
        let mut value = || argv.next().ok_or_else(|| format!("{flag} requires a value"));
        match flag.as_str() {
            "--passport" => passport = value()?,
            "--registry" => registry = value()?,
            "--project" => project = value()?,
            "--platform" => platform = value()?,
            "--profile" => profile = value()?,
            "--install-dir" => install_dir = Some(value()?),
            "--wait" => wait = value()?.parse().map_err(|_| "--wait expects seconds")?,
            "--no-open" => open = false,
            other => return Err(format!("unknown argument {other}")),
        }
    }
    let selector = SupportSelector::new(&project, &platform, &profile, &["glb", "vrm"]);
    let flow = PairingFlow::start(PassportClient::new(&passport, &project)?);

    let started = Instant::now();
    let mut shown = false;
    let session = loop {
        match flow.state() {
            PairingState::Starting => {}
            PairingState::AwaitingApproval {
                user_code,
                verification_url,
                expires_at,
            } => {
                if !shown {
                    shown = true;
                    println!("Approve in your browser: {verification_url}");
                    println!("Code: {user_code} (expires {expires_at})");
                    if open && let Err(error) = open_in_browser(&verification_url) {
                        eprintln!("{error}");
                    }
                }
            }
            PairingState::Connected => break flow.take_session().ok_or("session already taken")?,
            PairingState::Failed(error) => return Err(error),
            PairingState::Cancelled => return Err("pairing cancelled".into()),
        }
        if started.elapsed() > Duration::from_secs(wait) {
            flow.cancel();
            return Err(format!("no approval within {wait}s"));
        }
        std::thread::sleep(Duration::from_millis(200));
    };

    println!("Wallet {} connected.", session.wallet());
    let owned = session.supported(&selector);
    println!("{} owned avatar(s) approved for {project}/{platform}/{profile}:", owned.len());
    for protected in &owned {
        println!("  {}  {}", protected_slug(protected), protected.avatar_id);
    }
    let Some(directory) = install_dir else {
        return Ok(());
    };
    let store = AvatarStore::new(&directory, &registry, selector)?;
    for item in store.refresh()? {
        if owned.iter().any(|protected| protected == &item.protected) {
            let path = store.install(&item, |_| Ok(()))?;
            println!("installed {} -> {}", item.name, path.display());
        }
    }
    Ok(())
}
