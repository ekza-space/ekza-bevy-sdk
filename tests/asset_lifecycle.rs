#![cfg(feature = "http")]

use ekza_bevy_sdk::{
    assets::{
        AssetKind, AssetStore, CatalogAsset, MAX_WEAPON_BYTES, asset_slug, templates,
        validate_asset_id,
    },
    passport::{SupportSelector, validate_avatar_id},
    sha256_hex,
};
use serde_json::{Value, json};
use std::{
    collections::VecDeque,
    fs,
    io::{Read, Write},
    net::TcpListener,
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    thread,
    time::Duration,
};

const ID: &str = "ekza:weapon:11111111-2222-3333-4444-555555555555";

fn selector() -> SupportSelector {
    SupportSelector::new("omoba", "desktop", "handheld-glb-v1", &["glb"])
}

fn model() -> Vec<u8> {
    let mut json =
        br#"{"asset":{"version":"2.0"},"scene":0,"scenes":[{"nodes":[0]}],"nodes":[{}]}"#.to_vec();
    while json.len() % 4 != 0 {
        json.push(b' ');
    }
    let mut bytes = b"glTF".to_vec();
    bytes.extend_from_slice(&2_u32.to_le_bytes());
    bytes.extend_from_slice(&((20 + json.len()) as u32).to_le_bytes());
    bytes.extend_from_slice(&(json.len() as u32).to_le_bytes());
    bytes.extend_from_slice(b"JSON");
    bytes.extend(json);
    bytes
}

fn asset(url: &str, bytes: &[u8]) -> Value {
    json!({
        "assetKind":"weapon", "id":ID, "name":"Studio sword", "access":"free",
        "creator":{"name":"Example creator"}, "license":{"text":"CC0-1.0","attribution":"Example creator"},
        "projectSupport":[{"projectId":"omoba","platform":"desktop","profile":"handheld-glb-v1","status":"approved"}],
        "renditions":[{"platform":"desktop","profile":"handheld-glb-v1","format":"glb","sha256":sha256_hex(bytes),"sizeBytes":bytes.len(),"downloadUrl":url}]
    })
}

fn catalogue(items: Vec<Value>) -> Vec<u8> {
    serde_json::to_vec(&json!({"schema":"ekza.asset.catalog.v2","count":items.len(),"items":items}))
        .unwrap()
}

struct Directory(PathBuf);
impl Directory {
    fn new() -> Self {
        static SERIAL: AtomicU64 = AtomicU64::new(0);
        let root = std::env::temp_dir().join(format!(
            "ekza-assets-{}-{}",
            std::process::id(),
            SERIAL.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&root).unwrap();
        Self(root)
    }
}
impl Drop for Directory {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}

type Response = (u16, String, Vec<u8>);
struct Http {
    url: String,
    responses: Arc<Mutex<VecDeque<Response>>>,
    paths: Arc<Mutex<Vec<String>>>,
    stop: Arc<AtomicBool>,
    worker: Option<thread::JoinHandle<()>>,
}
impl Http {
    fn new() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        listener.set_nonblocking(true).unwrap();
        let responses = Arc::new(Mutex::new(VecDeque::<Response>::new()));
        let paths = Arc::new(Mutex::new(Vec::new()));
        let stop = Arc::new(AtomicBool::new(false));
        let (queue, requests, done) = (responses.clone(), paths.clone(), stop.clone());
        let worker = thread::spawn(move || {
            while !done.load(Ordering::Relaxed) {
                let (mut stream, _) = match listener.accept() {
                    Ok(connection) => connection,
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(2));
                        continue;
                    }
                    Err(error) => panic!("fixture accept: {error}"),
                };
                // BSD/macOS can inherit O_NONBLOCK from the listening socket.
                // A read timeout alone does not make that socket blocking.
                stream.set_nonblocking(false).unwrap();
                stream
                    .set_read_timeout(Some(Duration::from_secs(2)))
                    .unwrap();
                let mut request = Vec::new();
                let mut byte = [0];
                while !request.ends_with(b"\r\n\r\n") {
                    stream.read_exact(&mut byte).unwrap();
                    request.push(byte[0]);
                }
                requests.lock().unwrap().push(
                    String::from_utf8(request)
                        .unwrap()
                        .lines()
                        .next()
                        .unwrap()
                        .to_owned(),
                );
                let (status, headers, body) = queue.lock().unwrap().pop_front().unwrap_or((
                    500,
                    String::new(),
                    b"unexpected request".to_vec(),
                ));
                write!(stream, "HTTP/1.1 {status} Test\r\nContent-Length: {}\r\nConnection: close\r\n{headers}\r\n", body.len()).unwrap();
                stream.write_all(&body).unwrap();
            }
        });
        Self {
            url,
            responses,
            paths,
            stop,
            worker: Some(worker),
        }
    }
    fn push(&self, status: u16, headers: &str, body: Vec<u8>) {
        self.responses
            .lock()
            .unwrap()
            .push_back((status, headers.into(), body));
    }
}
impl Drop for Http {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        let result = self.worker.take().unwrap().join();
        if !thread::panicking() {
            result.unwrap();
        }
    }
}

#[test]
fn typed_approval_rejects_wrong_identity_kind_access_profile_and_revision() {
    let good = asset("https://example.test/sword.glb", &model());
    let parsed: CatalogAsset = serde_json::from_value(good.clone()).unwrap();
    assert_eq!(
        templates(&[parsed.clone(), parsed], AssetKind::Weapon, &selector()).len(),
        1
    );
    assert!(validate_asset_id(AssetKind::Weapon, ID).is_ok());
    assert!(
        validate_avatar_id(ID).is_err(),
        "weapon IDs never weaken the existing avatar boundary"
    );
    for (pointer, bad) in [
        ("/assetKind", json!("avatar")),
        (
            "/id",
            json!("ekza:asset:11111111-2222-3333-4444-555555555555"),
        ),
        ("/id", json!("ekza:weapon:../../model")),
        (
            "/id",
            json!("ekza:avatar:11111111-2222-3333-4444-555555555555"),
        ),
        ("/access", json!("owned")),
        ("/access", json!("unknown")),
        ("/projectSupport/0/status", json!("pending")),
        ("/projectSupport/0/projectId", json!("other-game")),
        ("/projectSupport/0/platform", json!("ios")),
        ("/projectSupport/0/profile", json!("humanoid-glb-v1")),
        ("/renditions/0/profile", json!("other-profile")),
        ("/renditions/0/format", json!("usdz")),
        ("/renditions/0/sha256", json!("a".repeat(63))),
        ("/renditions/0/sha256", json!("A".repeat(64))),
        ("/renditions/0/sizeBytes", json!(MAX_WEAPON_BYTES + 1)),
        ("/renditions/0/sizeBytes", json!(19)),
    ] {
        let mut item = good.clone();
        *item.pointer_mut(pointer).unwrap() = bad;
        let item = serde_json::from_value(item).unwrap();
        assert!(
            templates(&[item], AssetKind::Weapon, &selector()).is_empty(),
            "accepted invalid {pointer}"
        );
    }
    let mut missing_access = good.clone();
    missing_access.as_object_mut().unwrap().remove("access");
    assert!(
        templates(
            &[serde_json::from_value(missing_access).unwrap()],
            AssetKind::Weapon,
            &selector()
        )
        .is_empty()
    );
    let mut duplicate = good.clone();
    duplicate["renditions"]
        .as_array_mut()
        .unwrap()
        .push(good["renditions"][0].clone());
    assert!(
        templates(
            &[serde_json::from_value(duplicate).unwrap()],
            AssetKind::Weapon,
            &selector()
        )
        .is_empty()
    );
    let mut unknown = good;
    unknown["assetKind"] = json!("future-kind");
    assert!(serde_json::from_value::<CatalogAsset>(unknown).is_err());
}

#[test]
fn hosted_weapon_discovery_download_validator_offline_repair_and_withdrawal() {
    let http = Http::new();
    let directory = Directory::new();
    let bytes = model();
    let item = asset(&format!("{}/sword.glb", http.url), &bytes);
    let mut paid = item.clone();
    paid["access"] = json!("owned");
    let mut pending = item.clone();
    pending["projectSupport"][0]["status"] = json!("pending");
    http.push(200, "", catalogue(vec![item.clone(), paid, pending]));
    http.push(200, "", bytes.clone());
    let store = AssetStore::new(&directory.0, &http.url, AssetKind::Weapon, selector()).unwrap();
    let listed = store.refresh().unwrap();
    assert_eq!(listed.len(), 1);
    let weapon = &listed[0];
    let expected_slug = format!(
        "ekza-{}",
        &sha256_hex(format!("{ID}:{}", sha256_hex(&bytes)).as_bytes())[..32]
    );
    assert_eq!(
        weapon.slug, expected_slug,
        "keep pilot's exact identity algorithm"
    );
    assert_eq!(store.cached(), listed);
    assert_eq!(
        store
            .install(weapon, |_| Err("missing approved grip".into()))
            .unwrap_err(),
        "missing approved grip"
    );
    assert!(!store.model_path(&weapon.slug).exists());
    let installed = store
        .install(weapon, |actual| {
            assert_eq!(actual, bytes);
            Ok(())
        })
        .unwrap();
    assert_eq!(
        installed,
        directory
            .0
            .join("weapons")
            .join(format!("{}.glb", weapon.slug))
    );
    assert!(store.verify_installed(weapon).is_ok());
    assert!(
        store
            .install(weapon, |_| Err("validator must run on reused files".into()))
            .is_err()
    );
    fs::write(&installed, vec![b'x'; bytes.len()]).unwrap();
    assert!(store.verify_installed(weapon).is_err());
    store.install(weapon, |_| Ok(())).unwrap();
    assert_eq!(fs::read(&installed).unwrap(), bytes);
    let reopened = AssetStore::new(&directory.0, &http.url, AssetKind::Weapon, selector()).unwrap();
    assert_eq!(reopened.cached(), listed);
    reopened.install(weapon, |_| Ok(())).unwrap();
    assert_eq!(
        http.paths.lock().unwrap().len(),
        2,
        "verified installs and repairs reuse local bytes"
    );
    let first = http.paths.lock().unwrap()[0].clone();
    for field in [
        "GET /v2/assets?",
        "kind=weapon",
        "project=omoba",
        "platform=desktop",
        "profile=handheld-glb-v1",
    ] {
        assert!(first.contains(field), "{first}");
    }
    let mut wrong_selector = selector();
    wrong_selector.project_id = "another-game".into();
    let other =
        AssetStore::new(&directory.0, &http.url, AssetKind::Weapon, wrong_selector).unwrap();
    assert!(other.cached().is_empty());
    assert_ne!(other.catalogue_path(), store.catalogue_path());
    let other_origin = AssetStore::new(
        &directory.0,
        "https://other.example",
        AssetKind::Weapon,
        selector(),
    )
    .unwrap();
    assert!(other_origin.cached().is_empty());
    let avatar = AssetStore::new(
        &directory.0,
        &http.url,
        AssetKind::Avatar,
        SupportSelector::omoba_desktop(),
    )
    .unwrap();
    assert!(avatar.cached().is_empty());

    http.push(200, "", catalogue(vec![]));
    assert!(store.refresh().unwrap().is_empty());
    assert!(store.cached().is_empty());
    assert!(
        installed.exists(),
        "withdrawal removes eligibility, not bytes needed by in-flight rendering"
    );
    let mut revision = item;
    revision["renditions"][0]["sha256"] = json!("a".repeat(64));
    http.push(200, "", catalogue(vec![revision]));
    let updated = store.refresh().unwrap();
    assert_eq!(updated.len(), 1);
    assert_ne!(updated[0].slug, weapon.slug);
    assert_eq!(
        store.cached(),
        updated,
        "new revision replaces old eligibility"
    );
    assert_ne!(
        asset_slug(AssetKind::Weapon, ID, &"b".repeat(64)),
        weapon.slug
    );
}

#[test]
fn typed_catalogue_outages_preserve_last_success_without_legacy_fallback() {
    let unknown_kind = {
        let mut unknown = asset("https://example.test/sword.glb", &model());
        unknown["assetKind"] = json!("unrecognized");
        catalogue(vec![unknown])
    };
    for (status, headers, body) in [
        (404, "", b"{}".to_vec()),
        (503, "", b"{}".to_vec()),
        (200, "X-Studio-Status: unavailable\r\n", catalogue(vec![])),
        (200, "", b"not json".to_vec()),
        (
            200,
            "",
            br#"{"schema":"ekza.asset.catalog.v2","count":1,"items":[]}"#.to_vec(),
        ),
        (
            200,
            "",
            br#"{"schema":"ekza.asset.catalog.v2","count":0}"#.to_vec(),
        ),
        (
            200,
            "",
            br#"{"schema":"ekza.avatar.catalog.v2","count":0,"items":[]}"#.to_vec(),
        ),
        (200, "", unknown_kind),
    ] {
        let http = Http::new();
        let directory = Directory::new();
        let store =
            AssetStore::new(&directory.0, &http.url, AssetKind::Weapon, selector()).unwrap();
        http.push(
            200,
            "",
            catalogue(vec![asset("https://example.test/sword.glb", &model())]),
        );
        let previous = store.refresh().unwrap();
        let saved = fs::read(store.catalogue_path()).unwrap();
        http.push(status, headers, body);
        assert!(store.refresh().is_err());
        assert_eq!(fs::read(store.catalogue_path()).unwrap(), saved);
        assert_eq!(store.cached(), previous);
        assert_eq!(
            http.paths.lock().unwrap().len(),
            2,
            "no avatar or legacy request"
        );
    }
}

#[test]
fn corrupt_download_and_forged_store_metadata_never_install() {
    let http = Http::new();
    let directory = Directory::new();
    let bytes = model();
    let entry: CatalogAsset =
        serde_json::from_value(asset(&format!("{}/bad.glb", http.url), &bytes)).unwrap();
    let item = templates(&[entry], AssetKind::Weapon, &selector()).remove(0);
    let store = AssetStore::new(&directory.0, &http.url, AssetKind::Weapon, selector()).unwrap();
    for corrupt in [
        vec![b'x'; bytes.len()],
        bytes[..bytes.len() - 1].to_vec(),
        vec![b'x'; bytes.len() + 1],
    ] {
        http.push(200, "", corrupt);
        assert!(
            store
                .install(&item, |_| panic!("corrupt bytes reached game validator"))
                .is_err()
        );
        assert!(!store.model_path(&item.slug).exists());
    }
    let mut forged = item.clone();
    forged.slug = "../../escape".into();
    assert!(store.install(&forged, |_| Ok(())).is_err());
    forged = item.clone();
    forged.free = false;
    assert!(store.install(&forged, |_| Ok(())).is_err());
    forged = item;
    forged.support.project_id = "other-game".into();
    assert!(store.install(&forged, |_| Ok(())).is_err());
    assert_eq!(http.paths.lock().unwrap().len(), 3);
}
