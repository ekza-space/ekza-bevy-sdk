#![cfg(feature = "http")]
//! Link self-revoke: `DELETE /v1/account/session` ends only the caller's link,
//! and a game's local sign-out never waits for it or fails without it.
use ekza_bevy_sdk::{
    account::{AccountClient, AccountError, AccountSession},
    passport::SupportSelector,
};
use std::{
    io::{ErrorKind, Read, Write},
    net::{TcpListener, TcpStream},
    sync::mpsc::{self, Receiver},
    thread,
    time::{Duration, Instant},
};

const LIBRARY: &str = r#"{"schema":"ekza.account.library.v1","projectId":"omoba","account":{"username":"Saved player"},"items":[]}"#;
const TOKEN: &str = "private-test-token";

/// The request line and headers, lowercased, of one HTTP request.
fn read_head(socket: &mut TcpStream) -> String {
    let mut head = Vec::new();
    let mut chunk = [0; 1024];
    while !head.windows(4).any(|window| window == b"\r\n\r\n") {
        let read = socket.read(&mut chunk).unwrap();
        if read == 0 {
            break;
        }
        head.extend_from_slice(&chunk[..read]);
    }
    String::from_utf8(head).unwrap().to_ascii_lowercase()
}

fn respond(socket: &mut TcpStream, status: u16, body: &str) {
    write!(
        socket,
        "HTTP/1.1 {status} Status\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    )
    .unwrap();
}

/// One connection per response, in order; every request head is reported.
fn serve(
    responses: Vec<(u16, &'static str)>,
) -> (String, Receiver<String>, thread::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let (heads, received) = mpsc::channel();
    let worker = thread::spawn(move || {
        for (status, body) in responses {
            let (mut socket, _) = listener.accept().unwrap();
            heads.send(read_head(&mut socket)).unwrap();
            respond(&mut socket, status, body);
        }
    });
    (url, received, worker)
}

fn selector() -> SupportSelector {
    SupportSelector::new("omoba", "desktop", "humanoid-glb-v1", &["glb"])
}

fn connect(base: &str, received: &Receiver<String>) -> AccountSession {
    let session = AccountClient::new(base, "omoba")
        .unwrap()
        .session(TOKEN.into(), &selector())
        .unwrap();
    assert!(
        received
            .recv()
            .unwrap()
            .starts_with("get /v1/account/library ")
    );
    session
}

#[test]
fn revoke_sends_the_link_token_once_and_is_idempotent() {
    let (base, received, worker) = serve(vec![(200, LIBRARY), (204, ""), (204, "")]);
    let session = connect(&base, &received);
    session.revoke().unwrap();
    let head = received.recv().unwrap();
    assert!(
        head.starts_with("delete /v1/account/session http/1.1\r\n"),
        "{head}"
    );
    assert!(head.contains("\r\nauthorization: bearer private-test-token\r\n"));
    // The secret travels only in the Authorization header.
    assert_eq!(head.matches(TOKEN).count(), 1);
    // An already revoked link is also 204.
    session.revoke().unwrap();
    assert!(
        received
            .recv()
            .unwrap()
            .starts_with("delete /v1/account/session ")
    );
    worker.join().unwrap();
}

#[test]
fn revoke_maps_registry_answers_without_trusting_their_text() {
    let (base, received, worker) = serve(vec![
        (200, LIBRARY),
        (200, "{}"),
        (401, r#"{"error":"authentication_required"}"#),
        (403, ""),
        (404, "Not Found"),
        (405, r#"{"detail":"Method Not Allowed"}"#),
        (503, "internal detail"),
    ]);
    let session = connect(&base, &received);
    assert_eq!(session.revoke(), Ok(()));
    assert_eq!(session.revoke(), Err(AccountError::Unauthorized));
    assert_eq!(session.revoke(), Err(AccountError::Unauthorized));
    for _ in 0..3 {
        let Err(AccountError::Unavailable(message)) = session.revoke() else {
            panic!("an older or failing registry is an outage, not a revocation");
        };
        assert!(!message.contains("detail") && !message.contains("Not Found"));
    }
    assert_eq!(received.iter().count(), 6);
    worker.join().unwrap();
}

#[test]
fn a_saved_credential_is_revoked_only_against_its_own_registry_and_game() {
    let (base, received, worker) = serve(vec![(200, LIBRARY), (204, "")]);
    let credential = connect(&base, &received).credential();
    // A client for another game, or another registry, never sees the token.
    let stranger = TcpListener::bind("127.0.0.1:0").unwrap();
    stranger.set_nonblocking(true).unwrap();
    let elsewhere = format!("http://{}", stranger.local_addr().unwrap());
    for api in [
        AccountClient::new(&base, "ekza-space").unwrap(),
        AccountClient::new(&elsewhere, "omoba").unwrap(),
    ] {
        assert_eq!(
            api.revoke(&credential),
            Err(AccountError::InvalidCredential)
        );
    }
    assert_eq!(stranger.accept().unwrap_err().kind(), ErrorKind::WouldBlock);
    AccountClient::new(&base, "omoba")
        .unwrap()
        .revoke(&credential)
        .unwrap();
    let head = received.recv().unwrap();
    assert!(head.starts_with("delete /v1/account/session "));
    assert!(head.contains("\r\nauthorization: bearer private-test-token\r\n"));
    worker.join().unwrap();
}

#[test]
fn disconnect_returns_at_once_while_the_registry_is_slow() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let (heads, received) = mpsc::channel();
    let (release, released) = mpsc::channel::<()>();
    let worker = thread::spawn(move || {
        let (mut socket, _) = listener.accept().unwrap();
        heads.send(read_head(&mut socket)).unwrap();
        respond(&mut socket, 200, LIBRARY);
        let (mut socket, _) = listener.accept().unwrap();
        heads.send(read_head(&mut socket)).unwrap();
        // Answer only after the game has long moved on.
        released.recv_timeout(Duration::from_secs(10)).unwrap();
        respond(&mut socket, 204, "");
    });
    let session = connect(&base, &received);
    let started = Instant::now();
    session.disconnect();
    assert!(started.elapsed() < Duration::from_millis(500));
    // The revoke still reaches the registry from the background.
    let head = received.recv_timeout(Duration::from_secs(5)).unwrap();
    assert!(head.starts_with("delete /v1/account/session "));
    assert!(head.contains("\r\nauthorization: bearer private-test-token\r\n"));
    release.send(()).unwrap();
    worker.join().unwrap();
}

#[test]
fn signing_out_offline_neither_blocks_nor_fails() {
    let (base, received, worker) = serve(vec![(200, LIBRARY)]);
    let session = connect(&base, &received);
    // The registry is gone: its port is closed.
    worker.join().unwrap();
    let started = Instant::now();
    assert!(matches!(
        session.revoke(),
        Err(AccountError::Unavailable(_))
    ));
    let credential = session.credential();
    session.disconnect();
    AccountClient::new(&base, "omoba")
        .unwrap()
        .disconnect(credential);
    assert!(started.elapsed() < Duration::from_secs(3));
}
