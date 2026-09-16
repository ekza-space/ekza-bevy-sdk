# Ekza Bevy SDK

`ekza-bevy-sdk` is the first extracted SDK surface for using Ekza-Stellar universe character identities and 3D model metadata outside the Omoba Bevy game.

This repository is the standalone SDK home. Omoba Bevy pins an exact Git revision
of this crate, so a clean game checkout does not require a sibling SDK checkout.
Local Cargo overrides can be used while developing the two repositories together.

## Engine compatibility

This branch targets Bevy 0.19.1 and Rust 1.95 or later for the default Bevy feature.
Use a pinned older SDK revision with Bevy 0.18; Bevy handle/resource types from
different minor engine releases cannot be mixed. Identity, Passport and GLB
validation remain available with `default-features = false`.

## Public Surface

- `EkzaCharacter` - stable serde-compatible character ids (`ipfs`, `toka`, `wang`, `cube`).
- `BUILTIN_MODEL_MANIFEST` - built-in character-to-model metadata.
- `validation::validate_glb_bytes` - typed GLB validation report with extensible rules and issues.
- `validation::validate_glb_file` - file-level GLB validation helper for tooling.
- `passport` - serde-only purchased library, canonical avatar, approved project
  rendition and consumed-ticket types. `ProtectedAvatar::validate_bytes` checks
  exact size, caller-computed SHA-256 and GLB envelope;
  `validate_consumed_ticket` binds trusted approval to the exact roster rendition.
- `is_valid_glb_bytes` - compatibility shorthand for the default GLB validation rules.
- `bevy::EkzaModelCatalog` - Bevy resource-friendly catalog of `WorldAsset` and `Gltf` handles.
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

- Passport structs describe evidence, not ownership by themselves. The game
  server must consume a one-use project/session-bound ticket at its configured
  trusted passport API. Never authorize from a client-submitted mint, slug or
  JSON object. The native HTTP, SHA-256 and curated importer reference is the
  sibling Omoba `omoba-passport` crate; its runtime validates embedded humanoid
  skinning and the idle/walk/attack/cast/death profile before loading bytes.
- This crate does not perform account auth, network entitlement checks, CDN
  signing or legal asset licensing enforcement.
- Remote model downloads are blocking and intended for the current desktop prototype path.
- Gameplay protocol and MOBA-specific state remain in the game crates for now.
