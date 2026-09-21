#![cfg(feature = "http")]

use ekza_bevy_sdk::{passport::SupportSelector, store::AvatarStore};
use std::{
    fs,
    io::{Read, Write},
    net::TcpListener,
    sync::atomic::{AtomicU64, Ordering},
    thread,
    time::Duration,
};

const EMPTY: &str = r#"{"schema":"ekza.avatar.catalog.v2","count":0,"items":[]}"#;

fn server(
    responses: Vec<(u16, &'static str, String)>,
) -> (String, thread::JoinHandle<Vec<String>>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let handle = thread::spawn(move || {
        let mut paths = Vec::new();
        for (status, headers, body) in responses {
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            let mut request = Vec::new();
            let mut byte = [0];
            while !request.ends_with(b"\r\n\r\n") {
                stream.read_exact(&mut byte).unwrap();
                request.push(byte[0]);
            }
            paths.push(
                String::from_utf8(request)
                    .unwrap()
                    .lines()
                    .next()
                    .unwrap()
                    .to_string(),
            );
            write!(stream, "HTTP/1.1 {status} Test\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n{headers}\r\n{body}", body.len()).unwrap();
        }
        paths
    });
    (url, handle)
}

fn directory() -> std::path::PathBuf {
    static SERIAL: AtomicU64 = AtomicU64::new(0);
    let root = std::env::temp_dir().join(format!(
        "ekza-catalogue-{}-{}",
        std::process::id(),
        SERIAL.fetch_add(1, Ordering::Relaxed)
    ));
    fs::create_dir_all(&root).unwrap();
    root
}

#[test]
fn outages_do_not_fall_back_or_overwrite_the_cached_catalogue() {
    for (status, headers, body) in [
        (503, "", r#"{"error":{"code":"catalogue_unavailable"}}"#),
        (200, "X-Studio-Status: unavailable\r\n", EMPTY),
        (200, "", "not JSON"),
        (200, "", "{}"),
        (200, "", r#"{"schema":"ekza.avatar.catalog.v2","count":0}"#),
        (200, "", r#"{"schema":"unknown","count":0,"items":[]}"#),
        (
            200,
            "",
            r#"{"schema":"ekza.avatar.catalog.v2","count":1,"items":[]}"#,
        ),
    ] {
        let (url, handle) = server(vec![(status, headers, body.to_owned())]);
        let root = directory();
        let store = AvatarStore::new(&root, &url, SupportSelector::omoba_desktop()).unwrap();
        let previous = b"last complete catalogue must not be replaced";
        fs::write(root.join("store.json"), previous).unwrap();
        assert!(store.refresh().is_err());
        assert_eq!(fs::read(root.join("store.json")).unwrap(), previous);
        assert_eq!(handle.join().unwrap().len(), 1);
        fs::remove_dir_all(root).unwrap();
    }
}

#[test]
fn a_healthy_empty_catalogue_replaces_stale_entries() {
    let (url, handle) = server(vec![(200, "", EMPTY.to_owned())]);
    let root = directory();
    let store = AvatarStore::new(&root, &url, SupportSelector::omoba_desktop()).unwrap();
    fs::write(root.join("store.json"), b"stale").unwrap();
    assert!(store.refresh().unwrap().is_empty());
    assert!(store.cached().is_empty());
    assert!(
        String::from_utf8(fs::read(root.join("store.json")).unwrap())
            .unwrap()
            .contains("avatars")
    );
    handle.join().unwrap();
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn only_a_missing_v2_endpoint_uses_the_legacy_feed() {
    let (url, handle) = server(vec![
        (404, "", "{}".into()),
        (200, "", r#"{"avatars":[]}"#.into()),
    ]);
    let root = directory();
    let store = AvatarStore::new(&root, &url, SupportSelector::omoba_desktop()).unwrap();
    assert!(store.refresh().unwrap().is_empty());
    let paths = handle.join().unwrap();
    assert!(paths[0].starts_with("GET /v2/avatars?"));
    assert!(paths[1].starts_with("GET /v1/avatars "));
    fs::remove_dir_all(root).unwrap();
}
