#![cfg(feature = "http")]
use ekza_bevy_sdk::{
    account::{AccountClient, AccountCredential, AccountError},
    assets::{AssetKind, CatalogAsset, LibraryReason, asset_slug, templates, validate_item},
    passport::SupportSelector,
};
use serde_json::{Value, json};
use std::{
    io::{Read, Write},
    net::TcpListener,
    thread,
};
/// A library from a registry that predates the additive `assets` array.
const LEGACY_LIBRARY: &str = r#"{"schema":"ekza.account.library.v1","projectId":"omoba","account":{"username":"Saved player"},"items":[]}"#;
const LIBRARY_FIXTURE: &str = include_str!("fixtures/account-library-assets.json");
const WILDSPARK: &str = "ekza:weapon:11111111-2222-4333-8444-555555555555";
const HAMMER: &str = "ekza:weapon:38a9903e-0000-4000-8000-000000000001";
const AVATAR: &str = "ekza:avatar:aaaaaaaa-1111-4111-8111-aaaaaaaaaaaa";
fn server(statuses: Vec<u16>) -> (String, thread::JoinHandle<()>) {
    serve(
        statuses
            .into_iter()
            .map(|status| (status, LEGACY_LIBRARY.to_owned()))
            .collect(),
    )
}
/// One connection per response, in order.
fn serve(responses: Vec<(u16, String)>) -> (String, thread::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let worker = thread::spawn(move || {
        for (status, body) in responses {
            let (mut socket, _) = listener.accept().unwrap();
            let mut bytes = [0; 4096];
            assert!(
                socket.read(&mut bytes).unwrap() > 0,
                "client must send a request"
            );
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
fn weapons() -> SupportSelector {
    SupportSelector::new("omoba", "desktop", "handheld-glb-v1", &["glb"])
}
fn ids(items: &[ekza_bevy_sdk::assets::StoreAsset]) -> Vec<&str> {
    items.iter().map(|item| item.asset_id.as_str()).collect()
}
#[test]
fn saved_connection_restores_only_on_same_server_and_distinguishes_revocation() {
    let (base, worker) = server(vec![200, 200, 503, 401]);
    let api = AccountClient::new(&base, "omoba").unwrap();
    let session = api
        .session("private-test-token".into(), &selector())
        .unwrap();
    // No `assets` field: a connected session with an empty asset library.
    assert!(session.library_assets().is_empty());
    assert!(session.assets(AssetKind::Weapon, &weapons()).is_empty());
    assert!(!session.has_asset(WILDSPARK));
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

#[test]
fn library_assets_list_every_kind_and_filter_by_the_store_selector() {
    let mut cleared: Value = serde_json::from_str(LIBRARY_FIXTURE).unwrap();
    cleared["assets"] = json!([]);
    let (base, worker) = serve(vec![
        (200, LIBRARY_FIXTURE.to_owned()),
        (503, String::new()),
        (200, cleared.to_string()),
    ]);
    let api = AccountClient::new(&base, "omoba").unwrap();
    let mut session = api
        .session("private-test-token".into(), &selector())
        .unwrap();
    assert_eq!(session.username, "Spark tester");

    // Malformed, unknown-kind and kind/identity-mismatched entries are skipped
    // one by one; every well-formed entry survives in wire order. The library
    // lists one entry per revision: a newer android revision of the Wildspark
    // set comes ahead of the desktop one and both are kept.
    let listed: Vec<_> = session
        .library_assets()
        .iter()
        .map(|entry| (entry.asset.id.as_str(), entry.reason))
        .collect();
    assert_eq!(
        listed,
        [
            (AVATAR, Some(LibraryReason::Created)),
            (WILDSPARK, Some(LibraryReason::Claimed)),
            (WILDSPARK, Some(LibraryReason::Claimed)),
            (HAMMER, Some(LibraryReason::Saved)),
            (
                "ekza:weapon:22222222-0000-4000-8000-000000000002",
                Some(LibraryReason::Created)
            ),
            (
                "ekza:weapon:33333333-0000-4000-8000-000000000003",
                Some(LibraryReason::Saved)
            ),
            (
                "ekza:weapon:44444444-0000-4000-8000-000000000004",
                Some(LibraryReason::Saved)
            ),
            (
                "ekza:weapon:55555555-0000-4000-8000-000000000005",
                Some(LibraryReason::Saved)
            ),
            // An unknown reason keeps the entry.
            ("ekza:weapon:66666666-0000-4000-8000-000000000006", None),
        ]
    );
    assert_eq!(session.library_assets()[1].asset.name, "Wildspark set");
    assert_ne!(
        session.library_assets()[1].asset.renditions,
        session.library_assets()[2].asset.renditions
    );

    // Weapons: free, approved for exactly this project/platform/profile and
    // unambiguous. Pending, non-free, ambiguous and other-game items are not.
    let owned = session.assets(AssetKind::Weapon, &weapons());
    assert_eq!(ids(&owned), [WILDSPARK, HAMMER]);
    for item in &owned {
        assert_eq!(
            item.slug,
            asset_slug(
                AssetKind::Weapon,
                &item.asset_id,
                &item.support.rendition.sha256
            )
        );
        assert!(validate_item(item, AssetKind::Weapon, &weapons()).is_ok());
    }
    // Desktop gets the revision it pins, not the newer android one listed first.
    assert_eq!(
        owned[0].support.rendition.sha256,
        "0d5bea83abf3bdad4f16e0031b24978f8f27b612c2c046d75b4764d8fa1a29f4"
    );

    let phone = SupportSelector::new("omoba", "android", "handheld-glb-v1", &["glb"]);
    let on_phone = session.assets(AssetKind::Weapon, &phone);
    assert_eq!(
        ids(&on_phone),
        [
            WILDSPARK,
            "ekza:weapon:66666666-0000-4000-8000-000000000006"
        ]
    );
    assert_eq!(
        on_phone[0].support.rendition.sha256,
        "b83c93b2630659592e934aa25558100d6503c9ef024d404beccb4a5a7558e4d3"
    );
    let other_game = SupportSelector::new("ekza-space", "desktop", "handheld-glb-v1", &["glb"]);
    assert_eq!(
        ids(&session.assets(AssetKind::Weapon, &other_game)),
        ["ekza:weapon:55555555-0000-4000-8000-000000000005"]
    );
    assert_eq!(
        ids(&session.assets(AssetKind::Avatar, &selector())),
        [AVATAR]
    );
    assert!(session.assets(AssetKind::Weapon, &selector()).is_empty());
    assert!(session.assets(AssetKind::Avatar, &weapons()).is_empty());

    // The same StoreAssets the public catalogue would produce over the same
    // items, for every kind and selector.
    let fixture: Value = serde_json::from_str(LIBRARY_FIXTURE).unwrap();
    let public: Vec<CatalogAsset> = fixture["assets"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|item| serde_json::from_value(item.clone()).ok())
        .collect();
    for kind in [AssetKind::Weapon, AssetKind::Avatar] {
        for store in [&weapons(), &phone, &other_game, &selector()] {
            assert_eq!(
                session.assets(kind, store),
                templates(&public, kind, store),
                "{kind:?} {store:?}"
            );
        }
    }

    // Avatars keep their own list; a weapon never lands in `items`.
    assert_eq!(session.items.len(), 1);
    assert!(owned.iter().all(|weapon| !session.has(&weapon.slug)));
    assert!(
        session
            .items
            .iter()
            .all(|item| item.protected.avatar_id == AVATAR)
    );

    // Membership by canonical id or bare Studio UUID (any kind, any letter
    // case), whatever the approval or access.
    for id in [
        AVATAR,
        WILDSPARK,
        "ekza:weapon:22222222-0000-4000-8000-000000000002",
        "ekza:weapon:33333333-0000-4000-8000-000000000003",
        "11111111-2222-4333-8444-555555555555",
        "38A9903E-0000-4000-8000-000000000001",
        "aaaaaaaa-1111-4111-8111-aaaaaaaaaaaa",
    ] {
        assert!(session.has_asset(id), "{id}");
    }
    for id in [
        "ekza:emote:77777777-0000-4000-8000-000000000007",
        "ekza:avatar:bbbbbbbb-0000-4000-8000-00000000000b",
        "ekza:weapon:88888888-0000-4000-8000-000000000008",
        "ekza:weapon:99999999-0000-4000-8000-000000000009",
        "ekza:weapon:aaaaaaaa-0000-4000-8000-00000000000a",
        // Entries that were skipped stay unknown by UUID too.
        "77777777-0000-4000-8000-000000000007",
        "bbbbbbbb-0000-4000-8000-00000000000b",
        "aaaaaaaa-0000-4000-8000-00000000000a",
        // Canonical ids match exactly; near-misses never do.
        "ekza:weapon:11111111-2222-4333-8444-55555555555A",
        "EKZA:WEAPON:11111111-2222-4333-8444-555555555555",
        "ekza:avatar:11111111-2222-4333-8444-555555555555",
        " 11111111-2222-4333-8444-555555555555",
        "{11111111-2222-4333-8444-555555555555}",
        "11111111222243338444555555555555",
        "",
    ] {
        assert!(!session.has_asset(id), "{id}");
    }

    // An outage keeps the last library; a healthy empty one clears it.
    assert!(matches!(
        session.refresh_checked(),
        Err(AccountError::Unavailable(_))
    ));
    assert_eq!(
        ids(&session.assets(AssetKind::Weapon, &weapons())),
        [WILDSPARK, HAMMER]
    );
    session.refresh_checked().unwrap();
    assert!(session.library_assets().is_empty());
    assert!(!session.has_asset(WILDSPARK));
    assert_eq!(session.items.len(), 1);
    worker.join().unwrap();
}

#[test]
fn a_malformed_assets_field_never_drops_the_session() {
    let mut responses = Vec::new();
    for assets in [
        json!(null),
        json!({"items": []}),
        json!("assets"),
        json!([42, {}, null]),
    ] {
        let mut library: Value = serde_json::from_str(LEGACY_LIBRARY).unwrap();
        library["assets"] = assets;
        responses.push((200, library.to_string()));
    }
    let attempts = responses.len();
    let (base, worker) = serve(responses);
    let api = AccountClient::new(&base, "omoba").unwrap();
    for _ in 0..attempts {
        let session = api
            .session("private-test-token".into(), &selector())
            .unwrap();
        assert_eq!(session.username, "Saved player");
        assert!(session.library_assets().is_empty());
    }
    worker.join().unwrap();
}

#[test]
fn credentials_saved_before_library_assets_still_restore() {
    let (base, worker) = serve(vec![(200, LIBRARY_FIXTURE.to_owned())]);
    let root = std::env::temp_dir().join(format!("ekza-persist-legacy-{}", std::process::id()));
    std::fs::create_dir_all(&root).unwrap();
    let path = root.join("session.json");
    // The exact 0.7-0.9 file layout.
    std::fs::write(
        &path,
        format!(r#"{{"base":"{base}","project_id":"omoba","token":"private-test-token"}}"#),
    )
    .unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
    }
    let saved = AccountCredential::load(&path).unwrap().unwrap();
    let api = AccountClient::new(&base, "omoba").unwrap();
    let session = api.restore(&saved, &selector()).unwrap();
    assert!(session.has_asset(WILDSPARK));
    assert_eq!(
        ids(&session.assets(AssetKind::Weapon, &weapons())),
        [WILDSPARK, HAMMER]
    );
    worker.join().unwrap();
    std::fs::remove_dir_all(root).unwrap();
}
