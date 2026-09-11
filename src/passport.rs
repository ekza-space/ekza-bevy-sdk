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

pub fn validate_avatar_id(id: &str) -> Result<(), &'static str> {
    match id.strip_prefix("solana:devnet:avatar-data:") {
        Some(key) if valid_base58_key(key) => Ok(()),
        _ => Err("Invalid canonical devnet avatar identity"),
    }
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

impl ProtectedAvatar {
    pub fn validate(&self) -> Result<(), &'static str> {
        validate_avatar_id(&self.avatar_id)?;
        validate_omoba_support(&self.support)
    }

    /// The caller supplies SHA-256 from a trusted local implementation. Metadata
    /// equality alone never satisfies the byte integrity boundary.
    pub fn validate_bytes(&self, bytes: &[u8], actual_sha256: &str) -> Result<(), &'static str> {
        self.validate()?;
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

#[cfg(test)]
mod tests {
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
