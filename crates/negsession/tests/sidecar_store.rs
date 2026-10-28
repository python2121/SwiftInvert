//! `SidecarStoreTests` (Tests/SwiftInvertTests/SidecarStoreTests.swift) + the atomic-write and
//! sorted-envelope rules of session-spec §2.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::field_reassign_with_default
)]

mod common;

use std::path::Path;

use negkit::{ExposureSettings, NormalizedRect};
use negsession::SidecarStore;
use negsession::sidecar::{SIDECAR_VERSION, SidecarPayload};

fn source_in(dir: &Path) -> std::path::PathBuf {
    dir.join("IMG_0001.CR3")
}

#[test]
fn path_shapes() {
    let source = Path::new("/roll/IMG_0001.CR3");
    assert_eq!(
        SidecarStore::sidecar_path(source),
        Path::new("/roll/IMG_0001.swiftinvert.json")
    );
    assert_eq!(
        SidecarStore::legacy_sidecar_path(source),
        Path::new("/roll/IMG_0001.negswift.json")
    );
    // A source without an extension just gains the suffix (Swift's appendingPathExtension).
    assert_eq!(
        SidecarStore::sidecar_path(Path::new("/roll/frame")),
        Path::new("/roll/frame.swiftinvert.json")
    );
}

#[test]
fn save_then_load_round_trips() {
    let dir = common::scratch_dir("sidecar");
    let source = source_in(&dir);
    let mut settings = ExposureSettings::default();
    settings.grade = 140.0;
    settings.crop_rect = Some(NormalizedRect::new(0.1, 0.1, 0.8, 0.8));
    settings.levels_red = vec![[0.2, 0.3]];
    SidecarStore::save(&settings, &source).unwrap();
    assert_eq!(SidecarStore::load(&source), Some(settings));
}

#[test]
fn missing_sidecar_loads_none() {
    let dir = common::scratch_dir("sidecar");
    assert_eq!(SidecarStore::load(&source_in(&dir)), None);
}

#[test]
fn corrupt_sidecar_loads_none() {
    let dir = common::scratch_dir("sidecar");
    let source = source_in(&dir);
    std::fs::write(SidecarStore::sidecar_path(&source), "not json").unwrap();
    assert_eq!(SidecarStore::load(&source), None);
    // A JSON object that is not an envelope is also "absent".
    std::fs::write(SidecarStore::sidecar_path(&source), "{\"grade\": 90}").unwrap();
    assert_eq!(SidecarStore::load(&source), None);
}

#[test]
fn legacy_sidecar_is_read_as_fallback() {
    let dir = common::scratch_dir("sidecar");
    let source = source_in(&dir);
    let mut settings = ExposureSettings::default();
    settings.density = 0.7;
    let payload = serde_json::to_string(&SidecarPayload {
        version: SIDECAR_VERSION,
        settings,
    })
    .unwrap();
    std::fs::write(SidecarStore::legacy_sidecar_path(&source), payload).unwrap();
    assert_eq!(SidecarStore::load(&source).map(|s| s.density), Some(0.7));
}

#[test]
fn new_sidecar_wins_over_legacy() {
    let dir = common::scratch_dir("sidecar");
    let source = source_in(&dir);
    let mut old = ExposureSettings::default();
    old.density = 0.7;
    let mut new = ExposureSettings::default();
    new.density = 1.3;
    std::fs::write(
        SidecarStore::legacy_sidecar_path(&source),
        SidecarStore::encode(&old).unwrap(),
    )
    .unwrap();
    std::fs::write(
        SidecarStore::sidecar_path(&source),
        SidecarStore::encode(&new).unwrap(),
    )
    .unwrap();
    assert_eq!(SidecarStore::load(&source).map(|s| s.density), Some(1.3));
}

/// Saving migrates: the legacy file is deleted so edits can't fork across two sidecars.
#[test]
fn save_removes_the_legacy_sidecar() {
    let dir = common::scratch_dir("sidecar");
    let source = source_in(&dir);
    std::fs::write(
        SidecarStore::legacy_sidecar_path(&source),
        SidecarStore::encode(&ExposureSettings::default()).unwrap(),
    )
    .unwrap();
    SidecarStore::save(&ExposureSettings::default(), &source).unwrap();
    assert!(!SidecarStore::legacy_sidecar_path(&source).exists());
    assert!(SidecarStore::sidecar_path(&source).exists());
}

/// Atomic write: no temp file survives, and a save over an existing sidecar replaces it whole.
#[test]
fn save_is_atomic_and_leaves_no_temp_files() {
    let dir = common::scratch_dir("sidecar");
    let source = source_in(&dir);
    SidecarStore::save(&ExposureSettings::default(), &source).unwrap();
    let mut changed = ExposureSettings::default();
    changed.grade = 95.0;
    SidecarStore::save(&changed, &source).unwrap();
    let names: Vec<String> = std::fs::read_dir(&dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(
        names,
        vec!["IMG_0001.swiftinvert.json".to_owned()],
        "{names:?}"
    );
    assert_eq!(SidecarStore::load(&source).map(|s| s.grade), Some(95.0));
    // Unwritable destination → an error, not a panic.
    assert!(SidecarStore::save(&changed, Path::new("/proc/nope/IMG.CR3")).is_err());
}

/// The envelope: `{ "settings": {…sorted…}, "version": 1 }`, pretty-printed, keys sorted at
/// every level, the two optional rects omitted when absent (52 keys) and present as
/// `{height, width, x, y}` when set.
#[test]
fn envelope_is_versioned_pretty_and_sorted() {
    let text = SidecarStore::encode(&ExposureSettings::default()).unwrap();
    let value: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert_eq!(value["version"], serde_json::json!(SIDECAR_VERSION));
    let settings = value["settings"].as_object().unwrap();
    assert_eq!(settings.len(), 52);
    let keys: Vec<&String> = settings.keys().collect();
    let mut sorted = keys.clone();
    sorted.sort();
    assert_eq!(keys, sorted, "settings keys must be sorted");
    // Top level sorted too: "settings" before "version".
    let s_pos = text.find("\"settings\"").unwrap();
    let v_pos = text.find("\"version\"").unwrap();
    assert!(s_pos < v_pos);
    assert!(text.contains("\n  "), "pretty-printed");

    let mut with_rect = ExposureSettings::default();
    with_rect.crop_rect = Some(NormalizedRect::new(0.1, 0.2, 0.3, 0.4));
    let text = SidecarStore::encode(&with_rect).unwrap();
    let value: serde_json::Value = serde_json::from_str(&text).unwrap();
    let rect = value["settings"]["cropRect"].as_object().unwrap();
    let keys: Vec<&String> = rect.keys().collect();
    assert_eq!(keys, ["height", "width", "x", "y"]);
    assert_eq!(value["settings"].as_object().unwrap().len(), 53);
}

/// Missing keys = defaults (negkit's lenient decoder) — a minimal envelope decodes.
#[test]
fn missing_keys_decode_to_defaults() {
    let s = SidecarStore::parse(br#"{"version":1,"settings":{"grade":90}}"#).unwrap();
    assert_eq!(s.grade, 90.0);
    assert_eq!(s.density, 1.0);
    assert!(s.auto_exposure);
    let bare = SidecarStore::encode_settings(&ExposureSettings::default()).unwrap();
    let v: serde_json::Value = serde_json::from_str(&bare).unwrap();
    assert!(v.get("version").is_none());
    assert_eq!(v["grade"], serde_json::json!(115.0));
}

/// Two saves of ONE source racing from different threads (the debounced save vs an export
/// flush) must each produce a complete file: per-call temp names (review 7, finding 1).
#[test]
fn concurrent_saves_never_corrupt_the_sidecar() {
    let dir = common::scratch_dir("sidecar-race");
    let source = source_in(&dir);
    let threads: Vec<_> = (0..8)
        .map(|t| {
            let source = source.clone();
            std::thread::spawn(move || {
                for i in 0..40 {
                    let mut s = ExposureSettings::default();
                    s.grade = 60.0 + (t * 40 + i) as f64;
                    SidecarStore::save(&s, &source).unwrap();
                    // Every observation in between is a complete, decodable envelope.
                    assert!(
                        SidecarStore::load(&source).is_some(),
                        "partial sidecar observed"
                    );
                }
            })
        })
        .collect();
    for t in threads {
        t.join().unwrap();
    }
    let loaded = SidecarStore::load(&source).unwrap();
    assert!((60.0..=380.0).contains(&loaded.grade));
    let names: Vec<String> = std::fs::read_dir(&dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(
        names,
        vec!["IMG_0001.swiftinvert.json".to_owned()],
        "{names:?}"
    );
    // The temp names themselves: unique per call, beside the target.
    let target = SidecarStore::sidecar_path(&source);
    let a = negsession::atomic::temp_path(&target);
    let b = negsession::atomic::temp_path(&target);
    assert_ne!(a, b);
    assert_eq!(a.parent(), target.parent());
}
