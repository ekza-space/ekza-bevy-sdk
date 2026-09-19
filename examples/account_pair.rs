//! Connect this terminal to an Ekza account and list the library for a game.
//!
//! ```text
//! cargo run --example account_pair -- http://127.0.0.1:8137 omoba desktop humanoid-glb-v1
//! ```
//! Open the printed link, sign in to Ekza Studio and confirm the code. No wallet.
use std::time::Duration;

use ekza_bevy_sdk::{
    account::{AccountClient, AccountFlow},
    passport::{SupportSelector, pairing::PairingState},
    registry::DEFAULT_REGISTRY_URL,
};

fn main() -> Result<(), String> {
    let mut args = std::env::args().skip(1);
    let registry = args
        .next()
        .unwrap_or_else(|| DEFAULT_REGISTRY_URL.to_owned());
    let project = args.next().unwrap_or_else(|| "omoba".to_owned());
    let platform = args.next().unwrap_or_else(|| "desktop".to_owned());
    let profile = args.next().unwrap_or_else(|| "humanoid-glb-v1".to_owned());
    let selector = SupportSelector::new(&project, &platform, &profile, &["glb", "vrm"]);
    let flow = AccountFlow::start(AccountClient::new(&registry, &project)?, selector);
    let mut shown = false;
    loop {
        match flow.state() {
            PairingState::AwaitingApproval {
                user_code,
                verification_url,
                ..
            } if !shown => {
                println!("code {user_code}\nopen {verification_url}");
                shown = true;
            }
            PairingState::Connected => break,
            PairingState::Failed(reason) => return Err(reason),
            PairingState::Cancelled => return Err("cancelled".into()),
            _ => {}
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    let mut session = flow.take_session().ok_or("no session")?;
    println!(
        "connected as {} with {} avatar(s)",
        session.username,
        session.items.len()
    );
    // Give the player a moment to save something in the browser, then look again.
    if let Ok(wait) = std::env::var("EKZA_ACCOUNT_PAIR_WAIT") {
        std::thread::sleep(Duration::from_secs(wait.parse().unwrap_or(0)));
        session.refresh()?;
    }
    for item in &session.items {
        println!(
            "{} {} free={} {} bytes",
            item.slug, item.name, item.free, item.protected.support.rendition.size_bytes
        );
    }
    println!("library {} avatar(s)", session.items.len());
    Ok(())
}
