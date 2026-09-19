# Changelog

## 0.6.0 — 2026-09-20

### Added

- `account`: connect a game to an Ekza account with no wallet. `AccountClient` starts
  a device flow at `{registry}/v1/account/device`, `AccountFlow` runs it on a worker
  thread and reuses the renderable `passport::pairing::PairingState`, and
  `AccountSession` reads `{registry}/v1/account/library`: the avatars that account
  saved or created and that are approved for this game, as `StoreAvatar`s validated
  exactly like the public store. `AccountSession::refresh` re-reads it after the
  player saved something in the browser. The device code and the token stay in
  memory and have no `Debug`. The token is not an entitlement; admission stays the
  game server's decision.
- `examples/account_pair`: engine-free CLI that connects an account and lists the
  library for a project and rendition selector.

## 0.5.0 — 2026-09-19

### Added

- The unified catalogue `GET {registry}/v2/avatars`: `RegistryClient::catalog_v2`,
  `catalog::CatalogV2Avatar` and `store::templates_v2`. It carries on-chain templates
  and avatars published through Ekza Studio in one shape, already narrowed to what a
  project approved. `AvatarStore::refresh` prefers it and falls back to `/v1/avatars`
  for a registry that does not serve it yet.
- `StoreAvatar::free`: true only when the registry marked the avatar `"free"`. The
  boundary still pins the exact rendition (the slug is a hash of identity and
  rendition), but a game may admit it with no ownership proof. A missing or unknown
  access value is treated as owned. Persisted store documents from 0.4 load as owned.

### Changed

- `passport::validate_avatar_id` accepts a second identity scheme,
  `ekza:avatar:<uuid>`, next to `solana:devnet:avatar-data:<PDA>`. Studio avatars
  have no chain record. `passport::valid_uuid` is public.

## 0.4.1 — 2026-09-18

### Added

- `passport::pairing::PairingFlow`: non-blocking wallet pairing for a game
  loop. The device flow runs on a worker thread; the UI polls a renderable
  `PairingState` (code and link to show, connected, failed) and collects the
  `NativeSession` once. Secrets never appear in the state. Dropping the handle
  cancels the attempt.
- `passport::pairing::open_in_browser`: opens the approval page in the default
  browser on desktop, only for HTTPS or explicit-localhost links.
- `examples/passport_pair`: engine-free CLI that pairs a wallet, lists the
  avatars it owns for a project selector and optionally installs them through
  `AvatarStore`. Doubles as an integration probe against any passport.

## 0.4.0 — 2026-09-18

The SDK now delivers the whole Ekza avatar catalogue to a game instead of a
hard-coded five-character manifest.

### Added

- `catalog`: unified `EkzaAvatar` over the free library
  (`/library/avatars`), the approved registry (`/v1/avatars`, including
  `projectSupport`) and the passport catalogue (`/api/passport/catalog`);
  `merge_avatars`, deterministic `slug()`, rendition selection helpers.
- `registry` (`http`): `RegistryClient` (paginated library, catalogue,
  resolve) and `PassportCatalogClient`. HTTPS only, loopback `http://`
  allowed for development, bounded JSON reads.
- `cache` (`http`): `AssetCache` — content-addressed, atomic, verified by
  declared size, SHA-256 and GLB envelope; thumbnails sniffed for their real
  container (`png`/`jpg`/`webp`).
- `roster`: `Roster`/`RosterEntry` manifest that is a strict superset of the
  Omoba `avatars/manifest.json` contract, plus `roster::sync::sync_roster`
  (`http`) that stages `avatars/<slug>.glb` and thumbnails and merges the
  manifest without overriding entries a game shipped by hand.
- `store`: runtime avatar store for a live game. `templates` keeps the
  registry templates approved for a `SupportSelector`; `AvatarStore` (`http`)
  refreshes and persists that catalogue for offline starts and installs one
  rendition on demand after size, SHA-256, GLB and a game-supplied validator
  pass. This is how a purchase made while the game is installed reaches its
  owner and every other player in the match.
- `passport::protected_slug` / `is_protected_slug`: the single
  `ekza-<sha256>` name of a paid rendition. `roster::sync` now stages
  passport-bound entries under it, so a game server can recompute the slug from
  a consumed ticket without any pre-synced manifest.
- `passport::client::NativeSession::from_parts` for hosts that pair through
  their own UI.
- `passport::SupportSelector`, `validate_project_support`,
  `ProtectedAvatar::validate_for` / `validate_bytes_for` for consumers other
  than Omoba.
- `passport::client` (`http`): native `PassportClient` (pairing, library,
  tickets, consume, verified download) generalized from the first Omoba
  integration, parameterized by project id.
- `bevy::EkzaRosterCatalog` / `load_roster_catalog` / `load_roster`.
- `examples/registry_sync`: CLI that fills any asset root from the public
  feeds (`--project`, `--platform/--profile`, `--approved-only`, `--prune`,
  `--offline-catalog`, `--dry-run`).
- Test fixtures captured from the real registry/passport payload shapes.

### Changed

- HTTP uses `rustls` (`reqwest` without default features): no system OpenSSL
  on Linux or mobile consumers.

- Features: `http` (reqwest with rustls + sha2) is new; `bevy` now implies
  `http`. `default = ["http", "bevy"]`. reqwest no longer pulls OpenSSL.
- `serde_json` is a regular dependency (roster I/O).

### Unchanged

- `EkzaCharacter`, `BUILTIN_MODEL_MANIFEST`, `validation`, the original
  `passport` types and `validate_omoba_support` keep their signatures, so
  Omoba's `shared`, `server` and `omoba-passport` crates build against this
  revision without changes.
