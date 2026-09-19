//! Blocking HTTP clients for the public Ekza avatar feeds (`http` feature).
//!
//! These clients only read public, unauthenticated catalogues. Ownership,
//! pairing and tickets live in [`crate::passport::client`].

use std::{io::Read, time::Duration};

use reqwest::{Url, blocking::Client, redirect::Policy};
use serde::de::DeserializeOwned;

use crate::catalog::{
    CatalogV2Avatar, CatalogV2Response, EkzaAvatar, LibraryPage, PassportCatalog,
    RegistryCatalogResponse, RegistryResolution, merge_avatars,
};

pub const DEFAULT_REGISTRY_URL: &str = "https://registry.ekza.io";
pub const DEFAULT_PASSPORT_URL: &str = "https://avatar.ekza.io/api/passport";
pub const LIBRARY_PAGE_SIZE: u64 = 100;
const MAX_JSON_BYTES: u64 = 8 * 1024 * 1024;

#[derive(Debug)]
pub enum RegistryError {
    InvalidUrl(String),
    Transport(String),
    Status { status: u16, url: String },
    TooLarge { url: String, limit: u64 },
    Decode { url: String, detail: String },
}

impl std::fmt::Display for RegistryError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidUrl(url) => write!(f, "invalid Ekza feed URL: {url}"),
            Self::Transport(detail) => write!(f, "Ekza feed request failed: {detail}"),
            Self::Status { status, url } => write!(f, "Ekza feed {url} answered HTTP {status}"),
            Self::TooLarge { url, limit } => {
                write!(f, "Ekza feed {url} exceeded the {limit} byte JSON limit")
            }
            Self::Decode { url, detail } => {
                write!(
                    f,
                    "Ekza feed {url} returned an unexpected document: {detail}"
                )
            }
        }
    }
}

impl std::error::Error for RegistryError {}

/// Accepts `https://` anywhere and plain `http://` only for loopback hosts, so
/// a mistyped production URL can never downgrade to cleartext.
pub fn feed_url(raw: &str) -> Result<Url, RegistryError> {
    let url = Url::parse(raw).map_err(|_| RegistryError::InvalidUrl(raw.to_string()))?;
    let loopback = matches!(
        url.host_str(),
        Some("localhost" | "127.0.0.1" | "[::1]" | "0.0.0.0")
    );
    if !(url.scheme() == "https" || (url.scheme() == "http" && loopback))
        || !url.username().is_empty()
        || url.password().is_some()
    {
        return Err(RegistryError::InvalidUrl(raw.to_string()));
    }
    Ok(url)
}

pub(crate) fn build_client(timeout: Duration) -> Result<Client, RegistryError> {
    Client::builder()
        .redirect(Policy::limited(3))
        .connect_timeout(Duration::from_secs(10))
        .timeout(timeout)
        .user_agent(concat!("ekza-bevy-sdk/", env!("CARGO_PKG_VERSION")))
        .build()
        .map_err(|error| RegistryError::Transport(error.to_string()))
}

pub(crate) fn get_json<T: DeserializeOwned>(client: &Client, url: Url) -> Result<T, RegistryError> {
    let display = url.to_string();
    let response = client
        .get(url)
        .header("accept", "application/json")
        .send()
        .map_err(|error| RegistryError::Transport(format!("{display}: {error}")))?;
    let status = response.status();
    let mut bytes = Vec::new();
    response
        .take(MAX_JSON_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| RegistryError::Transport(format!("{display}: {error}")))?;
    if bytes.len() as u64 > MAX_JSON_BYTES {
        return Err(RegistryError::TooLarge {
            url: display,
            limit: MAX_JSON_BYTES,
        });
    }
    if !status.is_success() {
        return Err(RegistryError::Status {
            status: status.as_u16(),
            url: display,
        });
    }
    serde_json::from_slice(&bytes).map_err(|error| RegistryError::Decode {
        url: display,
        detail: error.to_string(),
    })
}

/// Client for the registry (`ekza-mirror` backend): free library and the
/// approved devnet catalogue.
#[derive(Clone)]
pub struct RegistryClient {
    base: Url,
    client: Client,
}

impl RegistryClient {
    pub fn new(base_url: &str) -> Result<Self, RegistryError> {
        let base = feed_url(base_url)?;
        Ok(Self {
            base,
            client: build_client(Duration::from_secs(30))?,
        })
    }

    pub fn default_registry() -> Result<Self, RegistryError> {
        Self::new(DEFAULT_REGISTRY_URL)
    }

    pub fn base_url(&self) -> &str {
        self.base.as_str().trim_end_matches('/')
    }

    fn endpoint(&self, path: &str, query: &[(&str, String)]) -> Result<Url, RegistryError> {
        let mut url = self
            .base
            .join(path.trim_start_matches('/'))
            .map_err(|_| RegistryError::InvalidUrl(path.to_string()))?;
        // Preserve a base path prefix such as `https://wss.ekza.io/mirror`.
        if !self.base.path().trim_end_matches('/').is_empty() {
            let joined = format!(
                "{}/{}",
                self.base.path().trim_end_matches('/'),
                path.trim_start_matches('/')
            );
            url.set_path(&joined);
        }
        if !query.is_empty() {
            let mut pairs = url.query_pairs_mut();
            for (key, value) in query {
                pairs.append_pair(key, value);
            }
        }
        Ok(url)
    }

    /// One page of the free library.
    pub fn library_page(&self, limit: u64, offset: u64) -> Result<LibraryPage, RegistryError> {
        let url = self.endpoint(
            "library/avatars",
            &[
                ("limit", limit.clamp(1, LIBRARY_PAGE_SIZE).to_string()),
                ("offset", offset.to_string()),
            ],
        )?;
        get_json(&self.client, url)
    }

    /// Every free library avatar, paginated transparently. `max` bounds the
    /// total number of records fetched (`None` = whole library).
    pub fn library(&self, max: Option<u64>) -> Result<Vec<EkzaAvatar>, RegistryError> {
        let mut avatars = Vec::new();
        let mut offset = 0;
        loop {
            let remaining = max.map(|max| max.saturating_sub(avatars.len() as u64));
            if remaining == Some(0) {
                break;
            }
            let limit = remaining.map_or(LIBRARY_PAGE_SIZE, |r| r.min(LIBRARY_PAGE_SIZE));
            let page = self.library_page(limit, offset)?;
            let fetched = page.items.len() as u64;
            avatars.extend(page.items.into_iter().map(EkzaAvatar::from));
            offset += fetched;
            if fetched == 0 || offset >= page.total {
                break;
            }
        }
        Ok(avatars)
    }

    /// Approved catalogue, optionally narrowed to one platform/profile selector.
    pub fn catalog(
        &self,
        selector: Option<(&str, &str)>,
    ) -> Result<Vec<EkzaAvatar>, RegistryError> {
        let query: Vec<(&str, String)> = selector
            .map(|(platform, profile)| {
                vec![
                    ("platform", platform.to_string()),
                    ("profile", profile.to_string()),
                ]
            })
            .unwrap_or_default();
        let url = self.endpoint("v1/avatars", &query)?;
        let response: RegistryCatalogResponse = get_json(&self.client, url)?;
        let base = self.base_url().to_string();
        Ok(response
            .avatars
            .into_iter()
            .map(|avatar| avatar.into_avatar(&base))
            .collect())
    }

    /// The unified catalogue: on-chain templates and Ekza Studio avatars. With
    /// `project`, only avatars that project approved for the selected rendition
    /// are returned. A registry that predates `/v2/avatars` answers 404.
    pub fn catalog_v2(
        &self,
        project: Option<&str>,
        selector: Option<(&str, &str)>,
    ) -> Result<Vec<CatalogV2Avatar>, RegistryError> {
        let mut query: Vec<(&str, String)> = Vec::new();
        if let Some(project) = project {
            query.push(("project", project.to_string()));
        }
        if let Some((platform, profile)) = selector {
            query.push(("platform", platform.to_string()));
            query.push(("profile", profile.to_string()));
        }
        let url = self.endpoint("v2/avatars", &query)?;
        let response: CatalogV2Response = get_json(&self.client, url)?;
        Ok(response.items)
    }

    /// Exact rendition resolution for one avatar.
    pub fn resolve(
        &self,
        avatar_id: &str,
        platform: &str,
        profile: &str,
    ) -> Result<RegistryResolution, RegistryError> {
        let url = self.endpoint(
            &format!("v1/avatars/{avatar_id}/resolve"),
            &[
                ("platform", platform.to_string()),
                ("profile", profile.to_string()),
            ],
        )?;
        get_json(&self.client, url)
    }

    /// Library plus approved catalogue, merged by identity.
    pub fn all_avatars(&self, library_max: Option<u64>) -> Result<Vec<EkzaAvatar>, RegistryError> {
        let catalog = self.catalog(None)?;
        let library = self.library(library_max)?;
        Ok(merge_avatars([catalog, library]))
    }
}

/// Public (unauthenticated) storefront catalogue reader.
#[derive(Clone)]
pub struct PassportCatalogClient {
    base: Url,
    client: Client,
}

impl PassportCatalogClient {
    /// `base_url` ends in `/api/passport`.
    pub fn new(base_url: &str) -> Result<Self, RegistryError> {
        let base = feed_url(base_url)?;
        if !base.path().trim_end_matches('/').ends_with("/api/passport") {
            return Err(RegistryError::InvalidUrl(format!(
                "{base_url} (expected a path ending in /api/passport)"
            )));
        }
        Ok(Self {
            base,
            client: build_client(Duration::from_secs(30))?,
        })
    }

    pub fn default_storefront() -> Result<Self, RegistryError> {
        Self::new(DEFAULT_PASSPORT_URL)
    }

    pub fn catalog(&self) -> Result<Vec<EkzaAvatar>, RegistryError> {
        let url = Url::parse(&format!(
            "{}/catalog",
            self.base.as_str().trim_end_matches('/')
        ))
        .map_err(|_| RegistryError::InvalidUrl(self.base.to_string()))?;
        let catalog: PassportCatalog = get_json(&self.client, url)?;
        Ok(catalog.items.into_iter().map(EkzaAvatar::from).collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn feed_urls_reject_cleartext_and_credentials() {
        assert!(feed_url("https://registry.ekza.io").is_ok());
        assert!(feed_url("http://127.0.0.1:8080").is_ok());
        assert!(feed_url("http://registry.ekza.io").is_err());
        assert!(feed_url("https://user:pw@registry.ekza.io").is_err());
        assert!(feed_url("registry.ekza.io").is_err());
    }

    #[test]
    fn endpoints_keep_base_path_prefixes() {
        let client = RegistryClient::new("https://wss.ekza.io/mirror").unwrap();
        let url = client
            .endpoint("library/avatars", &[("limit", "5".into())])
            .unwrap();
        assert_eq!(
            url.as_str(),
            "https://wss.ekza.io/mirror/library/avatars?limit=5"
        );
        let root = RegistryClient::new("https://registry.ekza.io").unwrap();
        assert_eq!(
            root.endpoint("v1/avatars", &[]).unwrap().as_str(),
            "https://registry.ekza.io/v1/avatars"
        );
    }

    #[test]
    fn passport_client_requires_passport_path() {
        assert!(PassportCatalogClient::new("https://avatar.ekza.io").is_err());
        assert!(PassportCatalogClient::new("https://avatar.ekza.io/api/passport").is_ok());
    }
}
