//! Ekza Bevy SDK surface for character identity and 3D model integration.
//!
//! The crate is intentionally small in this first extraction slice: it owns stable
//! character ids, model manifest metadata, GLB validation, and an optional Bevy
//! integration layer for resolving model assets into handles.

use serde::{Deserialize, Serialize};

pub mod passport;
pub mod validation;

pub use validation::{
    GLB_HEADER_LEN, GLB_MAGIC, GlbValidationRules, ModelFormat, ModelValidationIssue,
    ModelValidationReport, SUPPORTED_GLB_VERSION, validate_glb_bytes, validate_glb_file,
};

/// Stable character ids shared by the game, protocol, and future SDK consumers.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EkzaCharacter {
    #[default]
    Ipfs,
    Toka,
    Wang,
    Cube,
    /// CC0 VRM humanoid avatar (glTF 2.0 binary loaded through the glTF pipeline).
    Paco,
}

impl EkzaCharacter {
    pub const ALL: [Self; 5] = [Self::Ipfs, Self::Toka, Self::Wang, Self::Cube, Self::Paco];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Ipfs => "IPFS",
            Self::Toka => "Toka",
            Self::Wang => "Wang",
            Self::Cube => "Cube",
            Self::Paco => "Paco",
        }
    }

    pub fn slug(self) -> &'static str {
        match self {
            Self::Ipfs => "ipfs",
            Self::Toka => "toka",
            Self::Wang => "wang",
            Self::Cube => "cube",
            Self::Paco => "paco",
        }
    }
}

/// Static source metadata for one SDK model entry.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ModelSource {
    /// A GLB file already present under the consumer's Bevy asset root.
    LocalGlb {
        path: &'static str,
        scene_label: &'static str,
    },
    /// A remote GLB file to cache under the consumer's Bevy asset root.
    RemoteGlb {
        url: &'static str,
        cache_path: &'static str,
        scene_label: &'static str,
    },
    /// Consumer should render its own primitive fallback.
    PrimitiveFallback,
}

/// Built-in Ekza-Stellar character model metadata.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ModelEntry {
    pub character: EkzaCharacter,
    pub display_name: &'static str,
    pub source: ModelSource,
    pub locomotion_animations: bool,
}

pub const IPFS_CHARACTER_URL: &str =
    "https://ipfs.io/ipfs/QmWMYVUF2pa4GkoMgquyY8nmYjQJDP9yxnSBvjVqH7EJQr";
pub const IPFS_CHARACTER_CACHE_PATH: &str =
    "downloaded/QmWMYVUF2pa4GkoMgquyY8nmYjQJDP9yxnSBvjVqH7EJQr.glb";

pub const BUILTIN_MODEL_MANIFEST: [ModelEntry; 5] = [
    ModelEntry {
        character: EkzaCharacter::Ipfs,
        display_name: "IPFS",
        source: ModelSource::RemoteGlb {
            url: IPFS_CHARACTER_URL,
            cache_path: IPFS_CHARACTER_CACHE_PATH,
            scene_label: "Scene0",
        },
        locomotion_animations: false,
    },
    ModelEntry {
        character: EkzaCharacter::Toka,
        display_name: "Toka",
        source: ModelSource::LocalGlb {
            path: "downloaded/toka.glb",
            scene_label: "Scene0",
        },
        locomotion_animations: true,
    },
    ModelEntry {
        character: EkzaCharacter::Wang,
        display_name: "Wang",
        source: ModelSource::LocalGlb {
            path: "downloaded/wang.glb",
            scene_label: "Scene0",
        },
        locomotion_animations: true,
    },
    ModelEntry {
        character: EkzaCharacter::Cube,
        display_name: "Cube",
        source: ModelSource::PrimitiveFallback,
        locomotion_animations: false,
    },
    ModelEntry {
        character: EkzaCharacter::Paco,
        display_name: "Paco",
        // VRM 0.x is a glTF 2.0 binary; the asset is staged as `.glb` so the
        // standard glTF loader picks it up. The avatar ships no animation clips,
        // so it renders as a static skinned mesh (locomotion_animations: false).
        source: ModelSource::LocalGlb {
            path: "downloaded/paco.glb",
            scene_label: "Scene0",
        },
        locomotion_animations: false,
    },
];

pub fn builtin_model_entry(character: EkzaCharacter) -> &'static ModelEntry {
    BUILTIN_MODEL_MANIFEST
        .iter()
        .find(|entry| entry.character == character)
        .expect("every EkzaCharacter must have a built-in model entry")
}

pub fn is_valid_glb_bytes(bytes: &[u8]) -> bool {
    validate_glb_bytes(bytes, &GlbValidationRules::default()).is_valid()
}

#[cfg(feature = "bevy")]
pub mod bevy;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn character_ids_keep_protocol_snake_case() {
        assert_eq!(
            serde_json::to_string(&EkzaCharacter::Ipfs).unwrap(),
            "\"ipfs\""
        );
        assert_eq!(
            serde_json::to_string(&EkzaCharacter::Toka).unwrap(),
            "\"toka\""
        );
        assert_eq!(
            serde_json::to_string(&EkzaCharacter::Wang).unwrap(),
            "\"wang\""
        );
        assert_eq!(
            serde_json::to_string(&EkzaCharacter::Cube).unwrap(),
            "\"cube\""
        );
        assert_eq!(
            serde_json::to_string(&EkzaCharacter::Paco).unwrap(),
            "\"paco\""
        );
    }

    #[test]
    fn manifest_covers_every_character() {
        for character in EkzaCharacter::ALL {
            assert_eq!(builtin_model_entry(character).character, character);
        }
    }

    #[test]
    fn glb_header_validation_checks_magic_version_and_declared_length() {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&GLB_MAGIC);
        bytes.extend_from_slice(&SUPPORTED_GLB_VERSION.to_le_bytes());
        bytes.extend_from_slice(&(GLB_HEADER_LEN as u32).to_le_bytes());
        assert!(is_valid_glb_bytes(&bytes));

        let mut bad_magic = bytes.clone();
        bad_magic[0] = b'X';
        assert!(!is_valid_glb_bytes(&bad_magic));

        let mut bad_length = bytes;
        bad_length[8..12].copy_from_slice(&64_u32.to_le_bytes());
        assert!(!is_valid_glb_bytes(&bad_length));
    }
}
