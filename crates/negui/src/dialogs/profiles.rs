//! Choose Default Settings… (`ProfileWindows.swift`'s picker, session-spec §2): "None" and
//! "SwiftInvert Default" lead the list and can't be renamed, updated or deleted; user profiles
//! follow in creation order. Accept makes the selected row the active profile — what Reset All
//! and every frame opened without a sidecar get (`ProfileStore::default_settings`). Create New
//! snapshots the CURRENT frame's adjustments (geometry stripped by the store).
//!
//! **Seam — the profile-editor WINDOW mode is not built.** The Mac's "Create/Edit" opens the
//! whole app under `AppModel(profileEditor: true)`: one shared adjustments draft applied to
//! every frame (geometry per frame), and NO sidecar ever written. Here "Update from Current"
//! stands in for editing (it re-snapshots the open frame's adjustments into the selected user
//! profile). A future editor mode needs: a draft `ExposureSettings` the app renders every
//! frame with (`draft.keeping_geometry(&frame_settings)`), `autosave` forced off and the
//! export/paste flush guards, and a Save that calls [`update_profile`].

use negkit::ExposureSettings;
use negsession::ProfileStore;
use negsession::profiles::{SettingsProfile, is_reserved_profile_id};

/// Name used when the field is left empty.
pub const UNTITLED: &str = "Untitled Profile";

/// Whether a row may be renamed / updated / deleted (user profiles only).
pub fn can_modify(id: &str) -> bool {
    !is_reserved_profile_id(id)
}

/// `base` (trimmed; [`UNTITLED`] when empty), suffixed " 2", " 3"… until no profile in the list
/// (reserved rows included, case-insensitive) carries it.
pub fn unique_name(store: &ProfileStore, base: &str, except_id: Option<&str>) -> String {
    let base = match base.trim() {
        "" => UNTITLED,
        b => b,
    };
    let taken = |name: &str| {
        store.all().iter().any(|p| {
            p.name.eq_ignore_ascii_case(name)
                && except_id.is_none_or(|id| !p.id.eq_ignore_ascii_case(id))
        })
    };
    if !taken(base) {
        return base.to_owned();
    }
    (2..)
        .map(|n| format!("{base} {n}"))
        .find(|n| !taken(n))
        .unwrap_or_else(|| base.to_owned())
}

/// Create a user profile from `settings` (adjustments only); returns its id.
pub fn create_profile(
    store: &mut ProfileStore,
    name: &str,
    settings: &ExposureSettings,
) -> Option<String> {
    let profile = SettingsProfile::new(unique_name(store, name, None), settings);
    let id = profile.id.clone();
    store.upsert(profile).then_some(id)
}

/// Rename a user profile (reserved rows refused; the name is made unique).
pub fn rename_profile(store: &mut ProfileStore, id: &str, name: &str) -> bool {
    if !can_modify(id) {
        return false;
    }
    let Some(mut p) = store.find(id) else {
        return false;
    };
    p.name = unique_name(store, name, Some(id));
    store.upsert(p)
}

/// Replace a user profile's adjustments with `settings` (the stand-in for the editor).
pub fn update_profile(store: &mut ProfileStore, id: &str, settings: &ExposureSettings) -> bool {
    if !can_modify(id) {
        return false;
    }
    let Some(mut p) = store.find(id) else {
        return false;
    };
    p.settings = settings.clone();
    store.upsert(p)
}

/// Delete a user profile (reserved rows refused; deleting the active one activates "None").
pub fn delete_profile(store: &mut ProfileStore, id: &str) -> bool {
    can_modify(id) && store.delete(id)
}

/// The open picker.
pub struct ProfilesDialog {
    /// The highlighted row (Accept makes it active).
    pub selected: String,
    pub new_name: String,
    pub rename: String,
    confirm_delete: bool,
}

impl ProfilesDialog {
    pub fn new(store: &ProfileStore) -> Self {
        let active = store.active();
        Self {
            selected: active.id.clone(),
            new_name: String::new(),
            rename: active.name,
            confirm_delete: false,
        }
    }

    fn select(&mut self, p: &SettingsProfile) {
        self.selected = p.id.clone();
        self.rename = p.name.clone();
        self.confirm_delete = false;
    }

    /// Draw. Returns `(close, store changed, status line)`.
    pub fn ui(
        &mut self,
        ctx: &egui::Context,
        store: &mut ProfileStore,
        current: Option<&ExposureSettings>,
    ) -> (bool, bool, Option<String>) {
        let (mut close, mut changed, mut status) = (false, false, None);
        egui::Modal::new(egui::Id::new("choose-default-settings")).show(ctx, |ui| {
            ui.set_width(400.0);
            ui.heading("Choose Default Settings");
            ui.label(
                egui::RichText::new(
                    "Applied to frames opened without saved settings and by Reset All.",
                )
                .weak()
                .size(11.0),
            );
            ui.add_space(6.0);
            let rows = store.all();
            egui::Frame::group(ui.style()).show(ui, |ui| {
                egui::ScrollArea::vertical()
                    .max_height(200.0)
                    .auto_shrink([false, true])
                    .show(ui, |ui| {
                        for p in &rows {
                            let active = p.id.eq_ignore_ascii_case(store.active_id());
                            let mut text = p.name.clone();
                            if active {
                                text.push_str("  (default)");
                            }
                            if !can_modify(&p.id) {
                                text.push_str("  · built-in");
                            }
                            let sel = p.id.eq_ignore_ascii_case(&self.selected);
                            let r = ui.selectable_label(sel, text);
                            if r.clicked() {
                                self.select(p);
                            }
                            if r.double_clicked() && store.set_active(&p.id) {
                                changed = true;
                                close = true;
                                status = Some(format!("Default settings: {}", p.name));
                            }
                        }
                    });
            });
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                ui.add(
                    egui::TextEdit::singleline(&mut self.new_name)
                        .hint_text(UNTITLED)
                        .desired_width(200.0),
                );
                let create = ui
                    .add_enabled(
                        current.is_some(),
                        egui::Button::new("Create New from Current"),
                    )
                    .on_hover_text("Snapshot the open frame's adjustments (no crop/rotation)")
                    .on_disabled_hover_text("Open a frame first");
                if create.clicked()
                    && let Some(s) = current
                    && let Some(id) = create_profile(store, &self.new_name, s)
                {
                    changed = true;
                    self.new_name.clear();
                    if let Some(p) = store.find(&id) {
                        self.select(&p);
                    }
                }
            });
            let modifiable = can_modify(&self.selected);
            ui.add_enabled_ui(modifiable, |ui| {
                ui.horizontal(|ui| {
                    ui.add(egui::TextEdit::singleline(&mut self.rename).desired_width(200.0));
                    if ui.button("Rename").clicked()
                        && rename_profile(store, &self.selected, &self.rename)
                    {
                        changed = true;
                        if let Some(p) = store.find(&self.selected) {
                            self.rename = p.name;
                        }
                    }
                });
                ui.horizontal(|ui| {
                    if ui
                        .add_enabled(current.is_some(), egui::Button::new("Update from Current"))
                        .on_hover_text("Replace this profile's adjustments with the open frame's")
                        .clicked()
                        && let Some(s) = current
                        && update_profile(store, &self.selected, s)
                    {
                        changed = true;
                        status = Some("Profile updated".to_owned());
                    }
                    let label = if self.confirm_delete {
                        "Really delete?"
                    } else {
                        "Delete…"
                    };
                    if ui.button(label).clicked() {
                        if self.confirm_delete {
                            if delete_profile(store, &self.selected) {
                                changed = true;
                                let active = store.active();
                                self.select(&active);
                            }
                        } else {
                            self.confirm_delete = true;
                        }
                    }
                });
            });
            ui.separator();
            ui.horizontal(|ui| {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button("Use as Default").clicked() {
                        if store.set_active(&self.selected) {
                            changed = true;
                            status = Some(format!("Default settings: {}", store.active().name));
                        }
                        close = true;
                    }
                    if ui.button("Close").clicked() {
                        close = true;
                    }
                });
            });
        });
        (close, changed, status)
    }
}
