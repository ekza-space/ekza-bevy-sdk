//! Verified, content-addressed download cache for avatar renditions and
//! thumbnails (`http` feature).
//!
//! Every file is written atomically under `<root>/<sha256>.<ext>`. A rendition
//! that advertises a hash is checked byte-for-byte; one without a hash (free
//! library VRMs on IPFS) is hashed on arrival so the cache key is still stable.
//! Binary glTF renditions additionally pass the SDK GLB envelope check before
//! they are kept.

use std::{
    fs,
    io::Read,
    path::{Path, PathBuf},
    time::Duration,
};

use reqwest::blocking::Client;

use crate::{
    GlbValidationRules,
    catalog::AvatarRendition,
    registry::{RegistryError, build_client, feed_url},
    sha256::sha256_hex,
    validate_glb_bytes,
};

/// Upper bound for one model download when the feed declares no size.
pub const DEFAULT_MAX_MODEL_BYTES: u64 = 64 * 1024 * 1024;
pub const DEFAULT_MAX_THUMBNAIL_BYTES: u64 = 8 * 1024 * 1024;

#[derive(Debug)]
pub enum CacheError {
    Registry(RegistryError),
    Io(std::io::Error),
    SizeMismatch { expected: u64, actual: u64 },
    HashMismatch { expected: String, actual: String },
    TooLarge { limit: u64 },
    InvalidModel(String),
    UnsupportedThumbnail,
}

impl std::fmt::Display for CacheError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Registry(error) => write!(f, "{error}"),
            Self::Io(error) => write!(f, "cache I/O failed: {error}"),
            Self::SizeMismatch { expected, actual } => {
                write!(f, "download size {actual} differs from declared {expected}")
            }
            Self::HashMismatch { expected, actual } => {
                write!(f, "download SHA-256 {actual} differs from declared {expected}")
            }
            Self::TooLarge { limit } => write!(f, "download exceeded the {limit} byte limit"),
            Self::InvalidModel(detail) => write!(f, "downloaded model is invalid: {detail}"),
            Self::UnsupportedThumbnail => write!(f, "thumbnail is not a PNG, JPEG or WebP image"),
        }
    }
}

impl std::error::Error for CacheError {}

impl From<RegistryError> for CacheError {
    fn from(error: RegistryError) -> Self {
        Self::Registry(error)
    }
}

impl From<std::io::Error> for CacheError {
    fn from(error: std::io::Error) -> Self {
        Self::Io(error)
    }
}

/// A file that passed every applicable check.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CachedFile {
    pub path: PathBuf,
    pub sha256: String,
    pub size_bytes: u64,
    /// True when the file was already present and valid, so no request was made.
    pub reused: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ImageKind {
    Png,
    Jpeg,
    WebP,
}

impl ImageKind {
    pub fn extension(self) -> &'static str {
        match self {
            Self::Png => "png",
            Self::Jpeg => "jpg",
            Self::WebP => "webp",
        }
    }
}

/// Sniff the actual image container; several catalogue thumbnails are JPEGs
/// published under a `.png` name and engines pick decoders by extension.
pub fn sniff_image(bytes: &[u8]) -> Option<ImageKind> {
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        Some(ImageKind::Png)
    } else if bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
        Some(ImageKind::Jpeg)
    } else if bytes.len() >= 12 && &bytes[0..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        Some(ImageKind::WebP)
    } else {
        None
    }
}

pub fn atomic_write(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let temporary = path.with_extension(format!("part-{}", std::process::id()));
    fs::write(&temporary, bytes)?;
    fs::rename(&temporary, path)
}

#[derive(Clone)]
pub struct AssetCache {
    root: PathBuf,
    client: Client,
    max_model_bytes: u64,
    max_thumbnail_bytes: u64,
}

impl AssetCache {
    pub fn new(root: impl Into<PathBuf>) -> Result<Self, CacheError> {
        Ok(Self {
            root: root.into(),
            client: build_client(Duration::from_secs(300))?,
            max_model_bytes: DEFAULT_MAX_MODEL_BYTES,
            max_thumbnail_bytes: DEFAULT_MAX_THUMBNAIL_BYTES,
        })
    }

    pub fn with_limits(mut self, max_model_bytes: u64, max_thumbnail_bytes: u64) -> Self {
        self.max_model_bytes = max_model_bytes;
        self.max_thumbnail_bytes = max_thumbnail_bytes;
        self
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    fn model_path(&self, sha256: &str, extension: &str) -> PathBuf {
        self.root.join(format!("{sha256}.{extension}"))
    }

    fn download(&self, url: &str, limit: u64) -> Result<Vec<u8>, CacheError> {
        let url = feed_url(url)?;
        let display = url.to_string();
        let response = self
            .client
            .get(url)
            .send()
            .map_err(|error| RegistryError::Transport(format!("{display}: {error}")))?;
        let status = response.status();
        if !status.is_success() {
            return Err(RegistryError::Status {
                status: status.as_u16(),
                url: display,
            }
            .into());
        }
        if response.content_length().is_some_and(|size| size > limit) {
            return Err(CacheError::TooLarge { limit });
        }
        let mut bytes = Vec::new();
        response
            .take(limit + 1)
            .read_to_end(&mut bytes)
            .map_err(|error| RegistryError::Transport(format!("{display}: {error}")))?;
        if bytes.len() as u64 > limit {
            return Err(CacheError::TooLarge { limit });
        }
        Ok(bytes)
    }

    /// Validate model bytes against the rendition's declared size/hash and,
    /// for binary glTF formats, the GLB envelope.
    pub fn verify_model(rendition: &AvatarRendition, bytes: &[u8]) -> Result<String, CacheError> {
        if let Some(expected) = rendition.size_bytes
            && expected != bytes.len() as u64
        {
            return Err(CacheError::SizeMismatch {
                expected,
                actual: bytes.len() as u64,
            });
        }
        let actual = sha256_hex(bytes);
        if let Some(expected) = &rendition.sha256
            && !expected.eq_ignore_ascii_case(&actual)
        {
            return Err(CacheError::HashMismatch {
                expected: expected.clone(),
                actual,
            });
        }
        if rendition.is_gltf_binary() {
            let report = validate_glb_bytes(bytes, &GlbValidationRules::default());
            if !report.is_valid() {
                let issues: Vec<String> =
                    report.issues().iter().map(ToString::to_string).collect();
                return Err(CacheError::InvalidModel(issues.join("; ")));
            }
        }
        Ok(actual)
    }

    /// Return a verified local copy of the rendition, downloading only when the
    /// cache has no valid file for it.
    pub fn fetch_model(&self, rendition: &AvatarRendition) -> Result<CachedFile, CacheError> {
        let extension = rendition.file_extension();
        if let Some(sha256) = &rendition.sha256 {
            let path = self.model_path(&sha256.to_ascii_lowercase(), extension);
            if let Ok(existing) = fs::read(&path)
                && Self::verify_model(rendition, &existing).is_ok()
            {
                return Ok(CachedFile {
                    path,
                    sha256: sha256.to_ascii_lowercase(),
                    size_bytes: existing.len() as u64,
                    reused: true,
                });
            }
        } else {
            // No declared hash: reuse a previous download of the same URL via
            // its index entry.
            let index = self.url_index_path(&rendition.url, extension);
            if let Ok(sha256) = fs::read_to_string(&index) {
                let sha256 = sha256.trim().to_string();
                let path = self.model_path(&sha256, extension);
                if let Ok(existing) = fs::read(&path)
                    && sha256_hex(&existing) == sha256
                    && Self::verify_model(rendition, &existing).is_ok()
                {
                    return Ok(CachedFile {
                        path,
                        sha256,
                        size_bytes: existing.len() as u64,
                        reused: true,
                    });
                }
            }
        }

        let limit = rendition
            .size_bytes
            .map_or(self.max_model_bytes, |size| size.min(self.max_model_bytes));
        let bytes = self.download(&rendition.url, limit)?;
        let sha256 = Self::verify_model(rendition, &bytes)?;
        let path = self.model_path(&sha256, extension);
        atomic_write(&path, &bytes)?;
        if rendition.sha256.is_none() {
            atomic_write(&self.url_index_path(&rendition.url, extension), sha256.as_bytes())?;
        }
        Ok(CachedFile {
            path,
            sha256,
            size_bytes: bytes.len() as u64,
            reused: false,
        })
    }

    fn url_index_path(&self, url: &str, extension: &str) -> PathBuf {
        self.root
            .join("by-url")
            .join(format!("{}.{extension}.sha256", sha256_hex(url.as_bytes())))
    }

    /// Download a thumbnail, sniffing the real image type for the extension.
    pub fn fetch_thumbnail(&self, url: &str) -> Result<(CachedFile, ImageKind), CacheError> {
        let index = self.root.join("by-url").join(format!(
            "{}.thumb.sha256",
            sha256_hex(url.as_bytes())
        ));
        if let Ok(entry) = fs::read_to_string(&index) {
            let entry = entry.trim();
            if let Some((sha256, extension)) = entry.split_once(' ') {
                let path = self.root.join(format!("{sha256}.{extension}"));
                if let Ok(existing) = fs::read(&path)
                    && sha256_hex(&existing) == sha256
                    && let Some(kind) = sniff_image(&existing)
                {
                    return Ok((
                        CachedFile {
                            path,
                            sha256: sha256.to_string(),
                            size_bytes: existing.len() as u64,
                            reused: true,
                        },
                        kind,
                    ));
                }
            }
        }
        let bytes = self.download(url, self.max_thumbnail_bytes)?;
        let kind = sniff_image(&bytes).ok_or(CacheError::UnsupportedThumbnail)?;
        let sha256 = sha256_hex(&bytes);
        let path = self.root.join(format!("{sha256}.{}", kind.extension()));
        atomic_write(&path, &bytes)?;
        atomic_write(&index, format!("{sha256} {}", kind.extension()).as_bytes())?;
        Ok((
            CachedFile {
                path,
                sha256,
                size_bytes: bytes.len() as u64,
                reused: false,
            },
            kind,
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{GLB_HEADER_LEN, GLB_MAGIC, SUPPORTED_GLB_VERSION};

    fn glb(extra: usize) -> Vec<u8> {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&GLB_MAGIC);
        bytes.extend_from_slice(&SUPPORTED_GLB_VERSION.to_le_bytes());
        bytes.extend_from_slice(&((GLB_HEADER_LEN + extra) as u32).to_le_bytes());
        bytes.extend(std::iter::repeat_n(0u8, extra));
        bytes
    }

    fn rendition(bytes: &[u8], hashed: bool) -> AvatarRendition {
        AvatarRendition {
            id: "sha256:test".into(),
            format: "glb".into(),
            platform: "desktop".into(),
            profile: "humanoid-glb-v1".into(),
            url: "https://example.test/a.glb".into(),
            sha256: hashed.then(|| sha256_hex(bytes)),
            size_bytes: Some(bytes.len() as u64),
            media_type: None,
        }
    }

    #[test]
    fn verify_model_enforces_size_hash_and_envelope() {
        let bytes = glb(8);
        let good = rendition(&bytes, true);
        assert_eq!(AssetCache::verify_model(&good, &bytes).unwrap(), sha256_hex(&bytes));

        let mut wrong_size = good.clone();
        wrong_size.size_bytes = Some(1);
        assert!(matches!(
            AssetCache::verify_model(&wrong_size, &bytes),
            Err(CacheError::SizeMismatch { .. })
        ));

        let mut wrong_hash = good.clone();
        wrong_hash.sha256 = Some("0".repeat(64));
        assert!(matches!(
            AssetCache::verify_model(&wrong_hash, &bytes),
            Err(CacheError::HashMismatch { .. })
        ));

        let mut corrupt = bytes.clone();
        corrupt[0] = b'X';
        let unhashed = rendition(&corrupt, false);
        assert!(matches!(
            AssetCache::verify_model(&unhashed, &corrupt),
            Err(CacheError::InvalidModel(_))
        ));

        let mut usdz = rendition(b"PK\x03\x04", true);
        usdz.format = "usdz".into();
        assert!(AssetCache::verify_model(&usdz, b"PK\x03\x04").is_ok());
    }

    #[test]
    fn cached_hashed_model_is_reused_without_network() {
        let root = std::env::temp_dir().join(format!("ekza-cache-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let cache = AssetCache::new(&root).unwrap();
        let bytes = glb(4);
        let rendition = rendition(&bytes, true);
        let path = root.join(format!("{}.glb", sha256_hex(&bytes)));
        atomic_write(&path, &bytes).unwrap();
        let cached = cache.fetch_model(&rendition).unwrap();
        assert!(cached.reused);
        assert_eq!(cached.path, path);
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn image_sniffing_uses_real_container() {
        assert_eq!(sniff_image(b"\x89PNG\r\n\x1a\n...."), Some(ImageKind::Png));
        assert_eq!(sniff_image(&[0xFF, 0xD8, 0xFF, 0xE0]), Some(ImageKind::Jpeg));
        assert_eq!(sniff_image(b"RIFF\0\0\0\0WEBPVP8 "), Some(ImageKind::WebP));
        assert_eq!(sniff_image(b"glTF"), None);
    }
}
