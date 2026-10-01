# Ekza Bevy SDK

`ekza-bevy-sdk` brings every Ekza avatar — the free CC0 library, the approved
devnet templates and a player's purchased NFTs — into a 2D or 3D game. The core
is engine-agnostic Rust; Bevy helpers are an optional feature.

The rule the SDK enforces: **a game gets a list of avatars it may use, the
exact file for each one, and a way to verify the bytes**. Ownership and
approval decisions stay with the passport service and the registry operator;
a file format, a URL or a slug is never an entitlement.

## Where avatars come from

| Feed | Endpoint | Content | SDK entry point |
| --- | --- | --- | --- |
| Free library | `GET https://registry.ekza.io/library/avatars` | Hundreds of CC0 VRM avatars (Open Source Avatars) pinned to IPFS, with thumbnails | `RegistryClient::library` |
| Approved registry | `GET https://registry.ekza.io/v1/avatars` | Devnet-minted templates with hashed per-platform renditions and per-project approvals (`projectSupport`) | `RegistryClient::catalog` |
| Passport catalogue | `GET https://avatar.ekza.io/api/passport/catalog` | Purchasable templates and their approved game renditions (public, no ownership claim) | `PassportCatalogClient::catalog` |
| Passport library | `GET …/api/passport/library` (bearer) | The paired wallet's purchased avatars | `passport::client::PassportClient` |

All feeds normalize into one `catalog::EkzaAvatar` (`id`, `origin`, name,
license, thumbnail, `renditions[]`, `project_support[]`), merged by identity
with `catalog::merge_avatars`.

## Quick start: sync all avatars into a game

```bash
# Everything public into an asset root (Omoba layout), preferring the
# operator-approved Omoba rendition when one exists.
cargo run --example registry_sync --no-default-features --features http -- \
    --asset-root ../omoba-bevy/client/assets \
    --project omoba --platform desktop --profile humanoid-glb-v1

# Only avatars explicitly approved for your project.
cargo run --example registry_sync --no-default-features --features http -- \
    --asset-root assets --project my-game --approved-only
```

Registry and passport templates are products, even when their approved file
is publicly downloadable. By default (`--protected passport`) they are staged
only when approved for `--project` and the manifest entry carries a `passport`
boundary, so an Omoba-style server still demands a consumed ticket before
admitting the cosmetic; `--protected skip` leaves them out; `--protected free`
stages them as free cosmetics for previews only. Free library avatars are
always staged as free.

Result under `<asset-root>/avatars/`: `manifest.json` (roster), `<slug>.glb`
per avatar (VRM is a glTF binary and is staged as `.glb`), `<slug>.png|jpg`
thumbnails, and a content-addressed `.ekza-cache/` of verified downloads.
Re-running is idempotent: valid cached files are reused, entries a game
shipped by hand keep precedence over synced ones, `--prune` drops synced
entries no longer in the feeds.

From code:

```rust
use ekza_bevy_sdk::{
    catalog::merge_avatars,
    registry::RegistryClient,
    roster::sync::{SyncOptions, sync_roster},
};

let registry = RegistryClient::default_registry()?;
let avatars = merge_avatars([registry.catalog(None)?, registry.library(None)?]);

let mut options = SyncOptions::new("assets");
options.project_id = Some("omoba".into());
options.platform = "desktop".into();
options.profile = "humanoid-glb-v1".into();
let report = sync_roster(&avatars, &options)?;
println!("{} avatars staged", report.staged.len());
```

## Loading in Bevy

```rust
use ekza_bevy_sdk::bevy::{EkzaRosterCatalog, load_roster_catalog};

fn setup(mut commands: Commands, asset_server: Res<AssetServer>) {
    let catalog = load_roster_catalog(&asset_server, Path::new("assets"));
    for slug in catalog.slugs() {
        if let Some(scene) = catalog.scene(slug) {
            commands.spawn(SceneRoot(scene));
        }
    }
    commands.insert_resource(catalog);
}
```

`EkzaRosterCatalog` keys handles by roster slug and exposes thumbnails
(`avatars/<file>`) for 2D pickers. Non-Bevy engines read `manifest.json`
directly: every entry has `model`, `sha256`, `size_bytes`, `format` and
`approved_projects`.

## Runtime store: purchases reach a game that is already installed

`roster::sync` fills an asset root at build time. A live game also needs to
show an avatar that was published or bought *after* it shipped, to its owner
and to everyone else in the match. `store::AvatarStore` does that:

```rust
use ekza_bevy_sdk::{passport::SupportSelector, store::AvatarStore};

let selector = SupportSelector::new("my-game", "desktop", "humanoid-glb-v1", &["glb"]);
let store = AvatarStore::new(user_data_dir, "https://registry.ekza.io", selector)?;
let offline = store.cached();          // last catalogue, no network
let items = store.refresh()?;          // templates an operator approved for my-game
let path = store.install(&items[0], |bytes| my_rig_check(bytes))?; // verified, then placed
```

A failed refresh leaves the persisted catalogue unchanged. Only a missing v2
endpoint (HTTP 404) triggers v1 compatibility; outages and incomplete catalogues
are errors. Callers can continue showing `store.cached()` until recovery. Server
admission remains authoritative.

Every item is named by `passport::protected_slug` (`ekza-<sha256>` over the
avatar identity and the exact rendition hash). That one name goes on the wire:

- the **owner's client** installs the file, asks the passport for a one-use
  ticket and joins with `slug + ticket`;
- the **game server** consumes the ticket at its configured passport origin
  and recomputes the slug from the grant, so it needs no avatar files and no
  pre-synced manifest;
- **other clients** map the slug back to a rendition through the public
  catalogue and install it on demand, showing a stand-in model until then.

The catalogue carries no entitlement: it says what a slug *is*, never who may
wear it. In Bevy, mount the store root as an asset source
(`app.register_asset_source("ekza", AssetSourceBuilder::platform_default(root, None))`
before `DefaultPlugins`) and load `ekza://avatars/<slug>.glb`.

## Wallet pairing inside a game

A game cannot block its main thread for a browser approval.
`passport::pairing::PairingFlow` runs the device flow on a worker thread:

```rust
use ekza_bevy_sdk::passport::{client::PassportClient, pairing::{PairingFlow, PairingState, open_in_browser}};

let flow = PairingFlow::start(PassportClient::new("https://avatar.ekza.io/api/passport", "my-game")?);
// once per frame:
match flow.state() {
    PairingState::AwaitingApproval { user_code, verification_url, .. } => {
        // show both; optionally open_in_browser(&verification_url) once
    }
    PairingState::Connected => { let session = flow.take_session(); /* owned avatars */ }
    PairingState::Failed(reason) => { /* show it, offer retry */ }
    _ => {}
}
```

`cargo run --example passport_pair --no-default-features --features http -- --project my-game`
does the same from a terminal and is a quick probe against any passport.

## Purchased avatars in a native game

```rust
use ekza_bevy_sdk::passport::{SupportSelector, client::{PassportClient, pair_interactively}};

let selector = SupportSelector::new("my-game", "desktop", "humanoid-glb-v1", &["glb", "vrm"]);
let api = PassportClient::new("https://avatar.ekza.io/api/passport", "my-game")?;
let session = pair_interactively(api)?;             // player approves in the browser
for protected in session.supported(&selector) {      // owned + approved for my-game
    let bytes = session.api.download(&protected, &selector)?; // size, SHA-256, GLB verified
    let ticket = session.ticket(&protected, &selector, "match-42")?; // one-use, session-bound
    // send ticket.ticket to the game server; the server calls
    // PassportClient::consume(ticket, "match-42") at its configured origin
    // and checks the returned identity/rendition with ProtectedAvatar::validate_for.
}
```

## Public surface

- `catalog` — `EkzaAvatar`, `AvatarRendition`, `ProjectApproval`, wire types
  for all three feeds, `merge_avatars`, `slugify`.
- `registry` (`http`) — `RegistryClient` (library, catalogue, resolve),
  `PassportCatalogClient`; HTTPS only except loopback.
- `cache` (`http`) — `AssetCache::fetch_model` / `fetch_thumbnail`, atomic
  content-addressed writes, size/SHA-256/GLB checks, real image-type sniffing.
- `roster` — `Roster`, `RosterEntry` (Omoba `manifest.json` superset),
  `roster::sync::sync_roster` (`http`).
- `passport` — `PurchasedLibrary`, `ProtectedAvatar`, `ConsumedTicket`,
  `SupportSelector`, `validate_project_support`; `passport::client` (`http`)
  — pairing, library, tickets, verified downloads.
- `validation` — `validate_glb_bytes` / `validate_glb_file`.
- `bevy` (`bevy`) — `EkzaRosterCatalog`, `load_roster_catalog`, plus the
  legacy `EkzaModelCatalog` / `load_builtin_model_catalog`.
- Legacy: `EkzaCharacter`, `BUILTIN_MODEL_MANIFEST` (first Omoba slice).

Features: `http` (reqwest + sha2; rustls, no OpenSSL), `bevy` (implies `http`).
Servers and shared crates use `default-features = false`; add `http` when they
need the passport client.

## Developer tools

```bash
cargo run --example model_check -- path/to/model.glb --strict
cargo run --example model_cache -- --asset-root ../omoba-bevy/client/assets --all
cargo run --example model_viewer -- --character ipfs --asset-root assets
cargo run --example registry_sync --no-default-features --features http -- --help
```

## Boundaries

- Animation clips are a game profile concern. Omoba expects `idle`/`walk`
  (and `attack`/`cast`/`death`) clips embedded in each GLB and bakes them
  with `scripts/retarget_animations.py`; raw library VRMs ship none, so run
  that step after `registry_sync`, or publish a project rendition in the
  registry that already carries the clips (as Robert's `desktop /
  humanoid-glb-v1` rendition does).
- Passport structs describe evidence, not ownership. Servers consume one-use
  tickets at their configured passport origin and never trust a
  client-submitted mint, slug or JSON object.
- This crate does not handle account passwords, payments, CDN signing or licence
  enforcement. Downloads are blocking; run them off the render thread.
- Network downgrade is refused: `http://` is accepted only for loopback hosts.

## Persist an Ekza account connection

After device-code approval, save `session.credential()` with
`AccountCredential::save(path)` in a dedicated private directory. On startup,
load it with `AccountCredential::load(path)` and call
`AccountClient::restore(&credential, &selector)`. Restore validates the exact
Registry URL and project before sending the token, then reads the live library.
No cached display name is trusted as an identity.

Use `refresh_checked()` periodically. `AccountError::Unauthorized` means the
connection expired or was revoked: clear the session and its file. Preserve the
credential on `Unavailable` and retry. `InvalidCredential` prevents using a saved
connection against a different server or project. Logout removes the file;
Studio Account can revoke a connection server-side.

New Registry connections last up to 90 days. Existing connections keep their
original expiration. Tokens are opaque, project-scoped credentials, not wallet
keys or Studio administration sessions. Never log or publish the credential.
The file helper uses atomic replacement and Unix owner-only permissions; native
applications may instead store its serialized form in their platform keychain.

### Native LAN development

Debug builds may explicitly set `EKZA_DEV_HTTP_HOST` to one private IPv4 address (runtime environment or Cargo build environment). Registry, account approval, verified downloads and browser links then accept HTTP on that exact host. This does not skip authentication, approval, size or hash checks. Release builds ignore this setting; use HTTPS there. Never use a production account on a plain-HTTP test network. `passport::pairing::safe_url_with_query` is available for native platform browser bridges.

## Approved weapons and typed assets

SDK 0.8 adds `assets::{AssetKind, AssetStore, StoreAsset}` and
`RegistryClient::assets_v2` for `/v2/assets`. Weapons have canonical IDs
`ekza:weapon:<uuid>`; they cannot pass avatar identity validation. The typed
catalogue uses `ekza.asset.catalog.v2` with `count`, `items`, an explicit
`assetKind`, and the existing rendition/project approval fields.

```rust,no_run
use ekza_bevy_sdk::{assets::{AssetKind, AssetStore}, passport::SupportSelector};
# fn example() -> Result<(), String> {
let store = AssetStore::new(
    "/writable/game/ekza-store",
    "https://registry.ekza.io",
    AssetKind::Weapon,
    SupportSelector::new("omoba", "desktop", "handheld-glb-v1", &["glb"]),
)?;
let approved = store.refresh()?;
// Run off the rendering thread. Validate the game's static GLB/grip contract
// inside the callback before making the returned path renderable.
# fn validate_handheld_glb(_: &[u8]) -> Result<(), String> { Ok(()) }
if let Some(item) = approved.first() {
    let model = store.install(item, validate_handheld_glb)?;
}
# Ok(()) }
```

`assets::templates` and `assets::validate_item` expose the same approval checks
to game servers that only need catalogue metadata. Only explicitly **free**,
exactly approved GLB renditions qualify. Paid weapons require a future
entitlement flow and are deliberately excluded. Unknown kinds/identities and
unapproved or ambiguous rendition selectors fail closed.

Files use `weapons/<slug>.glb` and the shared verified `.ekza-cache`. Catalogue
metadata is isolated by registry origin, asset kind and selector, so avatar and
weapon stores can share one writable engine asset source. A failed/incomplete
refresh preserves the last complete catalogue; a successful empty catalogue
removes eligibility but retains bytes. Reused installs still run the consumer
validator. A server must independently read approvals and check current selection
eligibility: having cached bytes, or passing `install` an older approved item,
does not prove a release is still available. The SDK does not accept gameplay
packets or decide server refresh/revocation policy.

## License

Licensed under either of

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE) or <https://www.apache.org/licenses/LICENSE-2.0>)
- MIT license ([LICENSE-MIT](LICENSE-MIT) or <https://opensource.org/licenses/MIT>)

at your option. This is the same dual license used across the Bevy ecosystem,
so the SDK can be embedded in open-source and closed-source games alike. The
copyright holder also offers earlier revisions of this repository under the
same terms.

The license covers this crate's code and the data formats it defines. It does
not grant rights to avatars or other content served by an Ekza registry (each
asset carries its own terms), to Ekza services, or to the Ekza name and logo;
see [TRADEMARKS.md](TRADEMARKS.md).

### Contribution

Unless you explicitly state otherwise, any contribution intentionally submitted
for inclusion in the work by you, as defined in the Apache-2.0 license, shall be
dual licensed as above, without any additional terms or conditions.
