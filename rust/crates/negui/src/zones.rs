//! Darkroom read-outs (wave 4 / U3): the spot densitometer read-out with its 11-cell zone strip
//! (`DensitometerReadout.swift`, controls-spec §10) and the zone-system overlay
//! (`ZoneOverlay.swift`, NegPy ae56f8b) — both over [`negsession::DensitometerState`], which
//! adopts the DISPLAYED rgba8 bitmap once per render (sRGB-encoded here, so it adopts with
//! [`negsession::Encoding::Srgb`]) and is probed per pointer move. The strip, the probe and the
//! overlay read the same bytes through the same ruler, so they can never disagree.

use egui::{Align2, Color32, Rect, Sense, Stroke, StrokeKind, pos2, vec2};
use negsession::densitometer::{Reading, mid_gray_encoded, zone_roman};
use negsession::{DensitometerState, Encoding, RenderOutput, ZoneGrid};

use crate::loupe::{CanvasOverlay, LoupeState, content_rect};
use crate::theme::Tokens;

/// Zone-strip cell size (points) — `ZoneStrip.cellWidth/cellHeight`.
pub const STRIP_CELL: [f32; 2] = [9.0, 10.0];
/// Reserved read-out width so the bar doesn't jump (`DensitometerReadout`: 210 pt).
pub const READOUT_WIDTH: f32 = 210.0;

/// What the control bar's read-out shows.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Readout {
    pub reading: Option<Reading>,
    /// Zone armed for placement (orange cell; text "Zone N → click photo").
    pub armed: Option<f64>,
}

impl Readout {
    /// The read-out text (`DensitometerReadout.text`).
    pub fn text(&self) -> String {
        if let Some(z) = self.armed {
            return format!("Zone {} → click photo", zone_roman(z));
        }
        match &self.reading {
            Some(r) => format!("D {:.2} · Zone {}", r.print_density, zone_roman(r.zone)),
            None => "hover to meter".to_owned(),
        }
    }
}

/// A strip cell's lightness through the ruler's own hinge (`ZoneStrip.shade`): an approximate
/// visual ruler — the number is the value, not the swatch.
pub fn strip_shade(zone: usize) -> f64 {
    let mid = mid_gray_encoded();
    let z = zone as f64;
    if z <= 5.0 {
        z / 5.0 * mid
    } else {
        mid + (z - 5.0) / 5.0 * (1.0 - mid)
    }
}

/// The marked strip cell for a probed zone (`Int(min(max(z, 0), 10).rounded(.down))`).
pub fn marked_cell(zone: f64) -> usize {
    zone.clamp(0.0, 10.0).floor() as usize
}

/// Draw the zone strip + read-out text; returns the zone of a clicked cell.
pub fn readout_ui(ui: &mut egui::Ui, readout: &Readout) -> Option<f64> {
    let t = Tokens::get(ui.ctx());
    let mut clicked = None;
    let marked = readout.reading.map(|r| marked_cell(r.zone));
    let armed = readout.armed.map(|z| z.round().clamp(0.0, 10.0) as usize);
    let idle = marked.is_none() && armed.is_none();
    let strip_w = 11.0 * STRIP_CELL[0] + 10.0;
    let size = vec2(READOUT_WIDTH, STRIP_CELL[1] + 4.0);
    let (rect, _) = ui.allocate_exact_size(size, Sense::hover());
    let strip = Rect::from_min_size(
        pos2(rect.left(), rect.center().y - STRIP_CELL[1] / 2.0),
        vec2(strip_w, STRIP_CELL[1]),
    );
    let p = ui.painter_at(rect.expand(2.0));
    let opacity = if idle { 0.35 } else { 1.0 };
    for z in 0..=10usize {
        let x = strip.left() + z as f32 * (STRIP_CELL[0] + 1.0);
        let cell = Rect::from_min_size(pos2(x, strip.top()), vec2(STRIP_CELL[0], STRIP_CELL[1]));
        let v = (strip_shade(z).clamp(0.0, 1.0) * 255.0).round() as u8;
        p.rect_filled(cell, 0.0, Color32::from_gray(v).gamma_multiply(opacity));
        if armed == Some(z) {
            p.rect_stroke(
                cell,
                0.0,
                Stroke::new(1.5, Color32::from_rgb(255, 149, 0)),
                StrokeKind::Inside,
            );
        } else if marked == Some(z) {
            p.rect_stroke(cell, 0.0, Stroke::new(1.5, t.accent), StrokeKind::Inside);
        }
        let r = ui
            .interact(cell, ui.id().with(("zone-cell", z)), Sense::click())
            .on_hover_text(format!(
                "Zone {} — click, then click the photo to place that tone here",
                zone_roman(z as f64)
            ));
        if r.clicked() {
            clicked = Some(z as f64);
        }
    }
    p.rect_stroke(
        strip.expand(0.5),
        1.0,
        Stroke::new(0.5, t.text_secondary.gamma_multiply(0.35)),
        StrokeKind::Outside,
    );
    p.text(
        pos2(strip.right() + 8.0, rect.center().y),
        Align2::LEFT_CENTER,
        readout.text(),
        t.mono(10.0),
        if readout.reading.is_none() && readout.armed.is_none() {
            t.text_tertiary
        } else {
            t.text_secondary
        },
    );
    clicked
}

/// The densitometer + overlay state the app carries.
#[derive(Debug, Default)]
pub struct ZonesState {
    pub densitometer: DensitometerState,
    /// The overlay grid, recomputed per adopt / toggle while the overlay is on.
    pub grid: Option<ZoneGrid>,
    /// Last grid computation time (ms) — the overlay's cost.
    pub grid_ms: f32,
}

impl ZonesState {
    /// Adopt a render's bytes (moved, never copied) and refresh the grid when the overlay is on.
    pub fn adopt(&mut self, output: RenderOutput, overlay_on: bool) {
        let (w, h) = (output.width, output.height);
        let bitmap = match output.frame {
            negsession::Frame::Rgba8(bytes) => Some((bytes, w, h)),
            _ => None,
        };
        self.densitometer.adopt(bitmap, Encoding::Srgb);
        self.refresh_grid(overlay_on);
    }

    /// Recompute (or drop) the overlay grid.
    pub fn refresh_grid(&mut self, overlay_on: bool) {
        if overlay_on {
            let t0 = std::time::Instant::now();
            self.grid = self.densitometer.zone_grid();
            self.grid_ms = t0.elapsed().as_secs_f32() * 1000.0;
        } else {
            self.grid = None;
        }
    }

    /// Selection change: forget the bitmap.
    pub fn clear(&mut self) {
        self.densitometer.adopt(None, Encoding::Srgb);
        self.grid = None;
    }

    /// Probe at FRAME-normalized `(u, v)` (the canvas' hover), mapped onto the bitmap through
    /// its content window; `None` clears.
    pub fn probe(&mut self, frame_uv: Option<(f32, f32)>, window: &negkit::NormalizedRect) {
        match frame_uv {
            Some((u, v)) => {
                let (bu, bv) = frame_to_bitmap(f64::from(u), f64::from(v), window);
                self.densitometer.probe(bu, bv);
            }
            None => self.densitometer.clear(),
        }
    }
}

/// Frame-normalized → bitmap-normalized through the content window.
pub fn frame_to_bitmap(u: f64, v: f64, w: &negkit::NormalizedRect) -> (f64, f64) {
    let bu = if w.width > 0.0 {
        (u - w.x) / w.width
    } else {
        u
    };
    let bv = if w.height > 0.0 {
        (v - w.y) / w.height
    } else {
        v
    };
    (bu, bv)
}

/// The overlay layer (`ZoneOverlay`): edges between differing neighbours (dark underlay +
/// white line), one bold numeral per merged region, paper black / white in red.
pub struct ZoneOverlayLayer<'a> {
    pub grid: &'a ZoneGrid,
}

/// Region edges of a grid as cell-space segments `((c0, r0), (c1, r1))`.
pub fn region_edges(grid: &ZoneGrid) -> Vec<[(usize, usize); 2]> {
    let mut out = Vec::new();
    for r in 0..grid.rows {
        for c in 0..grid.cols {
            let z = grid.zone(c, r);
            if c + 1 < grid.cols && grid.zone(c + 1, r) != z {
                out.push([(c + 1, r), (c + 1, r + 1)]);
            }
            if r + 1 < grid.rows && grid.zone(c, r + 1) != z {
                out.push([(c, r + 1), (c + 1, r + 1)]);
            }
        }
    }
    out
}

impl CanvasOverlay for ZoneOverlayLayer<'_> {
    fn paint(&mut self, ui: &mut egui::Ui, frame_rect: Rect, zoom_state: &LoupeState) {
        let g = self.grid;
        if g.cols == 0 || g.rows == 0 {
            return;
        }
        // The grid is over the BITMAP: place it at the content window.
        let r = content_rect(frame_rect, &zoom_state.content_window);
        let (cw, ch) = (r.width() / g.cols as f32, r.height() / g.rows as f32);
        let at = |c: usize, row: usize| pos2(r.left() + c as f32 * cw, r.top() + row as f32 * ch);
        let p = ui.painter();
        let edges = region_edges(g);
        for e in &edges {
            p.line_segment(
                [at(e[0].0, e[0].1), at(e[1].0, e[1].1)],
                Stroke::new(2.5, Color32::from_black_alpha(140)),
            );
        }
        for e in &edges {
            p.line_segment(
                [at(e[0].0, e[0].1), at(e[1].0, e[1].1)],
                Stroke::new(1.0, Color32::from_white_alpha(230)),
            );
        }
        let size = (cw.min(ch) * 0.55).clamp(9.0, 22.0);
        let font = egui::FontId::proportional(size);
        for l in &g.labels {
            let c = pos2(
                r.left() + (l.col as f32 + 0.5) * cw,
                r.top() + (l.row as f32 + 0.5) * ch,
            );
            let paper_end = l.zone == 0 || l.zone == 10;
            let text = zone_roman(f64::from(l.zone));
            let color = if paper_end {
                Color32::from_rgb(255, 59, 48)
            } else {
                Color32::WHITE
            };
            bold_text(p, c, Align2::CENTER_CENTER, &text, font.clone(), color);
        }
    }

    fn handle(&mut self, _response: &egui::Response, _frame_rect: Rect) -> bool {
        false
    }
}

/// Bold-looking text with a dark underlay (the Mac's shadow pass): a ring of dark copies, then
/// the text twice offset by a fraction of a point.
pub fn bold_text(
    p: &egui::Painter,
    at: egui::Pos2,
    align: Align2,
    text: &str,
    font: egui::FontId,
    color: Color32,
) {
    let shadow = Color32::from_black_alpha(150);
    for (dx, dy) in [(-1.0, 0.0), (1.0, 0.0), (0.0, -1.0), (0.0, 1.0), (1.0, 1.0)] {
        p.text(at + vec2(dx, dy), align, text, font.clone(), shadow);
    }
    p.text(at, align, text, font.clone(), color);
    p.text(at + vec2(0.4, 0.0), align, text, font, color);
}
