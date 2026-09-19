//! Non-blocking wallet pairing for a game loop (`http` feature).
//!
//! [`pair_interactively`](super::client::pair_interactively) blocks a terminal
//! tool for up to ten minutes; a game cannot. [`PairingFlow`] runs the same
//! device flow on a worker thread and exposes a state the UI polls once per
//! frame:
//!
//! ```no_run
//! # use ekza_bevy_sdk::passport::{client::PassportClient, pairing::{PairingFlow, PairingState}};
//! # let api = PassportClient::new("https://avatar.ekza.io/api/passport", "my-game").unwrap();
//! let flow = PairingFlow::start(api);
//! // every frame:
//! match flow.state() {
//!     PairingState::AwaitingApproval { user_code, verification_url, .. } => { /* show both */ }
//!     PairingState::Connected => { let session = flow.take_session(); /* owned avatars */ }
//!     PairingState::Failed(reason) => { /* show, offer retry */ }
//!     PairingState::Starting | PairingState::Cancelled => {}
//! }
//! ```
//!
//! The device code and the bearer token never leave this module except inside
//! the returned [`NativeSession`]; [`PairingState`] is safe to log and render.

use std::{
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

use super::client::{NativeSession, PairingPoll, PassportClient, safe_url};

const PAIRING_TIMEOUT: Duration = Duration::from_secs(600);
const CANCEL_SLICE: Duration = Duration::from_millis(100);

/// Public, renderable pairing progress.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PairingState {
    /// Asking the passport for a pairing code.
    Starting,
    /// Show `user_code` and `verification_url`; the player approves in a browser.
    AwaitingApproval {
        user_code: String,
        verification_url: String,
        expires_at: String,
    },
    /// Approved; collect the session with [`PairingFlow::take_session`].
    Connected,
    /// Safe, action-oriented message; start a new flow to retry.
    Failed(String),
    Cancelled,
}

struct Shared {
    state: PairingState,
    session: Option<NativeSession>,
}

/// Handle to a pairing attempt running on a worker thread. Dropping it cancels
/// the attempt.
pub struct PairingFlow {
    shared: Arc<Mutex<Shared>>,
    cancelled: Arc<AtomicBool>,
}

impl PairingFlow {
    pub fn start(api: PassportClient) -> Self {
        Self::start_with(api, None)
    }

    /// `poll_interval` overrides the passport-suggested interval (clamped to
    /// 3..=10 s by default, matching the service rate limit). Tests only.
    fn start_with(api: PassportClient, poll_interval: Option<Duration>) -> Self {
        let shared = Arc::new(Mutex::new(Shared {
            state: PairingState::Starting,
            session: None,
        }));
        let cancelled = Arc::new(AtomicBool::new(false));
        let (worker_shared, worker_cancelled) = (shared.clone(), cancelled.clone());
        std::thread::spawn(move || {
            let outcome = run(&api, &worker_shared, &worker_cancelled, poll_interval);
            let mut shared = worker_shared.lock().unwrap();
            match outcome {
                Ok(Some(session)) => {
                    shared.session = Some(session);
                    shared.state = PairingState::Connected;
                }
                Ok(None) => shared.state = PairingState::Cancelled,
                Err(error) => shared.state = PairingState::Failed(error),
            }
        });
        Self { shared, cancelled }
    }

    pub fn state(&self) -> PairingState {
        self.shared.lock().unwrap().state.clone()
    }

    /// True until the flow reaches `Connected`, `Failed` or `Cancelled`.
    pub fn in_progress(&self) -> bool {
        matches!(
            self.state(),
            PairingState::Starting | PairingState::AwaitingApproval { .. }
        )
    }

    /// The paired session, once. `None` before approval and after it was taken.
    pub fn take_session(&self) -> Option<NativeSession> {
        self.shared.lock().unwrap().session.take()
    }

    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::Relaxed);
    }
}

impl Drop for PairingFlow {
    fn drop(&mut self) {
        self.cancel();
    }
}

fn run(
    api: &PassportClient,
    shared: &Mutex<Shared>,
    cancelled: &AtomicBool,
    poll_interval: Option<Duration>,
) -> Result<Option<NativeSession>, String> {
    let pairing = api.pair()?;
    let interval =
        poll_interval.unwrap_or_else(|| Duration::from_secs(pairing.interval.clamp(3, 10)));
    shared.lock().unwrap().state = PairingState::AwaitingApproval {
        user_code: pairing.user_code.clone(),
        verification_url: pairing.verification_url.clone(),
        expires_at: pairing.expires_at.clone(),
    };
    let started = Instant::now();
    loop {
        let wake = Instant::now() + interval;
        while Instant::now() < wake {
            if cancelled.load(Ordering::Relaxed) {
                return Ok(None);
            }
            std::thread::sleep(CANCEL_SLICE.min(interval));
        }
        if started.elapsed() > PAIRING_TIMEOUT {
            return Err("Pairing expired. Connect again.".into());
        }
        match api.poll(&pairing.device_code)? {
            PairingPoll::Pending => {}
            PairingPoll::Approved {
                access_token,
                wallet,
                ..
            } => return api.session(access_token, &wallet).map(Some),
        }
    }
}

/// Open the wallet approval page in the player's default browser. Only an
/// HTTPS (or explicit localhost) URL without credentials is ever handed to the
/// operating system. Unsupported platforms return an error so the game can
/// fall back to showing the link.
pub fn open_in_browser(url: &str) -> Result<(), String> {
    let url = safe_url_with_query(url)?;
    let mut command = if cfg!(target_os = "macos") {
        let mut command = std::process::Command::new("open");
        command.arg(&url);
        command
    } else if cfg!(target_os = "windows") {
        let mut command = std::process::Command::new("rundll32");
        command.args(["url.dll,FileProtocolHandler", &url]);
        command
    } else if cfg!(all(
        unix,
        not(target_os = "ios"),
        not(target_os = "android")
    )) {
        let mut command = std::process::Command::new("xdg-open");
        command.arg(&url);
        command
    } else {
        return Err("Open the link on this device manually".into());
    };
    command
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map(drop)
        .map_err(|_| "Could not open the browser; open the link manually".to_string())
}

/// Verification links carry the public user code as a query; everything else
/// follows the passport URL rules.
fn safe_url_with_query(raw: &str) -> Result<String, String> {
    let (base, _query) = raw.split_once('?').unwrap_or((raw, ""));
    if raw.contains('#') || raw.chars().any(char::is_whitespace) || raw.starts_with('-') {
        return Err("Invalid wallet verification link".into());
    }
    safe_url(base)?;
    Ok(raw.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        io::{Read, Write},
        net::TcpListener,
    };

    fn wait_for(flow: &PairingFlow, done: impl Fn(&PairingState) -> bool) -> PairingState {
        let started = Instant::now();
        loop {
            let state = flow.state();
            if done(&state) || started.elapsed() > Duration::from_secs(10) {
                return state;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    #[test]
    fn flow_reports_code_then_connects_without_exposing_secrets() {
        let wallet = "2".repeat(32);
        let device_code = "private-device-code-0123456789";
        // The verification link must name the passport's own origin.
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let origin = format!("http://{}", listener.local_addr().unwrap());
        let bodies = vec![
            format!(
                r#"{{"deviceCode":"{device_code}","userCode":"ABCD-1234","verificationUrl":"{origin}/connect?code=ABCD-1234","expiresAt":"2099-01-01T00:00:00Z","interval":3}}"#
            ),
            r#"{"status":"pending"}"#.to_owned(),
            format!(
                r#"{{"status":"approved","accessToken":"private-access-token","expiresAt":"2099-01-01T00:00:00Z","wallet":"{wallet}"}}"#
            ),
            format!(
                r#"{{"schema":"ekza.passport.library.v1","network":"solana-devnet","wallet":"{wallet}","expiresAt":"2099-01-01T00:00:00Z","items":[]}}"#
            ),
        ];
        let server = std::thread::spawn(move || {
            for body in bodies {
                let (mut stream, _) = listener.accept().unwrap();
                let mut buffer = vec![0u8; 16 * 1024];
                let _ = stream.read(&mut buffer).unwrap();
                let reply = format!(
                    "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                    body.len()
                );
                stream.write_all(reply.as_bytes()).unwrap();
            }
        });

        let api = PassportClient::new(&format!("{origin}/api/passport"), "my-game").unwrap();
        let flow = PairingFlow::start_with(api, Some(Duration::from_millis(20)));
        let awaiting = wait_for(&flow, |state| {
            matches!(state, PairingState::AwaitingApproval { .. })
        });
        let PairingState::AwaitingApproval {
            user_code,
            verification_url,
            ..
        } = &awaiting
        else {
            panic!("expected a pairing code, got {awaiting:?}");
        };
        assert_eq!(user_code, "ABCD-1234");
        assert!(verification_url.starts_with(&origin));
        assert!(!format!("{awaiting:?}").contains(device_code));
        assert!(flow.in_progress());

        let done = wait_for(&flow, |state| {
            !matches!(state, PairingState::AwaitingApproval { .. })
        });
        assert_eq!(done, PairingState::Connected);
        assert!(!flow.in_progress());
        let session = flow.take_session().expect("session is handed over once");
        assert_eq!(session.wallet(), wallet);
        assert!(flow.take_session().is_none());
        server.join().unwrap();
    }

    #[test]
    fn unreachable_passport_fails_with_safe_copy_and_cancel_stops_the_worker() {
        // Nothing listens on this port.
        let probe = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = probe.local_addr().unwrap().port();
        drop(probe);
        let api = PassportClient::new(&format!("http://127.0.0.1:{port}/api/passport"), "my-game")
            .unwrap();
        let flow = PairingFlow::start_with(api, Some(Duration::from_millis(20)));
        let failed = wait_for(&flow, |state| matches!(state, PairingState::Failed(_)));
        assert_eq!(
            failed,
            PairingState::Failed("Passport service could not be reached. Retry.".into())
        );
    }

    #[test]
    fn only_safe_links_reach_the_operating_system() {
        assert!(safe_url_with_query("https://avatar.ekza.io/connect?code=ABCD-1234").is_ok());
        assert!(safe_url_with_query("http://127.0.0.1:5190/connect?code=ABCD").is_ok());
        for bad in [
            "http://avatar.ekza.io/connect",
            "https://user:pw@avatar.ekza.io/connect",
            "file:///etc/passwd",
            "-a Calculator",
            "https://avatar.ekza.io/connect#frag",
            "https://avatar.ekza.io/con nect",
        ] {
            assert!(safe_url_with_query(bad).is_err(), "{bad}");
        }
    }
}
