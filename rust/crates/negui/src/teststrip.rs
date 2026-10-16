//! The test-strip layer (wave 4 / U3) — `TestStripLayer` (`TestStripView.swift`) + the
//! `AppModel` test-strip state (session-spec §7, NegPy e4bc450 / a2455ab): the frame printed as
//! a 5×5 mosaic of REAL renders, Brightness across (brighter →), Grade down, drawn over the
//! canvas with no gridlines (only the hovered patch is outlined, the current-settings rungs are
//! accented). Single click → full-frame preview of that patch (click again = back to the grid);
//! double click (grid or preview) → commit density + grade as one "Test strip pick". Singles
//! are HELD for the double-click interval on purpose (a mis-fired confirm would commit
//! settings). While a strip is up the rotate commands turn the LADDER (orientation 0–3,
//! session-sticky). Any edit, navigation, tool, baseline or HQ change clears it; a generation
//! counter fences stale builds.

use egui::{Align2, Color32, Rect, Stroke, StrokeKind, TextureHandle, pos2};
use negkit::ExposureSettings;
use negkit::test_strip::{self, COLS, DENSITIES_BY_COLUMN, GRADES_BY_ROW, ROWS, RowCol};
use negsession::{RenderOutput, TestStripOutput};

use crate::loupe::{CanvasOverlay, LoupeState, content_rect, pill};
use crate::theme::Tokens;
use crate::zones::bold_text;

/// What the layer asks the app to do.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StripAction {
    /// Render this patch full-frame (the preview stage).
    Preview { row: usize, col: usize },
    /// Click while previewing: back to the grid.
    BackToGrid,
    /// Double click: commit the patch's density + grade.
    Confirm { row: usize, col: usize },
}

/// The one-patch preview stage.
pub struct StripPreview {
    pub row: usize,
    pub col: usize,
    pub texture: TextureHandle,
    pub content_window: negkit::NormalizedRect,
}

/// A built strip on screen.
pub struct StripUp {
    pub texture: TextureHandle,
    /// The mosaic's geometry (its pixels live in the texture).
    pub content_window: negkit::NormalizedRect,
    pub size: [usize; 2],
    pub current: RowCol,
    /// Settings the strip was built from (its non-density/grade context).
    pub base: ExposureSettings,
    pub orientation: i32,
    pub preview: Option<StripPreview>,
}

/// The app's test-strip state.
#[derive(Default)]
pub struct TestStripController {
    /// Ladder orientation (0–3 × 90° CW), session-sticky.
    pub orientation: i32,
    /// Fences stale builds and previews.
    pub generation: u64,
    pub building: bool,
    /// The settings snapshot a pending build was requested with.
    pub snapshot: Option<ExposureSettings>,
    pub strip: Option<StripUp>,
    /// Last build time (ms, worker side).
    pub build_ms: f32,
    /// The patch whose preview render is in flight.
    pub requested_preview: Option<(usize, usize)>,
    hovered: Option<RowCol>,
    /// A single click waiting out the double-click interval: (time, cell or None = back).
    pending_single: Option<(f64, StripAction)>,
    actions: Vec<StripAction>,
}

impl std::fmt::Debug for TestStripController {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TestStripController")
            .field("orientation", &self.orientation)
            .field("generation", &self.generation)
            .field("building", &self.building)
            .field("up", &self.strip.is_some())
            .finish()
    }
}

/// `TestStripLayer.axisLabel`: the ladder value constant along a display column (or row) under
/// `orientation`, and whether it is the rung nearest the base settings.
pub fn axis_label(
    index: usize,
    is_column: bool,
    orientation: i32,
    base: &ExposureSettings,
) -> (String, bool) {
    let (a, b) = if is_column {
        (
            test_strip::cell(0, index, orientation),
            test_strip::cell(1, index, orientation),
        )
    } else {
        (
            test_strip::cell(index, 0, orientation),
            test_strip::cell(index, 1, orientation),
        )
    };
    let (Some(a), Some(b)) = (a, b) else {
        return (String::new(), false);
    };
    let nearest = test_strip::nearest_cell(base.density, base.grade);
    if a.density == b.density {
        let accented = DENSITIES_BY_COLUMN.get(nearest.col) == Some(&a.density);
        (format!("{:.1}", 2.0 - a.density), accented)
    } else {
        let accented = GRADES_BY_ROW.get(nearest.row) == Some(&a.grade);
        (format!("R{:.0}", a.grade), accented)
    }
}

/// The preview badge text (`TestStripLayer.previewBadge`).
pub fn preview_badge(row: usize, col: usize, orientation: i32) -> String {
    match test_strip::cell(row, col, orientation) {
        Some(c) => format!(
            "Brightness {:.1} · Grade R{:.0}  —  double-click to keep, click to compare others, Esc to cancel",
            2.0 - c.density,
            c.grade
        ),
        None => String::new(),
    }
}

/// The settings a patch commits onto `current` (density + grade only; autos untouched — the
/// patches were rendered under them).
pub fn picked_settings(
    current: &ExposureSettings,
    row: usize,
    col: usize,
    orientation: i32,
) -> Option<ExposureSettings> {
    let c = test_strip::cell(row, col, orientation)?;
    Some(ExposureSettings {
        density: c.density,
        grade: c.grade,
        ..current.clone()
    })
}

fn image_of(rgba: &[u8], w: usize, h: usize) -> Option<egui::ColorImage> {
    (w > 0 && h > 0 && rgba.len() == w * h * 4)
        .then(|| egui::ColorImage::from_rgba_unmultiplied([w, h], rgba))
}

impl TestStripController {
    /// Showing or building (the toggle shows active; rotate turns the ladder).
    pub fn owns_canvas(&self) -> bool {
        self.strip.is_some() || self.building
    }

    /// Start a build: bumps the generation and returns it.
    pub fn begin_build(&mut self, snapshot: ExposureSettings) -> u64 {
        self.generation += 1;
        self.building = true;
        self.snapshot = Some(snapshot);
        self.strip = None;
        self.pending_single = None;
        self.generation
    }

    /// `clearTestStrip`. Returns whether it owned the canvas.
    pub fn clear(&mut self) -> bool {
        let owned = self.owns_canvas();
        self.generation += 1;
        self.building = false;
        self.snapshot = None;
        self.strip = None;
        self.hovered = None;
        self.pending_single = None;
        self.requested_preview = None;
        self.actions.clear();
        owned
    }

    /// Adopt a finished build (the caller checked the generation and that nothing changed).
    pub fn adopt(&mut self, ctx: &egui::Context, out: &TestStripOutput, base: ExposureSettings) {
        self.building = false;
        self.snapshot = None;
        let Some(img) = image_of(&out.rgba8, out.width, out.height) else {
            return;
        };
        let texture = ctx.load_texture("test-strip", img, egui::TextureOptions::LINEAR);
        self.strip = Some(StripUp {
            texture,
            content_window: out.content_window,
            size: [out.width, out.height],
            current: out.current,
            base,
            orientation: out.orientation,
            preview: None,
        });
    }

    /// Adopt a patch preview render.
    pub fn adopt_preview(
        &mut self,
        ctx: &egui::Context,
        row: usize,
        col: usize,
        out: &RenderOutput,
    ) {
        let Some(strip) = &mut self.strip else {
            return;
        };
        let Some(img) = out
            .frame
            .rgba8()
            .and_then(|b| image_of(b, out.width, out.height))
        else {
            return;
        };
        let texture = ctx.load_texture("test-strip-preview", img, egui::TextureOptions::LINEAR);
        strip.preview = Some(StripPreview {
            row,
            col,
            texture,
            content_window: out.content_window,
        });
    }

    /// Back to the grid.
    pub fn back_to_grid(&mut self) {
        if let Some(s) = &mut self.strip {
            s.preview = None;
        }
    }

    /// Drain the layer's actions, releasing a held single click once the double-click interval
    /// has passed. Asks for a repaint while one is held.
    pub fn take_actions(&mut self, ctx: &egui::Context) -> Vec<StripAction> {
        if let Some((t, action)) = self.pending_single {
            let (now, delay) = (
                ctx.input(|i| i.time),
                ctx.options(|o| o.input_options.max_double_click_delay),
            );
            if now - t > delay {
                self.pending_single = None;
                self.actions.push(action);
            } else {
                ctx.request_repaint_after(std::time::Duration::from_secs_f64(
                    (delay - (now - t)).max(0.0) + 0.01,
                ));
            }
        }
        std::mem::take(&mut self.actions)
    }

    fn cell_under(&self, p: egui::Pos2, frame_rect: Rect) -> Option<RowCol> {
        let strip = self.strip.as_ref()?;
        let r = content_rect(frame_rect, &strip.content_window);
        if !r.contains(p) {
            return None;
        }
        Some(test_strip::cell_at(
            f64::from((p.x - r.left()) / r.width().max(1.0)),
            f64::from((p.y - r.top()) / r.height().max(1.0)),
        ))
    }
}

impl CanvasOverlay for TestStripController {
    fn paint(&mut self, ui: &mut egui::Ui, frame_rect: Rect, zoom_state: &LoupeState) {
        let t = Tokens::get(ui.ctx());
        let clip = ui.clip_rect();
        let p = ui.painter();
        let Some(strip) = &self.strip else {
            if self.building {
                pill(
                    p,
                    pos2(clip.center().x, clip.top() + 12.0),
                    Align2::CENTER_TOP,
                    "Printing test strip…",
                    &t,
                );
            }
            return;
        };
        let _ = zoom_state;
        let uv = Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0));
        if let Some(pv) = &strip.preview {
            let r = content_rect(frame_rect, &pv.content_window);
            p.rect_filled(frame_rect, 0.0, Color32::BLACK);
            p.image(pv.texture.id(), r, uv, Color32::WHITE);
            pill(
                p,
                pos2(
                    frame_rect.center().x,
                    frame_rect.bottom().min(clip.bottom()) - 10.0,
                ),
                Align2::CENTER_BOTTOM,
                &preview_badge(pv.row, pv.col, strip.orientation),
                &t,
            );
            return;
        }
        let r = content_rect(frame_rect, &strip.content_window);
        p.rect_filled(frame_rect, 0.0, Color32::BLACK);
        p.image(strip.texture.id(), r, uv, Color32::WHITE);
        let (cw, ch) = (r.width() / COLS as f32, r.height() / ROWS as f32);
        let size = (cw.min(ch) * 0.14).clamp(10.0, 16.0);
        let font = egui::FontId::proportional(size);
        for c in 0..COLS {
            let (text, accent) = axis_label(c, true, strip.orientation, &strip.base);
            bold_text(
                p,
                pos2(r.left() + (c as f32 + 0.5) * cw, r.top() + size * 0.9),
                Align2::CENTER_CENTER,
                &text,
                font.clone(),
                if accent { t.accent } else { Color32::WHITE },
            );
        }
        for row in 0..ROWS {
            let (text, accent) = axis_label(row, false, strip.orientation, &strip.base);
            bold_text(
                p,
                pos2(r.left() + size * 1.2, r.top() + (row as f32 + 0.5) * ch),
                Align2::LEFT_CENTER,
                &text,
                font.clone(),
                if accent { t.accent } else { Color32::WHITE },
            );
        }
        if let Some(h) = self.hovered {
            let cell = Rect::from_min_size(
                pos2(r.left() + h.col as f32 * cw, r.top() + h.row as f32 * ch),
                egui::vec2(cw, ch),
            )
            .shrink(1.0);
            p.rect_stroke(
                cell,
                0.0,
                Stroke::new(3.0, Color32::from_black_alpha(150)),
                StrokeKind::Middle,
            );
            p.rect_stroke(
                cell,
                0.0,
                Stroke::new(1.5, Color32::WHITE),
                StrokeKind::Middle,
            );
            if let Some(c) = test_strip::cell(h.row, h.col, strip.orientation) {
                pill(
                    p,
                    pos2(
                        frame_rect.center().x,
                        frame_rect.bottom().min(clip.bottom()) - 10.0,
                    ),
                    Align2::CENTER_BOTTOM,
                    &TestStripOutput::label(&c),
                    &t,
                );
            }
        }
    }

    fn handle(&mut self, response: &egui::Response, frame_rect: Rect) -> bool {
        let Some(strip) = &self.strip else {
            return false;
        };
        let previewing = strip.preview.as_ref().map(|p| RowCol {
            row: p.row,
            col: p.col,
        });
        let pos = response.hover_pos().or(response.interact_pointer_pos());
        self.hovered = if previewing.is_none() {
            pos.and_then(|p| self.cell_under(p, frame_rect))
        } else {
            None
        };
        let now = response.ctx.input(|i| i.time);
        if response.double_clicked() {
            self.pending_single = None;
            let cell = previewing.or_else(|| {
                response
                    .interact_pointer_pos()
                    .and_then(|p| self.cell_under(p, frame_rect))
            });
            if let Some(c) = cell {
                self.actions.push(StripAction::Confirm {
                    row: c.row,
                    col: c.col,
                });
            }
        } else if response.clicked() && self.pending_single.is_none() {
            let action = if previewing.is_some() {
                Some(StripAction::BackToGrid)
            } else {
                response
                    .interact_pointer_pos()
                    .and_then(|p| self.cell_under(p, frame_rect))
                    .map(|c| StripAction::Preview {
                        row: c.row,
                        col: c.col,
                    })
            };
            if let Some(a) = action {
                self.pending_single = Some((now, a));
            }
        }
        // The strip owns clicks and double clicks (never the fit ↔ 100 % zoom).
        response.clicked() || response.double_clicked()
    }
}
