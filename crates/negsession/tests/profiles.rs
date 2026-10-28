//! `DefaultProfileTests` + `ProfileStoreTests` + `AuditProfileStoreRecoveryTests`
//! (Tests/SwiftInvertTests/).
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::field_reassign_with_default
)]

mod common;

use negkit::{ExposureSettings, NormalizedRect};
use negsession::profiles::{
    BUILTIN_PROFILE_ID, NONE_PROFILE_ID, builtin_default_settings, new_profile_id,
};
use negsession::{ProfileStore, SettingsGeometry, SettingsProfile};

/// When the profile evolves, update ONLY the zeroing list here — the equality check then proves
/// nothing else drifted (geometry included).
#[test]
fn profile_touches_only_its_documented_fields() {
    let mut s = builtin_default_settings();
    s.exposure_stops = 0.0;
    s.highlights = 0.0;
    s.shadows = 0.0;
    s.black_point_offset = 0.0;
    s.overall_contrast = 0.0;
    s.shadow_contrast = 0.0;
    s.pre_saturation = 1.15; // back to the STOCK default, not zero
    assert_eq!(s, ExposureSettings::default());
    assert_eq!(builtin_default_settings().exposure_stops, 0.6);
    assert_eq!(builtin_default_settings().shadow_contrast, 0.55);
}

#[test]
fn profile_carries_no_geometry_and_differs_from_stock() {
    let p = builtin_default_settings();
    assert_eq!(p.crop_rect, None);
    assert_eq!(p.analysis_rect, None);
    assert_eq!(p.rotation, 0);
    assert!(!p.flip_horizontal);
    assert_eq!(p.fine_rotation, 0.0);
    assert_ne!(p, ExposureSettings::default());
}

#[test]
fn none_first_then_builtin_none_active_by_default() {
    let store = ProfileStore::new();
    let ids: Vec<String> = store.all().into_iter().map(|p| p.id).collect();
    assert_eq!(ids, [NONE_PROFILE_ID, BUILTIN_PROFILE_ID]);
    assert_eq!(store.active_id(), NONE_PROFILE_ID);
    assert_eq!(store.active().name, "None");
    assert_eq!(store.all()[1].name, "SwiftInvert Default");
    assert_eq!(store.default_settings(), ExposureSettings::default());
}

/// Choosing the built-in default survives a persistence round trip.
#[test]
fn choosing_default_persists() {
    let dir = common::scratch_dir("profiles");
    let path = dir.join("profiles.json");
    let mut store = ProfileStore::new();
    assert!(store.set_active(BUILTIN_PROFILE_ID));
    store.save(&path).unwrap();
    let reloaded = ProfileStore::load(&path);
    assert_eq!(reloaded.active_id(), BUILTIN_PROFILE_ID);
    assert_eq!(reloaded.active().settings, builtin_default_settings());
    // The blob carries the two UserDefaults keys and ONLY user profiles.
    let json = std::fs::read_to_string(&path).unwrap();
    let v: serde_json::Value = serde_json::from_str(&json).unwrap();
    assert_eq!(v["activeProfileID"], serde_json::json!(BUILTIN_PROFILE_ID));
    assert_eq!(v["settingsProfiles"].as_array().unwrap().len(), 0);
}

#[test]
fn upsert_strips_geometry_and_round_trips() {
    let dir = common::scratch_dir("profiles");
    let path = dir.join("profiles.json");
    let mut store = ProfileStore::new();
    let mut s = builtin_default_settings();
    s.vibrance = 1.4;
    s.crop_rect = Some(NormalizedRect::new(0.1, 0.1, 0.5, 0.5));
    s.rotation = 90;
    let id = new_profile_id();
    assert!(store.upsert(SettingsProfile {
        id: id.clone(),
        name: "Warm".into(),
        settings: s
    }));
    assert!(store.set_active(&id));

    let saved = store.active().settings;
    assert_eq!(saved.vibrance, 1.4);
    assert_eq!(saved.crop_rect, None);
    assert_eq!(saved.rotation, 0);

    store.save(&path).unwrap();
    let reloaded = ProfileStore::load(&path);
    assert_eq!(reloaded.active_id(), id);
    assert_eq!(reloaded.active().settings, saved);
    assert_eq!(reloaded.all().len(), 3);

    // Upsert by id replaces in place (creation order kept).
    let other = SettingsProfile::new("Cool", &ExposureSettings::default());
    store.upsert(other.clone());
    store.upsert(SettingsProfile {
        id: id.clone(),
        name: "Warmer".into(),
        settings: ExposureSettings::default(),
    });
    let names: Vec<String> = store
        .user_profiles()
        .iter()
        .map(|p| p.name.clone())
        .collect();
    assert_eq!(names, ["Warmer", "Cool"]);
}

#[test]
fn reserved_profiles_cannot_be_upserted_or_deleted() {
    let mut store = ProfileStore::new();
    let mut hijack = SettingsProfile::builtin();
    hijack.settings.vibrance = 9.0;
    assert!(!store.upsert(hijack));
    let mut hijack_none = SettingsProfile::none();
    hijack_none.settings.vibrance = 9.0;
    assert!(!store.upsert(hijack_none));
    assert!(!store.delete(BUILTIN_PROFILE_ID));
    assert!(!store.delete(NONE_PROFILE_ID));
    assert_eq!(store.all().len(), 2);
    assert_eq!(store.all()[0].settings, ExposureSettings::default());
    assert_eq!(store.all()[1].settings, builtin_default_settings());
}

#[test]
fn deleting_active_falls_back_to_none() {
    let mut store = ProfileStore::new();
    let p = SettingsProfile::new("Temp", &ExposureSettings::default());
    let id = p.id.clone();
    store.upsert(p);
    store.set_active(&id);
    assert!(store.delete(&id));
    assert_eq!(store.active_id(), NONE_PROFILE_ID);
    assert!(store.user_profiles().is_empty());
    assert!(!store.set_active("not-a-profile"));
}

#[test]
fn keeping_geometry_merges_both_ways() {
    let mut look = ExposureSettings::default();
    look.exposure_stops = 1.0;
    let mut frame = ExposureSettings::default();
    frame.rotation = 180;
    frame.crop_rect = Some(NormalizedRect::new(0.0, 0.0, 0.5, 0.5));
    frame.analysis_rect_fine_rotation = 2.0;

    let composed = look.keeping_geometry(&frame);
    assert_eq!(composed.exposure_stops, 1.0);
    assert_eq!(composed.rotation, 180);
    assert_eq!(composed.crop_rect, frame.crop_rect);
    assert_eq!(composed.analysis_rect_fine_rotation, 2.0);
    assert_eq!(composed.adjustments_only().rotation, 0);
    assert_eq!(composed.adjustments_only().crop_rect, None);
    assert_eq!(composed.adjustments_only().exposure_stops, 1.0);
}

/// Garbage persistence → an empty store with None active; an active id that matches no profile
/// is ignored.
#[test]
fn garbage_blob_yields_empty_store_and_none_active() {
    let store = ProfileStore::from_json("not json at all");
    assert!(store.user_profiles().is_empty());
    assert_eq!(store.active_id(), NONE_PROFILE_ID);
    let store = ProfileStore::from_json(
        r#"{"settingsProfiles":[],"activeProfileID":"12345678-1234-4234-8234-123456789ABC"}"#,
    );
    assert_eq!(store.active_id(), NONE_PROFILE_ID);
    assert_eq!(store.active().settings, ExposureSettings::default());
    let dir = common::scratch_dir("profiles");
    assert_eq!(
        ProfileStore::load(&dir.join("missing.json")),
        ProfileStore::new()
    );
}

#[test]
fn new_profile_ids_are_uuid_shaped_and_unique() {
    let a = new_profile_id();
    let b = new_profile_id();
    assert_ne!(a, b);
    assert_eq!(a.len(), 36);
    assert_eq!(a.matches('-').count(), 4);
    assert_eq!(&a[14..15], "4");
    assert!(negsession::profiles::is_reserved_profile_id(
        &NONE_PROFILE_ID.to_lowercase()
    ));
    assert!(!negsession::profiles::is_reserved_profile_id(&a));
}

/// One malformed entry is skipped; the rest and `activeProfileID` survive (review 7, finding
/// 12 — Swift reads the two keys independently). Ids must be UUID-shaped on insert.
#[test]
fn malformed_profile_entries_are_skipped_not_fatal() {
    let good = new_profile_id();
    let json = format!(
        r#"{{"settingsProfiles":[{{"id":"{good}","name":"Warm","settings":{{"vibrance":1.4}}}},{{"id":"broken"}},{{"id":"","name":"Empty id","settings":{{}}}},42],"activeProfileID":"{good}"}}"#
    );
    let store = ProfileStore::from_json(&json);
    assert_eq!(store.user_profiles().len(), 1);
    assert_eq!(store.user_profiles()[0].name, "Warm");
    assert_eq!(store.user_profiles()[0].settings.vibrance, 1.4);
    assert_eq!(store.active_id(), good);
    // Missing list, present active id → None active, no crash.
    let store =
        ProfileStore::from_json(r#"{"activeProfileID":"00000000-0000-0000-0000-000000000001"}"#);
    assert_eq!(store.active_id(), BUILTIN_PROFILE_ID);
    assert!(ProfileStore::from_json("[]").user_profiles().is_empty());

    let mut store = ProfileStore::new();
    for bad in [
        "",
        "not-a-uuid",
        "12345678-1234-1234-1234-12345678901",
        "zzzzzzzz-1234-1234-1234-123456789012",
    ] {
        assert!(
            !store.upsert(SettingsProfile {
                id: bad.to_owned(),
                name: "x".into(),
                settings: ExposureSettings::default()
            }),
            "accepted id {bad:?}"
        );
    }
    assert!(store.upsert(SettingsProfile {
        id: "12345678-1234-4234-8234-123456789ABC".to_owned(),
        name: "ok".into(),
        settings: ExposureSettings::default()
    }));
    assert!(negsession::profiles::is_uuid_shaped(&new_profile_id()));
    assert!(!negsession::profiles::is_uuid_shaped(""));
}
