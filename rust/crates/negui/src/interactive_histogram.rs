//! The Interactive Histogram window (wave 4 / U3) — `InteractiveHistogram.swift`
//! (controls-spec §5): double-click the sidebar histogram → a large R/G/B plot where each
//! channel's tonal axis is reshaped by dragging. A press grabs an existing anchor near its line
//! (within [`GRAB_TOLERANCE`]) or PLANTS a new one whose input is the tone currently DISPLAYED
//! there (the inverse of the remap in effect, so you grab what you SEE); the drag moves its
//! output, clamped between the neighbours' outputs so the map stays monotone; releasing leaves
//! it planted. ≤ [`LEVELS_MAX_POINTS`] anchors per channel; a ✕ under each removes it; per
//! channel and all-channel resets. Drags are bracketed by `set_control_editing` (one history
//! entry on release); the plot re-bins live from every render.

use egui::{Color32, Mesh, Rect, Sense, Shape, Stroke, pos2, vec2};
use negkit::derive::{LEVELS_CLAMP, LEVELS_MAX_POINTS};
use negkit::reference::{Histogram256, levels_inverse_remap};
use negkit::{ExposureSettings, V2};

use crate::theme::Tokens;

/// Minimum normalized gap between neighbouring anchors, both axes.
pub const GAP: f64 = 0.01;
/// Grab tolerance around an existing anchor's line, normalized.
pub const GRAB_TOLERANCE: f64 = 0.015;
/// Plot padding (points) — the Swift Canvas' `.padding(6)`.
pub const PLOT_PAD: f32 = 6.0;

/// The edited channel.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Channel {
    #[default]
    Red,
    Green,
    Blue,
}

impl Channel {
    pub const ALL: [Channel; 3] = [Channel::Red, Channel::Green, Channel::Blue];

    pub fn index(self) -> usize {
        match self {
            Channel::Red => 0,
            Channel::Green => 1,
            Channel::Blue => 2,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Channel::Red => "Red",
            Channel::Green => "Green",
            Channel::Blue => "Blue",
        }
    }

    pub fn color(self) -> Color32 {
        match self {
            Channel::Red => Color32::from_rgb(255, 59, 48),
            Channel::Green => Color32::from_rgb(52, 199, 89),
            Channel::Blue => Color32::from_rgb(10, 132, 255),
        }
    }

    pub fn points(self, s: &ExposureSettings) -> &Vec<V2> {
        match self {
            Channel::Red => &s.levels_red,
            Channel::Green => &s.levels_green,
            Channel::Blue => &s.levels_blue,
        }
    }

    pub fn points_mut(self, s: &mut ExposureSettings) -> &mut Vec<V2> {
        match self {
            Channel::Red => &mut s.levels_red,
            Channel::Green => &mut s.levels_green,
            Channel::Blue => &mut s.levels_blue,
        }
    }
}

/// Window state.
#[derive(Clone, Copy, Debug, Default)]
pub struct LevelsWindow {
    pub open: bool,
    pub channel: Channel,
    /// Index of the anchor being dragged.
    pub drag_index: Option<usize>,
}

/// What the window asks the app to do (settings are edited in place).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LevelsAction {
    DragBegin,
    DragEnd,
    /// Arm a named history label before the in-place edit that follows it.
    Label(String),
}

fn nearest_by_output(pts: &[V2], x: f64) -> Option<usize> {
    pts.iter()
        .enumerate()
        .min_by(|a, b| (a.1[1] - x).abs().total_cmp(&(b.1[1] - x).abs()))
        .map(|(i, _)| i)
}

/// `grabOrPlantAnchor`: the anchor to drag for a press at displayed position `x` — an existing
/// one near its line, the nearest at capacity, else a newly planted anchor (input = the
/// inverse-remapped tone, clamped to the neighbours' gap and the endpoint clamp; output = `x`).
/// Returns the (possibly grown) anchor list and the index; `None` when there's no room.
pub fn grab_or_plant(pts: &[V2], x: f64) -> Option<(Vec<V2>, usize)> {
    if let Some(i) = nearest_by_output(pts, x)
        && pts
            .get(i)
            .is_some_and(|p| (p[1] - x).abs() <= GRAB_TOLERANCE)
    {
        return Some((pts.to_vec(), i));
    }
    if pts.len() >= LEVELS_MAX_POINTS {
        return nearest_by_output(pts, x).map(|i| (pts.to_vec(), i));
    }
    let input = levels_inverse_remap(x, pts);
    let lo = pts
        .iter()
        .rev()
        .find(|p| p[0] < input)
        .map_or(0.0, |p| p[0])
        + GAP;
    let hi = pts.iter().find(|p| p[0] > input).map_or(1.0, |p| p[0]) - GAP;
    if lo >= hi {
        return None;
    }
    let clamped = input
        .max(lo.max(LEVELS_CLAMP))
        .min(hi.min(1.0 - LEVELS_CLAMP));
    let mut next = pts.to_vec();
    let at = next
        .iter()
        .position(|p| p[0] > clamped)
        .unwrap_or(next.len());
    next.insert(at, [clamped, x]);
    Some((next, at))
}

/// `moveAnchor`: anchor `i`'s output → `x`, clamped between the neighbours' outputs (+ gap)
/// and the endpoint clamp. `None` when nothing changes.
pub fn move_anchor(pts: &[V2], i: usize, x: f64) -> Option<Vec<V2>> {
    let cur = pts.get(i)?;
    let lo = if i > 0 {
        pts.get(i - 1).map_or(0.0, |p| p[1])
    } else {
        0.0
    } + GAP;
    let hi = pts.get(i + 1).map_or(1.0, |p| p[1]) - GAP;
    let y = x.max(lo.max(LEVELS_CLAMP)).min(hi.min(1.0 - LEVELS_CLAMP));
    if cur[1] == y {
        return None;
    }
    let mut next = pts.to_vec();
    if let Some(p) = next.get_mut(i) {
        p[1] = y;
    }
    Some(next)
}

/// Show the window (when open). Edits `settings` in place.
pub fn show(
    ctx: &egui::Context,
    state: &mut LevelsWindow,
    settings: &mut ExposureSettings,
    bins: Option<&Histogram256>,
    enabled: bool,
) -> Vec<LevelsAction> {
    let mut actions = Vec::new();
    if !state.open {
        return actions;
    }
    let mut open = true;
    egui::Window::new("Interactive Histogram")
        .id(egui::Id::new("interactive-histogram"))
        .open(&mut open)
        .default_size(vec2(560.0, 360.0))
        .min_size(vec2(520.0, 300.0))
        .resizable(true)
        .collapsible(false)
        .show(ctx, |ui| {
            ui.add_enabled_ui(enabled, |ui| {
                body(ui, state, settings, bins, &mut actions);
            });
        });
    if !open {
        if state.drag_index.take().is_some() {
            actions.push(LevelsAction::DragEnd);
        }
        state.open = false;
    }
    actions
}

fn body(
    ui: &mut egui::Ui,
    state: &mut LevelsWindow,
    settings: &mut ExposureSettings,
    bins: Option<&Histogram256>,
    actions: &mut Vec<LevelsAction>,
) {
    let t = Tokens::get(ui.ctx());
    let ch = state.channel;
    ui.horizontal(|ui| {
        for c in Channel::ALL {
            if ui.selectable_label(state.channel == c, c.name()).clicked()
                && state.drag_index.is_none()
            {
                state.channel = c;
            }
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let any = !settings.levels_red.is_empty()
                || !settings.levels_green.is_empty()
                || !settings.levels_blue.is_empty();
            if ui
                .add_enabled(any, egui::Button::new("Reset All"))
                .clicked()
            {
                actions.push(LevelsAction::Label("Levels reset".into()));
                settings.levels_red.clear();
                settings.levels_green.clear();
                settings.levels_blue.clear();
            }
            if ui
                .add_enabled(
                    !ch.points(settings).is_empty(),
                    egui::Button::new(format!("Reset {}", ch.name())),
                )
                .clicked()
            {
                actions.push(LevelsAction::Label(format!("{} levels reset", ch.name())));
                ch.points_mut(settings).clear();
            }
        });
    });
    ui.add_space(6.0);
    let avail = ui.available_size();
    let plot_h = (avail.y - 18.0 - 40.0).max(160.0);
    let (rect, resp) = ui.allocate_exact_size(vec2(avail.x, plot_h), Sense::click_and_drag());
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 6.0, Color32::from_black_alpha(217));
    let plot = rect.shrink(PLOT_PAD);
    if let Some(bins) = bins {
        for c in Channel::ALL.into_iter().filter(|c| *c != ch) {
            draw_channel(&painter, plot, bins, c, 0.25);
        }
        draw_channel(&painter, plot, bins, ch, 0.9);
    }
    let pts = ch.points(settings).clone();
    for (i, a) in pts.iter().enumerate() {
        let dragged = state.drag_index == Some(i);
        let x = plot.left() + a[1] as f32 * plot.width();
        painter.line_segment(
            [pos2(x, plot.top()), pos2(x, plot.bottom())],
            Stroke::new(
                if dragged { 2.0 } else { 1.2 },
                ch.color().gamma_multiply(if dragged { 1.0 } else { 0.75 }),
            ),
        );
        if dragged {
            let xi = plot.left() + a[0] as f32 * plot.width();
            let shapes = Shape::dashed_line(
                &[pos2(xi, plot.top()), pos2(xi, plot.bottom())],
                Stroke::new(1.0, Color32::from_white_alpha(77)),
                3.0,
                3.0,
            );
            painter.extend(shapes);
        }
    }
    // drag: grab or plant on press, move while held, plant on release
    let to_x =
        |p: egui::Pos2| f64::from((p.x - plot.left()) / plot.width().max(1.0)).clamp(0.0, 1.0);
    if resp.is_pointer_button_down_on()
        && let Some(p) = resp.interact_pointer_pos()
    {
        let x = to_x(p);
        if state.drag_index.is_none()
            && let Some((next, i)) = grab_or_plant(&pts, x)
        {
            actions.push(LevelsAction::DragBegin);
            state.drag_index = Some(i);
            *ch.points_mut(settings) = next;
        }
        if let Some(i) = state.drag_index
            && let Some(next) = move_anchor(ch.points(settings), i, x)
        {
            *ch.points_mut(settings) = next;
        }
    } else if state.drag_index.take().is_some() {
        actions.push(LevelsAction::DragEnd);
    }
    // ✕ removal row
    let (row, _) = ui.allocate_exact_size(vec2(avail.x, 18.0), Sense::hover());
    let mut remove: Option<usize> = None;
    for (i, a) in ch.points(settings).iter().enumerate() {
        let c = pos2(plot.left() + a[1] as f32 * plot.width(), row.center().y);
        let r = Rect::from_center_size(c, vec2(14.0, 14.0));
        let resp = ui
            .interact(r, ui.id().with(("levels-x", i)), Sense::click())
            .on_hover_text("Remove this anchor");
        let col = if resp.hovered() {
            t.text
        } else {
            t.text_secondary
        };
        // A drawn ✕ (the bundled fonts have no U+2715).
        ui.painter().circle_filled(c, 6.0, col.gamma_multiply(0.35));
        let d = 2.5;
        for (a, b) in [(vec2(-d, -d), vec2(d, d)), (vec2(-d, d), vec2(d, -d))] {
            ui.painter()
                .line_segment([c + a, c + b], Stroke::new(1.3, col));
        }
        if resp.clicked() {
            remove = Some(i);
        }
    }
    if let Some(i) = remove
        && state.drag_index.is_none()
    {
        actions.push(LevelsAction::Label(format!("{} anchor removed", ch.name())));
        let pts = ch.points_mut(settings);
        if i < pts.len() {
            pts.remove(i);
        }
    }
    ui.label(
        egui::RichText::new(
            "Drag in the plot to move tones: left of the grab stretches, right compresses. \
             Release to plant an anchor — further drags work between anchors; × removes one.",
        )
        .size(10.5)
        .color(t.text_secondary),
    );
}

/// One channel: linear, peak-normalized filled area (fill @ 0.55·opacity) + outline.
fn draw_channel(p: &egui::Painter, plot: Rect, bins: &Histogram256, ch: Channel, opacity: f32) {
    let off = ch.index() * 256;
    let slice: Vec<u32> = (0..256)
        .map(|i| bins.get(off + i).copied().unwrap_or(0))
        .collect();
    let peak = slice.iter().copied().max().unwrap_or(1).max(1) as f32;
    let pt = |i: usize, v: u32| {
        pos2(
            plot.left() + i as f32 / 255.0 * plot.width(),
            plot.bottom() - plot.height() * (v as f32 / peak),
        )
    };
    let fill = ch.color().gamma_multiply(opacity * 0.55);
    let mut mesh = Mesh::default();
    for (i, v) in slice.iter().enumerate() {
        let top = pt(i, *v);
        let base = mesh.vertices.len() as u32;
        mesh.colored_vertex(top, fill);
        mesh.colored_vertex(pos2(top.x, plot.bottom()), fill);
        if i > 0 {
            mesh.add_triangle(base - 2, base - 1, base);
            mesh.add_triangle(base - 1, base, base + 1);
        }
    }
    p.add(Shape::mesh(mesh));
    let line: Vec<egui::Pos2> = slice.iter().enumerate().map(|(i, v)| pt(i, *v)).collect();
    p.add(Shape::line(
        line,
        Stroke::new(1.0, ch.color().gamma_multiply(opacity)),
    ));
}
