//! Engine-agnostic avatar catalogue model.
//!
//! Ekza publishes avatars through three public feeds. All three are normalized
//! into one [`EkzaAvatar`] so a game only ever deals with a single shape:
//!
//! | Feed | Endpoint | What it lists |
//! | --- | --- | --- |
//! | Library | `GET {registry}/library/avatars` | Free CC0 avatars (VRM on IPFS). |
//! | Registry | `GET {registry}/v1/avatars` | Devnet-minted templates with exact, hashed per-platform renditions and per-project approvals. |
//! | Passport | `GET {storefront}/api/passport/catalog` | Purchasable templates and their approved game renditions. |
//!
//! Nothing here performs I/O; see [`crate::registry`] for the HTTP client and
//! [`crate::cache`] for verified downloads (both behind the `http` feature).

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::passport::{ProjectSupport as PassportProjectSupport, PurchasedAvatar};

/// Where an avatar record came from. Identity rules differ per origin, so the
/// origin is kept next to the raw `id` instead of being encoded into it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum AvatarOrigin {
    /// Free library entry; `id` is the library UUID.
    Library,
    /// Approved registry template; `id` is `solana:<cluster>:avatar-data:<PDA>`.
    Registry,
    /// Storefront passport catalogue; `id` is the same canonical avatar id.
    Passport,
    /// Locally staged by the consumer (shipped roster, hand-imported model).
    Local,
}

/// One downloadable file of an avatar for a given platform/profile pair.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AvatarRendition {
    /// Stable rendition id (`sha256:<hex>` for registry renditions, a CID for
    /// library models, the storefront rendition id for passport entries).
    pub id: String,
    /// `glb`, `vrm0`, `vrm1`, `vrm`, `usdz`, ...
    pub format: String,
    /// `universal`, `desktop`, `ios`, ...
    pub platform: String,
    /// Compatibility profile such as `vrm-humanoid-v0` or `humanoid-glb-v1`.
    pub profile: String,
    /// Absolute download URL (registry-relative `assetPath` values are resolved
    /// against the registry base URL by the client).
    pub url: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sha256: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size_bytes: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub media_type: Option<String>,
}

impl AvatarRendition {
    /// True for every format Bevy's glTF loader can open (VRM is a GLB container).
    pub fn is_gltf_binary(&self) -> bool {
        matches!(self.format.as_str(), "glb" | "vrm" | "vrm0" | "vrm1")
    }

    /// File extension a consumer should stage this rendition under.
    pub fn file_extension(&self) -> &'static str {
        match self.format.as_str() {
            "glb" => "glb",
            "vrm" | "vrm0" | "vrm1" => "vrm",
            "usdz" => "usdz",
            "gltf" => "gltf",
            _ => "bin",
        }
    }
}

/// Operator approval of one rendition selector for one project.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectApproval {
    pub project_id: String,
    pub platform: String,
    pub profile: String,
    pub status: String,
}

/// Unified avatar record shared by every feed.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EkzaAvatar {
    pub origin: AvatarOrigin,
    /// Raw feed identity (see [`AvatarOrigin`]).
    pub id: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub collection: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub author: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub license: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thumbnail_url: Option<String>,
    /// Original creator source (arweave/ipfs URL) when the feed exposes it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_url: Option<String>,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub renditions: Vec<AvatarRendition>,
    #[serde(default)]
    pub project_support: Vec<ProjectApproval>,
}

impl EkzaAvatar {
    /// Exact rendition for a platform/profile selector.
    pub fn rendition(&self, platform: &str, profile: &str) -> Option<&AvatarRendition> {
        self.renditions
            .iter()
            .find(|rendition| rendition.platform == platform && rendition.profile == profile)
    }

    /// Rendition explicitly approved for `project_id`, if the feed carries one.
    pub fn approved_rendition(&self, project_id: &str) -> Option<&AvatarRendition> {
        self.project_support
            .iter()
            .filter(|support| support.project_id == project_id && support.status == "approved")
            .find_map(|support| self.rendition(&support.platform, &support.profile))
    }

    /// Best rendition for a glTF-based engine: an explicit project approval
    /// first, then the requested selector, then any binary glTF file.
    pub fn gltf_rendition(
        &self,
        project_id: Option<&str>,
        platform: &str,
        profile: &str,
    ) -> Option<&AvatarRendition> {
        project_id
            .and_then(|project| self.approved_rendition(project))
            .filter(|rendition| rendition.is_gltf_binary())
            .or_else(|| self.rendition(platform, profile))
            .filter(|rendition| rendition.is_gltf_binary())
            .or_else(|| {
                self.renditions
                    .iter()
                    .find(|rendition| rendition.is_gltf_binary())
            })
    }

    pub fn is_approved_for(&self, project_id: &str) -> bool {
        self.approved_rendition(project_id).is_some()
    }

    /// Deterministic filesystem/protocol slug: kebab-case of the name, with a
    /// short identity hash appended so two avatars sharing a name never collide.
    pub fn slug(&self) -> String {
        let base = slugify(&self.name);
        let suffix = short_hash(&self.id);
        if base.is_empty() {
            format!("ekza-{suffix}")
        } else {
            format!("{base}-{suffix}")
        }
    }
}

/// Lowercase ASCII kebab-case, non-alphanumerics collapsed to single dashes.
pub fn slugify(value: &str) -> String {
    let mut slug = String::with_capacity(value.len());
    let mut pending_dash = false;
    for ch in value.chars() {
        if ch.is_ascii_alphanumeric() {
            if pending_dash && !slug.is_empty() {
                slug.push('-');
            }
            pending_dash = false;
            slug.push(ch.to_ascii_lowercase());
        } else {
            // Separators and non-ASCII letters: keep a dash boundary, drop the char.
            pending_dash = true;
        }
    }
    slug.truncate(48);
    while slug.ends_with('-') {
        slug.pop();
    }
    slug
}

/// FNV-1a 64-bit hex prefix; stable, dependency-free, and good enough to keep
/// slugs unique across a few thousand catalogue entries.
pub fn short_hash(value: &str) -> String {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in value.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{:06x}", hash & 0xff_ffff)
}

// --- Library feed (`/library/avatars`) ---------------------------------------

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LibraryAvatar {
    pub id: String,
    #[serde(default)]
    pub safe_id: Option<String>,
    pub name: String,
    #[serde(default)]
    pub collection_id: Option<String>,
    #[serde(default)]
    pub collection_name: Option<String>,
    #[serde(default)]
    pub author: Option<String>,
    #[serde(default)]
    pub license: Option<String>,
    #[serde(default)]
    pub format: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub model_cid: Option<String>,
    #[serde(default)]
    pub thumbnail_cid: Option<String>,
    #[serde(default)]
    pub usdz_cid: Option<String>,
    #[serde(default)]
    pub model_size_bytes: Option<u64>,
    #[serde(default)]
    pub thumbnail_size_bytes: Option<u64>,
    #[serde(default)]
    pub usdz_size_bytes: Option<u64>,
    #[serde(default)]
    pub usdz_sha256: Option<String>,
    #[serde(default)]
    pub model_url: Option<String>,
    #[serde(default)]
    pub thumbnail_url: Option<String>,
    #[serde(default)]
    pub usdz_url: Option<String>,
    #[serde(default)]
    pub starter_pack: bool,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LibraryPage {
    pub items: Vec<LibraryAvatar>,
    pub total: u64,
    pub limit: u64,
    pub offset: u64,
}

impl From<LibraryAvatar> for EkzaAvatar {
    fn from(item: LibraryAvatar) -> Self {
        let mut renditions = Vec::new();
        let format = item
            .format
            .as_deref()
            .map(str::to_ascii_lowercase)
            .unwrap_or_else(|| "vrm".to_string());
        if let (Some(cid), Some(url)) = (&item.model_cid, &item.model_url) {
            renditions.push(AvatarRendition {
                id: cid.clone(),
                format: format.clone(),
                platform: "universal".into(),
                profile: if format.starts_with("vrm") {
                    "vrm-humanoid-v0".into()
                } else {
                    "gltf-v2".into()
                },
                url: url.clone(),
                sha256: None,
                size_bytes: item.model_size_bytes,
                media_type: None,
            });
        }
        if let (Some(cid), Some(url)) = (&item.usdz_cid, &item.usdz_url) {
            renditions.push(AvatarRendition {
                id: cid.clone(),
                format: "usdz".into(),
                platform: "ios".into(),
                profile: "arkit-body-v1".into(),
                url: url.clone(),
                sha256: item.usdz_sha256.clone(),
                size_bytes: item.usdz_size_bytes,
                media_type: Some("model/vnd.usdz+zip".into()),
            });
        }
        let mut tags = item.tags;
        if item.starter_pack {
            tags.push("starter-pack".into());
        }
        EkzaAvatar {
            origin: AvatarOrigin::Library,
            id: item.id,
            name: item.name,
            collection: item.collection_name.or(item.collection_id),
            author: item.author,
            license: item.license,
            description: item.description,
            thumbnail_url: item.thumbnail_url,
            source_url: None,
            tags,
            renditions,
            project_support: Vec::new(),
        }
    }
}

// --- Registry feed (`/v1/avatars`) --------------------------------------------

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RegistryLicense {
    pub spdx: String,
    #[serde(default)]
    pub attribution: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RegistryProvenance {
    #[serde(default)]
    pub network: String,
    #[serde(default)]
    pub program: String,
    #[serde(default)]
    pub creator: String,
    #[serde(default)]
    pub avatar_data_pda: String,
    #[serde(default)]
    pub avatar_data_index: u64,
    #[serde(default)]
    pub metadata_uri: String,
    #[serde(default)]
    pub metadata_sha256: String,
    #[serde(default)]
    pub source_model_uri: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RegistryRendition {
    pub id: String,
    pub platform: String,
    pub format: String,
    #[serde(default)]
    pub media_type: Option<String>,
    pub profile: String,
    #[serde(default)]
    pub canonical_uri: Option<String>,
    /// Absolute HTTPS URL (public responses always carry it).
    #[serde(default)]
    pub download_url: Option<String>,
    /// Registry-relative path (source catalogue files); resolved by the client.
    #[serde(default)]
    pub asset_path: Option<String>,
    pub sha256: String,
    pub size_bytes: u64,
    #[serde(default)]
    pub min_os_version: Option<String>,
    #[serde(default)]
    pub status: Option<String>,
    #[serde(default)]
    pub rig_fit: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RegistryAvatar {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub thumbnail_url: Option<String>,
    #[serde(default)]
    pub license: Option<RegistryLicense>,
    #[serde(default)]
    pub provenance: Option<RegistryProvenance>,
    #[serde(default)]
    pub renditions: Vec<RegistryRendition>,
    #[serde(default)]
    pub project_support: Vec<ProjectApproval>,
}

/// `GET /v1/avatars` response.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RegistryCatalogResponse {
    #[serde(default)]
    pub schema: String,
    #[serde(default)]
    pub catalog_revision: String,
    #[serde(default)]
    pub count: u64,
    pub avatars: Vec<RegistryAvatar>,
}

/// Source catalogue document (`avatars.devnet.json`), also accepted so a game
/// can ship an offline copy of the registry.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RegistryCatalogDocument {
    #[serde(default)]
    pub schema: String,
    #[serde(default)]
    pub network: String,
    #[serde(default)]
    pub program: String,
    pub items: Vec<RegistryAvatar>,
}

/// `GET /v1/avatars/{id}/resolve` response.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RegistryResolution {
    #[serde(default)]
    pub schema: String,
    #[serde(default)]
    pub catalog_revision: String,
    pub avatar: RegistryAvatar,
    pub rendition: RegistryRendition,
}

impl RegistryAvatar {
    /// Normalize with registry-relative `assetPath` values resolved against
    /// `base_url` (for example `https://registry.ekza.io`).
    pub fn into_avatar(self, base_url: &str) -> EkzaAvatar {
        let base = base_url.trim_end_matches('/');
        let renditions = self
            .renditions
            .into_iter()
            .filter_map(|rendition| {
                let url = rendition.download_url.clone().or_else(|| {
                    rendition
                        .asset_path
                        .as_ref()
                        .map(|path| format!("{base}/{}", path.trim_start_matches('/')))
                })?;
                Some(AvatarRendition {
                    id: rendition.id,
                    format: rendition.format,
                    platform: rendition.platform,
                    profile: rendition.profile,
                    url,
                    sha256: Some(rendition.sha256),
                    size_bytes: Some(rendition.size_bytes),
                    media_type: rendition.media_type,
                })
            })
            .collect();
        let source_url = self
            .provenance
            .as_ref()
            .map(|provenance| provenance.source_model_uri.clone())
            .filter(|uri| !uri.is_empty());
        EkzaAvatar {
            origin: AvatarOrigin::Registry,
            id: self.id,
            name: self.name,
            collection: self
                .provenance
                .as_ref()
                .map(|provenance| format!("Ekza {}", provenance.network))
                .or_else(|| Some("Ekza registry".into())),
            author: self
                .license
                .as_ref()
                .and_then(|license| license.attribution.clone())
                .or_else(|| {
                    self.provenance
                        .as_ref()
                        .map(|provenance| provenance.creator.clone())
                        .filter(|creator| !creator.is_empty())
                }),
            license: self.license.map(|license| license.spdx),
            description: None,
            thumbnail_url: self.thumbnail_url,
            source_url,
            tags: Vec::new(),
            renditions,
            project_support: self.project_support,
        }
    }
}

// --- Passport public catalogue (`/api/passport/catalog`) ----------------------

/// `GET /api/passport/catalog` response: templates and approved renditions,
/// never an ownership claim.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PassportCatalog {
    #[serde(default)]
    pub schema: String,
    #[serde(default)]
    pub network: String,
    pub items: Vec<PassportCatalogItem>,
}

/// One rendition of a [`CatalogV2Avatar`]; the URL is always absolute.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CatalogV2Rendition {
    pub platform: String,
    pub profile: String,
    pub format: String,
    #[serde(default)]
    pub media_type: Option<String>,
    pub sha256: String,
    pub size_bytes: u64,
    pub download_url: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CatalogV2License {
    #[serde(default)]
    pub text: String,
    #[serde(default)]
    pub attribution: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CatalogV2Creator {
    #[serde(default)]
    pub name: String,
}

/// An avatar from `GET {registry}/v2/avatars`: on-chain templates and avatars
/// published through Ekza Studio in one shape. `access` is `"free"` when a game
/// may admit the avatar with no ownership proof, `"owned"` when wearing it needs a
/// passport ticket. Anything else is treated as owned.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CatalogV2Avatar {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub thumbnail_url: Option<String>,
    #[serde(default)]
    pub license: Option<CatalogV2License>,
    #[serde(default)]
    pub creator: Option<CatalogV2Creator>,
    #[serde(default)]
    pub access: String,
    #[serde(default)]
    pub renditions: Vec<CatalogV2Rendition>,
    #[serde(default)]
    pub project_support: Vec<ProjectApproval>,
}

/// `GET /v2/avatars` response.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CatalogV2Response {
    #[serde(default)]
    pub schema: String,
    #[serde(default)]
    pub count: usize,
    #[serde(default)]
    pub items: Vec<CatalogV2Avatar>,
}

impl CatalogV2Avatar {
    /// Only an explicit `"free"` counts; a missing or unknown value never does.
    pub fn is_free(&self) -> bool {
        self.access == "free"
    }

    pub fn into_avatar(self) -> EkzaAvatar {
        EkzaAvatar {
            origin: AvatarOrigin::Registry,
            id: self.id,
            name: self.name,
            collection: Some("Ekza".into()),
            author: self
                .creator
                .map(|creator| creator.name)
                .filter(|name| !name.is_empty()),
            license: self
                .license
                .map(|license| license.text)
                .filter(|text| !text.is_empty()),
            description: self.description.filter(|text| !text.is_empty()),
            thumbnail_url: self.thumbnail_url,
            source_url: None,
            tags: Vec::new(),
            renditions: self
                .renditions
                .into_iter()
                .map(|rendition| AvatarRendition {
                    id: format!("sha256:{}", rendition.sha256),
                    format: rendition.format,
                    platform: rendition.platform,
                    profile: rendition.profile,
                    url: rendition.download_url,
                    sha256: Some(rendition.sha256),
                    size_bytes: Some(rendition.size_bytes),
                    media_type: rendition.media_type,
                })
                .collect(),
            project_support: self.project_support,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PassportCatalogItem {
    pub avatar_id: String,
    pub name: String,
    #[serde(default)]
    pub thumbnail_url: Option<String>,
    #[serde(default)]
    pub support: Vec<PassportProjectSupport>,
}

fn passport_support_to_parts(
    support: &[PassportProjectSupport],
) -> (Vec<AvatarRendition>, Vec<ProjectApproval>) {
    let mut renditions: Vec<AvatarRendition> = Vec::new();
    let mut approvals = Vec::new();
    for entry in support {
        let rendition = &entry.rendition;
        if !renditions.iter().any(|existing| {
            existing.platform == entry.platform && existing.profile == entry.profile
        }) {
            renditions.push(AvatarRendition {
                id: rendition.id.clone(),
                format: rendition.format.clone(),
                platform: entry.platform.clone(),
                profile: entry.profile.clone(),
                url: rendition.url.clone(),
                sha256: Some(rendition.sha256.clone()),
                size_bytes: Some(rendition.size_bytes),
                media_type: None,
            });
        }
        approvals.push(ProjectApproval {
            project_id: entry.project_id.clone(),
            platform: entry.platform.clone(),
            profile: entry.profile.clone(),
            status: entry.status.clone(),
        });
    }
    (renditions, approvals)
}

impl From<PassportCatalogItem> for EkzaAvatar {
    fn from(item: PassportCatalogItem) -> Self {
        let (renditions, project_support) = passport_support_to_parts(&item.support);
        EkzaAvatar {
            origin: AvatarOrigin::Passport,
            id: item.avatar_id,
            name: item.name,
            collection: Some("Ekza avatars".into()),
            author: None,
            license: None,
            description: None,
            thumbnail_url: item.thumbnail_url,
            source_url: None,
            tags: Vec::new(),
            renditions,
            project_support,
        }
    }
}

impl From<&PurchasedAvatar> for EkzaAvatar {
    fn from(item: &PurchasedAvatar) -> Self {
        let (renditions, project_support) = passport_support_to_parts(&item.support);
        EkzaAvatar {
            origin: AvatarOrigin::Passport,
            id: item.avatar_id.clone(),
            name: item.name.clone(),
            collection: Some("Ekza purchased avatars".into()),
            author: None,
            license: None,
            description: None,
            thumbnail_url: Some(item.thumbnail_url.clone()).filter(|url| !url.is_empty()),
            source_url: None,
            tags: vec![format!("mint:{}", item.mint)],
            renditions,
            project_support,
        }
    }
}

// --- Merging -----------------------------------------------------------------

/// Merge several feeds by id. Later feeds enrich earlier ones: renditions and
/// approvals are unioned by selector, metadata fills gaps, and a registry or
/// passport origin wins over a library origin for the same id.
pub fn merge_avatars(feeds: impl IntoIterator<Item = Vec<EkzaAvatar>>) -> Vec<EkzaAvatar> {
    let mut order: Vec<String> = Vec::new();
    let mut merged: HashMap<String, EkzaAvatar> = HashMap::new();
    for feed in feeds {
        for avatar in feed {
            match merged.get_mut(&avatar.id) {
                None => {
                    order.push(avatar.id.clone());
                    merged.insert(avatar.id.clone(), avatar);
                }
                Some(existing) => existing.absorb(avatar),
            }
        }
    }
    order
        .into_iter()
        .filter_map(|id| merged.remove(&id))
        .collect()
}

impl EkzaAvatar {
    fn absorb(&mut self, other: EkzaAvatar) {
        if self.origin == AvatarOrigin::Library && other.origin != AvatarOrigin::Library {
            self.origin = other.origin;
        }
        if self.collection.is_none() {
            self.collection = other.collection;
        }
        if self.author.is_none() {
            self.author = other.author;
        }
        if self.license.is_none() {
            self.license = other.license;
        }
        if self.description.is_none() {
            self.description = other.description;
        }
        if self.thumbnail_url.is_none() {
            self.thumbnail_url = other.thumbnail_url;
        }
        if self.source_url.is_none() {
            self.source_url = other.source_url;
        }
        for tag in other.tags {
            if !self.tags.contains(&tag) {
                self.tags.push(tag);
            }
        }
        for rendition in other.renditions {
            match self.renditions.iter_mut().find(|existing| {
                existing.platform == rendition.platform && existing.profile == rendition.profile
            }) {
                Some(existing) => {
                    // Hashed renditions are more trustworthy than bare URLs.
                    if existing.sha256.is_none() && rendition.sha256.is_some() {
                        *existing = rendition;
                    }
                }
                None => self.renditions.push(rendition),
            }
        }
        for approval in other.project_support {
            if !self.project_support.contains(&approval) {
                self.project_support.push(approval);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const REGISTRY_FIXTURE: &str = include_str!("../tests/fixtures/registry-v1-avatars.json");
    const LIBRARY_FIXTURE: &str = include_str!("../tests/fixtures/library-avatars.json");
    const PASSPORT_FIXTURE: &str = include_str!("../tests/fixtures/passport-catalog.json");

    #[test]
    fn registry_response_normalizes_renditions_and_resolves_asset_paths() {
        let response: RegistryCatalogResponse = serde_json::from_str(REGISTRY_FIXTURE).unwrap();
        assert_eq!(response.avatars.len(), 2);
        let robert = response.avatars[1]
            .clone()
            .into_avatar("https://registry.ekza.io/");
        assert_eq!(robert.origin, AvatarOrigin::Registry);
        assert_eq!(robert.license.as_deref(), Some("CC0-1.0"));
        assert_eq!(robert.renditions.len(), 3);
        let omoba = robert.approved_rendition("omoba").expect("omoba approval");
        assert_eq!(omoba.format, "glb");
        assert!(omoba.url.starts_with("https://registry.ekza.io/v1/assets/"));
        assert_eq!(omoba.size_bytes, Some(1_885_072));
        assert!(robert.is_approved_for("omoba"));
        let devil = response.avatars[0]
            .clone()
            .into_avatar("https://registry.ekza.io");
        assert!(!devil.is_approved_for("omoba"));
        assert_eq!(
            devil
                .gltf_rendition(Some("omoba"), "desktop", "humanoid-glb-v1")
                .map(|r| r.format.as_str()),
            Some("vrm0")
        );
    }

    #[test]
    fn library_page_normalizes_to_universal_vrm_renditions() {
        let page: LibraryPage = serde_json::from_str(LIBRARY_FIXTURE).unwrap();
        assert_eq!(page.total, 462);
        let witch: EkzaAvatar = page.items[0].clone().into();
        assert_eq!(witch.origin, AvatarOrigin::Library);
        assert_eq!(witch.name, "Witch");
        assert_eq!(witch.collection.as_deref(), Some("100Avatars R1"));
        let model = witch
            .gltf_rendition(None, "universal", "vrm-humanoid-v0")
            .unwrap();
        assert_eq!(model.format, "vrm");
        assert_eq!(model.file_extension(), "vrm");
        assert!(model.url.contains(&model.id));
        assert!(witch.tags.contains(&"starter-pack".to_string()));
        assert_eq!(witch.slug(), format!("witch-{}", short_hash(&witch.id)));
    }

    #[test]
    fn passport_catalog_items_carry_approvals() {
        let catalog: PassportCatalog = serde_json::from_str(PASSPORT_FIXTURE).unwrap();
        let avatar: EkzaAvatar = catalog.items[0].clone().into();
        assert_eq!(avatar.origin, AvatarOrigin::Passport);
        assert_eq!(avatar.project_support.len(), 3);
        let omoba = avatar.approved_rendition("omoba").unwrap();
        assert_eq!(omoba.sha256.as_deref().map(str::len), Some(64));
    }

    #[test]
    fn merge_unions_feeds_by_identity() {
        let response: RegistryCatalogResponse = serde_json::from_str(REGISTRY_FIXTURE).unwrap();
        let catalog: PassportCatalog = serde_json::from_str(PASSPORT_FIXTURE).unwrap();
        let registry: Vec<EkzaAvatar> = response
            .avatars
            .into_iter()
            .map(|avatar| avatar.into_avatar("https://registry.ekza.io"))
            .collect();
        let passport: Vec<EkzaAvatar> = catalog.items.into_iter().map(Into::into).collect();
        let merged = merge_avatars([registry, passport]);
        assert_eq!(merged.len(), 2, "same canonical id must merge");
        let robert = merged.iter().find(|a| a.name == "Robert").unwrap();
        assert_eq!(robert.origin, AvatarOrigin::Registry);
        assert_eq!(robert.project_support.len(), 3);
    }

    #[test]
    fn slugs_are_stable_and_filesystem_safe() {
        assert_eq!(slugify("MeganTheFox"), "meganthefox");
        assert_eq!(slugify("Lady Koi (R3) #12"), "lady-koi-r3-12");
        assert_eq!(slugify("Ведьма"), "");
        let avatar = EkzaAvatar {
            origin: AvatarOrigin::Library,
            id: "00df253c-4665-4cbf-b420-761a1cc4f9dd".into(),
            name: "Ведьма".into(),
            collection: None,
            author: None,
            license: None,
            description: None,
            thumbnail_url: None,
            source_url: None,
            tags: vec![],
            renditions: vec![],
            project_support: vec![],
        };
        assert!(avatar.slug().starts_with("ekza-"));
        assert_eq!(avatar.slug(), avatar.slug());
    }
}
