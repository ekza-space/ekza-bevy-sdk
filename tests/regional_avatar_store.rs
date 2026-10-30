#![cfg(feature = "http")]

use ekza_bevy_sdk::{passport::SupportSelector, sha256_hex, store::AvatarStore};
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

const ID: &str = "ekza:avatar:11111111-2222-3333-4444-555555555555";

fn model() -> Vec<u8> {
    let mut json =
        br#"{"asset":{"version":"2.0"},"scene":0,"scenes":[{"nodes":[0]}],"nodes":[{}]}"#.to_vec();
    while !json.len().is_multiple_of(4) {
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

fn catalogue(origin: &str, populated: bool) -> Vec<u8> {
    let items = if populated {
        vec![json!({
            "id":ID, "name":"One canonical regional avatar", "access":"free",
            "projectSupport":[{"projectId":"omoba","platform":"desktop","profile":"humanoid-glb-v1","status":"approved"}],
            "renditions":[{"platform":"desktop","profile":"humanoid-glb-v1","format":"glb",
                "sha256":sha256_hex(&model()),"sizeBytes":model().len(),"downloadUrl":format!("{origin}/model.glb")}]
        })]
    } else {
        Vec::new()
    };
    serde_json::to_vec(
        &json!({"schema":"ekza.avatar.catalog.v2","count":items.len(),"items":items}),
    )
    .unwrap()
}

struct Directory(PathBuf);
impl Directory {
    fn new() -> Self {
        static SERIAL: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "ekza-regional-avatar-{}-{}",
            std::process::id(),
            SERIAL.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        Self(path)
    }
}
impl Drop for Directory {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}

type Response = (u16, Vec<u8>);
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
                let (status, body) = queue
                    .lock()
                    .unwrap()
                    .pop_front()
                    .unwrap_or((500, b"unexpected network request".to_vec()));
                write!(
                    stream,
                    "HTTP/1.1 {status} Test\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                )
                .unwrap();
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

    fn push(&self, status: u16, body: Vec<u8>) {
        self.responses.lock().unwrap().push_back((status, body));
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
fn two_origins_isolate_approvals_but_reuse_canonical_identity_and_verified_bytes() {
    let root = Directory::new();
    let global = Http::new();
    let regional = Http::new();
    let first = AvatarStore::new(&root.0, &global.url, SupportSelector::omoba_desktop()).unwrap();
    let second =
        AvatarStore::new(&root.0, &regional.url, SupportSelector::omoba_desktop()).unwrap();
    assert_ne!(first.catalogue_path(), second.catalogue_path());
    global.push(200, catalogue(&global.url, true));
    let global_item = first.refresh().unwrap().remove(0);
    global.push(200, model());
    let installed = first.install(&global_item, |_| Ok(())).unwrap();
    assert_eq!(fs::read(&installed).unwrap(), model());
    assert!(
        second.cached().is_empty(),
        "a new origin must not inherit approval"
    );

    regional.push(200, catalogue(&regional.url, true));
    let regional_item = second.refresh().unwrap().remove(0);
    assert_eq!(
        regional_item.protected.avatar_id,
        global_item.protected.avatar_id
    );
    assert_eq!(regional_item.slug, global_item.slug);
    assert_ne!(
        regional_item.protected.support.rendition.url,
        global_item.protected.support.rendition.url
    );
    assert_eq!(second.model_path(&regional_item.slug), installed);
    // Remove only the installed copy. The second origin must reuse verified
    // content-addressed bytes, rather than requesting its different model URL.
    fs::remove_file(&installed).unwrap();
    second.install(&regional_item, |_| Ok(())).unwrap();
    assert_eq!(fs::read(&installed).unwrap(), model());
    assert_eq!(global.paths.lock().unwrap().len(), 2);
    assert_eq!(regional.paths.lock().unwrap().len(), 1);
    assert!(first.verify_installed(&global_item).is_ok());
    assert!(second.verify_installed(&regional_item).is_ok());
}

#[test]
fn regional_withdrawal_and_outage_do_not_revoke_or_import_another_origins_approval() {
    let root = Directory::new();
    let global = Http::new();
    let regional = Http::new();
    let first = AvatarStore::new(&root.0, &global.url, SupportSelector::omoba_desktop()).unwrap();
    let second =
        AvatarStore::new(&root.0, &regional.url, SupportSelector::omoba_desktop()).unwrap();
    global.push(200, catalogue(&global.url, true));
    first.refresh().unwrap();
    let global_metadata = fs::read(first.catalogue_path()).unwrap();
    regional.push(503, b"unavailable".to_vec());
    assert!(second.refresh().is_err());
    assert!(second.cached().is_empty());
    assert!(!second.catalogue_path().exists());

    regional.push(200, catalogue(&regional.url, true));
    second.refresh().unwrap();
    let regional_metadata = fs::read(second.catalogue_path()).unwrap();
    regional.push(503, b"unavailable".to_vec());
    assert!(second.refresh().is_err());
    assert_eq!(
        fs::read(second.catalogue_path()).unwrap(),
        regional_metadata
    );
    assert_eq!(
        second.cached().len(),
        1,
        "outage preserves this origin's last success"
    );

    regional.push(200, catalogue(&regional.url, false));
    assert!(second.refresh().unwrap().is_empty());
    assert!(
        second.cached().is_empty(),
        "healthy empty withdraws eligibility"
    );
    assert_eq!(first.cached().len(), 1);
    assert_eq!(fs::read(first.catalogue_path()).unwrap(), global_metadata);
    global.push(503, b"unavailable".to_vec());
    assert!(first.refresh().is_err());
    assert_eq!(first.cached().len(), 1);
    assert!(second.cached().is_empty());
}

#[test]
fn legacy_or_copied_documents_cannot_prove_a_new_origins_approvals() {
    let root = Directory::new();
    let global = Http::new();
    let first = AvatarStore::new(&root.0, &global.url, SupportSelector::omoba_desktop()).unwrap();
    global.push(200, catalogue(&global.url, true));
    first.refresh().unwrap();
    let original = fs::read(first.catalogue_path()).unwrap();
    let mut legacy: Value = serde_json::from_slice(&original).unwrap();
    legacy.as_object_mut().unwrap().remove("namespace");
    let legacy = serde_json::to_vec(&legacy).unwrap();
    fs::write(root.0.join("store.json"), &legacy).unwrap();
    let regional = AvatarStore::new(
        &root.0,
        "https://regional.example",
        SupportSelector::omoba_desktop(),
    )
    .unwrap();
    assert!(regional.cached().is_empty());
    fs::write(regional.catalogue_path(), &legacy).unwrap();
    assert!(
        regional.cached().is_empty(),
        "legacy metadata lacks authority even under a new filename"
    );
    fs::write(regional.catalogue_path(), &original).unwrap();
    assert!(
        regional.cached().is_empty(),
        "the embedded namespace must match too"
    );
    assert_eq!(fs::read(root.0.join("store.json")).unwrap(), legacy);
    assert_eq!(first.cached().len(), 1);
}

#[test]
fn complete_selector_and_registry_base_path_are_isolated_but_equivalent_urls_are_stable() {
    let root = Directory::new();
    let global = Http::new();
    let first = AvatarStore::new(&root.0, &global.url, SupportSelector::omoba_desktop()).unwrap();
    global.push(200, catalogue(&global.url, true));
    first.refresh().unwrap();
    let equivalent = AvatarStore::new(
        &root.0,
        &format!("{}/", global.url),
        SupportSelector::omoba_desktop(),
    )
    .unwrap();
    assert_eq!(first.catalogue_path(), equivalent.catalogue_path());
    assert_eq!(equivalent.cached().len(), 1);

    let mut changed_formats = SupportSelector::omoba_desktop();
    changed_formats.formats = vec!["glb".into()];
    for selector in [
        SupportSelector::new(
            "another-game",
            "desktop",
            "humanoid-glb-v1",
            &["glb", "vrm"],
        ),
        SupportSelector::new("omoba", "ios", "humanoid-glb-v1", &["glb", "vrm"]),
        SupportSelector::new("omoba", "desktop", "another-profile", &["glb", "vrm"]),
        changed_formats,
    ] {
        let other = AvatarStore::new(&root.0, &global.url, selector).unwrap();
        assert_ne!(first.catalogue_path(), other.catalogue_path());
        assert!(other.cached().is_empty());
    }
    let path = AvatarStore::new(
        &root.0,
        &format!("{}/regional", global.url),
        SupportSelector::omoba_desktop(),
    )
    .unwrap();
    assert_ne!(first.catalogue_path(), path.catalogue_path());
    assert!(path.cached().is_empty());
}

#[test]
fn relation_survives_cache_and_invalid_refresh_cannot_replace_the_last_complete_catalogue() {
    const BASE: &str = "ekza:avatar:aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee";
    let root = Directory::new();
    let regional = Http::new();
    let store = AvatarStore::new(&root.0, &regional.url, SupportSelector::omoba_desktop()).unwrap();
    let mut document: Value = serde_json::from_slice(&catalogue(&regional.url, true)).unwrap();
    document["items"][0]["baseModelId"] = json!(BASE);
    regional.push(200, serde_json::to_vec(&document).unwrap());
    let items = store.refresh().unwrap();
    assert_eq!(items[0].base_model_id.as_deref(), Some(BASE));
    assert_eq!(store.cached(), items);
    let previous = fs::read(store.catalogue_path()).unwrap();

    for invalid in [ID, "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee"] {
        document["items"][0]["baseModelId"] = json!(invalid);
        regional.push(200, serde_json::to_vec(&document).unwrap());
        assert!(store.refresh().is_err());
        assert_eq!(fs::read(store.catalogue_path()).unwrap(), previous);
        assert_eq!(store.cached(), items);
    }
    assert_eq!(
        regional.paths.lock().unwrap().len(),
        3,
        "decode errors must not trigger v1 fallback"
    );

    let mut old_cache: Value = serde_json::from_slice(&previous).unwrap();
    old_cache["avatars"][0]
        .as_object_mut()
        .unwrap()
        .remove("baseModelId");
    fs::write(
        store.catalogue_path(),
        serde_json::to_vec(&old_cache).unwrap(),
    )
    .unwrap();
    assert_eq!(store.cached().len(), 1);
    assert!(store.cached()[0].base_model_id.is_none());
    old_cache["avatars"][0]["baseModelId"] = json!(ID);
    fs::write(
        store.catalogue_path(),
        serde_json::to_vec(&old_cache).unwrap(),
    )
    .unwrap();
    assert!(
        store.cached().is_empty(),
        "persisted self-reference must fail revalidation too"
    );
}
