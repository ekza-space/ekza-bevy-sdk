//! Typed, approved free assets for a running game.
//!
//! Weapons and avatars share catalogue and download transport, but retain their
//! own canonical identities and game validators. This module grants no paid
//! ownership. Only an explicit `access: "free"` is eligible for installation.

use serde::{Deserialize, Serialize};

use crate::{
    catalog::{CatalogV2Creator, CatalogV2License, CatalogV2Rendition, ProjectApproval},
    passport::{ProjectSupport, Rendition, SupportSelector, validate_project_support},
};

pub const CATALOG_SCHEMA: &str = "ekza.asset.catalog.v2";
pub const STORE_SCHEMA: &str = "ekza.asset.store.v1";
pub const MAX_WEAPON_BYTES: u64 = 8 * 1024 * 1024;

/// Unknown kinds fail deserialization; they never fall back to an avatar.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AssetKind {
    Avatar,
    Weapon,
}

impl AssetKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Avatar => "avatar",
            Self::Weapon => "weapon",
        }
    }

    pub fn directory(self) -> &'static str {
        match self {
            Self::Avatar => "avatars",
            Self::Weapon => "weapons",
        }
    }

    pub fn max_bytes(self) -> u64 {
        match self {
            Self::Avatar => crate::passport::MAX_RENDITION_BYTES,
            Self::Weapon => MAX_WEAPON_BYTES,
        }
    }
}

/// `/v2/assets` adds an explicit kind to the existing catalogue item fields.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CatalogAsset {
    pub asset_kind: AssetKind,
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

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AssetCatalogResponse {
    pub schema: String,
    pub count: usize,
    pub items: Vec<CatalogAsset>,
}

/// Public, exact-rendition approval. Safe to persist and use on a game server;
/// it contains no credential and must come from the server's own registry read.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StoreAsset {
    pub slug: String,
    pub asset_id: String,
    pub asset_kind: AssetKind,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub author: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub license: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thumbnail_url: Option<String>,
    pub support: ProjectSupport,
    pub free: bool,
}

/// Do not extend `validate_avatar_id`: a weapon cannot pass an avatar boundary.
pub fn validate_asset_id(kind: AssetKind, id: &str) -> Result<(), &'static str> {
    match kind {
        AssetKind::Avatar => crate::passport::validate_avatar_id(id),
        AssetKind::Weapon
            if id
                .strip_prefix("ekza:weapon:")
                .is_some_and(crate::passport::valid_uuid) =>
        {
            Ok(())
        }
        AssetKind::Weapon => Err("Invalid canonical weapon identity"),
    }
}

/// Preserve the original handheld pilot's identity/content hash algorithm.
/// Avatar slugs remain compatible with `passport::protected_slug`.
pub fn asset_slug(kind: AssetKind, asset_id: &str, sha256: &str) -> String {
    match kind {
        AssetKind::Weapon => {
            let hash = crate::sha256_hex(format!("{asset_id}:{sha256}").as_bytes());
            format!("ekza-{}", &hash[..32])
        }
        AssetKind::Avatar => format!(
            "ekza-{}",
            crate::sha256_hex(format!("{asset_id}\n{sha256}").as_bytes())
        ),
    }
}

pub fn validate_item(
    item: &StoreAsset,
    kind: AssetKind,
    selector: &SupportSelector,
) -> Result<(), &'static str> {
    if item.asset_kind != kind || !item.free {
        return Err("Asset kind or free access does not match this store");
    }
    validate_asset_id(kind, &item.asset_id)?;
    validate_project_support(&item.support, selector)?;
    let rendition = &item.support.rendition;
    if rendition.size_bytes > kind.max_bytes()
        || rendition.format != "glb"
        || item.slug != asset_slug(kind, &item.asset_id, &rendition.sha256)
    {
        return Err("Asset slug, GLB format or size does not match its approved rendition");
    }
    Ok(())
}

/// Select explicitly free assets approved for precisely this game/profile.
/// Missing/unknown access, identities, revisions or approvals never qualify.
pub fn templates(
    assets: &[CatalogAsset],
    kind: AssetKind,
    selector: &SupportSelector,
) -> Vec<StoreAsset> {
    let mut seen = std::collections::HashSet::new();
    let mut result = Vec::new();
    for asset in assets {
        if asset.asset_kind != kind || asset.access != "free" {
            continue;
        }
        for approval in &asset.project_support {
            if approval.project_id != selector.project_id
                || approval.platform != selector.platform
                || approval.profile != selector.profile
                || approval.status != "approved"
            {
                continue;
            }
            // Ambiguous selectors must not choose whichever revision came first.
            let mut renditions = asset.renditions.iter().filter(|rendition| {
                rendition.platform == approval.platform && rendition.profile == approval.profile
            });
            let Some(rendition) = renditions.next() else {
                continue;
            };
            if renditions.next().is_some() {
                continue;
            }
            let item = StoreAsset {
                slug: asset_slug(kind, &asset.id, &rendition.sha256),
                asset_id: asset.id.clone(),
                asset_kind: kind,
                name: asset.name.clone(),
                author: asset.creator.as_ref().map(|creator| creator.name.clone()),
                license: asset.license.as_ref().map(|license| license.text.clone()),
                thumbnail_url: asset.thumbnail_url.clone(),
                support: ProjectSupport {
                    project_id: approval.project_id.clone(),
                    platform: approval.platform.clone(),
                    profile: approval.profile.clone(),
                    status: approval.status.clone(),
                    rendition: Rendition {
                        id: format!("sha256:{}", rendition.sha256),
                        url: rendition.download_url.clone(),
                        sha256: rendition.sha256.clone(),
                        size_bytes: rendition.size_bytes,
                        format: rendition.format.clone(),
                    },
                },
                free: true,
            };
            if validate_item(&item, kind, selector).is_ok() && seen.insert(item.slug.clone()) {
                result.push(item);
            }
        }
    }
    result
}

#[cfg(feature = "http")]
pub use runtime::AssetStore;

#[cfg(feature = "http")]
mod runtime {
    use super::*;
    use crate::{
        cache::{AssetCache, atomic_write},
        catalog::AvatarRendition,
        registry::RegistryClient,
    };
    use std::{
        fs,
        io::Read,
        path::{Path, PathBuf},
    };

    const MAX_CATALOG_BYTES: u64 = 8 * 1024 * 1024;

    #[derive(Serialize, Deserialize)]
    struct StoreDocument {
        schema: String,
        namespace: String,
        items: Vec<StoreAsset>,
    }

    /// Reusable verified free-asset install store. Rendering and attachment
    /// validation stay with the consuming game, supplied as an install callback.
    #[derive(Clone)]
    pub struct AssetStore {
        root: PathBuf,
        registry: RegistryClient,
        kind: AssetKind,
        selector: SupportSelector,
        namespace: String,
        cache: AssetCache,
    }

    impl AssetStore {
        pub fn new(
            root: impl Into<PathBuf>,
            registry_url: &str,
            kind: AssetKind,
            selector: SupportSelector,
        ) -> Result<Self, String> {
            let root = root.into();
            let registry = RegistryClient::new(registry_url).map_err(|error| error.to_string())?;
            let namespace = crate::sha256_hex(
                &serde_json::to_vec(&(
                    registry.base_url(),
                    kind,
                    &selector.project_id,
                    &selector.platform,
                    &selector.profile,
                    &selector.formats,
                ))
                .map_err(|error| error.to_string())?,
            );
            Ok(Self {
                cache: AssetCache::new(root.join(".ekza-cache"))
                    .map_err(|error| error.to_string())?
                    .with_limits(kind.max_bytes(), 1024 * 1024),
                registry,
                root,
                kind,
                selector,
                namespace,
            })
        }

        pub fn root(&self) -> &Path {
            &self.root
        }
        pub fn selector(&self) -> &SupportSelector {
            &self.selector
        }
        pub fn kind(&self) -> AssetKind {
            self.kind
        }

        /// Metadata is isolated by origin, kind and selector, so several stores
        /// can share one writable asset source without confusing approvals.
        pub fn catalogue_path(&self) -> PathBuf {
            self.root.join(format!(
                "assets-{}-{}.json",
                self.kind.as_str(),
                self.namespace
            ))
        }

        pub fn model_path(&self, slug: &str) -> PathBuf {
            self.root
                .join(self.kind.directory())
                .join(format!("{slug}.glb"))
        }

        /// Only a complete, healthy response replaces persisted eligibility.
        /// An empty response removes eligibility while retaining cached bytes.
        /// A missing typed endpoint is an error, never an avatar-feed fallback.
        pub fn refresh(&self) -> Result<Vec<StoreAsset>, String> {
            let assets = self
                .registry
                .assets_v2(
                    self.kind,
                    Some(&self.selector.project_id),
                    Some((&self.selector.platform, &self.selector.profile)),
                )
                .map_err(|error| error.to_string())?;
            let items = templates(&assets, self.kind, &self.selector);
            let document = StoreDocument {
                schema: STORE_SCHEMA.into(),
                namespace: self.namespace.clone(),
                items: items.clone(),
            };
            let bytes = serde_json::to_vec(&document).map_err(|error| error.to_string())?;
            if bytes.len() as u64 > MAX_CATALOG_BYTES {
                return Err("Asset catalogue exceeds the persisted size limit".into());
            }
            atomic_write(&self.catalogue_path(), &bytes).map_err(|error| error.to_string())?;
            Ok(items)
        }

        pub fn cached(&self) -> Vec<StoreAsset> {
            let Ok(file) = fs::File::open(self.catalogue_path()) else {
                return Vec::new();
            };
            let mut bytes = Vec::new();
            if file
                .take(MAX_CATALOG_BYTES + 1)
                .read_to_end(&mut bytes)
                .is_err()
                || bytes.len() as u64 > MAX_CATALOG_BYTES
            {
                return Vec::new();
            }
            let Ok(document) = serde_json::from_slice::<StoreDocument>(&bytes) else {
                return Vec::new();
            };
            if document.schema != STORE_SCHEMA || document.namespace != self.namespace {
                return Vec::new();
            }
            document
                .items
                .into_iter()
                .filter(|item| validate_item(item, self.kind, &self.selector).is_ok())
                .collect()
        }

        pub fn verify_installed(&self, item: &StoreAsset) -> Result<(), String> {
            self.verified_bytes(item).map(|_| ())
        }

        fn verified_bytes(&self, item: &StoreAsset) -> Result<Vec<u8>, String> {
            validate_item(item, self.kind, &self.selector)?;
            let expected = item.support.rendition.size_bytes;
            let file =
                fs::File::open(self.model_path(&item.slug)).map_err(|error| error.to_string())?;
            if file.metadata().map_err(|error| error.to_string())?.len() != expected {
                return Err("Installed asset size differs from approved rendition".into());
            }
            let mut bytes = Vec::new();
            file.take(expected + 1)
                .read_to_end(&mut bytes)
                .map_err(|error| error.to_string())?;
            AssetCache::verify_model(&rendition(item), &bytes)
                .map_err(|error| error.to_string())?;
            Ok(bytes)
        }

        /// Validate both reused and freshly downloaded bytes before rendering.
        /// This verifies a supplied approval, not its freshness: callers must
        /// choose from their current catalogue and servers independently admit it.
        pub fn install(
            &self,
            item: &StoreAsset,
            validator: impl FnOnce(&[u8]) -> Result<(), String>,
        ) -> Result<PathBuf, String> {
            validate_item(item, self.kind, &self.selector)?;
            let destination = self.model_path(&item.slug);
            if let Ok(bytes) = self.verified_bytes(item) {
                validator(&bytes)?;
                return Ok(destination);
            }
            let rendition = rendition(item);
            let cached = self
                .cache
                .fetch_model(&rendition)
                .map_err(|error| error.to_string())?;
            let file = fs::File::open(cached.path).map_err(|error| error.to_string())?;
            let mut bytes = Vec::new();
            file.take(self.kind.max_bytes() + 1)
                .read_to_end(&mut bytes)
                .map_err(|error| error.to_string())?;
            AssetCache::verify_model(&rendition, &bytes).map_err(|error| error.to_string())?;
            validator(&bytes)?;
            atomic_write(&destination, &bytes).map_err(|error| error.to_string())?;
            Ok(destination)
        }
    }

    fn rendition(item: &StoreAsset) -> AvatarRendition {
        let support = &item.support;
        let rendition = &support.rendition;
        AvatarRendition {
            id: rendition.id.clone(),
            format: rendition.format.clone(),
            platform: support.platform.clone(),
            profile: support.profile.clone(),
            url: rendition.url.clone(),
            sha256: Some(rendition.sha256.clone()),
            size_bytes: Some(rendition.size_bytes),
            media_type: None,
        }
    }
}
