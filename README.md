# Ekza Bevy SDK

`ekza-bevy-sdk` is the first extracted SDK surface for using Ekza-Stellar universe character identities and 3D model metadata outside the Omoba Bevy game.

This repository is the standalone SDK home. Omoba Bevy consumes it locally through a sibling path dependency, and the crate is shaped so it can later be published and consumed as a dependency by other Bevy projects.

## Public Surface

- `EkzaCharacter` - stable serde-compatible character ids (`ipfs`, `toka`, `wang`, `cube`).
- `BUILTIN_MODEL_MANIFEST` - built-in character-to-model metadata.
- `validation::validate_glb_bytes` - typed GLB validation report with extensible rules and issues.
- `validation::validate_glb_file` - file-level GLB validation helper for tooling.
- `is_valid_glb_bytes` - compatibility shorthand for the default GLB validation rules.
- `bevy::EkzaModelCatalog` - Bevy resource-friendly catalog of `Scene` and `Gltf` handles.
- `bevy::load_builtin_model_catalog` - resolves local downloaded GLBs and caches remote GLBs under a consumer asset root.

## Developer Tools

Validate a GLB file:

```bash
cargo run --example model_check -- path/to/model.glb --strict
```

Resolve and validate the built-in model sources for a consumer asset root:

```bash
cargo run --example model_cache -- --asset-root ../omoba-bevy/client/assets --all
```

Open the interactive model viewport:

```bash
cargo run --example model_viewer -- --character ipfs --asset-root assets
cargo run --example model_viewer -- --glb path/to/model.glb
```

Viewer controls: hold right mouse and drag to orbit, mouse wheel to zoom, `Esc` to quit.

## Current Boundaries

- This crate does not perform account auth, entitlement checks, CDN signing, or asset licensing enforcement yet.
- Remote model downloads are blocking and intended for the current desktop prototype path.
- Gameplay protocol and MOBA-specific state remain in the game crates for now.
