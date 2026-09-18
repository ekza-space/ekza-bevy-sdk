//! Runtime avatar store for a running game.
//!
//! [`roster::sync`](crate::roster::sync) fills an asset root at build or deploy
//! time. A live game needs the opposite direction too: a player buys an avatar
//! while the game is already installed, and every other player in the match has
//! to see it. [`AvatarStore`] covers that:
//!
//! 1. [`AvatarStore::refresh`] reads the public registry catalogue and keeps the
//!    templates an operator approved for this game's [`SupportSelector`].
//! 2. The result is persisted, so [`AvatarStore::cached`] lists the same
//!    templates on an offline start.
//! 3. [`AvatarStore::install`] downloads one rendition on demand, verifies size,
//!    SHA-256 and the GLB envelope, runs the game's own validator, and places
//!    the file at `<root>/avatars/<slug>.glb`.
//!
//! The catalogue is public and carries no entitlement. Whether a player may
//! *wear* a template is decided by the passport session on the client and by a
//! consumed ticket on the game server; the store only answers "what is this
//! slug and where are its verified bytes".

use serde::{Deserialize, Serialize};

use crate::{
    catalog::{AvatarOrigin, EkzaAvatar},
    passport::{
        ProjectSupport, ProtectedAvatar, Rendition, SupportSelector, protected_slug,
    },
};

pub const STORE_SCHEMA: &str = "ekza.store.v1";

/// One purchasable template approved for the consuming game.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StoreAvatar {
    /// [`protected_slug`] of `protected`; the id games put on the wire.
    pub slug: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub author: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub license: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thumbnail_url: Option<String>,
    pub protected: ProtectedAvatar,
}

#[cfg(feature = "http")]
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
struct StoreDocument {
    schema: String,
    avatars: Vec<StoreAvatar>,
}

/// Templates from `avatars` that carry an approval matching `selector` exactly
/// and a rendition with a declared hash and size. Free library and local
/// entries are never store items. Order follows the feed; duplicates by slug
/// are dropped.
pub fn templates(avatars: &[EkzaAvatar], selector: &SupportSelector) -> Vec<StoreAvatar> {
    let mut seen = std::collections::HashSet::new();
    let mut items = Vec::new();
    for avatar in avatars {
        if matches!(avatar.origin, AvatarOrigin::Library | AvatarOrigin::Local) {
            continue;
        }
        for approval in &avatar.project_support {
            if approval.project_id != selector.project_id
                || approval.platform != selector.platform
                || approval.profile != selector.profile
                || approval.status != "approved"
            {
                continue;
            }
            let Some(rendition) = avatar.rendition(&approval.platform, &approval.profile) else {
                continue;
            };
            let (Some(sha256), Some(size_bytes)) = (&rendition.sha256, rendition.size_bytes) else {
                continue;
            };
            let protected = ProtectedAvatar {
                avatar_id: avatar.id.clone(),
                support: ProjectSupport {
                    project_id: approval.project_id.clone(),
                    platform: approval.platform.clone(),
                    profile: approval.profile.clone(),
                    status: approval.status.clone(),
                    rendition: Rendition {
                        id: rendition.id.clone(),
                        url: rendition.url.clone(),
                        sha256: sha256.to_ascii_lowercase(),
                        size_bytes,
                        format: rendition.format.clone(),
                    },
                },
            };
            if protected.validate_for(selector).is_err() {
                continue;
            }
            let slug = protected_slug(&protected);
            if seen.insert(slug.clone()) {
                items.push(StoreAvatar {
                    slug,
                    name: avatar.name.clone(),
                    author: avatar.author.clone(),
                    license: avatar.license.clone(),
                    thumbnail_url: avatar.thumbnail_url.clone(),
                    protected,
                });
            }
        }
    }
    items
}

/// Re-validate a persisted or remote item: the slug must be the one derived
/// from its own boundary, and the boundary must satisfy `selector`.
pub fn validate_item(item: &StoreAvatar, selector: &SupportSelector) -> Result<(), &'static str> {
    item.protected.validate_for(selector)?;
    if protected_slug(&item.protected) != item.slug {
        return Err("Store slug does not match its approved rendition");
    }
    Ok(())
}

#[cfg(feature = "http")]
pub use runtime::AvatarStore;

#[cfg(feature = "http")]
mod runtime {
    use std::{
        fs,
        io::Read,
        path::{Path, PathBuf},
    };

    use super::{STORE_SCHEMA, StoreAvatar, StoreDocument, templates, validate_item};
    use crate::{
        cache::{AssetCache, atomic_write},
        catalog::AvatarRendition,
        passport::{ProtectedAvatar, SupportSelector},
        registry::RegistryClient,
        sha256::sha256_hex,
    };

    /// See the [module docs](super).
    #[derive(Clone)]
    pub struct AvatarStore {
        root: PathBuf,
        registry: RegistryClient,
        selector: SupportSelector,
        cache: AssetCache,
    }

    impl AvatarStore {
        /// `root` is a writable per-user directory owned by the game.
        pub fn new(
            root: impl Into<PathBuf>,
            registry_url: &str,
            selector: SupportSelector,
        ) -> Result<Self, String> {
            let root = root.into();
            Ok(Self {
                registry: RegistryClient::new(registry_url).map_err(|error| error.to_string())?,
                cache: AssetCache::new(root.join(".ekza-cache")).map_err(|error| error.to_string())?,
                selector,
                root,
            })
        }

        pub fn root(&self) -> &Path {
            &self.root
        }

        pub fn selector(&self) -> &SupportSelector {
            &self.selector
        }

        /// Installed model location for a slug, whether or not it exists yet.
        pub fn model_path(&self, slug: &str) -> PathBuf {
            self.root.join("avatars").join(format!("{slug}.glb"))
        }

        fn document_path(&self) -> PathBuf {
            self.root.join("store.json")
        }

        /// Fetch the approved catalogue and persist it for offline starts.
        pub fn refresh(&self) -> Result<Vec<StoreAvatar>, String> {
            let avatars = self
                .registry
                .catalog(None)
                .map_err(|error| error.to_string())?;
            let items = templates(&avatars, &self.selector);
            let document = StoreDocument {
                schema: STORE_SCHEMA.into(),
                avatars: items.clone(),
            };
            let bytes = serde_json::to_vec_pretty(&document).map_err(|error| error.to_string())?;
            atomic_write(&self.document_path(), &bytes).map_err(|error| error.to_string())?;
            Ok(items)
        }

        /// Last persisted catalogue; entries that no longer validate are dropped.
        pub fn cached(&self) -> Vec<StoreAvatar> {
            let Ok(raw) = fs::read(self.document_path()) else {
                return Vec::new();
            };
            let Ok(document) = serde_json::from_slice::<StoreDocument>(&raw) else {
                return Vec::new();
            };
            if document.schema != STORE_SCHEMA {
                return Vec::new();
            }
            document
                .avatars
                .into_iter()
                .filter(|item| validate_item(item, &self.selector).is_ok())
                .collect()
        }

        /// Exact-byte check of the installed file against its boundary.
        pub fn verify_installed(&self, item: &StoreAvatar) -> Result<(), String> {
            validate_item(item, &self.selector)?;
            let bytes = read_exact_size(
                &self.model_path(&item.slug),
                item.protected.support.rendition.size_bytes,
            )?;
            check(&item.protected, &self.selector, &bytes)
        }

        /// Download (or reuse the verified cache copy of) the approved rendition
        /// and install it. `validator` is the game's own profile check, e.g.
        /// "has a humanoid skin and the clips my animation graph needs"; it
        /// runs before anything is placed under `avatars/`.
        pub fn install(
            &self,
            item: &StoreAvatar,
            validator: impl FnOnce(&[u8]) -> Result<(), String>,
        ) -> Result<PathBuf, String> {
            validate_item(item, &self.selector)?;
            let destination = self.model_path(&item.slug);
            let rendition = &item.protected.support.rendition;
            if let Ok(bytes) = read_exact_size(&destination, rendition.size_bytes)
                && check(&item.protected, &self.selector, &bytes).is_ok()
            {
                validator(&bytes)?;
                return Ok(destination);
            }
            let cached = self
                .cache
                .fetch_model(&AvatarRendition {
                    id: rendition.id.clone(),
                    format: rendition.format.clone(),
                    platform: item.protected.support.platform.clone(),
                    profile: item.protected.support.profile.clone(),
                    url: rendition.url.clone(),
                    sha256: Some(rendition.sha256.clone()),
                    size_bytes: Some(rendition.size_bytes),
                    media_type: None,
                })
                .map_err(|error| error.to_string())?;
            let bytes = read_exact_size(&cached.path, rendition.size_bytes)?;
            check(&item.protected, &self.selector, &bytes)?;
            validator(&bytes)?;
            atomic_write(&destination, &bytes).map_err(|error| error.to_string())?;
            Ok(destination)
        }
    }

    fn check(
        protected: &ProtectedAvatar,
        selector: &SupportSelector,
        bytes: &[u8],
    ) -> Result<(), String> {
        protected
            .validate_bytes_for(selector, bytes, &sha256_hex(bytes))
            .map_err(str::to_owned)
    }

    fn read_exact_size(path: &Path, size_bytes: u64) -> Result<Vec<u8>, String> {
        let file = fs::File::open(path).map_err(|_| "Avatar is not installed".to_string())?;
        let actual = file
            .metadata()
            .map_err(|_| "Avatar metadata unavailable".to_string())?
            .len();
        if actual != size_bytes {
            return Err("Installed avatar size differs from the approved rendition".into());
        }
        let mut bytes = Vec::new();
        file.take(size_bytes + 1)
            .read_to_end(&mut bytes)
            .map_err(|_| "Installed avatar could not be read".to_string())?;
        Ok(bytes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::{AvatarRendition, ProjectApproval};

    fn template(name: &str, key: char, sha: char) -> EkzaAvatar {
        EkzaAvatar {
            origin: AvatarOrigin::Registry,
            id: format!("solana:devnet:avatar-data:{}", key.to_string().repeat(32)),
            name: name.into(),
            collection: None,
            author: Some("artist".into()),
            license: Some("CC0-1.0".into()),
            description: None,
            thumbnail_url: Some("https://registry.ekza.io/t.png".into()),
            source_url: None,
            tags: Vec::new(),
            renditions: vec![
                AvatarRendition {
                    id: "vrm".into(),
                    format: "vrm0".into(),
                    platform: "universal".into(),
                    profile: "vrm-humanoid-v0".into(),
                    url: "https://arweave.net/x".into(),
                    sha256: Some("c".repeat(64)),
                    size_bytes: Some(1000),
                    media_type: None,
                },
                AvatarRendition {
                    id: "omoba".into(),
                    format: "glb".into(),
                    platform: "desktop".into(),
                    profile: "humanoid-glb-v1".into(),
                    url: "https://registry.ekza.io/v1/assets/a.glb".into(),
                    sha256: Some(sha.to_string().repeat(64)),
                    size_bytes: Some(2000),
                    media_type: None,
                },
            ],
            project_support: vec![ProjectApproval {
                project_id: "omoba".into(),
                platform: "desktop".into(),
                profile: "humanoid-glb-v1".into(),
                status: "approved".into(),
            }],
        }
    }

    #[test]
    fn only_templates_approved_for_the_exact_selector_are_listed() {
        let approved = template("Robert", '1', 'a');
        let mut other_project = template("Rose", '2', 'b');
        other_project.project_support[0].project_id = "ekza-space".into();
        let mut pending = template("Devil", '3', 'd');
        pending.project_support[0].status = "pending".into();
        let mut unhashed = template("Rabbit", '4', 'e');
        unhashed.renditions[1].sha256 = None;
        let mut library = template("Agnes", '5', 'f');
        library.origin = AvatarOrigin::Library;

        let items = templates(
            &[approved.clone(), other_project, pending, unhashed, library, approved],
            &SupportSelector::omoba_desktop(),
        );
        assert_eq!(items.len(), 1);
        let item = &items[0];
        assert_eq!(item.name, "Robert");
        assert_eq!(item.protected.support.rendition.id, "omoba");
        assert!(crate::passport::is_protected_slug(&item.slug));
        assert!(validate_item(item, &SupportSelector::omoba_desktop()).is_ok());
    }

    #[test]
    fn slug_is_bound_to_identity_and_rendition_revision() {
        let selector = SupportSelector::omoba_desktop();
        let first = templates(&[template("Robert", '1', 'a')], &selector).remove(0);
        let revised = templates(&[template("Robert", '1', 'b')], &selector).remove(0);
        let sibling = templates(&[template("Robert", '2', 'a')], &selector).remove(0);
        assert_ne!(first.slug, revised.slug);
        assert_ne!(first.slug, sibling.slug);

        let mut tampered = first.clone();
        tampered.protected.support.rendition.sha256 = "b".repeat(64);
        assert!(validate_item(&tampered, &selector).is_err());
        let mut foreign = first;
        foreign.protected.support.project_id = "other".into();
        assert!(validate_item(&foreign, &selector).is_err());
    }

    #[cfg(feature = "http")]
    #[test]
    fn install_verifies_bytes_runs_the_game_validator_and_survives_offline() {
        use crate::{GLB_HEADER_LEN, GLB_MAGIC, SUPPORTED_GLB_VERSION, sha256_hex};

        let mut bytes = Vec::new();
        bytes.extend_from_slice(&GLB_MAGIC);
        bytes.extend_from_slice(&SUPPORTED_GLB_VERSION.to_le_bytes());
        bytes.extend_from_slice(&((GLB_HEADER_LEN + 16) as u32).to_le_bytes());
        bytes.extend(std::iter::repeat_n(0u8, 16));

        let selector = SupportSelector::omoba_desktop();
        let mut avatar = template("Robert", '1', 'a');
        avatar.renditions[1].sha256 = Some(sha256_hex(&bytes));
        avatar.renditions[1].size_bytes = Some(bytes.len() as u64);
        let item = templates(&[avatar], &selector).remove(0);

        let root = std::env::temp_dir().join(format!("ekza-store-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let store = AvatarStore::new(&root, "https://registry.ekza.io", selector).unwrap();
        assert!(store.cached().is_empty());
        assert!(store.verify_installed(&item).is_err());

        // Seed the content-addressed cache: install must not touch the network.
        let cache = root.join(".ekza-cache");
        std::fs::create_dir_all(&cache).unwrap();
        std::fs::write(cache.join(format!("{}.glb", sha256_hex(&bytes))), &bytes).unwrap();

        assert_eq!(
            store.install(&item, |_| Err("no clips".into())).unwrap_err(),
            "no clips"
        );
        assert!(!store.model_path(&item.slug).exists());

        let installed = store.install(&item, |_| Ok(())).unwrap();
        assert_eq!(installed, store.model_path(&item.slug));
        assert!(store.verify_installed(&item).is_ok());

        // A tampered install is detected and repaired from the cache.
        std::fs::write(&installed, vec![0u8; bytes.len()]).unwrap();
        assert!(store.verify_installed(&item).is_err());
        store.install(&item, |_| Ok(())).unwrap();
        assert!(store.verify_installed(&item).is_ok());
        let _ = std::fs::remove_dir_all(&root);
    }
}
