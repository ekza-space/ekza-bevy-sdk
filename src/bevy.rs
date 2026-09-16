use std::{collections::HashMap, fs, path::Path};

use ::bevy::{gltf::Gltf, prelude::*};

use crate::{
    BUILTIN_MODEL_MANIFEST, EkzaCharacter, GlbValidationRules, ModelEntry, ModelSource,
    validate_glb_bytes, validate_glb_file,
};

#[derive(Clone)]
pub struct EkzaModelHandles {
    pub scene: Option<Handle<WorldAsset>>,
    pub gltf: Option<Handle<Gltf>>,
    pub label: String,
}

#[derive(Resource, Clone, Default)]
pub struct EkzaModelCatalog {
    entries: HashMap<EkzaCharacter, EkzaModelHandles>,
}

impl EkzaModelCatalog {
    pub fn insert(&mut self, character: EkzaCharacter, handles: EkzaModelHandles) {
        self.entries.insert(character, handles);
    }

    pub fn handles_for(
        &self,
        character: EkzaCharacter,
    ) -> (Option<Handle<WorldAsset>>, Option<Handle<Gltf>>) {
        self.entries
            .get(&character)
            .map(|entry| (entry.scene.clone(), entry.gltf.clone()))
            .unwrap_or((None, None))
    }

    pub fn label_for(&self, character: EkzaCharacter) -> String {
        self.entries
            .get(&character)
            .map(|entry| entry.label.clone())
            .unwrap_or_else(|| character.as_str().to_string())
    }
}

pub fn load_builtin_model_catalog(
    asset_server: &AssetServer,
    asset_root: &Path,
) -> EkzaModelCatalog {
    let mut catalog = EkzaModelCatalog::default();
    for entry in BUILTIN_MODEL_MANIFEST {
        let handles = load_model_entry(entry, asset_server, asset_root);
        catalog.insert(entry.character, handles);
    }
    catalog
}

pub fn load_model_entry(
    entry: ModelEntry,
    asset_server: &AssetServer,
    asset_root: &Path,
) -> EkzaModelHandles {
    match entry.source {
        ModelSource::LocalGlb { path, scene_label } => {
            let final_path = asset_root.join(path);
            if final_path.exists() {
                match validate_glb_file(&final_path, &GlbValidationRules::default()) {
                    Ok(report) if report.is_valid() => {
                        return glb_handles(asset_server, path, scene_label, path.to_string());
                    }
                    Ok(report) => {
                        warn!(
                            "Local Ekza model {final_path:?} failed validation: {:?}",
                            report.issues()
                        );
                    }
                    Err(error) => {
                        warn!("Failed to read local Ekza model {final_path:?}: {error}");
                    }
                }
            }
            EkzaModelHandles {
                scene: None,
                gltf: None,
                label: entry.display_name.to_string(),
            }
        }
        ModelSource::RemoteGlb {
            url,
            cache_path,
            scene_label,
        } => {
            if cache_remote_glb(url, asset_root, cache_path).is_some() {
                glb_handles(
                    asset_server,
                    cache_path,
                    scene_label,
                    cache_path.to_string(),
                )
            } else {
                EkzaModelHandles {
                    scene: None,
                    gltf: None,
                    label: entry.display_name.to_string(),
                }
            }
        }
        ModelSource::PrimitiveFallback => EkzaModelHandles {
            scene: None,
            gltf: None,
            label: entry.display_name.to_string(),
        },
    }
}

pub fn cache_remote_glb(url: &str, asset_root: &Path, cache_path: &str) -> Option<String> {
    use reqwest::blocking as req_blocking;

    let final_path = asset_root.join(cache_path);
    if let Ok(existing) = fs::read(&final_path)
        && validate_glb_bytes(&existing, &GlbValidationRules::default()).is_valid()
    {
        return Some(cache_path.to_string());
    }

    if let Some(parent) = final_path.parent()
        && let Err(error) = fs::create_dir_all(parent)
    {
        warn!("Failed to create SDK model cache directory {parent:?}: {error}");
        return None;
    }

    let response = match req_blocking::get(url) {
        Ok(response) => response,
        Err(error) => {
            warn!("Failed to download Ekza model {url}: {error}");
            return None;
        }
    };
    let bytes = response
        .bytes()
        .map_err(|error| warn!("Failed to read bytes from Ekza model {url}: {error}"))
        .ok()?;
    let report = validate_glb_bytes(&bytes, &GlbValidationRules::default());
    if !report.is_valid() {
        warn!("Downloaded Ekza model from {url} is not a valid GLB.");
        return None;
    }
    if let Err(error) = fs::write(&final_path, &bytes) {
        warn!("Failed to write Ekza model cache file {final_path:?}: {error}");
        return None;
    }
    Some(cache_path.to_string())
}

fn glb_handles(
    asset_server: &AssetServer,
    path: &str,
    scene_label: &str,
    label: String,
) -> EkzaModelHandles {
    EkzaModelHandles {
        scene: Some(asset_server.load(format!("{path}#{scene_label}"))),
        gltf: Some(asset_server.load(path.to_string())),
        label,
    }
}
