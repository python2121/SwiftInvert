//! The Export sheet (`ExportSheet.swift`, session-spec §11) and the batch progress panel
//! (`AppModel.performExport`'s progress/cancel).
//!
//! Sheet: JPEG (quality 50…100 %, default 92) / TIFF 16-bit, sRGB (default) / Adobe RGB, resize
//! long edge (256…20000, step 500), next to originals / a chosen folder (in-app browser). Every
//! value is sticky independently (`ExportOptions`' own fields) through `AppPrefs`. Export is
//! disabled while a custom destination has no folder; Return exports, Escape cancels. Files
//! are named like the Mac (same basename, `.jpg`/`.tiff`, overwrite — written atomically).

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use egui::{Key, vec2};
use negsession::export::{JPEG_QUALITY_RANGE, LONG_EDGE_RANGE};
use negsession::{ExportColorSpace, ExportDestination, ExportFormat, ExportOptions};

use super::FolderBrowser;
use crate::engine::Evt;

/// The long-edge stepper's step (ExportSheet.swift).
pub const LONG_EDGE_STEP: u32 = 500;

/// The app's first-run export options: the Mac defaults except resize, which starts ON at
/// 3000 px (the Qt shell's sticky default — a 24 MP JPEG nobody asked for is the worse
/// surprise).
pub fn default_export_options() -> ExportOptions {
    ExportOptions {
        resize: true,
        ..ExportOptions::default()
    }
}

/// Clamp a loaded/edited option set into the sheet's ranges (a hand-edited blob can hold
/// anything).
pub fn sanitize(mut o: ExportOptions) -> ExportOptions {
    if !o.jpeg_quality.is_finite() {
        o.jpeg_quality = negsession::export::DEFAULT_JPEG_QUALITY;
    }
    o.jpeg_quality = o
        .jpeg_quality
        .clamp(JPEG_QUALITY_RANGE.0, JPEG_QUALITY_RANGE.1);
    o.max_long_edge = o.max_long_edge.clamp(LONG_EDGE_RANGE.0, LONG_EDGE_RANGE.1);
    o
}

/// What the sheet decided.
#[derive(Clone, Debug, PartialEq)]
pub enum SheetResult {
    Export(ExportOptions, Vec<PathBuf>),
    Cancel,
}

/// The open sheet.
pub struct ExportDialog {
    pub options: ExportOptions,
    pub targets: Vec<PathBuf>,
    browser: Option<FolderBrowser>,
}

impl ExportDialog {
    pub fn new(options: ExportOptions, targets: Vec<PathBuf>) -> Self {
        Self {
            options: sanitize(options),
            targets,
            browser: None,
        }
    }

    /// Draw; `Some` when the user exported or cancelled.
    pub fn ui(&mut self, ctx: &egui::Context) -> Option<SheetResult> {
        let mut result = None;
        let title = match self.targets.len() {
            1 => "Export 1 Image".to_owned(),
            n => format!("Export {n} Images"),
        };
        let o = &mut self.options;
        egui::Modal::new(egui::Id::new("export-sheet")).show(ctx, |ui| {
            ui.set_width(420.0);
            ui.heading(title);
            ui.add_space(6.0);
            egui::Grid::new("export-grid")
                .num_columns(2)
                .spacing(vec2(12.0, 8.0))
                .show(ui, |ui| {
                    ui.label("Format");
                    ui.horizontal(|ui| {
                        for f in [ExportFormat::Jpeg, ExportFormat::Tiff16] {
                            ui.radio_value(&mut o.format, f, f.label());
                        }
                    });
                    ui.end_row();
                    if o.format == ExportFormat::Jpeg {
                        ui.label("Quality");
                        let mut pct = o.jpeg_quality * 100.0;
                        let r = ui.add(
                            egui::Slider::new(
                                &mut pct,
                                JPEG_QUALITY_RANGE.0 * 100.0..=JPEG_QUALITY_RANGE.1 * 100.0,
                            )
                            .step_by(1.0)
                            .fixed_decimals(0)
                            .suffix("%"),
                        );
                        if r.changed() && pct.is_finite() {
                            o.jpeg_quality = (pct / 100.0).clamp(0.5, 1.0);
                        }
                        ui.end_row();
                    }
                    ui.label("Color space");
                    ui.horizontal(|ui| {
                        for c in [ExportColorSpace::Srgb, ExportColorSpace::AdobeRgb] {
                            ui.radio_value(&mut o.color_space, c, c.label());
                        }
                    });
                    ui.end_row();
                    ui.label("Size");
                    ui.horizontal(|ui| {
                        ui.checkbox(&mut o.resize, "Limit long edge to");
                        ui.add_enabled_ui(o.resize, |ui| {
                            ui.add(
                                egui::DragValue::new(&mut o.max_long_edge)
                                    .range(LONG_EDGE_RANGE.0..=LONG_EDGE_RANGE.1)
                                    .speed(10.0)
                                    .suffix(" px"),
                            );
                            if ui.small_button("−").clicked() {
                                o.max_long_edge = o
                                    .max_long_edge
                                    .saturating_sub(LONG_EDGE_STEP)
                                    .max(LONG_EDGE_RANGE.0);
                            }
                            if ui.small_button("+").clicked() {
                                o.max_long_edge = o
                                    .max_long_edge
                                    .saturating_add(LONG_EDGE_STEP)
                                    .min(LONG_EDGE_RANGE.1);
                            }
                        });
                    });
                    ui.end_row();
                    ui.label("Destination");
                    ui.vertical(|ui| {
                        let mut custom = o.use_custom_destination;
                        ui.radio_value(&mut custom, false, "Next to originals");
                        ui.horizontal(|ui| {
                            ui.radio_value(&mut custom, true, "Folder…");
                            if custom && ui.small_button("Choose…").clicked() {
                                self.browser =
                                    Some(FolderBrowser::new(o.custom_destination_path.as_deref()));
                            }
                        });
                        o.use_custom_destination = custom;
                        if custom {
                            let text = o
                                .custom_destination_path
                                .as_ref()
                                .map(|p| p.display().to_string())
                                .unwrap_or_else(|| "No folder chosen".to_owned());
                            ui.label(egui::RichText::new(text).weak().size(11.0));
                        }
                    });
                    ui.end_row();
                });
            if let Some(b) = &mut self.browser {
                ui.separator();
                ui.label(egui::RichText::new("Export folder").strong());
                if let Some(dir) = b.ui(ui, "Use This Folder") {
                    o.set_destination(ExportDestination::Folder(dir));
                    self.browser = None;
                }
            }
            ui.add_space(8.0);
            ui.separator();
            let valid = o.is_valid();
            ui.horizontal(|ui| {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let export = ui
                        .add_enabled(valid, egui::Button::new("Export"))
                        .on_disabled_hover_text("Choose a destination folder");
                    if export.clicked() {
                        result = Some(SheetResult::Export(o.clone(), self.targets.clone()));
                    }
                    if ui.button("Cancel").clicked() {
                        result = Some(SheetResult::Cancel);
                    }
                });
            });
            let (enter, esc) =
                ui.input(|i| (i.key_pressed(Key::Enter), i.key_pressed(Key::Escape)));
            if result.is_none() && esc {
                result = Some(SheetResult::Cancel);
            }
            if result.is_none() && enter && valid && self.browser.is_none() {
                result = Some(SheetResult::Export(o.clone(), self.targets.clone()));
            }
        });
        result
    }
}

/// A running batch.
pub struct ExportJob {
    pub generation: u64,
    pub total: usize,
    pub done: usize,
    pub current: Option<PathBuf>,
    cancel: Arc<AtomicBool>,
}

impl ExportJob {
    pub fn new(generation: u64, total: usize) -> Self {
        Self {
            generation,
            total,
            done: 0,
            current: None,
            cancel: Arc::new(AtomicBool::new(false)),
        }
    }

    pub fn cancel_flag(&self) -> Arc<AtomicBool> {
        Arc::clone(&self.cancel)
    }

    /// Ask the engine to stop before the next frame.
    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }

    /// Apply an engine event; `Some(status)` when the batch ended.
    pub fn on_event(&mut self, evt: &Evt) -> Option<String> {
        match evt {
            Evt::ExportProgress {
                generation,
                done,
                total,
                current,
            } if *generation == self.generation => {
                self.done = *done;
                self.total = *total;
                self.current = Some(current.clone());
                None
            }
            Evt::ExportFinished {
                generation,
                total,
                written,
                failed,
                cancelled,
            } if *generation == self.generation => Some(finish_status(
                *total,
                written.len(),
                failed.len(),
                *cancelled,
            )),
            _ => None,
        }
    }

    /// The bottom-right progress panel.
    pub fn ui(&mut self, ctx: &egui::Context) {
        let name = self
            .current
            .as_ref()
            .and_then(|p| p.file_name())
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        egui::Window::new("Exporting")
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::RIGHT_BOTTOM, vec2(-16.0, -48.0))
            .show(ctx, |ui| {
                ui.set_width(280.0);
                let frac = if self.total > 0 {
                    self.done as f32 / self.total as f32
                } else {
                    0.0
                };
                ui.label(format!(
                    "{} of {} — {name}",
                    (self.done + 1).min(self.total),
                    self.total
                ));
                ui.add(egui::ProgressBar::new(frac).show_percentage());
                let cancelling = self.cancel.load(Ordering::Relaxed);
                if ui
                    .add_enabled(!cancelling, egui::Button::new("Cancel"))
                    .clicked()
                {
                    self.cancel();
                }
            });
    }
}

/// The final status line (`performExport`'s three outcomes).
pub fn finish_status(total: usize, written: usize, failed: usize, cancelled: bool) -> String {
    if cancelled {
        format!("Export cancelled ({written} of {total} done)")
    } else if failed > 0 {
        format!("Exported {written} of {total} ({failed} failed)")
    } else {
        format!("Exported {written} image(s)")
    }
}
