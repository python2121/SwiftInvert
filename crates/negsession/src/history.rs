//! The per-image edit history — `AppModel`'s undo/redo state machine (`AppModel.swift:351-492`,
//! session-spec §10) and `HistoryLabels.swift`.
//!
//! The Mac rules: seeded with "Original conversion"; drag edits commit ONCE on the last release;
//! non-drag edits coalesce 700 ms after the last tick; a commit is a no-op when nothing changed,
//! truncates the redo tail, and takes an armed named label (Rotate, Crop, Reset all…) over the
//! field diff; undo flushes an uncommitted edit first (so it steps back exactly one); redo with an
//! uncommitted edit commits it and STOPS (the edit truncated the tail); jumping disarms any
//! pending label; unbounded (the Qt shell capped at 100 — the Mac wins).
//!
//! Timers are the front end's: [`History::COALESCE_WINDOW`] and [`PendingCommit`] give it the
//! numbers and the bookkeeping, the engine never sleeps.

use std::time::{Duration, Instant};

use negkit::ExposureSettings;

/// One history entry (full settings, so a jump is a plain assignment).
#[derive(Clone, Debug, PartialEq)]
pub struct HistoryEntry {
    pub id: u64,
    pub label: String,
    pub settings: ExposureSettings,
}

/// Per-image undo/redo state.
#[derive(Clone, Debug, PartialEq)]
pub struct History {
    entries: Vec<HistoryEntry>,
    index: usize,
    pending_label: Option<String>,
    next_id: u64,
}

impl History {
    /// Non-drag edits commit this long after the last tick (`scheduleHistoryCommit`).
    pub const COALESCE_WINDOW: Duration = Duration::from_millis(700);

    /// Label of the seed entry.
    pub const SEED_LABEL: &'static str = "Original conversion";

    /// Seed with the settings at open.
    pub fn new(settings_at_open: ExposureSettings) -> Self {
        Self {
            entries: vec![HistoryEntry {
                id: 0,
                label: Self::SEED_LABEL.to_owned(),
                settings: settings_at_open,
            }],
            index: 0,
            pending_label: None,
            next_id: 1,
        }
    }

    /// All entries, oldest first.
    pub fn entries(&self) -> &[HistoryEntry] {
        &self.entries
    }

    /// Index of the current entry.
    pub fn index(&self) -> usize {
        self.index
    }

    /// The current entry.
    pub fn current(&self) -> Option<&HistoryEntry> {
        self.entries.get(self.index)
    }

    pub fn can_undo(&self) -> bool {
        self.index > 0
    }

    pub fn can_redo(&self) -> bool {
        self.index + 1 < self.entries.len()
    }

    /// Arm a named label (set BEFORE the mutation; the next commit prefers it over the diff).
    pub fn set_pending_label(&mut self, label: Option<impl Into<String>>) {
        self.pending_label = label.map(Into::into);
    }

    /// The armed label, if any.
    pub fn pending_label(&self) -> Option<&str> {
        self.pending_label.as_deref()
    }

    /// True when `current` is newer than the current entry (inside the coalesce window) — undo
    /// can flush-and-revert it, so the menu item stays enabled even at index 0.
    pub fn has_uncommitted_edit(&self, current: &ExposureSettings) -> bool {
        self.current().is_some_and(|e| e.settings != *current)
    }

    /// Commit `current` as a new entry: no-op (and the armed label disarmed) when it equals the
    /// current entry; otherwise the redo tail is truncated and the entry appended with the armed
    /// label or the field diff. Returns whether an entry was added.
    pub fn commit(&mut self, current: &ExposureSettings) -> bool {
        let Some(head) = self.entries.get(self.index) else {
            return false;
        };
        if head.settings == *current {
            self.pending_label = None;
            return false;
        }
        let label = self
            .pending_label
            .take()
            .unwrap_or_else(|| history_label(&head.settings, current));
        self.entries.truncate(self.index + 1);
        self.entries.push(HistoryEntry {
            id: self.next_id,
            label,
            settings: current.clone(),
        });
        self.next_id = self.next_id.wrapping_add(1);
        self.index = self.entries.len() - 1;
        true
    }

    /// Commit with an explicit label (a named action in one call).
    pub fn commit_labeled(&mut self, label: impl Into<String>, current: &ExposureSettings) -> bool {
        self.pending_label = Some(label.into());
        self.commit(current)
    }

    /// Undo: flush an uncommitted `current` first, then step back one. Returns the settings to
    /// apply (the caller assigns them WITHOUT committing — `isNavigatingHistory`).
    pub fn undo(&mut self, current: &ExposureSettings) -> Option<ExposureSettings> {
        if self.has_uncommitted_edit(current) {
            self.commit(current);
        }
        if !self.can_undo() {
            return None;
        }
        self.index -= 1;
        self.current().map(|e| e.settings.clone())
    }

    /// Redo: an uncommitted edit is committed and the redo stops there (it truncated the tail —
    /// `None` is returned because nothing is to be applied); else step forward one.
    pub fn redo(&mut self, current: &ExposureSettings) -> Option<ExposureSettings> {
        if self.has_uncommitted_edit(current) {
            self.commit(current);
            return None;
        }
        if !self.can_redo() {
            return None;
        }
        self.index += 1;
        self.current().map(|e| e.settings.clone())
    }

    /// Jump to entry `index` (click in the panel). Disarms a pending label; `None` when out of
    /// range or already there.
    pub fn jump(&mut self, index: usize) -> Option<ExposureSettings> {
        if index >= self.entries.len() || index == self.index {
            return None;
        }
        self.pending_label = None;
        self.index = index;
        self.current().map(|e| e.settings.clone())
    }
}

/// Bookkeeping for the front end's coalesced commit and drag hold (`scheduleHistoryCommit` /
/// `setControlEditing`): a due time that drag sessions suspend.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PendingCommit {
    due: Option<Instant>,
    control_edit_count: u32,
}

impl PendingCommit {
    /// A non-drag edit ticked at `now`: (re)arm the coalesced commit unless a drag is held.
    pub fn schedule(&mut self, now: Instant) {
        if self.control_edit_count == 0 {
            self.due = Some(now + History::COALESCE_WINDOW);
        }
    }

    /// Cancel the armed commit.
    pub fn cancel(&mut self) {
        self.due = None;
    }

    /// Whether the armed commit has come due.
    pub fn is_due(&self, now: Instant) -> bool {
        self.due.is_some_and(|d| now >= d)
    }

    /// Whether a commit is armed.
    pub fn is_armed(&self) -> bool {
        self.due.is_some()
    }

    /// A draggable control reports begin/end. Beginning cancels the armed commit; the LAST end
    /// returns `true` = commit now (one entry per drag session).
    pub fn set_control_editing(&mut self, editing: bool) -> bool {
        if editing {
            self.control_edit_count += 1;
            self.due = None;
            false
        } else {
            self.control_edit_count = self.control_edit_count.saturating_sub(1);
            self.control_edit_count == 0
        }
    }

    /// Whether any control is held.
    pub fn is_dragging(&self) -> bool {
        self.control_edit_count > 0
    }
}

/// Human-readable label for a settings change (`historyLabel(from:to:)`): a field-by-field diff
/// in the Swift order; 0 changes → "Edit", 1–2 → joined with ", ", ≥ 3 → "Multiple changes".
pub fn history_label(a: &ExposureSettings, b: &ExposureSettings) -> String {
    let mut changes: Vec<String> = Vec::new();
    let mut add = |name: &str, changed: bool, value: String| {
        if changed {
            changes.push(if value.is_empty() {
                name.to_owned()
            } else {
                format!("{name} {value}")
            });
        }
    };
    let f2 = |v: f64| format!("{v:.2}");
    let f2s = |v: f64| format!("{v:+.2}");
    let f3 = |v: f64| format!("{v:.3}");
    let on = |v: bool| (if v { "on" } else { "off" }).to_owned();
    let none = String::new();

    add("Brightness", a.density != b.density, f2(2.0 - b.density));
    add("Grade", a.grade != b.grade, format!("{:.0}", b.grade));
    add(
        "Exposure",
        a.exposure_stops != b.exposure_stops,
        f2s(b.exposure_stops),
    );
    add(
        "Contrast",
        a.overall_contrast != b.overall_contrast,
        f2(b.overall_contrast),
    );
    add("Shadows", a.shadows != b.shadows, f2(b.shadows));
    add(
        "Shadow contrast",
        a.shadow_contrast != b.shadow_contrast,
        f2(b.shadow_contrast),
    );
    add(
        "Dark shadows",
        a.dark_shadows != b.dark_shadows,
        f2(b.dark_shadows),
    );
    add("Highlights", a.highlights != b.highlights, f2(b.highlights));
    add(
        "Highlight contrast",
        a.highlight_contrast != b.highlight_contrast,
        f2(b.highlight_contrast),
    );
    add(
        "Pre-saturation",
        a.pre_saturation != b.pre_saturation,
        f2(b.pre_saturation),
    );
    add(
        "Print saturation",
        a.print_saturation != b.print_saturation,
        f2(b.print_saturation),
    );
    add(
        "Separation damping",
        a.separation_damping != b.separation_damping,
        f2(b.separation_damping),
    );
    add(
        "Contrast mask",
        a.contrast_mask != b.contrast_mask,
        f2(b.contrast_mask),
    );
    add(
        "Mask spacer",
        a.mask_spacer != b.mask_spacer,
        f2(b.mask_spacer),
    );
    add("Hue trim", a.hue_trim != b.hue_trim, f2(b.hue_trim));
    add("Vibrance", a.vibrance != b.vibrance, f2(b.vibrance));
    add(
        "Skin protection",
        a.skin_protection != b.skin_protection,
        f2(b.skin_protection),
    );
    add("Saturation", a.saturation != b.saturation, f2(b.saturation));
    add("Red hue", a.red_hue != b.red_hue, f2s(b.red_hue));
    add(
        "Red saturation",
        a.red_saturation != b.red_saturation,
        f2(b.red_saturation),
    );
    add(
        "Yellow hue",
        a.yellow_hue != b.yellow_hue,
        f2s(b.yellow_hue),
    );
    add(
        "Yellow saturation",
        a.yellow_saturation != b.yellow_saturation,
        f2(b.yellow_saturation),
    );
    add("Green hue", a.green_hue != b.green_hue, f2s(b.green_hue));
    add(
        "Green saturation",
        a.green_saturation != b.green_saturation,
        f2(b.green_saturation),
    );
    add("Blue hue", a.blue_hue != b.blue_hue, f2s(b.blue_hue));
    add(
        "Blue saturation",
        a.blue_saturation != b.blue_saturation,
        f2(b.blue_saturation),
    );
    add(
        "Cast strength",
        a.cast_removal_strength != b.cast_removal_strength,
        f2(b.cast_removal_strength),
    );
    add("Temp", a.temp != b.temp, f2(b.temp));
    add("Tint", a.tint != b.tint, f2(b.tint));
    add(
        "Shadow color",
        a.color_shadows != b.color_shadows,
        none.clone(),
    );
    add("Mid color", a.color_mids != b.color_mids, none.clone());
    add(
        "Highlight color",
        a.color_highs != b.color_highs,
        none.clone(),
    );
    add("Red levels", a.levels_red != b.levels_red, none.clone());
    add(
        "Green levels",
        a.levels_green != b.levels_green,
        none.clone(),
    );
    add("Blue levels", a.levels_blue != b.levels_blue, none.clone());
    add(
        "WB trim",
        a.wb_cyan != b.wb_cyan || a.wb_magenta != b.wb_magenta || a.wb_yellow != b.wb_yellow,
        none.clone(),
    );
    add("Toe", a.toe != b.toe, f2(b.toe));
    add("Toe width", a.toe_width != b.toe_width, f2(b.toe_width));
    add("Shoulder", a.shoulder != b.shoulder, f2(b.shoulder));
    add(
        "Shoulder width",
        a.shoulder_width != b.shoulder_width,
        f2(b.shoulder_width),
    );
    add(
        "White point",
        a.white_point_offset != b.white_point_offset,
        f3(b.white_point_offset),
    );
    add(
        "Black point",
        a.black_point_offset != b.black_point_offset,
        f3(b.black_point_offset),
    );
    add(
        "Auto exposure",
        a.auto_exposure != b.auto_exposure,
        on(b.auto_exposure),
    );
    add(
        "Auto contrast",
        a.auto_normalize_contrast != b.auto_normalize_contrast,
        on(b.auto_normalize_contrast),
    );
    add(
        "Paper white",
        a.paper_dmin != b.paper_dmin,
        on(b.paper_dmin),
    );
    add("True black", a.true_black != b.true_black, on(b.true_black));
    add("Rotate", a.rotation != b.rotation, none.clone());
    add(
        "Fine rotation",
        a.fine_rotation != b.fine_rotation,
        format!("{:.1}°", b.fine_rotation),
    );
    add("Flip", a.flip_horizontal != b.flip_horizontal, none.clone());
    add(
        if b.crop_rect.is_none() {
            "Crop cleared"
        } else {
            "Crop"
        },
        a.crop_rect != b.crop_rect,
        none.clone(),
    );
    // analysisRectFineRotation is the angle the region was drawn at — it only ever changes as
    // part of a region edit, so it shares the label.
    add(
        if b.analysis_rect.is_none() {
            "Analysis region cleared"
        } else {
            "Analysis region"
        },
        a.analysis_rect != b.analysis_rect
            || a.analysis_rect_fine_rotation != b.analysis_rect_fine_rotation,
        none,
    );

    match changes.len() {
        0 => "Edit".to_owned(),
        1 | 2 => changes.join(", "),
        _ => "Multiple changes".to_owned(),
    }
}

/// The named labels `AppModel` arms before a mutation (so front ends share one spelling).
pub mod labels {
    pub const ROTATE_CW: &str = "Rotate 90° CW";
    pub const ROTATE_CCW: &str = "Rotate 90° CCW";
    pub const FLIP_HORIZONTAL: &str = "Flip horizontal";
    pub const CROP: &str = "Crop";
    pub const CROP_CLEARED: &str = "Crop cleared";
    pub const ANALYSIS_REGION: &str = "Analysis region";
    pub const ANALYSIS_REGION_CLEARED: &str = "Analysis region cleared";
    pub const CROP_AND_STRAIGHTEN_CLEARED: &str = "Crop & straighten cleared";
    pub const STRAIGHTEN_CLEARED: &str = "Straighten cleared";
    pub const CANCEL_CROP: &str = "Cancel crop";
    pub const RESET_ALL: &str = "Reset all";
    pub const START_FROM_SCRATCH: &str = "Start from scratch";
    pub const TEST_STRIP_PICK: &str = "Test strip pick";
    pub const ZONE_PLACEMENT: &str = "Zone placement";
    pub const PASTE_ADJUSTMENTS: &str = "Paste adjustments";
}
