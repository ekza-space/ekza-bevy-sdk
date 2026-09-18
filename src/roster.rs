//! Game-side roster manifest: the file a consumer ships or syncs under its
//! asset root so the runtime can list and load Ekza avatars without any
//! network access.
//!
//! The JSON layout is a strict superset of the Omoba `avatars/manifest.json`
//! contract (`slug`, `display_name`, `collection`, `license`, `source_url`,
//! `author`, `thumbnail`, `passport`), so Omoba reads a roster written by this
//! module unchanged, and other engines get the extra identity, hash and file
//! fields they need to stay verifiable.
//!
//! Layout produced by [`sync::sync_roster`] (`http` feature):
//!
//! ```text
//! <asset_root>/avatars/manifest.json
//! <asset_root>/avatars/<slug>.glb          binary glTF (VRM is staged as .glb)
//! <asset_root>/avatars/<slug>.png|jpg      thumbnail, extension by real container
//! <asset_root>/avatars/.ekza-cache/        content-addressed verified downloads
//! ```

use std::{fs, io, path::Path};

use serde::{Deserialize, Serialize};

use crate::{
    catalog::{AvatarOrigin, EkzaAvatar},
    passport::ProtectedAvatar,
};

pub const ROSTER_SCHEMA: &str = "ekza.roster.v1";

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RosterEntry {
    /// Filesystem/protocol id; the model is `avatars/<slug>.glb`.
    pub slug: String,
    pub display_name: String,
    pub collection: String,
    pub license: String,
    /// Creator source or the download URL this file was staged from.
    pub source_url: String,
    #[serde(default)]
    pub author: Option<String>,
    /// Thumbnail file name relative to the avatars directory.
    #[serde(default)]
    pub thumbnail: Option<String>,
    /// Paid-cosmetic boundary. Only present for purchased avatars imported
    /// through a trusted passport session; never inferred from the catalogue.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub passport: Option<ProtectedAvatar>,

    // --- SDK extension fields (ignored by consumers that predate them) ---
    /// Feed identity (`solana:devnet:avatar-data:<PDA>` or a library UUID).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ekza_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub origin: Option<AvatarOrigin>,
    /// Model file relative to the asset root (`avatars/<slug>.glb`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// Source rendition format (`glb`, `vrm0`, ...), before staging as `.glb`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub format: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub platform: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sha256: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size_bytes: Option<u64>,
    /// Projects whose operators approved this exact rendition.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub approved_projects: Vec<String>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Roster {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub schema: Option<String>,
    pub avatars: Vec<RosterEntry>,
}

impl Roster {
    pub fn read(path: &Path) -> io::Result<Self> {
        let raw = fs::read_to_string(path)?;
        serde_json::from_str(&raw).map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))
    }

    pub fn read_or_default(path: &Path) -> io::Result<Self> {
        match Self::read(path) {
            Ok(roster) => Ok(roster),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(Self::default()),
            Err(error) => Err(error),
        }
    }

    pub fn write(&self, path: &Path) -> io::Result<()> {
        let mut document = self.clone();
        document.schema = Some(ROSTER_SCHEMA.to_string());
        let bytes = serde_json::to_vec_pretty(&document)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let temporary = path.with_extension(format!("part-{}", std::process::id()));
        fs::write(&temporary, bytes)?;
        fs::rename(temporary, path)
    }

    pub fn get(&self, slug: &str) -> Option<&RosterEntry> {
        self.avatars.iter().find(|entry| entry.slug == slug)
    }

    /// Insert or replace by slug. Entries that were shipped by the game (no
    /// `ekza_id`) are never overwritten by synced ones, so a hand-tuned roster
    /// model keeps precedence over a catalogue copy with the same slug.
    pub fn upsert(&mut self, entry: RosterEntry) -> bool {
        match self.avatars.iter_mut().find(|existing| existing.slug == entry.slug) {
            Some(existing) if existing.ekza_id.is_none() && entry.ekza_id.is_some() => false,
            Some(existing) => {
                *existing = entry;
                true
            }
            None => {
                self.avatars.push(entry);
                true
            }
        }
    }

    /// Drop synced entries (those carrying `ekza_id`) that are absent from
    /// `keep`; shipped entries are untouched.
    pub fn retain_synced(&mut self, keep: &[String]) {
        self.avatars
            .retain(|entry| entry.ekza_id.is_none() || keep.contains(&entry.slug));
    }
}

impl RosterEntry {
    /// Build a roster entry for a catalogue avatar staged at
    /// `avatars/<slug>.glb`, describing the rendition that was staged.
    pub fn from_avatar(
        avatar: &EkzaAvatar,
        rendition: &crate::catalog::AvatarRendition,
        sha256: &str,
        size_bytes: u64,
        thumbnail: Option<String>,
    ) -> Self {
        let slug = avatar.slug();
        RosterEntry {
            model: Some(format!("avatars/{slug}.glb")),
            slug,
            display_name: avatar.name.clone(),
            collection: avatar
                .collection
                .clone()
                .unwrap_or_else(|| "Ekza".to_string()),
            license: avatar
                .license
                .clone()
                .filter(|license| !license.is_empty())
                .unwrap_or_else(|| "See creator terms".to_string()),
            source_url: avatar
                .source_url
                .clone()
                .unwrap_or_else(|| rendition.url.clone()),
            author: avatar.author.clone(),
            thumbnail,
            passport: None,
            ekza_id: Some(avatar.id.clone()),
            origin: Some(avatar.origin),
            format: Some(rendition.format.clone()),
            platform: Some(rendition.platform.clone()),
            profile: Some(rendition.profile.clone()),
            sha256: Some(sha256.to_string()),
            size_bytes: Some(size_bytes),
            approved_projects: avatar
                .project_support
                .iter()
                .filter(|support| {
                    support.status == "approved"
                        && support.platform == rendition.platform
                        && support.profile == rendition.profile
                })
                .map(|support| support.project_id.clone())
                .collect(),
        }
    }
}

#[cfg(feature = "http")]
pub mod sync {
    //! Pull catalogue avatars into a consumer asset root.

    use std::{
        fs,
        path::{Path, PathBuf},
    };

    use super::{Roster, RosterEntry};
    use crate::{
        cache::{AssetCache, CacheError},
        catalog::{AvatarOrigin, EkzaAvatar},
        passport::{ProjectSupport, ProtectedAvatar, Rendition, validate_avatar_id},
    };

    /// Build the passport boundary for an approved template rendition.
    pub fn protected_avatar(avatar: &EkzaAvatar, project_id: &str) -> Result<ProtectedAvatar, &'static str> {
        validate_avatar_id(&avatar.id)?;
        let approval = avatar
            .project_support
            .iter()
            .find(|support| support.project_id == project_id && support.status == "approved")
            .ok_or("no approval for this project")?;
        let rendition = avatar
            .rendition(&approval.platform, &approval.profile)
            .ok_or("approval references a missing rendition")?;
        let (Some(sha256), Some(size_bytes)) = (&rendition.sha256, rendition.size_bytes) else {
            return Err("approved rendition has no declared hash or size");
        };
        Ok(ProtectedAvatar {
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
        })
    }

    /// What to sync and where.
    #[derive(Clone, Debug)]
    pub struct SyncOptions {
        /// Consumer asset root; avatars land in `<asset_root>/avatars/`.
        pub asset_root: PathBuf,
        /// Manifest path; defaults to `<asset_root>/avatars/manifest.json`.
        pub manifest: Option<PathBuf>,
        /// Project id used to prefer operator-approved renditions (`omoba`).
        pub project_id: Option<String>,
        /// Fallback selector when no approval exists (`desktop`, `humanoid-glb-v1`).
        pub platform: String,
        pub profile: String,
        /// Only stage avatars explicitly approved for `project_id`.
        pub approved_only: bool,
        pub thumbnails: bool,
        /// Plan without downloading or writing.
        pub dry_run: bool,
        /// Remove previously synced entries that are no longer in the feed.
        pub prune: bool,
        /// How devnet/purchasable templates (registry and passport origins) are
        /// staged. Free library avatars are never affected.
        pub protected: ProtectedPolicy,
    }

    /// Registry and passport templates are products: even when their approved
    /// rendition is publicly downloadable, a game must not hand them out as
    /// free cosmetics.
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub enum ProtectedPolicy {
        /// Stage only renditions approved for `project_id`, and mark the
        /// roster entry with a `passport` boundary so the game server demands
        /// a consumed ticket before admitting it (Omoba semantics). Default.
        Passport,
        /// Stage nothing from registry/passport origins.
        Skip,
        /// Stage them as free cosmetics. Only for previews, demos and games
        /// without an entitlement check; never for a paid roster.
        Free,
    }

    impl SyncOptions {
        pub fn new(asset_root: impl Into<PathBuf>) -> Self {
            Self {
                asset_root: asset_root.into(),
                manifest: None,
                project_id: None,
                platform: "universal".into(),
                profile: "vrm-humanoid-v0".into(),
                approved_only: false,
                thumbnails: true,
                dry_run: false,
                prune: false,
                protected: ProtectedPolicy::Passport,
            }
        }

        pub fn avatars_dir(&self) -> PathBuf {
            self.asset_root.join("avatars")
        }

        pub fn manifest_path(&self) -> PathBuf {
            self.manifest
                .clone()
                .unwrap_or_else(|| self.avatars_dir().join("manifest.json"))
        }
    }

    #[derive(Clone, Debug, PartialEq, Eq)]
    pub enum SyncOutcome {
        Staged { slug: String, reused: bool },
        Skipped { reason: String },
        Failed { detail: String },
    }

    #[derive(Clone, Debug, Default)]
    pub struct SyncReport {
        pub staged: Vec<String>,
        pub reused: usize,
        pub skipped: Vec<(String, String)>,
        pub failed: Vec<(String, String)>,
        pub manifest: Option<PathBuf>,
    }

    /// Stage every eligible avatar and merge the manifest. Failures of single
    /// avatars are reported, not fatal, so one dead IPFS pin never blocks a
    /// whole sync.
    pub fn sync_roster(
        avatars: &[EkzaAvatar],
        options: &SyncOptions,
    ) -> Result<SyncReport, CacheError> {
        let avatars_dir = options.avatars_dir();
        let cache = AssetCache::new(avatars_dir.join(".ekza-cache"))?;
        let manifest_path = options.manifest_path();
        let mut roster = Roster::read_or_default(&manifest_path)?;
        let mut report = SyncReport::default();
        let mut synced_slugs = Vec::new();

        for avatar in avatars {
            let outcome = sync_one(avatar, options, &cache, &avatars_dir, &mut roster);
            match outcome {
                SyncOutcome::Staged { slug, reused } => {
                    if reused {
                        report.reused += 1;
                    }
                    synced_slugs.push(slug.clone());
                    report.staged.push(slug);
                }
                SyncOutcome::Skipped { reason } => report.skipped.push((avatar.name.clone(), reason)),
                SyncOutcome::Failed { detail } => report.failed.push((avatar.name.clone(), detail)),
            }
        }

        if options.prune {
            roster.retain_synced(&synced_slugs);
        }
        if !options.dry_run {
            roster.write(&manifest_path)?;
            report.manifest = Some(manifest_path);
        }
        Ok(report)
    }

    fn sync_one(
        avatar: &EkzaAvatar,
        options: &SyncOptions,
        cache: &AssetCache,
        avatars_dir: &Path,
        roster: &mut Roster,
    ) -> SyncOutcome {
        let project = options.project_id.as_deref();
        let is_protected = avatar.origin != AvatarOrigin::Library
            && avatar.origin != AvatarOrigin::Local;
        let approved = project.is_some_and(|project| avatar.is_approved_for(project));
        if options.approved_only && !approved {
            return SyncOutcome::Skipped {
                reason: "not approved for the requested project".into(),
            };
        }
        let mut passport = None;
        if is_protected {
            match options.protected {
                ProtectedPolicy::Skip => {
                    return SyncOutcome::Skipped {
                        reason: "registry/passport template (protected policy: skip)".into(),
                    };
                }
                ProtectedPolicy::Free => {}
                ProtectedPolicy::Passport => {
                    let Some(project) = project else {
                        return SyncOutcome::Skipped {
                            reason: "protected template needs --project to select an approval".into(),
                        };
                    };
                    if !approved {
                        return SyncOutcome::Skipped {
                            reason: format!("template is not approved for `{project}`"),
                        };
                    }
                    match protected_avatar(avatar, project) {
                        Ok(protected) => passport = Some(protected),
                        Err(reason) => return SyncOutcome::Skipped { reason: reason.into() },
                    }
                }
            }
        }
        let Some(rendition) = avatar.gltf_rendition(project, &options.platform, &options.profile)
        else {
            return SyncOutcome::Skipped {
                reason: "no binary glTF rendition".into(),
            };
        };
        if let Some(protected) = &passport
            && protected.support.rendition.sha256 != rendition.sha256.clone().unwrap_or_default()
        {
            return SyncOutcome::Skipped {
                reason: "approved rendition is not the one selected for staging".into(),
            };
        }
        // Paid entries use the shared passport slug so every tool and the game
        // server agree on one name per owned rendition.
        let slug = passport
            .as_ref()
            .map_or_else(|| avatar.slug(), crate::passport::protected_slug);
        if options.dry_run {
            return SyncOutcome::Staged { slug, reused: false };
        }
        let cached = match cache.fetch_model(rendition) {
            Ok(cached) => cached,
            Err(error) => {
                return SyncOutcome::Failed {
                    detail: error.to_string(),
                };
            }
        };
        let destination = avatars_dir.join(format!("{slug}.glb"));
        if let Err(error) = place(&cached.path, &destination) {
            return SyncOutcome::Failed {
                detail: format!("cannot stage {}: {error}", destination.display()),
            };
        }
        let thumbnail = if options.thumbnails {
            avatar.thumbnail_url.as_deref().and_then(|url| {
                let (file, kind) = cache.fetch_thumbnail(url).ok()?;
                let name = format!("{slug}.{}", kind.extension());
                place(&file.path, &avatars_dir.join(&name)).ok()?;
                Some(name)
            })
        } else {
            None
        };
        let mut entry = RosterEntry::from_avatar(
            avatar,
            rendition,
            &cached.sha256,
            cached.size_bytes,
            thumbnail,
        );
        entry.model = Some(format!("avatars/{slug}.glb"));
        entry.slug = slug.clone();
        entry.passport = passport;
        if !roster.upsert(entry) {
            return SyncOutcome::Skipped {
                reason: format!("shipped roster entry `{slug}` keeps precedence"),
            };
        }
        SyncOutcome::Staged {
            slug,
            reused: cached.reused,
        }
    }

    /// Copy a verified cache file to its staged name unless the staged copy is
    /// already byte-identical in size (the cache is content-addressed, so a
    /// size match under the same slug means the same file).
    fn place(source: &Path, destination: &Path) -> std::io::Result<()> {
        if let (Ok(existing), Ok(fresh)) = (fs::metadata(destination), fs::metadata(source))
            && existing.len() == fresh.len()
            && fs::read(destination)? == fs::read(source)?
        {
            return Ok(());
        }
        let temporary = destination.with_extension(format!("part-{}", std::process::id()));
        fs::copy(source, &temporary)?;
        fs::rename(temporary, destination)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::{AvatarRendition, ProjectApproval};

    fn avatar() -> EkzaAvatar {
        EkzaAvatar {
            origin: AvatarOrigin::Registry,
            id: "solana:devnet:avatar-data:3bfPehBVoBKXUUzmGGVT3tgQTYkpispqUKfPW1UktASL".into(),
            name: "Robert".into(),
            collection: None,
            author: None,
            license: Some("CC0-1.0".into()),
            description: None,
            thumbnail_url: None,
            source_url: Some("ar://gwG7".into()),
            tags: vec![],
            renditions: vec![AvatarRendition {
                id: "sha256:546d".into(),
                format: "glb".into(),
                platform: "desktop".into(),
                profile: "humanoid-glb-v1".into(),
                url: "https://registry.ekza.io/v1/assets/546d.glb".into(),
                sha256: Some("546d".into()),
                size_bytes: Some(10),
                media_type: None,
            }],
            project_support: vec![ProjectApproval {
                project_id: "omoba".into(),
                platform: "desktop".into(),
                profile: "humanoid-glb-v1".into(),
                status: "approved".into(),
            }],
        }
    }

    #[test]
    fn roster_entries_stay_omoba_compatible() {
        let avatar = avatar();
        let entry = RosterEntry::from_avatar(&avatar, &avatar.renditions[0], "546d", 10, None);
        let json = serde_json::to_value(&entry).unwrap();
        for required in ["slug", "display_name", "collection", "license", "source_url"] {
            assert!(json[required].is_string(), "{required} must be a string");
        }
        assert!(json.get("passport").is_none(), "no passport unless imported");
        assert_eq!(json["approved_projects"], serde_json::json!(["omoba"]));
        assert_eq!(entry.model.as_deref(), Some(format!("avatars/{}.glb", entry.slug).as_str()));

        // An Omoba-shaped manifest (no extension fields) round-trips.
        let omoba_manifest = r#"{"avatars":[{"author":"Polygonal-Mind","collection":"100Avatars R3","display_name":"Agnes","license":"CC0","slug":"agnes","source_url":"https://arweave.net/x","thumbnail":"agnes.jpg"}]}"#;
        let roster: Roster = serde_json::from_str(omoba_manifest).unwrap();
        assert_eq!(roster.avatars[0].slug, "agnes");
        assert!(roster.avatars[0].ekza_id.is_none());
    }

    #[test]
    fn shipped_entries_keep_precedence_and_prune_only_touches_synced() {
        let mut roster: Roster = serde_json::from_str(
            r#"{"avatars":[{"display_name":"Agnes","collection":"c","license":"CC0","slug":"agnes","source_url":"u"}]}"#,
        )
        .unwrap();
        let avatar = avatar();
        let mut synced = RosterEntry::from_avatar(&avatar, &avatar.renditions[0], "546d", 10, None);
        assert!(roster.upsert(synced.clone()));
        synced.slug = "agnes".into();
        assert!(!roster.upsert(synced), "shipped entry must not be replaced");
        assert_eq!(roster.avatars.len(), 2);
        roster.retain_synced(&[]);
        assert_eq!(roster.avatars.len(), 1);
        assert_eq!(roster.avatars[0].slug, "agnes");
    }
}
