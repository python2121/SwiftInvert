//! The sidebar histogram — `HistogramView.swift` (controls-spec §5). Bins are RAW COUNTS and the
//! HQ tier bins ~15× as many pixels as the proxy, so every bar is a function of the histogram's
//! SHAPE only: peak-normalise, then compress with a FIXED constant
//! (`log1p(f·100000)/log1p(100000)`) — never `log1p(count)/log1p(max)`, which adds
//! `ln(pixelCount)` to both ends. The BP/WP handles ride `blackPointOffset`/`whitePointOffset`.

use egui::{Align2, Color32, Pos2, Rect, Sense, Stroke, StrokeKind, Vec2, pos2, vec2};
use negkit::ExposureSettings;
use negkit::reference::Histogram256;

use crate::theme::Tokens;

/// The fixed log compression constant.
pub const LOG_COMPRESSION: f64 = 100_000.0;
/// Chart height (points).
pub const CHART_HEIGHT: f32 = 128.0;
/// Handle travel as a fraction of the chart width; full travel = 0.3 D.
pub const HANDLE_TRAVEL_FRACTION: f32 = 0.35;
pub const HANDLE_FULL_TRAVEL_D: f64 = 0.3;

/// Bar height 0…1 for `count` against the peak bin (`HistogramView.barHeight`).
pub fn bar_height(count: u32, max_count: u32, log: bool) -> f32 {
    let f = f64::from(count) / f64::from(max_count.max(1));
    let h = if log {
        (f * LOG_COMPRESSION).ln_1p() / LOG_COMPRESSION.ln_1p()
    } else {
        f
    };
    h.clamp(0.0, 1.0) as f32
}

/// Per-channel clip fractions at one end: `bins[c·256 + bin] / Σ bins[0..256]` (the R total).
pub fn clip_fractions(bins: &Histogram256, bin: usize) -> [f64; 3] {
    let total: f64 = bins
        .iter()
        .take(256)
        .map(|&c| f64::from(c))
        .sum::<f64>()
        .max(1.0);
    let at = |c: usize| bins.get(c * 256 + bin).map_or(0.0, |&v| f64::from(v)) / total;
    [at(0), at(1), at(2)]
}

/// Screen-composite of `colors` (each at `alpha` over black): `1 − Π(1 − a·c)`.
fn screen_of(colors: impl Iterator<Item = [f32; 3]>, alpha: f32) -> Color32 {
    let mut acc = [1.0f32; 3];
    for c in colors {
        for k in 0..3 {
            acc[k] *= 1.0 - alpha * c[k];
        }
    }
    let q = |v: f32| ((1.0 - v).clamp(0.0, 1.0) * 255.0 + 0.5) as u8;
    Color32::from_rgb(q(acc[0]), q(acc[1]), q(acc[2]))
}

/// UI state the panel keeps between frames.
#[derive(Clone, Debug, Default)]
pub struct HistogramState {
    /// Footer toggle; default log.
    pub linear: bool,
    drag: Option<HandleDrag>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct HandleDrag {
    white: bool,
    start_offset: f64,
    start_x: f32,
}

/// What the panel did this frame.
#[derive(Clone, Copy, Debug, Default)]
pub struct HistogramOut {
    pub changed: bool,
    pub drag_begin: bool,
    pub drag_end: bool,
    /// The chart was double-clicked (the interactive histogram window — later wave).
    pub double_clicked: bool,
}

/// Draw the histogram panel: chart, clip indicators, hover read-out, BP/WP handles, footer.
pub fn histogram_panel(
    ui: &mut egui::Ui,
    bins: Option<&Histogram256>,
    settings: &mut ExposureSettings,
    state: &mut HistogramState,
) -> HistogramOut {
    let t = Tokens::get(ui.ctx());
    let mut out = HistogramOut::default();
    let width = ui.available_width();
    let (chart, resp) = ui.allocate_exact_size(vec2(width, CHART_HEIGHT), Sense::click());
    let painter = ui.painter_at(chart);
    painter.rect_filled(chart, 6.0, Color32::from_black_alpha(224));
    if resp.double_clicked() {
        out.double_clicked = true;
    }

    // gridlines at quarters
    for q in 1..4 {
        let x = chart.left() + chart.width() * q as f32 / 4.0;
        painter.line_segment(
            [pos2(x, chart.top()), pos2(x, chart.bottom())],
            Stroke::new(1.0, Color32::from_white_alpha(20)),
        );
    }

    let Some(bins) = bins else {
        painter.text(
            chart.center(),
            Align2::CENTER_CENTER,
            "—",
            t.font(14.0),
            t.text_tertiary,
        );
        footer(ui, settings, state, &mut out, &t);
        return out;
    };

    let log = !state.linear;
    let inner = chart.shrink2(vec2(0.0, 4.0));
    let bar_w = inner.width() / 256.0;
    // Screen-blended channel fills (the Mac's `.screen` blend), exact per bin: stack the three
    // bar heights and paint each band with the screen composite of the channels covering it.
    let channel_colors = [[0.85f32, 0.1, 0.1], [0.1, 0.8, 0.15], [0.15, 0.25, 0.95]];
    let maxes: [u32; 3] = std::array::from_fn(|c| {
        bins.iter()
            .skip(c * 256)
            .take(256)
            .copied()
            .max()
            .unwrap_or(0)
    });
    for i in 0..256usize {
        let mut hs: [(f32, usize); 3] = std::array::from_fn(|c| {
            let count = bins.get(c * 256 + i).copied().unwrap_or(0);
            (bar_height(count, maxes[c], log) * inner.height(), c)
        });
        hs.sort_by(|a, b| a.0.total_cmp(&b.0));
        let x0 = inner.left() + bar_w * i as f32;
        let mut y_from = 0.0f32;
        for (k, (h, _)) in hs.iter().enumerate() {
            if *h <= y_from {
                continue;
            }
            // channels covering this band: this one and every taller one
            let color = screen_of(hs.iter().skip(k).map(|(_, c)| channel_colors[*c]), 0.85);
            painter.rect_filled(
                Rect::from_min_max(
                    pos2(x0, inner.bottom() - h),
                    pos2(x0 + bar_w + 0.5, inner.bottom() - y_from),
                ),
                0.0,
                color,
            );
            y_from = *h;
        }
    }
    // luma stroked
    let lmax = bins.iter().skip(768).take(256).copied().max().unwrap_or(0);
    let pts: Vec<Pos2> = (0..256usize)
        .map(|i| {
            let count = bins.get(768 + i).copied().unwrap_or(0);
            let h = bar_height(count, lmax, log) * inner.height();
            pos2(
                inner.left() + inner.width() * i as f32 / 255.0,
                inner.bottom() - h,
            )
        })
        .collect();
    painter.add(egui::Shape::line(
        pts,
        Stroke::new(1.0, Color32::from_white_alpha(190)),
    ));

    // clip indicators
    for (end_bin, left) in [(0usize, true), (255usize, false)] {
        let f = clip_fractions(bins, end_bin);
        if f.iter().any(|&v| v > 0.001) {
            let base_x = if left {
                chart.left() + 6.0
            } else {
                chart.right() - 6.0
            };
            let y = chart.top() + 6.0;
            let base = [
                Color32::from_rgb(230, 60, 60),
                Color32::from_rgb(60, 220, 70),
                Color32::from_rgb(70, 90, 240),
            ];
            for (c, col) in base.iter().enumerate() {
                let alpha = if f.get(c).copied().unwrap_or(0.0) > 0.001 {
                    1.0
                } else {
                    0.25
                };
                let dx = c as f32 * 9.0;
                let x = if left { base_x + dx } else { base_x - dx };
                let tri = if left {
                    vec![pos2(x, y), pos2(x + 8.0, y), pos2(x, y + 8.0)]
                } else {
                    vec![pos2(x, y), pos2(x - 8.0, y), pos2(x, y + 8.0)]
                };
                painter.add(egui::Shape::convex_polygon(
                    tri,
                    col.gamma_multiply(alpha),
                    Stroke::NONE,
                ));
            }
            let tip = format!(
                "{} clipping — R {:.1}%  G {:.1}%  B {:.1}%",
                if left { "Shadow" } else { "Highlight" },
                f[0] * 100.0,
                f[1] * 100.0,
                f[2] * 100.0
            );
            let hit = Rect::from_min_size(
                pos2(
                    if left {
                        chart.left()
                    } else {
                        chart.right() - 34.0
                    },
                    chart.top(),
                ),
                vec2(34.0, 16.0),
            );
            ui.interact(hit, ui.id().with("clip").with(left), Sense::hover())
                .on_hover_text(tip);
        }
    }

    // BP / WP handles
    let travel = chart.width() * HANDLE_TRAVEL_FRACTION;
    let bp = settings.black_point_offset;
    let wp = settings.white_point_offset;
    let black_x = chart.left() + ((-bp / HANDLE_FULL_TRAVEL_D).clamp(0.0, 1.0) as f32) * travel;
    let white_x = chart.right() - ((wp / HANDLE_FULL_TRAVEL_D).clamp(0.0, 1.0) as f32) * travel;
    for (white, x) in [(false, black_x), (true, white_x)] {
        let hit = Rect::from_center_size(pos2(x, chart.center().y), vec2(16.0, chart.height()));
        let id = ui.id().with(if white { "wp" } else { "bp" });
        let hr = ui.interact(hit, id, Sense::click_and_drag());
        let hovered = hr.hovered() || hr.dragged();
        let col = Color32::from_white_alpha(if hovered { 220 } else { 140 });
        painter.line_segment(
            [pos2(x, chart.top() + 8.0), pos2(x, chart.bottom())],
            Stroke::new(1.5, col),
        );
        painter.add(egui::Shape::convex_polygon(
            vec![
                pos2(x - 3.5, chart.top() + 1.0),
                pos2(x + 3.5, chart.top() + 1.0),
                pos2(x, chart.top() + 8.0),
            ],
            col,
            Stroke::NONE,
        ));
        if hr.double_clicked() {
            if white {
                settings.white_point_offset = 0.0;
            } else {
                settings.black_point_offset = 0.0;
            }
            out.changed = true;
            state.drag = None;
        } else if hr.drag_started() {
            if let Some(p) = hr.interact_pointer_pos() {
                state.drag = Some(HandleDrag {
                    white,
                    start_offset: if white { wp } else { bp },
                    start_x: p.x,
                });
                out.drag_begin = true;
            }
        } else if hr.dragged() {
            if let (Some(d), Some(p)) = (state.drag, hr.interact_pointer_pos())
                && d.white == white
            {
                let delta = f64::from((p.x - d.start_x) / travel) * HANDLE_FULL_TRAVEL_D;
                let v = if white {
                    (d.start_offset - delta).clamp(0.0, HANDLE_FULL_TRAVEL_D)
                } else {
                    (d.start_offset - delta).clamp(-HANDLE_FULL_TRAVEL_D, 0.0)
                };
                let slot = if white {
                    &mut settings.white_point_offset
                } else {
                    &mut settings.black_point_offset
                };
                if (*slot - v).abs() > 1e-12 {
                    *slot = v;
                    out.changed = true;
                }
            }
        } else if hr.drag_stopped() && state.drag.is_some_and(|d| d.white == white) {
            state.drag = None;
            out.drag_end = true;
        }
    }

    // hover read-out (only when no handle is being dragged)
    if state.drag.is_none()
        && let Some(p) = resp.hover_pos()
    {
        let bin = (((p.x - chart.left()) / chart.width() * 255.0) as i64).clamp(0, 255) as usize;
        let total: f64 = bins
            .iter()
            .take(256)
            .map(|&c| f64::from(c))
            .sum::<f64>()
            .max(1.0);
        let pct = |c: usize| bins.get(c * 256 + bin).map_or(0.0, |&v| f64::from(v)) / total * 100.0;
        let text = format!(
            "L {bin}  ·  R {:.1}  G {:.1}  B {:.1}",
            pct(0),
            pct(1),
            pct(2)
        );
        let galley = painter.layout_no_wrap(text, t.mono(8.5), Color32::from_white_alpha(230));
        let size = galley.size() + vec2(12.0, 4.0);
        let r = Rect::from_center_size(
            pos2(chart.center().x, chart.top() + 4.0 + size.y / 2.0),
            size,
        );
        painter.rect_filled(r, size.y / 2.0, Color32::from_black_alpha(180));
        painter.galley(r.min + vec2(6.0, 2.0), galley, Color32::WHITE);
    }

    footer(ui, settings, state, &mut out, &t);
    out
}

fn footer(
    ui: &mut egui::Ui,
    settings: &mut ExposureSettings,
    state: &mut HistogramState,
    out: &mut HistogramOut,
    t: &Tokens,
) {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 4.0;
        ui.label(
            egui::RichText::new("Shadows")
                .size(8.5)
                .color(t.text_tertiary),
        );
        if settings.black_point_offset != 0.0 && reset_x(ui, t, "bp-reset") {
            settings.black_point_offset = 0.0;
            out.changed = true;
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.label(
                egui::RichText::new("Highlights")
                    .size(8.5)
                    .color(t.text_tertiary),
            );
            if settings.white_point_offset != 0.0 && reset_x(ui, t, "wp-reset") {
                settings.white_point_offset = 0.0;
                out.changed = true;
            }
            ui.add_space(8.0);
            let label = if state.linear { "lin" } else { "log" };
            if ui
                .add(egui::Button::new(egui::RichText::new(label).size(9.0)).frame(false))
                .on_hover_text("Toggle log / linear scale")
                .clicked()
            {
                state.linear = !state.linear;
            }
        });
    });
}

/// The small ⨯ beside a label; true when clicked.
pub fn reset_x(ui: &mut egui::Ui, t: &Tokens, salt: &str) -> bool {
    let (rect, r) = ui.allocate_exact_size(Vec2::splat(12.0), Sense::click());
    let _ = salt;
    let col = if r.hovered() { t.text } else { t.text_tertiary };
    let p = ui.painter();
    let c = rect.center();
    let d = 3.0;
    p.line_segment([c + vec2(-d, -d), c + vec2(d, d)], Stroke::new(1.2, col));
    p.line_segment([c + vec2(-d, d), c + vec2(d, -d)], Stroke::new(1.2, col));
    if r.hovered() {
        p.rect_stroke(
            rect,
            3.0,
            Stroke::new(1.0, t.button_border),
            StrokeKind::Inside,
        );
    }
    r.on_hover_text("Reset").clicked()
}
