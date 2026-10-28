//! Default-settings profiles — `Sources/SwiftInvert/DefaultProfile.swift` + `ProfileStore.swift`
//! (session-spec §2).
//!
//! A profile is adjustments only: geometry (crop, rotation, straighten, analysis region) is a
//! per-frame fact and is stripped on save. Two reserved rows lead the list, resolved from code
//! each launch — never persisted, editable or deletable: "None" (stock, NegPy-neutral settings;
//! the out-of-the-box active choice) and "SwiftInvert Default" (the house look). Everything that
//! asks "what does a fresh frame get?" reads [`ProfileStore::active`]'s settings. Stock
//! `ExposureSettings::default()` stays parity-neutral — fixtures and negcli never see a profile.
//!
//! Persistence is one JSON blob ([`PersistedProfiles`], the two UserDefaults keys as fields) the
//! app stores wherever it keeps settings; this crate takes a path and knows no OS directories.

use std::hash::{BuildHasher, Hasher};
use std::path::Path;

use negkit::ExposureSettings;
use serde::{Deserialize, Serialize};

use crate::Result;
use crate::atomic;

/// "None" — stock settings, first in the list, the default active choice.
pub const NONE_PROFILE_ID: &str = "00000000-0000-0000-0000-000000000000";
/// "SwiftInvert Default" — the built-in house look.
pub const BUILTIN_PROFILE_ID: &str = "00000000-0000-0000-0000-000000000001";

/// A named profile. `id` is a UUID string (the Swift `UUID` Codable form, uppercase hex; any
/// case is accepted and compared case-insensitively).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SettingsProfile {
    pub id: String,
    pub name: String,
    pub settings: ExposureSettings,
}

impl SettingsProfile {
    /// A new user profile with a fresh id and `settings` stripped to adjustments.
    pub fn new(name: impl Into<String>, settings: &ExposureSettings) -> Self {
        Self {
            id: new_profile_id(),
            name: name.into(),
            settings: settings.adjustments_only(),
        }
    }

    /// The reserved "None" row.
    pub fn none() -> Self {
        Self {
            id: NONE_PROFILE_ID.to_owned(),
            name: "None".to_owned(),
            settings: ExposureSettings::default(),
        }
    }

    /// The reserved built-in row.
    pub fn builtin() -> Self {
        Self {
            id: BUILTIN_PROFILE_ID.to_owned(),
            name: "SwiftInvert Default".to_owned(),
            settings: builtin_default_settings(),
        }
    }
}

/// The code-derived house profile ("SwiftInvert Default"). Derived 2026-07-25 from the aggregate
/// of hand corrections across two test rolls, re-validated 2026-07-31 — see the Swift file's
/// history for the reasoning behind each value. Evolve it there and here together.
pub fn builtin_default_settings() -> ExposureSettings {
    ExposureSettings {
        exposure_stops: 0.6,
        highlights: -0.2,
        shadows: 0.3,
        black_point_offset: -0.09,
        overall_contrast: 0.2,
        shadow_contrast: 0.55,
        pre_saturation: 1.25,
        ..ExposureSettings::default()
    }
}

/// Whether `id` is a UUID-shaped string (`8-4-4-4-12` hex, any case) — the only ids the store
/// accepts, so a blob can't smuggle an empty or colliding key in.
pub fn is_uuid_shaped(id: &str) -> bool {
    let groups: Vec<&str> = id.split('-').collect();
    groups.len() == 5
        && groups
            .iter()
            .zip([8usize, 4, 4, 4, 12])
            .all(|(g, n)| g.len() == n && g.bytes().all(|b| b.is_ascii_hexdigit()))
}

/// Whether `id` names one of the two reserved rows.
pub fn is_reserved_profile_id(id: &str) -> bool {
    id.eq_ignore_ascii_case(NONE_PROFILE_ID) || id.eq_ignore_ascii_case(BUILTIN_PROFILE_ID)
}

/// A fresh random-looking UUID-v4-shaped id (no `uuid`/`rand` dependency: time, pid, a counter
/// and the std hasher's per-process random keys — unique within and across processes for this
/// purpose, not cryptographic).
pub fn new_profile_id() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let mut h = std::collections::hash_map::RandomState::new().build_hasher();
    h.write_u128(nanos);
    h.write_u64(n);
    h.write_u32(std::process::id());
    let a = h.finish();
    h.write_u64(a);
    let b = h.finish();
    // RFC 4122 version/variant nibbles.
    let hi = (a & 0xFFFF_FFFF_FFFF_0FFF) | 0x0000_0000_0000_4000;
    let lo = (b & 0x3FFF_FFFF_FFFF_FFFF) | 0x8000_0000_0000_0000;
    format!(
        "{:08X}-{:04X}-{:04X}-{:04X}-{:012X}",
        hi >> 32,
        (hi >> 16) & 0xFFFF,
        hi & 0xFFFF,
        lo >> 48,
        lo & 0xFFFF_FFFF_FFFF
    )
}

/// The single list of what counts as geometry (`ExposureSettings.keepingGeometry(of:)`), shared
/// by Paste Adjustments, the profile editor's per-frame compose and profile stripping.
pub trait SettingsGeometry: Sized {
    /// Copy with `target`'s geometry kept in place.
    fn keeping_geometry(&self, target: &Self) -> Self;
    /// Adjustments with stock geometry — what a profile stores.
    fn adjustments_only(&self) -> Self;
}

impl SettingsGeometry for ExposureSettings {
    fn keeping_geometry(&self, target: &Self) -> Self {
        let mut next = self.clone();
        next.rotation = target.rotation;
        next.flip_horizontal = target.flip_horizontal;
        next.fine_rotation = target.fine_rotation;
        next.crop_rect = target.crop_rect;
        next.analysis_rect = target.analysis_rect;
        next.analysis_rect_fine_rotation = target.analysis_rect_fine_rotation;
        next
    }

    fn adjustments_only(&self) -> Self {
        self.keeping_geometry(&ExposureSettings::default())
    }
}

/// The persisted blob: the two UserDefaults keys (`settingsProfiles` = USER profiles only,
/// `activeProfileID`) as one JSON object.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PersistedProfiles {
    #[serde(default)]
    pub settings_profiles: Vec<SettingsProfile>,
    #[serde(default, rename = "activeProfileID")]
    pub active_profile_id: Option<String>,
}

/// The profile library and which profile is the app's default look.
#[derive(Clone, Debug, PartialEq)]
pub struct ProfileStore {
    user_profiles: Vec<SettingsProfile>,
    active_id: String,
}

impl Default for ProfileStore {
    fn default() -> Self {
        Self::new()
    }
}

impl ProfileStore {
    /// Empty store, "None" active.
    pub fn new() -> Self {
        Self {
            user_profiles: Vec::new(),
            active_id: NONE_PROFILE_ID.to_owned(),
        }
    }

    /// Restore from a persisted blob: user profiles as stored (geometry re-stripped defensively);
    /// the active id only if it names a profile in the list, else "None".
    pub fn from_persisted(persisted: PersistedProfiles) -> Self {
        let mut store = Self::new();
        store.user_profiles = persisted
            .settings_profiles
            .into_iter()
            .filter(|p| !is_reserved_profile_id(&p.id))
            .map(|mut p| {
                p.settings = p.settings.adjustments_only();
                p
            })
            .collect();
        if let Some(id) = persisted.active_profile_id
            && store.find(&id).is_some()
        {
            store.active_id = id;
        }
        store
    }

    /// Parse a persisted blob leniently, the way Swift reads its two UserDefaults keys
    /// independently: a malformed profile entry is skipped (the rest and `activeProfileID`
    /// survive — one bad row must not make the next save persist an empty list); a blob that
    /// is not a JSON object → an empty store.
    pub fn from_json(json: &str) -> Self {
        let Ok(serde_json::Value::Object(map)) = serde_json::from_str::<serde_json::Value>(json)
        else {
            return Self::new();
        };
        let settings_profiles = match map.get("settingsProfiles") {
            Some(serde_json::Value::Array(items)) => items
                .iter()
                .filter_map(|v| serde_json::from_value::<SettingsProfile>(v.clone()).ok())
                .filter(|p| is_uuid_shaped(&p.id))
                .collect(),
            _ => Vec::new(),
        };
        let active_profile_id = map
            .get("activeProfileID")
            .and_then(|v| v.as_str())
            .map(str::to_owned);
        Self::from_persisted(PersistedProfiles {
            settings_profiles,
            active_profile_id,
        })
    }

    /// Load from a file; missing or corrupt → an empty store.
    pub fn load(path: &Path) -> Self {
        std::fs::read_to_string(path)
            .map(|s| Self::from_json(&s))
            .unwrap_or_default()
    }

    /// The persisted form.
    pub fn persisted(&self) -> PersistedProfiles {
        PersistedProfiles {
            settings_profiles: self.user_profiles.clone(),
            active_profile_id: Some(self.active_id.clone()),
        }
    }

    /// The persisted blob as pretty JSON.
    pub fn to_json(&self) -> Result<String> {
        Ok(serde_json::to_string_pretty(&self.persisted())?)
    }

    /// Save to a file (atomic: a per-call temp beside it, fsync, rename — [`crate::atomic`]).
    pub fn save(&self, path: &Path) -> Result<()> {
        let text = self.to_json()?;
        atomic::write_atomic(path, text.as_bytes())
    }

    /// None, then the built-in default, then user profiles in creation order.
    pub fn all(&self) -> Vec<SettingsProfile> {
        let mut v = Vec::with_capacity(2 + self.user_profiles.len());
        v.push(SettingsProfile::none());
        v.push(SettingsProfile::builtin());
        v.extend(self.user_profiles.iter().cloned());
        v
    }

    /// User profiles only (what persists).
    pub fn user_profiles(&self) -> &[SettingsProfile] {
        &self.user_profiles
    }

    /// The active profile's id.
    pub fn active_id(&self) -> &str {
        &self.active_id
    }

    /// The active profile ("None" if the id resolves to nothing).
    pub fn active(&self) -> SettingsProfile {
        self.find(&self.active_id)
            .unwrap_or_else(SettingsProfile::none)
    }

    /// `DefaultProfile.settings`: what a fresh frame gets.
    pub fn default_settings(&self) -> ExposureSettings {
        self.active().settings
    }

    /// Look a profile up by id (reserved rows included).
    pub fn find(&self, id: &str) -> Option<SettingsProfile> {
        self.all()
            .into_iter()
            .find(|p| p.id.eq_ignore_ascii_case(id))
    }

    /// Make `id` the default look; `false` (unchanged) when it names no profile.
    pub fn set_active(&mut self, id: &str) -> bool {
        match self.find(id) {
            Some(p) => {
                self.active_id = p.id;
                true
            }
            None => false,
        }
    }

    /// Insert or update a user profile (geometry stripped here, so no caller can persist a crop
    /// into a profile). Reserved ids and ids that are not UUID-shaped are refused (`false`).
    pub fn upsert(&mut self, profile: SettingsProfile) -> bool {
        if is_reserved_profile_id(&profile.id) || !is_uuid_shaped(&profile.id) {
            return false;
        }
        let mut stripped = profile;
        stripped.settings = stripped.settings.adjustments_only();
        match self
            .user_profiles
            .iter_mut()
            .find(|p| p.id.eq_ignore_ascii_case(&stripped.id))
        {
            Some(slot) => *slot = stripped,
            None => self.user_profiles.push(stripped),
        }
        true
    }

    /// Remove a user profile; deleting the active one falls back to "None". Reserved ids are
    /// refused (`false`).
    pub fn delete(&mut self, id: &str) -> bool {
        if is_reserved_profile_id(id) {
            return false;
        }
        let before = self.user_profiles.len();
        self.user_profiles
            .retain(|p| !p.id.eq_ignore_ascii_case(id));
        if self.active_id.eq_ignore_ascii_case(id) {
            self.active_id = NONE_PROFILE_ID.to_owned();
        }
        self.user_profiles.len() != before
    }
}
