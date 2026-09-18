//! Native passport transport for any game (`http` feature): device pairing,
//! the wallet's purchased library, one-use tickets and verified downloads.
//!
//! Generalized from the first Omoba integration. Secrets (device code, bearer
//! token) stay in memory and never appear in URLs, logs or `Debug` output.
//! A game server consumes tickets at its *configured* passport origin; a
//! client-submitted mint, slug or JSON object is never an entitlement.

use std::{io::Read, time::Duration};

use reqwest::{Url, blocking::Client, redirect::Policy};
use serde::{Deserialize, de::DeserializeOwned};
use serde_json::{Value, json};

use super::{
    ConsumedTicket, ProjectSupport, ProtectedAvatar, PurchasedAvatar, PurchasedLibrary,
    SupportSelector, valid_base58_key, validate_avatar_id, validate_project_support,
};
use crate::sha256::sha256_hex;

pub const PASSPORT_URL_ENV: &str = "EKZA_PASSPORT_URL";
pub const LIBRARY_SCHEMA: &str = "ekza.passport.library.v1";
const MAX_JSON_BYTES: u64 = 2 * 1024 * 1024;

#[derive(Clone)]
pub struct PassportClient {
    base: Url,
    client: Client,
    project_id: String,
}

// No Debug: deviceCode is a secret.
#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DevicePairing {
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

#[derive(Clone, Deserialize)]
#[serde(tag = "status", rename_all = "lowercase")]
pub enum PairingPoll {
    Pending,
    Approved {
        #[serde(rename = "accessToken")]
        access_token: String,
        #[serde(rename = "expiresAt")]
        expires_at: String,
        wallet: String,
    },
}

#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AvatarTicket {
    pub ticket: String,
    pub expires_at: String,
    pub avatar: PurchasedAvatar,
    pub support: ProjectSupport,
}

/// A paired wallet session. The bearer token is private.
#[derive(Clone)]
pub struct NativeSession {
    pub api: PassportClient,
    token: String,
    pub library: PurchasedLibrary,
}

pub fn safe_url(raw: &str) -> Result<Url, String> {
    let url = Url::parse(raw).map_err(|_| "Invalid passport URL".to_string())?;
    let local = matches!(url.host_str(), Some("localhost" | "127.0.0.1" | "[::1]"));
    if !(url.scheme() == "https" || (url.scheme() == "http" && local))
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err("Use HTTPS, or explicit localhost development, without URL credentials".into());
    }
    Ok(url)
}

pub fn valid_session_id(session_id: &str) -> bool {
    !session_id.is_empty()
        && session_id.len() <= 64
        && session_id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.'))
}

impl PassportClient {
    /// `base` ends in `/api/passport`; `project_id` is this game's registered id.
    pub fn new(base: &str, project_id: &str) -> Result<Self, String> {
        let base = safe_url(base)?;
        if !base.path().trim_end_matches('/').ends_with("/api/passport") {
            return Err("Passport URL must end in /api/passport".into());
        }
        if project_id.is_empty() || project_id.len() > 64 {
            return Err("Invalid project id".into());
        }
        let client = Client::builder()
            .redirect(Policy::none())
            .connect_timeout(Duration::from_secs(3))
            .timeout(Duration::from_secs(15))
            .build()
            .map_err(|_| "Unable to initialize passport HTTPS transport".to_string())?;
        Ok(Self {
            base,
            client,
            project_id: project_id.to_string(),
        })
    }

    pub fn from_env(project_id: &str) -> Result<Self, String> {
        let base = std::env::var(PASSPORT_URL_ENV)
            .map_err(|_| format!("Set {PASSPORT_URL_ENV} to the trusted storefront passport API"))?;
        Self::new(&base, project_id)
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
        let endpoint = format!("{}/{}", self.base.as_str().trim_end_matches('/'), path);
        let mut request = if let Some(body) = body {
            self.client.post(endpoint).json(&body)
        } else {
            self.client.get(endpoint)
        };
        if let Some(token) = token {
            request = request.bearer_auth(token);
        }
        let response = request
            .send()
            .map_err(|_| "Passport service could not be reached. Retry.".to_string())?;
        let status = response.status();
        let mut bytes = Vec::new();
        response
            .take(MAX_JSON_BYTES + 1)
            .read_to_end(&mut bytes)
            .map_err(|_| "Passport response was interrupted".to_string())?;
        if bytes.len() as u64 > MAX_JSON_BYTES {
            return Err("Passport response is too large".into());
        }
        if !status.is_success() {
            // Remote error text may leak details; map status to safe copy.
            return Err(match status.as_u16() {
                401 | 403 => {
                    "Wallet approval expired or this avatar is not owned/supported. Pair again."
                        .into()
                }
                404 | 410 => "Pairing, avatar, or ticket expired or is no longer available.".into(),
                409 => "Pairing or ticket was already used. Start a fresh approval.".into(),
                429 => "Too many passport requests. Wait and retry.".into(),
                _ => format!("Passport request failed (HTTP {}). Retry.", status.as_u16()),
            });
        }
        serde_json::from_slice(&bytes).map_err(|_| "Invalid passport response".into())
    }

    /// Start device pairing; show `user_code` and `verification_url` to the
    /// player, keep `device_code` private.
    pub fn pair(&self) -> Result<DevicePairing, String> {
        let pairing: DevicePairing =
            self.request("device", None, Some(json!({"projectId": self.project_id})))?;
        let verification = Url::parse(&pairing.verification_url)
            .map_err(|_| "Invalid wallet verification link".to_string())?;
        if verification.origin() != self.base.origin()
            || !verification.username().is_empty()
            || verification.password().is_some()
            || pairing.user_code.is_empty()
            || pairing.device_code.len() < 16
            || pairing.verification_url.contains(&pairing.device_code)
        {
            return Err("Invalid wallet verification link or pairing code".into());
        }
        Ok(pairing)
    }

    pub fn poll(&self, device_code: &str) -> Result<PairingPoll, String> {
        self.request(
            "device/poll",
            None,
            Some(json!({"deviceCode": device_code})),
        )
    }

    /// Load the wallet's purchased library for an approved token.
    pub fn session(&self, token: String, expected_wallet: &str) -> Result<NativeSession, String> {
        let library: PurchasedLibrary = self.request("library", Some(&token), None)?;
        if library.schema != LIBRARY_SCHEMA || library.wallet != expected_wallet {
            return Err("Passport library belongs to a different wallet or schema".into());
        }
        for avatar in &library.items {
            validate_avatar_id(&avatar.avatar_id)?;
            if !valid_base58_key(&avatar.mint) {
                return Err("Invalid owned NFT mint".into());
            }
        }
        Ok(NativeSession {
            api: self.clone(),
            token,
            library,
        })
    }

    /// Server side: consume a one-use ticket for this project and game session.
    pub fn consume(&self, ticket: &str, session_id: &str) -> Result<ConsumedTicket, String> {
        if ticket.len() < 16 || ticket.len() > 4096 || !valid_session_id(session_id) {
            return Err("Missing or malformed avatar admission proof".into());
        }
        self.request(
            "ticket/consume",
            None,
            Some(json!({
                "ticket": ticket, "projectId": self.project_id, "sessionId": session_id,
            })),
        )
    }

    /// Download an approved rendition and verify size, SHA-256 and GLB envelope.
    /// The request carries no bearer token, cookies or redirects.
    pub fn download(
        &self,
        protected: &ProtectedAvatar,
        selector: &SupportSelector,
    ) -> Result<Vec<u8>, String> {
        protected.validate_for(selector)?;
        let url = safe_url(&protected.support.rendition.url)?;
        let response = self
            .client
            .get(url)
            .send()
            .map_err(|_| "Avatar rendition download failed".to_string())?;
        if !response.status().is_success() {
            return Err("Avatar rendition is unavailable".into());
        }
        let limit = protected.support.rendition.size_bytes;
        if response.content_length().is_some_and(|size| size > limit) {
            return Err("Avatar exceeds its approved size".into());
        }
        let mut bytes = Vec::new();
        response
            .take(limit + 1)
            .read_to_end(&mut bytes)
            .map_err(|_| "Avatar download was interrupted".to_string())?;
        verify_bytes(protected, selector, &bytes)?;
        Ok(bytes)
    }
}

/// Exact-byte verification of an approved rendition for `selector`.
pub fn verify_bytes(
    protected: &ProtectedAvatar,
    selector: &SupportSelector,
    bytes: &[u8],
) -> Result<(), String> {
    protected.validate_for(selector)?;
    protected
        .validate_bytes_for(selector, bytes, &sha256_hex(bytes))
        .map_err(str::to_owned)
}

impl NativeSession {
    /// Rebuild a session from an already approved token and its library, e.g.
    /// after the host paired through its own UI. Validation matches
    /// [`PassportClient::session`] except for the network round trip.
    pub fn from_parts(
        api: PassportClient,
        token: String,
        library: PurchasedLibrary,
    ) -> Result<Self, String> {
        if library.schema != LIBRARY_SCHEMA {
            return Err("Passport library has an unknown schema".into());
        }
        for avatar in &library.items {
            validate_avatar_id(&avatar.avatar_id)?;
            if !valid_base58_key(&avatar.mint) {
                return Err("Invalid owned NFT mint".into());
            }
        }
        Ok(Self { api, token, library })
    }

    pub fn wallet(&self) -> &str {
        &self.library.wallet
    }

    /// Owned avatars that carry an approval matching `selector`.
    pub fn supported(&self, selector: &SupportSelector) -> Vec<ProtectedAvatar> {
        self.library
            .items
            .iter()
            .flat_map(|avatar| {
                avatar
                    .support
                    .iter()
                    .filter(|support| validate_project_support(support, selector).is_ok())
                    .map(|support| ProtectedAvatar {
                        avatar_id: avatar.avatar_id.clone(),
                        support: support.clone(),
                    })
            })
            .collect()
    }

    pub fn owns(&self, protected: &ProtectedAvatar) -> Option<&PurchasedAvatar> {
        self.library.items.iter().find(|avatar| {
            avatar.avatar_id == protected.avatar_id
                && avatar
                    .support
                    .iter()
                    .any(|support| support == &protected.support)
        })
    }

    /// Request a one-use, project- and session-bound ticket for an owned avatar.
    pub fn ticket(
        &self,
        protected: &ProtectedAvatar,
        selector: &SupportSelector,
        session_id: &str,
    ) -> Result<AvatarTicket, String> {
        protected.validate_for(selector)?;
        let avatar = self
            .owns(protected)
            .ok_or("This wallet does not own this supported avatar")?;
        if !valid_session_id(session_id) {
            return Err("Invalid game session".into());
        }
        let response: AvatarTicket = self.api.request(
            "ticket",
            Some(&self.token),
            Some(json!({
                "projectId": self.api.project_id, "avatarId": avatar.avatar_id,
                "mint": avatar.mint, "sessionId": session_id,
            })),
        )?;
        if response.avatar.avatar_id != protected.avatar_id
            || response.avatar.mint != avatar.mint
            || response.support != protected.support
            || response.ticket.len() < 16
            || response.ticket.len() > 4096
        {
            return Err("Passport ticket does not match the selected avatar".into());
        }
        Ok(response)
    }
}

/// Terminal pairing loop for tools and first integrations. Prints only the
/// public user code and verification link.
pub fn pair_interactively(api: PassportClient) -> Result<NativeSession, String> {
    let pairing = api.pair()?;
    eprintln!("Connect your wallet: {}", pairing.verification_url);
    eprintln!(
        "Approval code: {}. Waiting for browser approval (expires {}).",
        pairing.user_code, pairing.expires_at
    );
    let started = std::time::Instant::now();
    loop {
        if started.elapsed() > Duration::from_secs(600) {
            return Err("Pairing expired. Retry.".into());
        }
        std::thread::sleep(Duration::from_secs(pairing.interval.clamp(3, 10)));
        match api.poll(&pairing.device_code)? {
            PairingPoll::Pending => {}
            PairingPoll::Approved {
                access_token,
                wallet,
                ..
            } => {
                return api.session(access_token, &wallet);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn client_requires_passport_path_and_project() {
        assert!(PassportClient::new("https://avatar.ekza.io/api/passport", "omoba").is_ok());
        assert!(PassportClient::new("http://127.0.0.1:5190/api/passport", "omoba").is_ok());
        assert!(PassportClient::new("http://avatar.ekza.io/api/passport", "omoba").is_err());
        assert!(PassportClient::new("https://avatar.ekza.io/api", "omoba").is_err());
        assert!(PassportClient::new("https://avatar.ekza.io/api/passport", "").is_err());
    }

    #[test]
    fn session_ids_are_bounded_tokens() {
        assert!(valid_session_id("match-42.a_b"));
        assert!(!valid_session_id(""));
        assert!(!valid_session_id("has space"));
        assert!(!valid_session_id(&"x".repeat(65)));
    }
}
