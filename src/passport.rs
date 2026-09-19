//! Chain-independent delivery contract for purchased Ekza avatars.
//!
//! A catalogue identity describes the template; `mint` identifies the owned
//! instance. The host verifies ownership with its trusted passport service.
//! These types never turn a compatible file format into an entitlement.

use serde::{Deserialize, Serialize};

pub const OMOBA_PROJECT: &str = "omoba";
pub const MAX_RENDITION_BYTES: u64 = 50 * 1024 * 1024;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Rendition {
    pub id: String,
    pub url: String,
    pub sha256: String,
    pub size_bytes: u64,
    pub format: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectSupport {
    pub project_id: String,
    pub platform: String,
    pub profile: String,
    pub status: String,
    pub rendition: Rendition,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PurchasedAvatar {
    pub avatar_id: String,
    pub mint: String,
    pub name: String,
    pub thumbnail_url: String,
    pub support: Vec<ProjectSupport>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PurchasedLibrary {
    pub schema: String,
    pub network: String,
    pub wallet: String,
    pub expires_at: String,
    pub items: Vec<PurchasedAvatar>,
}

/// Public roster metadata. Never contains a bearer token or a buyer's mint.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProtectedAvatar {
    pub avatar_id: String,
    pub support: ProjectSupport,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConsumedTicket {
    pub wallet: String,
    pub avatar_id: String,
    pub mint: String,
    pub expires_at: String,
    pub support: ProjectSupport,
}

pub fn valid_base58_key(value: &str) -> bool {
    (32..=44).contains(&value.len())
        && value.bytes().all(|byte| matches!(byte, b'1'..=b'9' | b'A'..=b'H' | b'J'..=b'N' | b'P'..=b'Z' | b'a'..=b'k' | b'm'..=b'z'))
}

/// A canonical avatar identity. Two schemes exist: an on-chain template
/// (`solana:devnet:avatar-data:<base58 PDA>`) and an avatar published through Ekza
/// Studio (`ekza:avatar:<uuid>`), which has no chain record at all.
pub fn validate_avatar_id(id: &str) -> Result<(), &'static str> {
    if let Some(key) = id.strip_prefix("solana:devnet:avatar-data:") {
        return if valid_base58_key(key) {
            Ok(())
        } else {
            Err("Invalid canonical devnet avatar identity")
        };
    }
    match id.strip_prefix("ekza:avatar:") {
        Some(uuid) if valid_uuid(uuid) => Ok(()),
        _ => Err("Invalid canonical avatar identity"),
    }
}

/// Lowercase canonical UUID text (8-4-4-4-12), as PostgreSQL prints it.
pub fn valid_uuid(value: &str) -> bool {
    value.len() == 36
        && value.bytes().enumerate().all(|(index, byte)| match index {
            8 | 13 | 18 | 23 => byte == b'-',
            _ => byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte),
        })
}

/// Which approval a consumer is willing to accept: its own project id plus the
/// exact platform/profile selector its runtime can load.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SupportSelector {
    pub project_id: String,
    pub platform: String,
    pub profile: String,
    /// Accepted rendition formats (`glb`, `vrm`, ...).
    pub formats: Vec<String>,
}

impl SupportSelector {
    pub fn new(project_id: &str, platform: &str, profile: &str, formats: &[&str]) -> Self {
        Self {
            project_id: project_id.into(),
            platform: platform.into(),
            profile: profile.into(),
            formats: formats.iter().map(|format| format.to_string()).collect(),
        }
    }

    /// Omoba desktop: `desktop` / `humanoid-glb-v1`, binary glTF only.
    pub fn omoba_desktop() -> Self {
        Self::new(OMOBA_PROJECT, "desktop", "humanoid-glb-v1", &["glb", "vrm"])
    }
}

/// Generic approval check: explicit project approval, exact selector, an
/// accepted format and a well-formed rendition hash/size.
pub fn validate_project_support(
    support: &ProjectSupport,
    selector: &SupportSelector,
) -> Result<(), &'static str> {
    let rendition = &support.rendition;
    if support.project_id != selector.project_id || support.status != "approved" {
        return Err("Avatar is not explicitly approved for this project");
    }
    if support.platform != selector.platform
        || support.profile != selector.profile
        || rendition.id.is_empty()
    {
        return Err("Unsupported rendition compatibility profile");
    }
    if !selector
        .formats
        .iter()
        .any(|format| format == &rendition.format)
    {
        return Err("Rendition format is not accepted by this project");
    }
    if !(20..=MAX_RENDITION_BYTES).contains(&rendition.size_bytes)
        || rendition.sha256.len() != 64
        || !rendition
            .sha256
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
    {
        return Err("Invalid approved rendition size or SHA-256");
    }
    Ok(())
}

pub fn validate_omoba_support(support: &ProjectSupport) -> Result<(), &'static str> {
    let rendition = &support.rendition;
    if support.project_id != OMOBA_PROJECT || support.status != "approved" {
        return Err("Avatar is not explicitly approved for Omoba");
    }
    if support.platform != "desktop"
        || support.profile != "humanoid-glb-v1"
        || rendition.id.is_empty()
    {
        return Err("Unsupported Omoba rendition compatibility profile");
    }
    if rendition.format != "glb" && rendition.format != "vrm" {
        return Err("Omoba requires a binary glTF rendition");
    }
    validate_project_support(support, &SupportSelector::omoba_desktop())
        .map_err(|_| "Invalid approved rendition size or SHA-256")
}

/// Protocol and filesystem slug of a paid avatar: `ekza-<sha256 hex>` over the
/// canonical identity and the exact rendition hash. Two templates sharing a
/// geometry never collapse into one entry, and a new rendition revision gets a
/// new slug. Every consumer derives the same value from public catalogue data,
/// so a game server can recompute it from a consumed ticket alone.
pub fn protected_slug(protected: &ProtectedAvatar) -> String {
    let key = format!(
        "{}\n{}",
        protected.avatar_id, protected.support.rendition.sha256
    );
    format!("ekza-{}", crate::sha256::sha256_hex(key.as_bytes()))
}

/// True for a well-formed [`protected_slug`] value.
pub fn is_protected_slug(slug: &str) -> bool {
    slug.strip_prefix("ekza-").is_some_and(|hex| {
        hex.len() == 64
            && hex
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
    })
}

impl ProtectedAvatar {
    /// Omoba desktop validation (kept for the first consumer).
    pub fn validate(&self) -> Result<(), &'static str> {
        validate_avatar_id(&self.avatar_id)?;
        validate_omoba_support(&self.support)
    }

    /// Validation against any consumer's own selector.
    pub fn validate_for(&self, selector: &SupportSelector) -> Result<(), &'static str> {
        validate_avatar_id(&self.avatar_id)?;
        validate_project_support(&self.support, selector)
    }

    /// The caller supplies SHA-256 from a trusted local implementation. Metadata
    /// equality alone never satisfies the byte integrity boundary.
    pub fn validate_bytes(&self, bytes: &[u8], actual_sha256: &str) -> Result<(), &'static str> {
        self.validate()?;
        self.check_bytes(bytes, actual_sha256)
    }

    /// Byte check against any consumer selector.
    pub fn validate_bytes_for(
        &self,
        selector: &SupportSelector,
        bytes: &[u8],
        actual_sha256: &str,
    ) -> Result<(), &'static str> {
        self.validate_for(selector)?;
        self.check_bytes(bytes, actual_sha256)
    }

    fn check_bytes(&self, bytes: &[u8], actual_sha256: &str) -> Result<(), &'static str> {
        if bytes.len() as u64 != self.support.rendition.size_bytes
            || actual_sha256 != self.support.rendition.sha256
        {
            return Err("Avatar rendition failed its size or SHA-256 check");
        }
        if bytes.len() < 12
            || u32::from_le_bytes(bytes[8..12].try_into().unwrap()) as usize != bytes.len()
            || !crate::is_valid_glb_bytes(bytes)
        {
            return Err("Avatar rendition is not a valid binary glTF");
        }
        Ok(())
    }

    /// Only invoke after consuming a ticket at the operator-configured origin.
    pub fn validate_consumed_ticket(&self, ticket: &ConsumedTicket) -> Result<(), &'static str> {
        self.validate()?;
        if ticket.avatar_id != self.avatar_id
            || ticket.support != self.support
            || !valid_base58_key(&ticket.wallet)
            || !valid_base58_key(&ticket.mint)
        {
            return Err("Ticket does not grant this exact approved avatar rendition");
        }
        Ok(())
    }
}

#[cfg(feature = "http")]
pub mod client;
#[cfg(feature = "http")]
pub mod pairing;

#[cfg(test)]
mod tests {
    #[test]
    fn both_identity_schemes_are_accepted_and_nothing_else() {
        assert!(
            super::validate_avatar_id(&format!("solana:devnet:avatar-data:{}", "1".repeat(32)))
                .is_ok()
        );
        assert!(
            super::validate_avatar_id("ekza:avatar:2f0c1f0e-7b1a-4c55-9d53-0a6d3c1b9e77").is_ok()
        );
        for bad in [
            "ekza:avatar:2F0C1F0E-7B1A-4C55-9D53-0A6D3C1B9E77",
            "ekza:avatar:2f0c1f0e7b1a4c559d530a6d3c1b9e77",
            "ekza:avatar:2f0c1f0e-7b1a-4c55-9d53-0a6d3c1b9e7",
            "ekza:avatar:../../etc/passwd-0000-0000-000000000",
            "ekza:avatar:",
            "ekza:space:2f0c1f0e-7b1a-4c55-9d53-0a6d3c1b9e77",
            "solana:mainnet:avatar-data:11111111111111111111111111111111",
            "",
        ] {
            assert!(super::validate_avatar_id(bad).is_err(), "{bad:?}");
        }
    }

    use super::*;

    fn protected() -> ProtectedAvatar {
        ProtectedAvatar {
            avatar_id: format!("solana:devnet:avatar-data:{}", "1".repeat(32)),
            support: ProjectSupport {
                project_id: "omoba".into(),
                platform: "desktop".into(),
                profile: "humanoid-glb-v1".into(),
                status: "approved".into(),
                rendition: Rendition {
                    id: "revision-1".into(),
                    url: "https://example.test/a.glb".into(),
                    sha256: "a".repeat(64),
                    size_bytes: 20,
                    format: "glb".into(),
                },
            },
        }
    }

    #[test]
    fn consumer_approval_is_required_in_addition_to_format() {
        let mut asset = protected();
        assert!(asset.validate().is_ok());
        asset.support.status = "pending".into();
        assert!(asset.validate().is_err());
        asset.support.status = "approved".into();
        asset.support.project_id = "ekza-space".into();
        assert!(asset.validate().is_err());
    }

    #[test]
    fn generic_selector_matches_any_project() {
        let mut asset = protected();
        assert!(
            asset
                .validate_for(&SupportSelector::omoba_desktop())
                .is_ok()
        );
        let space = SupportSelector::new("ekza-space", "universal", "vrm-humanoid-v0", &["vrm0"]);
        assert!(asset.validate_for(&space).is_err());
        asset.support.project_id = "ekza-space".into();
        asset.support.platform = "universal".into();
        asset.support.profile = "vrm-humanoid-v0".into();
        asset.support.rendition.format = "vrm0".into();
        assert!(asset.validate_for(&space).is_ok());
        assert!(asset.validate().is_err());
    }

    #[test]
    fn copied_or_other_network_identity_is_invalid() {
        assert!(validate_avatar_id("https://example.test/same-file.glb").is_err());
        assert!(
            validate_avatar_id(&format!("solana:mainnet:avatar-data:{}", "1".repeat(32))).is_err()
        );
    }

    #[test]
    fn exact_rendition_and_actual_bytes_must_match() {
        let asset = protected();
        assert!(asset.validate_bytes(b"bad", &"a".repeat(64)).is_err());
        let mut ticket = ConsumedTicket {
            wallet: "2".repeat(32),
            mint: "3".repeat(32),
            expires_at: "future".into(),
            avatar_id: asset.avatar_id.clone(),
            support: asset.support.clone(),
        };
        assert!(asset.validate_consumed_ticket(&ticket).is_ok());
        ticket.support.rendition.sha256 = "b".repeat(64);
        assert!(asset.validate_consumed_ticket(&ticket).is_err());
    }
}
