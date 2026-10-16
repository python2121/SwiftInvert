//! Canvas tools — Crop & Straighten and Crop for Analysis (CLAUDE.md §"Unified Crop &
//! Straighten", session-spec §6 "Tool modes" + "Geometry commits"; wave 4 / U2b).
//!
//! The pure half (geometry commits, hit-testing, the clamped corner/move drags) is free
//! functions tested in `tests/tools.rs`; the interactive half is two [`CanvasOverlay`]s the app
//! hands the loupe while a tool is up:
//!
//! - [`CropTool`] takes the canvas over (`CropBoxOverlay.swift`): the engine renders the WHOLE
//!   frame at 0° (`Session::render_tool_preview(ToolMode::Crop)`), the overlay paints it as a
//!   mesh rotated by the live straighten angle about the canvas centre at
//!   [`crop_fit_scale`] (the θ-rotated bounding box fits), and the axis-aligned box over it —
//!   dim surround, thirds, four corner handles resizing about the opposite corner (min
//!   [`MIN_BOX_POINTS`]), interior move; both drags slide along the rotated frame's edges by
//!   bisecting against `box_fits`. The in-canvas bar carries Straighten (±45°, 0.1°), ⨯, Cancel
//!   and Apply. Straightening is display-only until Apply (no re-render per tick).
//! - [`AnalysisTool`] draws a `SelectionOverlay` rect on the uncropped live-angle preview; the
//!   release commits it ([`commit_analysis_region`]).
//!
//! Coordinates follow `negkit::crop_geometry`: a box lives in rotated space (screen-aligned
//! axes, origin at the frame centre, pixels of the orientation-only frame `frame_size`).

use egui::{Color32, CursorIcon, Pos2, Rect, Shape, Stroke, pos2, vec2};
use negkit::NormalizedRect;
use negkit::crop_geometry::{
    RotatedBox, box_fits, box_from_rect, constrain, inscribed_size, normalized_rect, remap_crop,
    rotate,
};
use negkit::{ExposureSettings, V2};
use negsession::history::labels;

use crate::loupe::{CANVAS_INSET, CanvasOverlay, LoupeState, pill};
use crate::theme::Tokens;

/// Minimum box side in screen points while dragging a corner (CropBoxOverlay.swift).
pub const MIN_BOX_POINTS: f64 = 24.0;
/// Straighten range (degrees) and commit precision.
pub const STRAIGHTEN_RANGE: (f64, f64) = (-45.0, 45.0);
pub const STRAIGHTEN_STEP: f64 = 0.1;

/// What `commit_crop` decided.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum CropCommit {
    /// Nothing changed (label disarmed).
    Unchanged,
    /// `crop_rect` set to this rect ("Crop").
    Crop(NormalizedRect),
    /// The box was (nearly) full-frame: crop cleared ("Crop cleared").
    Cleared,
}

impl CropCommit {
    /// The pending history label for this outcome.
    pub fn label(self) -> Option<&'static str> {
        match self {
            CropCommit::Unchanged => None,
            CropCommit::Crop(_) => Some(labels::CROP),
            CropCommit::Cleared => Some(labels::CROP_CLEARED),
        }
    }
}

/// The box shown when the user has not drawn one yet (`CropBoxOverlay.defaultBox`): the
/// committed crop at the angle it was committed, else the full inscribed rect at `radians`.
pub fn default_box(
    committed: Option<&NormalizedRect>,
    committed_radians: f64,
    frame: V2,
    radians: f64,
) -> RotatedBox {
    match committed {
        Some(rect) => box_from_rect(rect, frame, committed_radians),
        None => {
            let ins = inscribed_size(frame, radians);
            RotatedBox {
                center: [0.0, 0.0],
                half_extents: [ins[0] / 2.0, ins[1] / 2.0],
            }
        }
    }
}

/// The desired box constrained to the live angle — what is drawn and what gets committed.
pub fn effective_box(desired: RotatedBox, frame: V2, radians: f64) -> RotatedBox {
    constrain(desired.center, desired.half_extents, radians, frame, 1.0)
}

/// Whether a constrained box is "no crop" (`DetailView.commitCrop`): centre within 0.5 % of
/// the inscribed width and both extents within 1 % of the inscribed rect.
pub fn is_full_frame(b: &RotatedBox, frame: V2, radians: f64) -> bool {
    let ins = inscribed_size(frame, radians);
    let len = (b.center[0] * b.center[0] + b.center[1] * b.center[1]).sqrt();
    len < 0.005 * ins[0]
        && (b.half_extents[0] * 2.0 - ins[0]).abs() < 0.01 * ins[0]
        && (b.half_extents[1] * 2.0 - ins[1]).abs() < 0.01 * ins[1]
}

/// `DetailView.commitCrop`: constrain the desired box at `radians`, convert to a
/// [`NormalizedRect`] over the inscribed rect, treat a near-full box as "no crop". Writes
/// `settings.crop_rect`; the caller arms [`CropCommit::label`] before the write lands in history.
pub fn commit_crop(
    settings: &mut ExposureSettings,
    desired: RotatedBox,
    frame: V2,
    radians: f64,
) -> CropCommit {
    if !(frame[0] > 0.0 && frame[1] > 0.0) {
        return CropCommit::Unchanged;
    }
    let b = effective_box(desired, frame, radians);
    if is_full_frame(&b, frame, radians) {
        if settings.crop_rect.is_some() {
            settings.crop_rect = None;
            return CropCommit::Cleared;
        }
        return CropCommit::Unchanged;
    }
    let rect = normalized_rect(b.center, b.half_extents, frame, radians);
    if settings.crop_rect == Some(rect) {
        return CropCommit::Unchanged;
    }
    settings.crop_rect = Some(rect);
    CropCommit::Crop(rect)
}

/// `AppModel.commitFineRotation`: set the angle (rounded to [`STRAIGHTEN_STEP`], clamped to
/// [`STRAIGHTEN_RANGE`]) and remap a committed crop so its content does not drift with the
/// inscribed auto-crop. Returns whether anything changed.
pub fn commit_fine_rotation(settings: &mut ExposureSettings, degrees: f64, frame: V2) -> bool {
    let degrees = (degrees / STRAIGHTEN_STEP).round() * STRAIGHTEN_STEP;
    let degrees = degrees.clamp(STRAIGHTEN_RANGE.0, STRAIGHTEN_RANGE.1);
    let mut next = settings.clone();
    next.fine_rotation = degrees;
    if let Some(crop) = settings.crop_rect
        && frame[0] > 0.0
        && frame[1] > 0.0
    {
        next.crop_rect = Some(remap_crop(
            &crop,
            settings.fine_rotation.to_radians(),
            degrees.to_radians(),
            frame,
        ));
    }
    if next == *settings {
        return false;
    }
    *settings = next;
    true
}

/// `AppModel.commitSelection` for the analysis tool — the same rule as
/// `Session::commit_analysis_region` (the UI owns the settings mirror, so it is applied here and
/// reaches the session through the next render's `set_settings`): the rect AND the angle it was
/// drawn at. Rects thinner than 1 % are ignored (`SelectionOverlay`'s commit guard).
pub fn commit_analysis_region(settings: &mut ExposureSettings, rect: NormalizedRect) -> bool {
    if !(rect.width > 0.01 && rect.height > 0.01) {
        return false;
    }
    settings.analysis_rect = Some(rect);
    settings.analysis_rect_fine_rotation = settings.fine_rotation;
    true
}

/// Screen scale (points per frame pixel) that fits the θ-rotated bounding box of `frame` into
/// `area` (the crop tool pins the canvas to this fit; DetailView.swift:244-259).
pub fn crop_fit_scale(frame: V2, radians: f64, area: V2) -> f64 {
    let (s, c) = (radians.sin().abs(), radians.cos().abs());
    let bw = frame[0] * c + frame[1] * s;
    let bh = frame[0] * s + frame[1] * c;
    if !(bw > 0.0 && bh > 0.0) {
        return 1.0;
    }
    (area[0] / bw).min(area[1] / bh).max(1e-6)
}

// ───────────────────────────── screen mapping + hit-testing ─────────────────────────────

/// Height reserved under the crop canvas for the in-canvas straighten bar (points).
pub const CROP_BAR_HEIGHT: f32 = 44.0;
/// Corner-handle grab radius (points).
pub const HANDLE_RADIUS: f32 = 14.0;
/// Bisection steps for the clamped drags (2⁻²⁴ of the step — sub-pixel at any zoom).
const BISECT_STEPS: usize = 24;

/// The crop canvas's screen mapping: rotated-space pixel `p` lands at `center + p · scale`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CropView {
    pub center: V2,
    pub scale: f64,
}

impl CropView {
    /// The fit of the θ-rotated frame into `area` minus the bottom bar (the crop tool pins the
    /// canvas to this, whatever the loupe's zoom was).
    pub fn for_area(area: Rect, frame: V2, radians: f64) -> CropView {
        let h = (area.height() - CROP_BAR_HEIGHT).max(1.0);
        let scale = crop_fit_scale(frame, radians, [f64::from(area.width()), f64::from(h)]);
        CropView {
            center: [
                f64::from(area.center().x),
                f64::from(area.top()) + f64::from(h) / 2.0,
            ],
            scale,
        }
    }

    pub fn to_screen(&self, p: V2) -> Pos2 {
        pos2(
            (self.center[0] + p[0] * self.scale) as f32,
            (self.center[1] + p[1] * self.scale) as f32,
        )
    }

    pub fn to_rotated(&self, p: Pos2) -> V2 {
        [
            (f64::from(p.x) - self.center[0]) / self.scale,
            (f64::from(p.y) - self.center[1]) / self.scale,
        ]
    }

    /// A box's screen rect.
    pub fn box_rect(&self, b: &RotatedBox) -> Rect {
        Rect::from_two_pos(
            self.to_screen([
                b.center[0] - b.half_extents[0],
                b.center[1] - b.half_extents[1],
            ]),
            self.to_screen([
                b.center[0] + b.half_extents[0],
                b.center[1] + b.half_extents[1],
            ]),
        )
    }
}

/// Box corners in screen order: top-left, top-right, bottom-right, bottom-left.
pub const CORNER_SIGNS: [V2; 4] = [[-1.0, -1.0], [1.0, -1.0], [1.0, 1.0], [-1.0, 1.0]];

/// What a press on the crop canvas grabbed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CropHit {
    /// A corner handle (index into [`CORNER_SIGNS`]).
    Corner(usize),
    /// The box interior (move).
    Inside,
    None,
}

/// Hit-test a screen point against a box's handles (within `radius`, nearest wins) then its
/// interior.
pub fn hit_test(b: &RotatedBox, view: &CropView, p: Pos2, radius: f32) -> CropHit {
    let mut best: Option<(usize, f32)> = None;
    for (i, s) in CORNER_SIGNS.iter().enumerate() {
        let c = view.to_screen([
            b.center[0] + s[0] * b.half_extents[0],
            b.center[1] + s[1] * b.half_extents[1],
        ]);
        let d = c.distance(p);
        if d <= radius && best.is_none_or(|(_, bd)| d < bd) {
            best = Some((i, d));
        }
    }
    if let Some((i, _)) = best {
        return CropHit::Corner(i);
    }
    if view.box_rect(b).contains(p) {
        CropHit::Inside
    } else {
        CropHit::None
    }
}

/// Move from `start` toward `target` as far as `fits` allows, axis by axis (x then y), so a drag
/// pushed past an edge slides along it. `start` itself is returned when it does not fit.
pub fn slide(start: V2, target: V2, fits: impl Fn(V2) -> bool) -> V2 {
    if fits(target) {
        return target;
    }
    if !fits(start) {
        return start;
    }
    let mut p = start;
    for axis in 0..2 {
        let mut q = p;
        q[axis] = target[axis];
        if fits(q) {
            p = q;
            continue;
        }
        let (mut lo, mut hi) = (0.0f64, 1.0f64);
        for _ in 0..BISECT_STEPS {
            let mid = (lo + hi) / 2.0;
            let mut m = p;
            m[axis] = p[axis] + (target[axis] - p[axis]) * mid;
            if fits(m) {
                lo = mid;
            } else {
                hi = mid;
            }
        }
        p[axis] += (target[axis] - p[axis]) * lo;
    }
    p
}

/// Corner drag: the opposite corner stays put, the dragged corner follows `target` (rotated
/// space) without flipping past the anchor or shrinking under `min_side`, clamped inside the
/// θ-rotated frame.
pub fn drag_corner(
    start: RotatedBox,
    corner: usize,
    target: V2,
    min_side: f64,
    frame: V2,
    radians: f64,
) -> RotatedBox {
    let s = CORNER_SIGNS.get(corner).copied().unwrap_or([1.0, 1.0]);
    let anchor = [
        start.center[0] - s[0] * start.half_extents[0],
        start.center[1] - s[1] * start.half_extents[1],
    ];
    let make = |c: V2| -> RotatedBox {
        let dx = (s[0] * (c[0] - anchor[0])).max(min_side);
        let dy = (s[1] * (c[1] - anchor[1])).max(min_side);
        let corner = [anchor[0] + s[0] * dx, anchor[1] + s[1] * dy];
        RotatedBox {
            center: [(anchor[0] + corner[0]) / 2.0, (anchor[1] + corner[1]) / 2.0],
            half_extents: [dx / 2.0, dy / 2.0],
        }
    };
    let from = [
        start.center[0] + s[0] * start.half_extents[0],
        start.center[1] + s[1] * start.half_extents[1],
    ];
    let fits = |c: V2| {
        let b = make(c);
        box_fits(b.center, b.half_extents, radians, frame, 1e-6)
    };
    make(slide(from, target, fits))
}

/// Interior drag: translate by `delta` (rotated-space pixels), clamped inside the θ-rotated
/// frame (sliding along its edges).
pub fn move_box(start: RotatedBox, delta: V2, frame: V2, radians: f64) -> RotatedBox {
    let target = [start.center[0] + delta[0], start.center[1] + delta[1]];
    let fits = |c: V2| box_fits(c, start.half_extents, radians, frame, 1e-6);
    RotatedBox {
        center: slide(start.center, target, fits),
        half_extents: start.half_extents,
    }
}

/// Round to [`STRAIGHTEN_STEP`] and clamp to [`STRAIGHTEN_RANGE`].
pub fn snap_angle(degrees: f64) -> f64 {
    if !degrees.is_finite() {
        return 0.0;
    }
    ((degrees / STRAIGHTEN_STEP).round() * STRAIGHTEN_STEP)
        .clamp(STRAIGHTEN_RANGE.0, STRAIGHTEN_RANGE.1)
}

// ───────────────────────────── the crop tool ─────────────────────────────

/// What the crop tool asks the app to do.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ToolAction {
    /// Commit the box + angle and leave the tool.
    ApplyCrop,
    /// Restore the entry snapshot and leave the tool.
    CancelCrop,
    /// The analysis rect was drawn.
    CommitAnalysis(NormalizedRect),
}

#[derive(Clone, Copy, Debug)]
struct CropDrag {
    hit: CropHit,
    start: RotatedBox,
    origin: Pos2,
}

/// Crop & Straighten while `ToolMode::Crop` is up.
#[derive(Clone, Debug)]
pub struct CropTool {
    /// `(fine_rotation, crop_rect)` at entry, for Cancel/Escape.
    pub snapshot: (f64, Option<NormalizedRect>),
    /// The live straighten angle (degrees, display-only until Apply).
    pub angle: f64,
    /// The box the user drew (`None` = follow the committed crop).
    pub desired: Option<RotatedBox>,
    /// The committed crop/angle the default box follows (re-synced from the settings each frame).
    pub committed: (f64, Option<NormalizedRect>),
    /// Orientation-only proxy frame dims of the last layout.
    pub frame: V2,
    drag: Option<CropDrag>,
    action: Option<ToolAction>,
}

impl CropTool {
    /// Enter the tool over `settings` (snapshot for Cancel).
    pub fn new(settings: &ExposureSettings, frame: V2) -> Self {
        let committed = (settings.fine_rotation, settings.crop_rect);
        CropTool {
            snapshot: committed,
            angle: settings.fine_rotation,
            desired: None,
            committed,
            frame,
            drag: None,
            action: None,
        }
    }

    /// Follow the settings (a sidebar ⨯ while the tool is up) and the rendered frame dims.
    pub fn sync(&mut self, settings: &ExposureSettings, frame: V2) {
        self.committed = (settings.fine_rotation, settings.crop_rect);
        self.sync_frame(frame);
    }

    fn sync_frame(&mut self, frame: V2) {
        if frame != self.frame && frame[0] > 0.0 && frame[1] > 0.0 {
            // A rotation/flip changed the frame: a drawn box no longer means anything.
            self.desired = None;
            self.frame = frame;
        }
    }

    /// The box drawn now (desired or default, constrained at the live angle).
    pub fn current_box(&self) -> RotatedBox {
        let radians = self.angle.to_radians();
        let desired = self.desired.unwrap_or_else(|| {
            default_box(
                self.committed.1.as_ref(),
                self.committed.0.to_radians(),
                self.frame,
                radians,
            )
        });
        effective_box(desired, self.frame, radians)
    }

    /// Apply (`DetailView.commitCrop` + `commitFineRotation`): a drawn box commits at the
    /// snapped angle; with no box drawn only the angle moves (a committed crop is remapped so
    /// its content stays put). Returns the history label to arm, if any.
    pub fn apply(&self, settings: &mut ExposureSettings) -> Option<&'static str> {
        let degrees = snap_angle(self.angle);
        match self.desired {
            Some(b) => {
                let commit = commit_crop(settings, b, self.frame, degrees.to_radians());
                settings.fine_rotation = degrees;
                commit.label()
            }
            None => {
                commit_fine_rotation(settings, degrees, self.frame);
                None
            }
        }
    }

    /// Cancel: restore the entry snapshot. Returns whether anything changed.
    pub fn cancel(&self, settings: &mut ExposureSettings) -> bool {
        let (angle, crop) = self.snapshot;
        let changed = settings.fine_rotation != angle || settings.crop_rect != crop;
        settings.fine_rotation = angle;
        settings.crop_rect = crop;
        changed
    }

    /// The screen mapping over the canvas `area` (the inset fit minus the bar).
    pub fn layout(&self, area: Rect) -> CropView {
        CropView::for_area(
            area.shrink(CANVAS_INSET),
            self.frame,
            self.angle.to_radians(),
        )
    }

    fn paint_bar(&mut self, ui: &mut egui::Ui, canvas: Rect, t: &Tokens) {
        let area = canvas.shrink(CANVAS_INSET);
        let w = area.width().min(620.0);
        let bar = Rect::from_min_size(
            pos2(
                area.center().x - w / 2.0,
                area.bottom() - CROP_BAR_HEIGHT + 6.0,
            ),
            vec2(w, CROP_BAR_HEIGHT - 8.0),
        );
        ui.painter()
            .rect_filled(bar, 8.0, Color32::from_black_alpha(200));
        let mut child = ui.new_child(
            egui::UiBuilder::new()
                .max_rect(bar.shrink2(vec2(10.0, 4.0)))
                .layout(egui::Layout::left_to_right(egui::Align::Center)),
        );
        child.spacing_mut().item_spacing.x = 8.0;
        child.label(
            egui::RichText::new("Straighten")
                .size(11.0)
                .color(Color32::WHITE),
        );
        let buttons_w = 150.0;
        child.spacing_mut().slider_width = (child.available_width() - buttons_w - 90.0).max(60.0);
        let mut angle = self.angle;
        child.add(
            egui::Slider::new(&mut angle, STRAIGHTEN_RANGE.0..=STRAIGHTEN_RANGE.1)
                .step_by(STRAIGHTEN_STEP)
                .fixed_decimals(1)
                .suffix("°"),
        );
        if angle.is_finite() && (angle - self.angle).abs() > 1e-12 {
            self.angle = snap_angle(angle);
        }
        if self.angle.abs() > 1e-9 && crate::histogram::reset_x(&mut child, t, "straighten-x") {
            self.angle = 0.0;
        }
        child.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let apply = egui::Button::new(
                egui::RichText::new("Apply")
                    .size(11.0)
                    .color(Color32::WHITE),
            )
            .fill(t.accent);
            if ui.add(apply).on_hover_text("Commit (Return)").clicked() {
                self.action = Some(ToolAction::ApplyCrop);
            }
            if ui
                .add(egui::Button::new(egui::RichText::new("Cancel").size(11.0)))
                .on_hover_text("Discard (Esc)")
                .clicked()
            {
                self.action = Some(ToolAction::CancelCrop);
            }
        });
    }
}

impl CanvasOverlay for CropTool {
    fn takes_over(&self) -> bool {
        true
    }

    fn handle(&mut self, response: &egui::Response, _frame_rect: Rect) -> bool {
        let cv = self.layout(response.rect);
        let current = self.current_box();
        let min_side = MIN_BOX_POINTS / cv.scale.max(1e-9);
        let radians = self.angle.to_radians();
        if let Some(p) = response.hover_pos()
            && self.drag.is_none()
        {
            match hit_test(&current, &cv, p, HANDLE_RADIUS) {
                CropHit::Corner(0 | 2) => response.ctx.set_cursor_icon(CursorIcon::ResizeNwSe),
                CropHit::Corner(_) => response.ctx.set_cursor_icon(CursorIcon::ResizeNeSw),
                CropHit::Inside => response.ctx.set_cursor_icon(CursorIcon::Grab),
                CropHit::None => {}
            }
        }
        if response.drag_started() {
            let origin = response
                .ctx
                .input(|i| i.pointer.press_origin())
                .or(response.interact_pointer_pos());
            if let Some(origin) = origin {
                let hit = hit_test(&current, &cv, origin, HANDLE_RADIUS);
                self.drag = (hit != CropHit::None).then_some(CropDrag {
                    hit,
                    start: current,
                    origin,
                });
            }
        }
        if let Some(d) = self.drag
            && response.dragged()
            && let Some(p) = response.interact_pointer_pos()
        {
            let next = match d.hit {
                CropHit::Corner(k) => {
                    drag_corner(d.start, k, cv.to_rotated(p), min_side, self.frame, radians)
                }
                CropHit::Inside => {
                    response.ctx.set_cursor_icon(CursorIcon::Grabbing);
                    let delta = p - d.origin;
                    move_box(
                        d.start,
                        [f64::from(delta.x) / cv.scale, f64::from(delta.y) / cv.scale],
                        self.frame,
                        radians,
                    )
                }
                CropHit::None => d.start,
            };
            self.desired = Some(next);
        }
        if response.drag_stopped() {
            self.drag = None;
        }
        true
    }

    fn paint(&mut self, ui: &mut egui::Ui, _frame_rect: Rect, zoom_state: &LoupeState) {
        let t = Tokens::get(ui.ctx());
        let area = ui.max_rect();
        let (fw, fh) = zoom_state.frame_size;
        self.sync_frame([fw as f64, fh as f64]);
        let Some(presented) = zoom_state.frame else {
            return;
        };
        let cv = self.layout(area);
        let radians = self.angle.to_radians();
        let painter = ui.painter_at(area);
        // the frame, rotated about the canvas centre
        let (hw, hh) = (self.frame[0] / 2.0, self.frame[1] / 2.0);
        let mut mesh = egui::Mesh::with_texture(presented.texture);
        for (c, uv) in [
            ([-hw, -hh], pos2(0.0, 0.0)),
            ([hw, -hh], pos2(1.0, 0.0)),
            ([hw, hh], pos2(1.0, 1.0)),
            ([-hw, hh], pos2(0.0, 1.0)),
        ] {
            mesh.vertices.push(egui::epaint::Vertex {
                pos: cv.to_screen(rotate(c, radians)),
                uv,
                color: Color32::WHITE,
            });
        }
        mesh.add_triangle(0, 1, 2);
        mesh.add_triangle(0, 2, 3);
        painter.add(Shape::mesh(mesh));

        // dim surround, box, thirds, handles
        let b = cv.box_rect(&self.current_box());
        let dim = Color32::from_black_alpha(150);
        let a = area;
        for r in [
            Rect::from_min_max(a.min, pos2(a.right(), b.top())),
            Rect::from_min_max(pos2(a.left(), b.bottom()), a.max),
            Rect::from_min_max(pos2(a.left(), b.top()), pos2(b.left(), b.bottom())),
            Rect::from_min_max(pos2(b.right(), b.top()), pos2(a.right(), b.bottom())),
        ] {
            if r.is_positive() {
                painter.rect_filled(r, 0.0, dim);
            }
        }
        let thin = Stroke::new(1.0, Color32::from_white_alpha(110));
        for f in [1.0 / 3.0, 2.0 / 3.0] {
            let x = b.left() + b.width() * f;
            let y = b.top() + b.height() * f;
            painter.line_segment([pos2(x, b.top()), pos2(x, b.bottom())], thin);
            painter.line_segment([pos2(b.left(), y), pos2(b.right(), y)], thin);
        }
        painter.rect_stroke(
            b,
            0.0,
            Stroke::new(1.0, Color32::WHITE),
            egui::StrokeKind::Middle,
        );
        let len = 14.0f32.min(b.width() / 2.0).min(b.height() / 2.0);
        let heavy = Stroke::new(3.0, Color32::WHITE);
        for (corner, (dx, dy)) in [
            (b.left_top(), (1.0, 1.0)),
            (b.right_top(), (-1.0, 1.0)),
            (b.right_bottom(), (-1.0, -1.0)),
            (b.left_bottom(), (1.0, -1.0)),
        ] {
            painter.line_segment([corner, corner + vec2(dx * len, 0.0)], heavy);
            painter.line_segment([corner, corner + vec2(0.0, dy * len)], heavy);
        }
        pill(
            &painter,
            pos2(a.center().x, a.top() + 10.0),
            egui::Align2::CENTER_TOP,
            "Crop & Straighten — drag the corners or the box · Return applies · Esc cancels",
            &t,
        );
        self.paint_bar(ui, area, &t);
    }
}

// ───────────────────────────── the analysis tool ─────────────────────────────

/// Crop for Analysis while `ToolMode::AnalysisRegion` is up (`SelectionOverlay`).
#[derive(Clone, Debug, Default)]
pub struct AnalysisTool {
    /// The committed region when it was drawn at the current angle (shown until a new drag).
    pub existing: Option<NormalizedRect>,
    drag: Option<((f64, f64), (f64, f64))>,
    action: Option<ToolAction>,
}

/// A screen point normalized over `frame_rect`, clamped to `[0, 1]`.
pub fn normalize_in(frame_rect: Rect, p: Pos2) -> (f64, f64) {
    let w = f64::from(frame_rect.width()).max(1e-9);
    let h = f64::from(frame_rect.height()).max(1e-9);
    (
        ((f64::from(p.x) - f64::from(frame_rect.left())) / w).clamp(0.0, 1.0),
        ((f64::from(p.y) - f64::from(frame_rect.top())) / h).clamp(0.0, 1.0),
    )
}

/// A normalized rect over `frame_rect` → screen.
pub fn rect_on(frame_rect: Rect, r: &NormalizedRect) -> Rect {
    Rect::from_min_size(
        pos2(
            frame_rect.left() + (r.x as f32) * frame_rect.width(),
            frame_rect.top() + (r.y as f32) * frame_rect.height(),
        ),
        vec2(
            (r.width as f32) * frame_rect.width(),
            (r.height as f32) * frame_rect.height(),
        ),
    )
}

impl AnalysisTool {
    /// Enter over `settings`: the committed region shows when it was drawn at this angle.
    pub fn new(settings: &ExposureSettings) -> Self {
        AnalysisTool {
            existing: settings.analysis_rect.filter(|_| {
                (settings.analysis_rect_fine_rotation - settings.fine_rotation).abs() < 1e-9
            }),
            drag: None,
            action: None,
        }
    }

    /// The rect being drawn, if any.
    pub fn drawing(&self) -> Option<NormalizedRect> {
        self.drag.map(|(a, b)| NormalizedRect::from_corners(a, b))
    }
}

impl CanvasOverlay for AnalysisTool {
    fn handle(&mut self, response: &egui::Response, frame_rect: Rect) -> bool {
        if response.hover_pos().is_some_and(|p| frame_rect.contains(p)) {
            response.ctx.set_cursor_icon(CursorIcon::Crosshair);
        }
        if response.drag_started() {
            let origin = response
                .ctx
                .input(|i| i.pointer.press_origin())
                .or(response.interact_pointer_pos());
            if let Some(o) = origin.filter(|o| frame_rect.contains(*o)) {
                let n = normalize_in(frame_rect, o);
                self.drag = Some((n, n));
            }
        }
        let active = self.drag.is_some();
        if let Some((a, _)) = self.drag
            && response.dragged()
            && let Some(p) = response.interact_pointer_pos()
        {
            self.drag = Some((a, normalize_in(frame_rect, p)));
        }
        if response.drag_stopped()
            && let Some(rect) = self.drawing()
        {
            self.drag = None;
            if rect.width > 0.01 && rect.height > 0.01 {
                self.existing = Some(rect);
                self.action = Some(ToolAction::CommitAnalysis(rect));
            }
        }
        active
    }

    fn paint(&mut self, ui: &mut egui::Ui, frame_rect: Rect, _zoom_state: &LoupeState) {
        let t = Tokens::get(ui.ctx());
        let area = ui.max_rect();
        let painter = ui.painter_at(area);
        if let Some(r) = self.drawing().or(self.existing) {
            let fr = frame_rect;
            let s = rect_on(fr, &r);
            let dim = Color32::from_black_alpha(120);
            for d in [
                Rect::from_min_max(fr.min, pos2(fr.right(), s.top())),
                Rect::from_min_max(pos2(fr.left(), s.bottom()), fr.max),
                Rect::from_min_max(pos2(fr.left(), s.top()), pos2(s.left(), s.bottom())),
                Rect::from_min_max(pos2(s.right(), s.top()), pos2(fr.right(), s.bottom())),
            ] {
                if d.is_positive() {
                    painter.rect_filled(d, 0.0, dim);
                }
            }
            painter.rect_stroke(s, 0.0, Stroke::new(1.5, t.accent), egui::StrokeKind::Middle);
        }
        pill(
            &painter,
            pos2(area.center().x, area.top() + 10.0),
            egui::Align2::CENTER_TOP,
            "Crop for Analysis — drag on the image to select the area · Esc leaves",
            &t,
        );
    }
}

// ───────────────────────────── the app's tool state ─────────────────────────────

/// The interactive tool state the app carries (`ToolMode` stays the app's; this is what the
/// overlays need while a tool is up).
#[derive(Debug, Default)]
pub struct ToolState {
    pub crop: Option<CropTool>,
    pub analysis: Option<AnalysisTool>,
}

impl ToolState {
    /// Any tool up?
    pub fn active(&self) -> bool {
        self.crop.is_some() || self.analysis.is_some()
    }

    /// Drop both tools.
    pub fn clear(&mut self) {
        self.crop = None;
        self.analysis = None;
    }

    /// The overlays to hand the loupe this frame.
    pub fn overlays(&mut self) -> Vec<&mut dyn CanvasOverlay> {
        let mut v: Vec<&mut dyn CanvasOverlay> = Vec::new();
        if let Some(c) = &mut self.crop {
            v.push(c);
        }
        if let Some(a) = &mut self.analysis {
            v.push(a);
        }
        v
    }

    /// What the overlays asked for since the last call.
    pub fn take_actions(&mut self) -> Vec<ToolAction> {
        let mut v = Vec::new();
        if let Some(a) = self.crop.as_mut().and_then(|c| c.action.take()) {
            v.push(a);
        }
        if let Some(a) = self.analysis.as_mut().and_then(|t| t.action.take()) {
            v.push(a);
        }
        v
    }
}

// ───────────────────────────── selftest passes ─────────────────────────────

/// The tools/dialogs passes of `--selftest` (driven from `crate::selftest`): crop mode capture
/// (`<base>_crop.png`), the applied crop (`<base>_cropped.png`, then undone), an analysis-rect
/// drag (`<base>_analysis.png`), the dialogs (`<base>_export.png`, `<base>_profiles.png`), a
/// JPEG + TIFF export into a folder verified for existence and ICC tags, and the HQ-on
/// full-tier adjust→frame latency. Sidecars are never written (autosave is off there).
pub mod selftest {
    use std::path::{Path, PathBuf};
    use std::time::{Duration, Instant};

    use egui_kittest::Harness;
    use negkit::crop_geometry::RotatedBox;
    use negsession::{ExportDestination, ExportFormat, HqMode, RenderTier, ToolMode};

    use crate::app::{Command, NegApp};
    use crate::selftest::sibling;

    /// Which passes to run.
    #[derive(Clone, Debug, Default)]
    pub struct Passes {
        pub crop: bool,
        pub export_dir: Option<PathBuf>,
        pub dialogs: bool,
        pub latency: bool,
    }

    fn settle(h: &mut Harness<'_, NegApp>, n: usize) {
        for _ in 0..n {
            h.step();
            std::thread::sleep(Duration::from_millis(16));
        }
    }

    /// Step until `done` or `timeout`.
    fn wait(
        h: &mut Harness<'_, NegApp>,
        timeout: Duration,
        mut done: impl FnMut(&NegApp) -> bool,
    ) -> Result<Duration, String> {
        let t0 = Instant::now();
        while t0.elapsed() < timeout {
            h.step();
            if done(h.state()) {
                return Ok(t0.elapsed());
            }
            if let Some(e) = h.state().selftest_failure() {
                return Err(e);
            }
            std::thread::sleep(Duration::from_millis(2));
        }
        let a = h.state();
        Err(format!(
            "selftest: timed out after {timeout:?} (frames {}, busy {}, tool {:?}, status {:?})",
            a.frames_rendered(),
            a.busy(),
            a.tool_mode(),
            a.status_message()
        ))
    }

    fn capture(h: &mut Harness<'_, NegApp>, out: &Path, suffix: &str) -> Result<PathBuf, String> {
        let image = h.render()?;
        let path = sibling(out, suffix);
        image
            .save(&path)
            .map_err(|e| format!("{}: {e}", path.display()))?;
        log::info!("selftest: wrote {}", path.display());
        Ok(path)
    }

    fn next_frame(h: &mut Harness<'_, NegApp>) -> Result<Duration, String> {
        let before = h.state().frames_rendered();
        wait(h, Duration::from_secs(60), |a| {
            a.frames_rendered() > before && !a.busy()
        })
    }

    /// Run the requested passes.
    pub fn run(h: &mut Harness<'_, NegApp>, out: &Path, p: &Passes) -> Result<(), String> {
        if p.crop {
            crop_pass(h, out)?;
            analysis_pass(h, out)?;
        }
        if p.dialogs {
            h.state_mut().run(Command::Export);
            settle(h, 6);
            capture(h, out, "_export")?;
            h.state_mut().run(Command::Escape);
            h.state_mut().run(Command::ChooseDefaults);
            settle(h, 6);
            capture(h, out, "_profiles")?;
            h.state_mut().run(Command::Escape);
            settle(h, 2);
        }
        if let Some(dir) = &p.export_dir {
            export_pass(h, dir)?;
        }
        if p.latency {
            latency_pass(h)?;
        }
        Ok(())
    }

    fn crop_pass(h: &mut Harness<'_, NegApp>, out: &Path) -> Result<(), String> {
        let before = h.state().settings().clone();
        h.state_mut().run(Command::ToggleCrop);
        if h.state().tool_mode() != ToolMode::Crop {
            return Err("selftest: crop mode did not engage".into());
        }
        next_frame(h)?;
        if let Some(tool) = h.state_mut().crop_tool_mut() {
            tool.angle = 4.0;
            let f = tool.frame;
            tool.desired = Some(RotatedBox {
                center: [f[0] * 0.05, -f[1] * 0.03],
                half_extents: [f[0] * 0.33, f[1] * 0.3],
            });
        }
        settle(h, 6);
        capture(h, out, "_crop")?;
        // Apply → the committed, straightened crop renders; then undo it.
        h.state_mut().run(Command::ApplyTool);
        next_frame(h)?;
        settle(h, 4);
        let s = h.state().settings().clone();
        if s.crop_rect.is_none() || (s.fine_rotation - 4.0).abs() > 1e-9 {
            return Err(format!(
                "selftest: crop apply did not commit (crop {:?}, angle {})",
                s.crop_rect, s.fine_rotation
            ));
        }
        capture(h, out, "_cropped")?;
        h.state_mut().run(Command::Undo);
        next_frame(h)?;
        settle(h, 2);
        if h.state().settings() != &before {
            return Err("selftest: undo did not restore the pre-crop settings".into());
        }
        // Enter + cancel leaves the settings untouched (latest-wins may fold its two renders).
        let frames = h.state().frames_rendered();
        h.state_mut().run(Command::ToggleCrop);
        settle(h, 2);
        h.state_mut().run(Command::Escape);
        if h.state().settings() != &before || h.state().tool_mode() != ToolMode::None {
            return Err("selftest: crop cancel changed the settings".into());
        }
        wait(h, Duration::from_secs(60), |a| {
            a.frames_rendered() > frames && !a.busy()
        })
        .map(|_| ())
    }

    fn analysis_pass(h: &mut Harness<'_, NegApp>, out: &Path) -> Result<(), String> {
        h.state_mut().run(Command::ToggleAnalysis);
        next_frame(h)?;
        settle(h, 2);
        // Drag a rect across the middle of the canvas (library 320 pt, sidebar ~290 pt).
        let (a, b) = (egui::pos2(560.0, 300.0), egui::pos2(860.0, 560.0));
        let button = |pos, pressed| egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        };
        h.event(egui::Event::PointerMoved(a));
        h.step();
        h.event(button(a, true));
        h.step();
        for i in 1..=8 {
            let t = i as f32 / 8.0;
            h.event(egui::Event::PointerMoved(a + (b - a) * t));
            h.step();
        }
        capture(h, out, "_analysis")?;
        h.event(button(b, false));
        settle(h, 3);
        h.remove_cursor();
        let r = h.state().settings().analysis_rect;
        if r.is_none() || h.state().tool_mode() != ToolMode::None {
            return Err(format!("selftest: analysis drag did not commit ({r:?})"));
        }
        log::info!("selftest: analysis region {r:?}");
        h.state_mut().run(Command::Undo);
        next_frame(h).map(|_| ())
    }

    fn export_pass(h: &mut Harness<'_, NegApp>, dir: &Path) -> Result<(), String> {
        std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        let Some(sel) = h.state().selection().map(Path::to_path_buf) else {
            return Err("selftest: nothing selected to export".into());
        };
        for format in [ExportFormat::Jpeg, ExportFormat::Tiff16] {
            let mut options = crate::dialogs::export::default_export_options()
                .with_destination(ExportDestination::Folder(dir.to_path_buf()));
            options.format = format;
            let dest = options.destination_path(&sel);
            let t0 = Instant::now();
            if !h.state_mut().start_export_for_selection(options) {
                return Err("selftest: export refused".into());
            }
            wait(h, Duration::from_secs(180), |a| !a.exporting())?;
            let status = h.state().status_message().unwrap_or_default().to_owned();
            let icc = icc_profile_len(&dest, format)?;
            log::info!(
                "selftest: exported {} in {:.2} s ({status}); ICC profile {icc} bytes",
                dest.display(),
                t0.elapsed().as_secs_f32()
            );
            if icc == 0 {
                return Err(format!(
                    "selftest: {} carries no ICC profile",
                    dest.display()
                ));
            }
        }
        Ok(())
    }

    /// The embedded ICC profile's size (0 = none); errors when the file is missing/unreadable.
    pub fn icc_profile_len(path: &Path, format: ExportFormat) -> Result<usize, String> {
        use image::ImageDecoder;
        let file = std::fs::File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
        let reader = std::io::BufReader::new(file);
        let icc = match format {
            ExportFormat::Jpeg => {
                image::codecs::jpeg::JpegDecoder::new(reader).and_then(|mut d| d.icc_profile())
            }
            ExportFormat::Tiff16 => {
                image::codecs::tiff::TiffDecoder::new(reader).and_then(|mut d| d.icc_profile())
            }
        }
        .map_err(|e| format!("{}: {e}", path.display()))?;
        Ok(icc.map_or(0, |v| v.len()))
    }

    fn nudge_latency(h: &mut Harness<'_, NegApp>) -> Result<Duration, String> {
        let before = h.state().frames_rendered();
        let t0 = Instant::now();
        h.state_mut().bench_nudge();
        wait(h, Duration::from_secs(20), |a| a.frames_rendered() > before)?;
        Ok(t0.elapsed())
    }

    fn latency_pass(h: &mut Harness<'_, NegApp>) -> Result<(), String> {
        let mut proxy = Vec::new();
        for _ in 0..5 {
            proxy.push(nudge_latency(h)?.as_secs_f32() * 1000.0);
        }
        h.state_mut().run(Command::SetHq(HqMode::On));
        let decode = wait(h, Duration::from_secs(60), |a| {
            a.rendered_tier() == RenderTier::Full
        })?;
        let mut full = Vec::new();
        for _ in 0..5 {
            full.push(nudge_latency(h)?.as_secs_f32() * 1000.0);
        }
        // The zero-copy full tier still feeds the densitometer (its proxy probe frame).
        if let Some(r) = h.state().canvas_frame_rect() {
            h.hover_at(r.center());
            settle(h, 4);
            let readout = h.state().densitometer_readout();
            h.remove_cursor();
            if readout.reading.is_none() {
                return Err("selftest: no densitometer reading on the full tier".into());
            }
            log::info!(
                "selftest: full-tier densitometer read-out \"{}\"",
                readout.text()
            );
        }
        let med = |v: &mut Vec<f32>| {
            v.sort_by(f32::total_cmp);
            v.get(v.len() / 2).copied().unwrap_or(0.0)
        };
        log::info!(
            "selftest: latency adjust→frame (median of 5): proxy {:.1} ms, HQ-on full tier {:.1} ms (full decode {:.2} s); presenter {}",
            med(&mut proxy),
            med(&mut full),
            decode.as_secs_f32(),
            std::env::var(crate::presenter_gpu::PRESENTER_ENV).unwrap_or_else(|_| "auto".into())
        );
        h.state_mut().run(Command::SetHq(HqMode::Auto));
        Ok(())
    }
}
