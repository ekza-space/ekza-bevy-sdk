#![cfg(feature = "http")]
use ekza_bevy_sdk::{
    account::{AccountClient, AccountCredential, AccountError},
    passport::SupportSelector,
};
use std::{
    io::{Read, Write},
    net::TcpListener,
    thread,
};
fn server(statuses: Vec<u16>) -> (String, thread::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let worker = thread::spawn(move || {
        for status in statuses {
            let (mut socket, _) = listener.accept().unwrap();
            let mut bytes = [0; 4096];
            socket.read(&mut bytes).unwrap();
            let body = r#"{"schema":"ekza.account.library.v1","projectId":"omoba","account":{"username":"Saved player"},"items":[]}"#;
            write!(
                socket,
                "HTTP/1.1 {status} OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            )
            .unwrap();
        }
    });
    (url, worker)
}
fn selector() -> SupportSelector {
    SupportSelector::new("omoba", "desktop", "humanoid-glb-v1", &["glb"])
}
#[test]
fn saved_connection_restores_only_on_same_server_and_distinguishes_revocation() {
    let (base, worker) = server(vec![200, 200, 503, 401]);
    let api = AccountClient::new(&base, "omoba").unwrap();
    let session = api
        .session("private-test-token".into(), &selector())
        .unwrap();
    let root = std::env::temp_dir().join(format!("ekza-persist-{}", std::process::id()));
    let path = root.join("session.json");
    session.credential().save(&path).unwrap();
    let saved = AccountCredential::load(&path).unwrap().unwrap();
    let other = AccountClient::new(&base, "ekza-space").unwrap();
    assert!(matches!(
        other.restore(&saved, &selector()),
        Err(AccountError::InvalidCredential)
    ));
    let mut resumed = api.restore(&saved, &selector()).unwrap();
    assert_eq!(resumed.username, "Saved player");
    assert!(matches!(
        resumed.refresh_checked(),
        Err(AccountError::Unavailable(_))
    ));
    assert_eq!(resumed.username, "Saved player");
    assert!(matches!(
        resumed.refresh_checked(),
        Err(AccountError::Unauthorized)
    ));
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        assert!(AccountCredential::load(&path).is_err());
    }
    worker.join().unwrap();
    std::fs::remove_dir_all(root).unwrap();
}
