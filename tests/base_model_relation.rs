use ekza_bevy_sdk::{
    catalog::{
        CatalogV2Avatar, CatalogV2Response, EkzaAvatar, LibraryPage, PassportCatalog,
        RegistryCatalogResponse, merge_avatars,
    },
    passport::SupportSelector,
    store::{StoreAvatar, templates_v2, validate_item},
};
use serde_json::{Value, json};

const CHILD: &str = "ekza:avatar:11111111-2222-3333-4444-555555555555";
const BASE: &str = "ekza:avatar:aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee";

fn record() -> Value {
    json!({
        "id": CHILD, "name": "Alternate appearance", "baseModelId": BASE, "access": "free",
        "renditions": [{"platform":"desktop", "profile":"humanoid-glb-v1", "format":"glb",
            "sha256":"a".repeat(64), "sizeBytes":100, "downloadUrl":"https://regional.example/model.glb"}],
        "projectSupport":[{"projectId":"omoba", "platform":"desktop", "profile":"humanoid-glb-v1", "status":"approved"}]
    })
}

#[test]
fn relation_survives_v2_normalization_merging_and_store_serialization() {
    let wire: CatalogV2Avatar = serde_json::from_value(record()).unwrap();
    assert_eq!(wire.base_model_id.as_deref(), Some(BASE));
    assert_eq!(serde_json::to_value(&wire).unwrap()["baseModelId"], BASE);
    let normalized = wire.clone().into_avatar();
    assert_eq!(normalized.base_model_id.as_deref(), Some(BASE));
    let round_trip: EkzaAvatar =
        serde_json::from_slice(&serde_json::to_vec(&normalized).unwrap()).unwrap();
    assert_eq!(round_trip, normalized);

    let mut unclassified = normalized.clone();
    unclassified.base_model_id = None;
    let merged = merge_avatars([vec![unclassified], vec![normalized]]);
    assert_eq!(merged[0].base_model_id.as_deref(), Some(BASE));

    let selector = SupportSelector::omoba_desktop();
    let item = templates_v2(&[wire.clone()], &selector).remove(0);
    assert_eq!(item.base_model_id.as_deref(), Some(BASE));
    let store_round_trip: StoreAvatar =
        serde_json::from_slice(&serde_json::to_vec(&item).unwrap()).unwrap();
    assert_eq!(store_round_trip, item);
    let mut unclassified_wire = wire;
    unclassified_wire.base_model_id = None;
    assert_eq!(
        templates_v2(&[unclassified_wire], &selector)[0].slug,
        item.slug,
        "classification must not change canonical identity or rendition slugs"
    );
}

#[test]
fn absent_and_null_are_unclassified_and_old_normalized_and_store_documents_still_decode() {
    for parent in [None, Some(Value::Null)] {
        let mut raw = record();
        raw.as_object_mut().unwrap().remove("baseModelId");
        if let Some(value) = parent {
            raw["baseModelId"] = value;
        }
        let wire: CatalogV2Avatar = serde_json::from_value(raw).unwrap();
        assert!(wire.base_model_id.is_none());
        assert!(
            serde_json::to_value(&wire)
                .unwrap()
                .get("baseModelId")
                .is_none()
        );
        let avatar = wire.clone().into_avatar();
        let json = serde_json::to_value(&avatar).unwrap();
        assert!(json.get("baseModelId").is_none());
        assert!(
            serde_json::from_value::<EkzaAvatar>(json)
                .unwrap()
                .base_model_id
                .is_none()
        );
        let item = templates_v2(&[wire], &SupportSelector::omoba_desktop()).remove(0);
        let json = serde_json::to_value(item).unwrap();
        assert!(json.get("baseModelId").is_none());
        assert!(
            serde_json::from_value::<StoreAvatar>(json)
                .unwrap()
                .base_model_id
                .is_none()
        );
    }
}

#[test]
fn malformed_or_self_referential_v2_relations_reject_the_complete_document() {
    for invalid in [
        json!(CHILD),
        json!("11111111-2222-3333-4444-555555555555"),
        json!(""),
        json!(true),
        json!("ekza:avatar:AAAAAAAA-BBBB-CCCC-DDDD-EEEEEEEEEEEE"),
        json!("solana:devnet:avatar-data:11111111111111111111111111111111"),
        json!("ekza:avatar:aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee/other"),
    ] {
        let mut raw = record();
        raw["baseModelId"] = invalid.clone();
        assert!(
            serde_json::from_value::<CatalogV2Avatar>(raw.clone()).is_err(),
            "{invalid}"
        );
        assert!(
            serde_json::from_value::<CatalogV2Response>(json!({
                "schema":"ekza.avatar.catalog.v2", "count":2, "items":[record(),raw]
            }))
            .is_err(),
            "a malformed relation must not partially replace the catalogue"
        );
    }
}

#[test]
fn parent_never_grants_child_approval_and_programmatic_or_cached_invalid_relations_are_refused() {
    let selector = SupportSelector::omoba_desktop();
    let mut raw = record();
    raw["projectSupport"] = json!([]);
    let child: CatalogV2Avatar = serde_json::from_value(raw).unwrap();
    assert!(!child.clone().into_avatar().is_approved_for("omoba"));
    let mut parent: CatalogV2Avatar = serde_json::from_value(record()).unwrap();
    parent.id = BASE.into();
    parent.base_model_id = None;
    let listed = templates_v2(&[parent, child], &selector);
    assert_eq!(listed.len(), 1);
    assert_eq!(
        listed[0].protected.avatar_id, BASE,
        "only the explicitly approved parent is eligible"
    );

    let mut child: CatalogV2Avatar = serde_json::from_value(record()).unwrap();
    let mut item = templates_v2(&[child.clone()], &selector).remove(0);
    child.base_model_id = Some(CHILD.into());
    assert!(templates_v2(&[child], &selector).is_empty());
    item.base_model_id = Some(CHILD.into());
    assert!(validate_item(&item, &selector).is_err());
}

#[test]
fn all_legacy_feed_converters_leave_the_relation_unclassified() {
    let library: LibraryPage =
        serde_json::from_str(include_str!("fixtures/library-avatars.json")).unwrap();
    let registry: RegistryCatalogResponse =
        serde_json::from_str(include_str!("fixtures/registry-v1-avatars.json")).unwrap();
    let passport: PassportCatalog =
        serde_json::from_str(include_str!("fixtures/passport-catalog.json")).unwrap();
    let avatars: Vec<EkzaAvatar> = library
        .items
        .into_iter()
        .map(Into::into)
        .chain(
            registry
                .avatars
                .into_iter()
                .map(|item| item.into_avatar("https://registry.example")),
        )
        .chain(passport.items.into_iter().map(Into::into))
        .collect();
    assert!(!avatars.is_empty());
    assert!(avatars.iter().all(|avatar| avatar.base_model_id.is_none()));
}
