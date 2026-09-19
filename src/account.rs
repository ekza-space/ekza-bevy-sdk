//! Connect a game to an Ekza account without a wallet (`http` feature).
//!
//! The game asks the registry for a short code and shows it with a link. The player
//! opens the link, signs in to Ekza Studio and confirms. The game then holds a token
//! that can read one thing: which avatars in that account's library are approved for
//! this game. Items arrive in the unified catalogue shape, so they become
//! [`StoreAvatar`]s exactly like the rest of the store.
//!
//! ```no_run
//! # use ekza_bevy_sdk::{account::{AccountClient, AccountFlow}, passport::{SupportSelector, pairing::PairingState}};
//! let api = AccountClient::new("https://registry.ekza.io", "my-game").unwrap();
//! let flow = AccountFlow::start(api, SupportSelector::new("my-game", "desktop", "humanoid-glb-v1", &["glb"]));
//! // every frame:
//! match flow.state() {
//!     PairingState::AwaitingApproval { user_code, verification_url, .. } => { /* show both */ }
//!     PairingState::Connected => { let session = flow.take_session(); /* session.items */ }
//!     PairingState::Failed(reason) => { /* show, offer retry */ }
//!     PairingState::Starting | PairingState::Cancelled => {}
//! }
//! ```
//!
//! The device code and the token stay in memory and never appear in URLs, logs or
//! `Debug` output. The token is not an entitlement: a game server still decides
//! admission from its own registry read (free avatars) or a passport ticket (owned).

use std::{
    io::Read,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

use reqwest::{Url, blocking::Client, redirect::Policy};
use serde::{Deserialize, de::DeserializeOwned};
use serde_json::{Value, json};

use crate::{
    catalog::CatalogV2Avatar,
    passport::{SupportSelector, client::safe_url, pairing::PairingState},
    store::{StoreAvatar, templates_v2},
};

pub const LIBRARY_SCHEMA: &str = "ekza.account.library.v1";
const MAX_JSON_BYTES: u64 = 8 * 1024 * 1024;
const PAIRING_TIMEOUT: Duration = Duration::from_secs(600);
const CANCEL_SLICE: Duration = Duration::from_millis(100);

#[derive(Clone)]
pub struct AccountClient {
    base: Url,
    client: Client,
    project_id: String,
}

// No Debug: deviceCode is a secret.
#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountPairing {
    pub device_code: String,
    pub user_code: String,
    pub verification_url: String,
    pub expires_at: String,
    #[serde(default = "default_interval")]
    pub interval: u64,
}

fn default_interval() -> u64 {
    3
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize)]
pub struct AccountInfo {
    #[serde(default)]
    pub username: String,
}

#[derive(Clone, Deserialize)]
#[serde(tag = "status", rename_all = "lowercase")]
pub enum AccountPoll {
    Pending,
    Approved {
        #[serde(rename = "accessToken")]
        access_token: String,
        #[serde(rename = "expiresAt")]
        expires_at: String,
        #[serde(default)]
        account: AccountInfo,
    },
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct LibraryResponse {
    schema: String,
    project_id: String,
    #[serde(default)]
    account: AccountInfo,
    #[serde(default)]
    items: Vec<CatalogV2Avatar>,
}

/// A connected account. The bearer token is private.
#[derive(Clone)]
pub struct AccountSession {
    api: AccountClient,
    token: String,
    selector: SupportSelector,
    pub username: String,
    /// Library avatars approved for this game, as store items (slug, boundary, `free`).
    pub items: Vec<StoreAvatar>,
}

/// True for the eight-character code alphabet the registry issues.
pub fn valid_user_code(code: &str) -> bool {
    code.len() == 8
        && code
            .bytes()
            .all(|b| matches!(b, b'A'..=b'H' | b'J'..=b'N' | b'P'..=b'Z' | b'2'..=b'9'))
}

impl AccountClient {
    /// `base` is the registry origin, e.g. `https://registry.ekza.io`.
    pub fn new(base: &str, project_id: &str) -> Result<Self, String> {
        let base = safe_url(base)?;
        if project_id.is_empty() || project_id.len() > 64 {
            return Err("Invalid project id".into());
        }
        let client = Client::builder()
            .redirect(Policy::none())
            .connect_timeout(Duration::from_secs(3))
            .timeout(Duration::from_secs(15))
            .build()
            .map_err(|_| "Unable to initialize the account transport".to_string())?;
        Ok(Self {
            base,
            client,
            project_id: project_id.to_string(),
        })
    }

    pub fn project_id(&self) -> &str {
        &self.project_id
    }

    fn request<T: DeserializeOwned>(
        &self,
        path: &str,
        token: Option<&str>,
        body: Option<Value>,
    ) -> Result<T, String> {
        let endpoint = format!(
            "{}/v1/account/{path}",
            self.base.as_str().trim_end_matches('/')
        );
        let mut request = match body {
            Some(body) => self.client.post(endpoint).json(&body),
            None => self.client.get(endpoint),
        };
        if let Some(token) = token {
            request = request.bearer_auth(token);
        }
        let response = request
            .send()
            .map_err(|_| "Ekza could not be reached. Retry.".to_string())?;
        let status = response.status();
        let mut bytes = Vec::new();
        response
            .take(MAX_JSON_BYTES + 1)
            .read_to_end(&mut bytes)
            .map_err(|_| "The Ekza response was interrupted".to_string())?;
        if bytes.len() as u64 > MAX_JSON_BYTES {
            return Err("The Ekza response is too large".into());
        }
        if !status.is_success() {
            // Remote error text may leak details; map status to safe copy.
            return Err(match status.as_u16() {
                401 | 403 => {
                    "This game is no longer connected. Connect your Ekza account again.".into()
                }
                404 => "This game is not registered with Ekza.".into(),
                410 => "The connection code expired. Connect again.".into(),
                429 => "Too many connection attempts. Wait and retry.".into(),
                503 => "Account connection is not available on this Ekza server.".into(),
                _ => format!("Ekza request failed (HTTP {}). Retry.", status.as_u16()),
            });
        }
        serde_json::from_slice(&bytes).map_err(|_| "Invalid Ekza response".into())
    }

    /// Start a connection; show `user_code` and `verification_url`, keep
    /// `device_code` private.
    pub fn pair(&self) -> Result<AccountPairing, String> {
        let pairing: AccountPairing =
            self.request("device", None, Some(json!({"projectId": self.project_id})))?;
        let link = Url::parse(&pairing.verification_url)
            .map_err(|_| "Invalid connection link".to_string())?;
        let local = matches!(link.host_str(), Some("localhost" | "127.0.0.1" | "[::1]"));
        if !(link.scheme() == "https" || (link.scheme() == "http" && local))
            || !link.username().is_empty()
            || link.password().is_some()
            || link.fragment().is_some()
            || !valid_user_code(&pairing.user_code)
            || pairing.device_code.len() < 32
            || pairing.verification_url.contains(&pairing.device_code)
        {
            return Err("Invalid connection link or code".into());
        }
        Ok(pairing)
    }

    pub fn poll(&self, device_code: &str) -> Result<AccountPoll, String> {
        self.request(
            "device/poll",
            None,
            Some(json!({"deviceCode": device_code})),
        )
    }

    /// Load the account's library for this game with an approved token.
    pub fn session(
        &self,
        token: String,
        selector: &SupportSelector,
    ) -> Result<AccountSession, String> {
        let mut session = AccountSession {
            api: self.clone(),
            token,
            selector: selector.clone(),
            username: String::new(),
            items: Vec::new(),
        };
        session.refresh()?;
        Ok(session)
    }
}

impl AccountSession {
    /// Re-read the library, e.g. after the player saved an avatar in the browser.
    pub fn refresh(&mut self) -> Result<(), String> {
        let library: LibraryResponse = self.api.request("library", Some(&self.token), None)?;
        if library.schema != LIBRARY_SCHEMA || library.project_id != self.api.project_id {
            return Err("The Ekza library belongs to a different game or schema".into());
        }
        self.username = library.account.username;
        // The same validation as the public store: identity, exact selector, hash, size.
        self.items = templates_v2(&library.items, &self.selector);
        Ok(())
    }

    pub fn has(&self, slug: &str) -> bool {
        self.items.iter().any(|item| item.slug == slug)
    }
}

struct Shared {
    state: PairingState,
    session: Option<AccountSession>,
}

/// Non-blocking account connection for a game loop. Dropping it cancels the attempt.
pub struct AccountFlow {
    shared: Arc<Mutex<Shared>>,
    cancelled: Arc<AtomicBool>,
}

impl AccountFlow {
    pub fn start(api: AccountClient, selector: SupportSelector) -> Self {
        Self::start_with(api, selector, None)
    }

    fn start_with(
        api: AccountClient,
        selector: SupportSelector,
        poll_interval: Option<Duration>,
    ) -> Self {
        let shared = Arc::new(Mutex::new(Shared {
            state: PairingState::Starting,
            session: None,
        }));
        let cancelled = Arc::new(AtomicBool::new(false));
        let (worker_shared, worker_cancelled) = (shared.clone(), cancelled.clone());
        std::thread::spawn(move || {
            let outcome = run(
                &api,
                &selector,
                &worker_shared,
                &worker_cancelled,
                poll_interval,
            );
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

    pub fn in_progress(&self) -> bool {
        matches!(
            self.state(),
            PairingState::Starting | PairingState::AwaitingApproval { .. }
        )
    }

    /// The connected session, once.
    pub fn take_session(&self) -> Option<AccountSession> {
        self.shared.lock().unwrap().session.take()
    }

    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::Relaxed);
    }
}

impl Drop for AccountFlow {
    fn drop(&mut self) {
        self.cancel();
    }
}

fn run(
    api: &AccountClient,
    selector: &SupportSelector,
    shared: &Mutex<Shared>,
    cancelled: &AtomicBool,
    poll_interval: Option<Duration>,
) -> Result<Option<AccountSession>, String> {
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
            return Err("The connection code expired. Connect again.".into());
        }
        if let AccountPoll::Approved { access_token, .. } = api.poll(&pairing.device_code)? {
            return api.session(access_token, selector).map(Some);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn user_codes_use_only_the_unambiguous_alphabet() {
        assert!(valid_user_code("E9QCAG9V"));
        for bad in [
            "e9qcag9v",
            "E9QCAG9",
            "E9QCAG9VV",
            "E9QCAG0V",
            "E9QCAG1V",
            "E9QCAGIV",
            "E9QCAGOV",
            "",
        ] {
            assert!(!valid_user_code(bad), "{bad:?}");
        }
    }

    #[test]
    fn only_https_or_loopback_registries_are_accepted() {
        assert!(AccountClient::new("https://registry.ekza.io", "omoba").is_ok());
        assert!(AccountClient::new("http://127.0.0.1:8137", "omoba").is_ok());
        assert!(AccountClient::new("http://registry.ekza.io", "omoba").is_err());
        assert!(AccountClient::new("https://user:pass@registry.ekza.io", "omoba").is_err());
        assert!(AccountClient::new("https://registry.ekza.io", "").is_err());
    }

    #[test]
    fn poll_and_library_documents_parse_and_secrets_have_no_debug() {
        let pending: AccountPoll = serde_json::from_str(r#"{"status":"pending"}"#).unwrap();
        assert!(matches!(pending, AccountPoll::Pending));
        let approved: AccountPoll = serde_json::from_str(
            r#"{"status":"approved","accessToken":"t","expiresAt":"2026-10-20T00:00:00Z",
                "projectId":"omoba","account":{"username":"alice"}}"#,
        )
        .unwrap();
        assert!(
            matches!(approved, AccountPoll::Approved { account, .. } if account.username == "alice")
        );
        let library: LibraryResponse = serde_json::from_str(
            r#"{"schema":"ekza.account.library.v1","projectId":"omoba","account":{"username":"alice"},
                "expiresAt":"2026-10-20T00:00:00Z","items":[]}"#,
        )
        .unwrap();
        assert_eq!(
            (library.schema.as_str(), library.items.len()),
            (LIBRARY_SCHEMA, 0)
        );
    }
}
