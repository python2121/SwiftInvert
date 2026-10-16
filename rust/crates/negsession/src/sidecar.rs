//! Sidecar files — `Sources/SwiftInvert/SidecarStore.swift` (session-spec §2).
//!
//! Edits persist as `<basename>.swiftinvert.json` next to the source (NegPy's `.negpy` pattern):
//! human-inspectable, survives library moves, zero infrastructure. Envelope
//! `{ "version": 1, "settings": { …ExposureSettings… } }`, pretty-printed with sorted keys,
//! written atomically (temp file + rename). Sidecars written before the SwiftInvert rename
//! (`.negswift.json`) are read as a fallback and removed on the next save so edits can't fork
//! across two files. Missing keys decode to defaults (negkit's lenient per-field decoder); any
//! decode error = treat the file as absent.

use std::path::{Path, PathBuf};

use negkit::ExposureSettings;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::Result;
use crate::atomic;

/// The envelope's `version`.
pub const SIDECAR_VERSION: u32 = 1;

/// Current sidecar suffix (appended to the source's stem).
pub const SIDECAR_SUFFIX: &str = "swiftinvert.json";
/// Pre-rename suffix, read as a fallback.
pub const LEGACY_SIDECAR_SUFFIX: &str = "negswift.json";

/// `{ version, settings }`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SidecarPayload {
    pub version: u32,
    pub settings: ExposureSettings,
}

/// The sidecar store (stateless; functions over paths).
pub struct SidecarStore;

/// `source` with its extension replaced by `suffix` (Swift `deletingPathExtension()
/// .appendingPathExtension(suffix)`: a source without an extension just gains one).
fn sibling(source: &Path, suffix: &str) -> PathBuf {
    let stem = source
        .file_stem()
        .map(|s| s.to_os_string())
        .unwrap_or_default();
    let mut name = stem;
    name.push(".");
    name.push(suffix);
    source.with_file_name(name)
}

/// Recursively sort every object's keys (Swift's `.sortedKeys`), whatever serde_json's map
/// implementation preserves.
fn sorted(value: Value) -> Value {
    match value {
        Value::Object(map) => {
            let mut entries: Vec<(String, Value)> = map.into_iter().collect();
            entries.sort_by(|a, b| a.0.cmp(&b.0));
            Value::Object(entries.into_iter().map(|(k, v)| (k, sorted(v))).collect())
        }
        Value::Array(items) => Value::Array(items.into_iter().map(sorted).collect()),
        other => other,
    }
}

impl SidecarStore {
    /// `<dir>/<stem>.swiftinvert.json`.
    pub fn sidecar_path(source: &Path) -> PathBuf {
        sibling(source, SIDECAR_SUFFIX)
    }

    /// `<dir>/<stem>.negswift.json`.
    pub fn legacy_sidecar_path(source: &Path) -> PathBuf {
        sibling(source, LEGACY_SIDECAR_SUFFIX)
    }

    /// The first of (current, legacy) that exists AND decodes; `None` otherwise.
    pub fn load(source: &Path) -> Option<ExposureSettings> {
        [
            Self::sidecar_path(source),
            Self::legacy_sidecar_path(source),
        ]
        .iter()
        .find_map(|p| std::fs::read(p).ok().and_then(|bytes| Self::parse(&bytes)))
    }

    /// Decode an envelope; `None` on any error (the Swift `try?`).
    pub fn parse(bytes: &[u8]) -> Option<ExposureSettings> {
        serde_json::from_slice::<SidecarPayload>(bytes)
            .ok()
            .map(|p| p.settings)
    }

    /// The envelope as the file holds it: pretty-printed, keys sorted, trailing newline.
    pub fn encode(settings: &ExposureSettings) -> Result<String> {
        let payload = SidecarPayload {
            version: SIDECAR_VERSION,
            settings: settings.clone(),
        };
        let value = sorted(serde_json::to_value(&payload)?);
        let mut text = serde_json::to_string_pretty(&value)?;
        text.push('\n');
        Ok(text)
    }

    /// Bare settings as sorted-key JSON (the bridge's `si_sidecar_load` shape / `si_default_settings`).
    pub fn encode_settings(settings: &ExposureSettings) -> Result<String> {
        Ok(serde_json::to_string_pretty(&sorted(
            serde_json::to_value(settings)?,
        ))?)
    }

    /// Write the sidecar atomically (a per-call temp beside it, fsync, rename — see
    /// [`crate::atomic`]), then remove a legacy file. Callers debounce (1 s) and never save
    /// while restoring a frame or in the profile editor (crate docs).
    pub fn save(settings: &ExposureSettings, source: &Path) -> Result<()> {
        let target = Self::sidecar_path(source);
        let text = Self::encode(settings)?;
        atomic::write_atomic(&target, text.as_bytes())?;
        // A stale legacy sidecar would shadow nothing (new wins on load), but remove it so
        // edits don't fork across two files. Its absence is not an error.
        let _ = std::fs::remove_file(Self::legacy_sidecar_path(source));
        Ok(())
    }

    /// Delete both sidecar files (the "clear edits" action); missing files are fine.
    pub fn remove(source: &Path) {
        let _ = std::fs::remove_file(Self::sidecar_path(source));
        let _ = std::fs::remove_file(Self::legacy_sidecar_path(source));
    }
}
