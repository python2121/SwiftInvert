//! Dialogs (wave 4 / U2b): the Export sheet + batch progress ([`export`]) and Choose Default
//! Settings ([`profiles`]), plus the in-app folder browser both the export destination and
//! (in spirit) the shell's Open Folder use — no GTK/portal dependency.
//!
//! The app owns one [`DialogState`]; [`DialogState::ui`] draws whatever is open and returns
//! [`DialogAction`]s for the things only the app can do (start the engine job, persist the
//! profile store).

pub mod export;
pub mod profiles;

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use egui::Key;
use negkit::ExposureSettings;
use negsession::{ExportOptions, ProfileStore};

use crate::engine::Evt;
pub use export::{ExportDialog, ExportJob};
pub use profiles::ProfilesDialog;

/// What a dialog asks the app to do.
#[derive(Clone, Debug, PartialEq)]
pub enum DialogAction {
    /// Export `targets` (library order) with `options` (already validated; also the new sticky
    /// options).
    StartExport {
        options: ExportOptions,
        targets: Vec<PathBuf>,
    },
    /// The profile store changed (persist it; the active profile may be new).
    ProfilesChanged,
    /// A one-line status for the sidebar.
    Status(String),
}

/// Every dialog the app can show.
#[derive(Default)]
pub struct DialogState {
    pub export: Option<ExportDialog>,
    pub job: Option<ExportJob>,
    pub profiles: Option<ProfilesDialog>,
}

impl DialogState {
    /// A modal dialog is up (keys and canvas tools stand down).
    pub fn is_modal_open(&self) -> bool {
        self.export.is_some() || self.profiles.is_some()
    }

    /// A batch export is running.
    pub fn exporting(&self) -> bool {
        self.job.is_some()
    }

    /// Open the export sheet for `targets` with the sticky `options`.
    pub fn open_export(&mut self, options: ExportOptions, targets: Vec<PathBuf>) {
        if self.job.is_none() && !targets.is_empty() {
            self.export = Some(ExportDialog::new(options, targets));
        }
    }

    /// Open Choose Default Settings over `store`.
    pub fn open_profiles(&mut self, store: &ProfileStore) {
        self.profiles = Some(ProfilesDialog::new(store));
    }

    /// Escape: close the topmost dialog. `true` when one was closed.
    pub fn escape(&mut self) -> bool {
        if self.export.take().is_some() {
            return true;
        }
        self.profiles.take().is_some()
    }

    /// The engine accepted a job: track it; the returned flag cancels it.
    pub fn start_job(&mut self, generation: u64, total: usize) -> Arc<AtomicBool> {
        let job = ExportJob::new(generation, total);
        let cancel = job.cancel_flag();
        self.job = Some(job);
        cancel
    }

    /// Feed an export event; returns the final status line when the batch ended.
    pub fn on_engine_event(&mut self, evt: &Evt) -> Option<String> {
        let job = self.job.as_mut()?;
        let done = job.on_event(evt)?;
        self.job = None;
        Some(done)
    }

    /// Draw the open dialogs.
    pub fn ui(
        &mut self,
        ctx: &egui::Context,
        store: &mut ProfileStore,
        current: Option<&ExposureSettings>,
    ) -> Vec<DialogAction> {
        let mut actions = Vec::new();
        if let Some(d) = &mut self.export {
            match d.ui(ctx) {
                Some(export::SheetResult::Export(options, targets)) => {
                    actions.push(DialogAction::StartExport { options, targets });
                    self.export = None;
                }
                Some(export::SheetResult::Cancel) => self.export = None,
                None => {}
            }
        }
        if let Some(job) = &mut self.job {
            job.ui(ctx);
        }
        if let Some(d) = &mut self.profiles {
            let (close, changed, status) = d.ui(ctx, store, current);
            if changed {
                actions.push(DialogAction::ProfilesChanged);
            }
            if let Some(s) = status {
                actions.push(DialogAction::Status(s));
            }
            if close {
                self.profiles = None;
            }
        }
        actions
    }
}

/// The in-app folder browser: a path field, ↑ Parent, and the sub-folders of the typed path
/// (hidden ones skipped, natural order, capped at 500 rows).
#[derive(Clone, Debug, Default)]
pub struct FolderBrowser {
    pub text: String,
}

impl FolderBrowser {
    pub fn new(start: Option<&Path>) -> Self {
        let text = start
            .map(Path::to_path_buf)
            .or_else(|| std::env::var_os("HOME").map(PathBuf::from))
            .map(|p| p.display().to_string())
            .unwrap_or_default();
        Self { text }
    }

    /// The typed folder.
    pub fn current(&self) -> PathBuf {
        PathBuf::from(self.text.trim())
    }

    /// Draw it; `Some(folder)` when the user confirmed one (Enter in the field, "Use This
    /// Folder", or a double-click on a row).
    pub fn ui(&mut self, ui: &mut egui::Ui, confirm_label: &str) -> Option<PathBuf> {
        let mut chosen = None;
        ui.horizontal(|ui| {
            let r = ui.add(egui::TextEdit::singleline(&mut self.text).desired_width(f32::INFINITY));
            if r.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)) {
                chosen = Some(self.current());
            }
        });
        let current = self.current();
        ui.horizontal(|ui| {
            if ui.button("↑ Parent").clicked()
                && let Some(parent) = current.parent()
            {
                self.text = parent.display().to_string();
            }
            if ui
                .add_enabled(current.is_dir(), egui::Button::new(confirm_label))
                .clicked()
            {
                chosen = Some(current.clone());
            }
        });
        let dirs = subfolders(&current);
        egui::ScrollArea::vertical()
            .max_height(180.0)
            .auto_shrink([false, true])
            .show(ui, |ui| {
                for d in dirs.iter().take(500) {
                    let name = d
                        .file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_default();
                    let r = ui.selectable_label(false, format!("▸ {name}"));
                    if r.double_clicked() {
                        chosen = Some(d.clone());
                    } else if r.clicked() {
                        self.text = d.display().to_string();
                    }
                }
            });
        chosen.filter(|c| c.is_dir())
    }
}

/// Visible sub-folders of `dir`, natural order.
pub fn subfolders(dir: &Path) -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = std::fs::read_dir(dir)
        .map(|rd| {
            rd.flatten()
                .map(|e| e.path())
                .filter(|p| {
                    p.is_dir()
                        && !p
                            .file_name()
                            .is_some_and(|n| n.to_string_lossy().starts_with('.'))
                })
                .collect()
        })
        .unwrap_or_default();
    let name = |p: &PathBuf| {
        p.file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default()
    };
    dirs.sort_by(|a, b| negsession::library::natural_cmp(&name(a), &name(b)));
    dirs
}
