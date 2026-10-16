//! The canvas — `DetailView.swift` (controls-spec §10, §11) with the Qt gestures (§15 item 6):
//! fit-to-window laid out from `displayAspect` (never a pixel count), the bitmap placed at its
//! `contentWindow` inside that ideal rect (the tier-swap stability rule — place, never stretch),
//! wheel zoom anchored at the cursor, drag pan, double-click fit ↔ 100 %, pinch, the three
//! canvas colours, the under-canvas control bar and the overlays.
//!
//! **Frame source seam.** The loupe paints an `egui::TextureId`; where that texture comes from is
//! a [`FramePresenter`]. [`CpuPresenter`] takes the RGBA8 readback frame and uploads it
//! (`TextureHandle::set`, one handle reused). A GPU presenter would hand the render chain's
//! output texture to `egui_wgpu::Renderer::register_native_texture` and return that id — the app
//! layer owns it (negui never sees wgpu); the loupe code is identical for both.

use egui::{Align2, Color32, Pos2, Rect, Sense, Stroke, StrokeKind, Vec2, pos2, vec2};
use negkit::NormalizedRect;
use negsession::{HqMode, RenderOutput, RenderTier, ToolMode};

use crate::controls::GridLineType;
use crate::theme::Tokens;

/// Canvas inset (points).
pub const CANVAS_INSET: f32 = 12.0;
/// Zoom range (1 = fit).
pub const ZOOM_MIN: f32 = 1.0;
pub const ZOOM_MAX: f32 = 8.0;
/// Wheel zoom per notch (Qt: 1.0015^120).
pub const WHEEL_NOTCH_FACTOR: f32 = 1.197;

/// The three canvas colours (`AppModel.CanvasColor`); default very dark gray.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum CanvasColor {
    Gray,
    #[default]
    VeryDarkGray,
    Black,
}

impl CanvasColor {
    pub const ALL: [CanvasColor; 3] = [
        CanvasColor::Gray,
        CanvasColor::VeryDarkGray,
        CanvasColor::Black,
    ];

    pub fn color(self) -> Color32 {
        match self {
            CanvasColor::Gray => Color32::from_gray(128),
            CanvasColor::VeryDarkGray => Color32::from_gray(31),
            CanvasColor::Black => Color32::BLACK,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            CanvasColor::Gray => "Gray",
            CanvasColor::VeryDarkGray => "Very dark gray",
            CanvasColor::Black => "Black",
        }
    }

    /// The persisted raw string (UserDefaults `canvasColor`).
    pub fn key(self) -> &'static str {
        match self {
            CanvasColor::Gray => "gray",
            CanvasColor::VeryDarkGray => "veryDarkGray",
            CanvasColor::Black => "black",
        }
    }
}

/// What the loupe paints.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PresentedFrame {
    pub texture: egui::TextureId,
    /// Bitmap size in pixels.
    pub size: [usize; 2],
}

/// Turns a render output into a texture the loupe can paint. See the module docs.
pub trait FramePresenter {
    /// Present `output`; `None` when it carried nothing presentable.
    fn present(&mut self, ctx: &egui::Context, output: &RenderOutput) -> Option<PresentedFrame>;
    /// Present `output` together with the GPU texture that carries its pixels (the zero-copy
    /// path; `gpu` is `None` for readback frames). The default ignores the texture — dropping
    /// it frees its ring slot.
    fn present_frame(
        &mut self,
        ctx: &egui::Context,
        output: &RenderOutput,
        gpu: Option<crate::presenter_gpu::GpuFrame>,
    ) -> Option<PresentedFrame> {
        drop(gpu);
        self.present(ctx, output)
    }
    /// Drop any held texture (selection cleared).
    fn clear(&mut self) {}
}

/// The CPU/readback path: RGBA8 → `ColorImage` → one reused `TextureHandle`.
#[derive(Default)]
pub struct CpuPresenter {
    handle: Option<egui::TextureHandle>,
}

impl FramePresenter for CpuPresenter {
    fn present(&mut self, ctx: &egui::Context, output: &RenderOutput) -> Option<PresentedFrame> {
        let rgba = output.frame.rgba8()?;
        let size = [output.width, output.height];
        if rgba.len() != size[0] * size[1] * 4 {
            return None;
        }
        let image = egui::ColorImage::from_rgba_unmultiplied(size, rgba);
        match &mut self.handle {
            Some(h) => h.set(image, egui::TextureOptions::LINEAR),
            None => {
                self.handle = Some(ctx.load_texture("loupe", image, egui::TextureOptions::LINEAR))
            }
        }
        self.handle.as_ref().map(|h| PresentedFrame {
            texture: h.id(),
            size,
        })
    }

    fn clear(&mut self) {
        self.handle = None;
    }
}

/// Canvas view state.
#[derive(Clone, Debug)]
pub struct LoupeState {
    /// 1 = fit-to-window.
    pub zoom: f32,
    pub pan: Vec2,
    pub frame: Option<PresentedFrame>,
    pub display_aspect: f64,
    pub content_window: NormalizedRect,
    pub frame_size: (usize, usize),
    pub tier: RenderTier,
    /// The ideal frame rect of the last layout (screen points) — the densitometer's base.
    pub last_frame_rect: Option<Rect>,
}

impl Default for LoupeState {
    fn default() -> Self {
        Self {
            zoom: 1.0,
            pan: Vec2::ZERO,
            frame: None,
            display_aspect: 1.5,
            content_window: NormalizedRect {
                x: 0.0,
                y: 0.0,
                width: 1.0,
                height: 1.0,
            },
            frame_size: (0, 0),
            tier: RenderTier::Proxy,
            last_frame_rect: None,
        }
    }
}

impl LoupeState {
    /// Adopt a render output's geometry (the presented texture is set by the caller).
    pub fn adopt(&mut self, output: &RenderOutput, frame: Option<PresentedFrame>) {
        if output.display_aspect.is_finite() && output.display_aspect > 0.0 {
            self.display_aspect = output.display_aspect;
        }
        self.content_window = output.content_window;
        self.frame_size = output.frame_size;
        self.tier = output.tier;
        if frame.is_some() {
            self.frame = frame;
        }
    }

    /// `resetZoom()`: fit, no pan.
    pub fn reset_zoom(&mut self) {
        self.zoom = 1.0;
        self.pan = Vec2::ZERO;
    }

    /// Clear the picture (selection change).
    pub fn clear(&mut self) {
        self.frame = None;
        self.last_frame_rect = None;
    }
}

/// Aspect-fit `aspect` (w/h) inside `area`.
pub fn fit_rect(area: Rect, aspect: f32) -> Rect {
    let aspect = if aspect.is_finite() && aspect > 0.0 {
        aspect
    } else {
        1.5
    };
    let (aw, ah) = (area.width().max(1.0), area.height().max(1.0));
    let (w, h) = if aw / ah > aspect {
        (ah * aspect, ah)
    } else {
        (aw, aw / aspect)
    };
    Rect::from_center_size(area.center(), vec2(w, h))
}

/// Where the bitmap sits inside the ideal frame rect.
pub fn content_rect(frame: Rect, window: &NormalizedRect) -> Rect {
    let (w, h) = (frame.width(), frame.height());
    Rect::from_min_size(
        pos2(
            frame.left() + window.x as f32 * w,
            frame.top() + window.y as f32 * h,
        ),
        vec2(
            (window.width as f32 * w).max(1.0),
            (window.height as f32 * h).max(1.0),
        ),
    )
}

/// Pan clamped so the frame never leaves the area empty (Qt: pan clamped to the overflow).
pub fn clamp_pan(pan: Vec2, frame_size: Vec2, area: Rect) -> Vec2 {
    let over = ((frame_size - area.size()) / 2.0).max(Vec2::ZERO);
    vec2(pan.x.clamp(-over.x, over.x), pan.y.clamp(-over.y, over.y))
}

// --- U3: darkroom (the `CanvasOverlay` slot — the same shape the tools wave ships) ---
/// A canvas layer drawn over the bitmap (zone overlay, test strip, zone placement; the tools'
/// crop box). `frame_rect` is the ideal frame rect in screen points (the bitmap sits at its
/// `content_window` inside it).
pub trait CanvasOverlay {
    /// Paint over the bitmap (the ui is clipped to the canvas area).
    fn paint(&mut self, ui: &mut egui::Ui, frame_rect: Rect, zoom_state: &LoupeState);
    /// Pointer input on the canvas response (with the PREVIOUS frame's `frame_rect`); true =
    /// consumed, so the canvas skips its own drag-pan and double-click zoom.
    fn handle(&mut self, response: &egui::Response, frame_rect: Rect) -> bool;
    // --- U2: tools & dialogs ---
    /// Take the canvas over: zoom/pan pinned to fit, the loupe's gestures off and its own bitmap
    /// paint skipped (the crop tool paints the frame itself, rotated behind the box).
    fn takes_over(&self) -> bool {
        false
    }
    // --- end U2 ---
}
// --- end U3 ---

/// What the canvas needs to draw besides the loupe state.
pub struct CanvasInput<'a> {
    pub has_selection: bool,
    pub developing: bool,
    pub analyzing: bool,
    pub baseline: bool,
    pub error: Option<&'a str>,
    pub show_grid: bool,
    pub grid: GridLineType,
    pub canvas_color: CanvasColor,
    pub tool_mode: ToolMode,
    pub frame_label: Option<&'a str>,
}

/// Canvas feedback.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum CanvasAction {
    /// The zoom changed (drives HQ auto).
    ZoomChanged(f32),
    /// Pointer over the image at content-normalized (u, v); `None` when it left.
    Hover(Option<(f32, f32)>),
}

/// Draw the canvas into `area`.
pub fn canvas(
    ui: &mut egui::Ui,
    area: Rect,
    state: &mut LoupeState,
    input: &CanvasInput<'_>,
    overlays: &mut [&mut dyn CanvasOverlay],
) -> Vec<CanvasAction> {
    let t = Tokens::get(ui.ctx());
    let mut actions = Vec::new();
    let painter = ui.painter_at(area);
    painter.rect_filled(area, 0.0, input.canvas_color.color());
    let resp = ui.interact(area, ui.id().with("canvas"), Sense::click_and_drag());

    if !input.has_selection {
        painter.text(
            area.center(),
            Align2::CENTER_CENTER,
            "No image selected",
            t.font(14.0),
            t.text_tertiary,
        );
        return actions;
    }
    let Some(frame) = state.frame else {
        let text = if let Some(e) = input.error {
            e
        } else if input.developing {
            "Developing…"
        } else {
            ""
        };
        if !text.is_empty() {
            painter.text(
                area.center(),
                Align2::CENTER_CENTER,
                text,
                t.font(14.0),
                t.text_secondary,
            );
        }
        if input.analyzing {
            pill(
                &painter,
                pos2(area.left() + 12.0, area.bottom() - 12.0),
                Align2::LEFT_BOTTOM,
                "Analyzing…",
                &t,
            );
        }
        return actions;
    };

    // layout
    let fit = fit_rect(area.shrink(CANVAS_INSET), state.display_aspect as f32);
    let mut zoom = state.zoom.clamp(ZOOM_MIN, ZOOM_MAX);
    let mut pan = state.pan;
    let hover = resp.hover_pos();
    // --- U3: darkroom --- overlays see the pointer first.
    let mut consumed = false;
    if let Some(prev) = state.last_frame_rect {
        for o in overlays.iter_mut() {
            consumed |= o.handle(&resp, prev);
        }
    }
    // --- end U3 ---

    // --- U2: tools & dialogs --- a take-over (the crop tool) pins the fit.
    let takeover = overlays.iter().any(|o| o.takes_over());
    if takeover {
        zoom = 1.0;
        pan = Vec2::ZERO;
    }
    // --- end U2 ---

    // gestures
    if resp.hovered() && !takeover {
        let mut factor = 1.0f32;
        let mut anchor = hover.unwrap_or(area.center());
        let (pinch, trackpad_pan) = ui.input(|i| (i.zoom_delta(), i.smooth_scroll_delta));
        let mut used_wheel = false;
        ui.input(|i| {
            for e in &i.events {
                if let egui::Event::MouseWheel {
                    unit,
                    delta,
                    modifiers,
                    ..
                } = e
                {
                    if modifiers.command || modifiers.ctrl {
                        continue; // egui turns these into zoom_delta
                    }
                    match unit {
                        egui::MouseWheelUnit::Line | egui::MouseWheelUnit::Page => {
                            factor *= WHEEL_NOTCH_FACTOR.powf(delta.y);
                            used_wheel = true;
                        }
                        egui::MouseWheelUnit::Point => {}
                    }
                }
            }
            if let Some(mt) = i.multi_touch() {
                anchor = mt.center_pos;
            }
        });
        if (pinch - 1.0).abs() > 1e-4 {
            factor *= pinch;
        }
        if (factor - 1.0).abs() > 1e-4 {
            let old_size = fit.size() * zoom;
            let old_min = area.center() + pan - old_size / 2.0;
            let new_zoom = (zoom * factor).clamp(ZOOM_MIN, ZOOM_MAX);
            let new_size = fit.size() * new_zoom;
            let rel = (anchor - old_min) / old_size;
            let new_min = anchor - vec2(rel.x * new_size.x, rel.y * new_size.y);
            pan = new_min + new_size / 2.0 - area.center();
            zoom = new_zoom;
        } else if !used_wheel
            && zoom > 1.0
            && trackpad_pan != Vec2::ZERO
            && input.tool_mode == ToolMode::None
        {
            pan += trackpad_pan;
        }
        ui.input_mut(|i| i.smooth_scroll_delta = Vec2::ZERO);
    }
    if resp.dragged() && zoom > 1.0 && !takeover && !consumed {
        pan += resp.drag_delta();
    }
    if resp.double_clicked() && !takeover && !consumed {
        if zoom > 1.001 {
            zoom = 1.0;
            pan = Vec2::ZERO;
        } else {
            let bitmap_w = frame.size[0] as f32;
            let ppp = ui.ctx().pixels_per_point();
            let one_to_one = (bitmap_w / (fit.width().max(1.0) * ppp)).clamp(ZOOM_MIN, ZOOM_MAX);
            if one_to_one > 1.001 {
                let anchor = hover.unwrap_or(area.center());
                let old_size = fit.size();
                let old_min = fit.min;
                let new_size = old_size * one_to_one;
                let rel = (anchor - old_min) / old_size;
                let new_min = anchor - vec2(rel.x * new_size.x, rel.y * new_size.y);
                pan = new_min + new_size / 2.0 - area.center();
                zoom = one_to_one;
            }
        }
    }
    if zoom <= 1.0 {
        pan = Vec2::ZERO;
    }
    let frame_rect = Rect::from_center_size(area.center() + pan, fit.size() * zoom);
    pan = clamp_pan(pan, frame_rect.size(), area);
    let frame_rect = Rect::from_center_size(area.center() + pan, fit.size() * zoom);
    if (zoom - state.zoom).abs() > 1e-6 {
        actions.push(CanvasAction::ZoomChanged(zoom));
    }
    state.zoom = zoom;
    state.pan = pan;
    state.last_frame_rect = Some(frame_rect);

    // paint the bitmap at its content window
    let bitmap_rect = content_rect(frame_rect, &state.content_window);
    if !takeover {
        painter.image(
            frame.texture,
            bitmap_rect,
            Rect::from_min_max(Pos2::ZERO, pos2(1.0, 1.0)),
            Color32::WHITE,
        );
        if input.show_grid {
            grid_lines(&painter, frame_rect, input.grid);
        }
    }
    // --- U3: darkroom ---
    if !overlays.is_empty() {
        let mut oui = ui.new_child(egui::UiBuilder::new().max_rect(area));
        oui.set_clip_rect(area.intersect(ui.clip_rect()));
        for o in overlays.iter_mut() {
            o.paint(&mut oui, frame_rect, state);
        }
    }
    // --- end U3 ---

    // hover → content-normalized point (plain presentation only)
    let hover_uv = if input.tool_mode == ToolMode::None && !input.baseline {
        hover.filter(|p| frame_rect.contains(*p)).map(|p| {
            (
                (p.x - frame_rect.left()) / frame_rect.width(),
                (p.y - frame_rect.top()) / frame_rect.height(),
            )
        })
    } else {
        None
    };
    actions.push(CanvasAction::Hover(hover_uv));

    // overlays
    if input.baseline {
        pill(
            &painter,
            pos2(area.center().x, area.top() + 12.0),
            Align2::CENTER_TOP,
            "Original conversion",
            &t,
        );
    }
    if input.analyzing {
        pill(
            &painter,
            pos2(area.left() + 12.0, area.bottom() - 12.0),
            Align2::LEFT_BOTTOM,
            "Analyzing…",
            &t,
        );
    }
    if let Some(e) = input.error {
        pill(
            &painter,
            pos2(area.center().x, area.bottom() - 12.0),
            Align2::CENTER_BOTTOM,
            e,
            &t,
        );
    }
    if let Some(label) = input.frame_label {
        painter.text(
            pos2(area.right() - 10.0, area.top() + 8.0),
            Align2::RIGHT_TOP,
            label,
            t.font(10.0),
            Color32::from_white_alpha(120),
        );
    }
    actions
}

/// A translucent capsule label.
pub fn pill(painter: &egui::Painter, anchor: Pos2, align: Align2, text: &str, t: &Tokens) {
    let galley = painter.layout_no_wrap(text.to_owned(), t.font(11.0), Color32::WHITE);
    let size = galley.size() + vec2(16.0, 8.0);
    let rect = align.anchor_size(anchor, size);
    painter.rect_filled(rect, size.y / 2.0, Color32::from_black_alpha(170));
    painter.galley(rect.min + vec2(8.0, 4.0), galley, Color32::WHITE);
}

/// The grid overlay (CropRotationSection.swift:162-201).
pub fn grid_lines(painter: &egui::Painter, r: Rect, kind: GridLineType) {
    let mut segments: Vec<[Pos2; 2]> = Vec::new();
    let v = |x: f32| {
        [
            pos2(r.left() + r.width() * x, r.top()),
            pos2(r.left() + r.width() * x, r.bottom()),
        ]
    };
    let h = |y: f32| {
        [
            pos2(r.left(), r.top() + r.height() * y),
            pos2(r.right(), r.top() + r.height() * y),
        ]
    };
    match kind {
        GridLineType::Thirds => {
            for f in [1.0 / 3.0, 2.0 / 3.0] {
                segments.push(v(f));
                segments.push(h(f));
            }
        }
        GridLineType::Phi => {
            let phi = 0.381_966_01;
            for f in [phi, 1.0 - phi] {
                segments.push(v(f));
                segments.push(h(f));
            }
        }
        GridLineType::Grid => {
            for i in 1..8 {
                let f = i as f32 / 8.0;
                segments.push(v(f));
                segments.push(h(f));
            }
        }
        GridLineType::Diagonals => {
            segments.push([r.left_top(), r.right_bottom()]);
            segments.push([r.right_top(), r.left_bottom()]);
        }
    }
    for s in &segments {
        painter.line_segment(*s, Stroke::new(2.5, Color32::from_black_alpha(64)));
    }
    for s in &segments {
        painter.line_segment(*s, Stroke::new(1.0, Color32::from_white_alpha(140)));
    }
}

/// Control-bar feedback.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum BarAction {
    RotateCcw,
    RotateCw,
    Flip,
    CycleHq,
    ToggleZones,
    CanvasColor(CanvasColor),
    // --- U3: darkroom ---
    /// A zone-strip cell was clicked: arm (or disarm) that zone for placement.
    ArmZone(f64),
    // --- end U3 ---
}

/// Control-bar inputs.
pub struct BarInput<'a> {
    pub has_selection: bool,
    pub hq_mode: HqMode,
    pub hq_active: bool,
    pub zones_on: bool,
    pub canvas_color: CanvasColor,
    // --- U3: darkroom ---
    /// The spot densitometer read-out + zone strip.
    pub readout: &'a crate::zones::Readout,
    // --- end U3 ---
}

/// The under-canvas control bar (DetailView.swift:102-191).
pub fn control_bar(ui: &mut egui::Ui, input: &BarInput<'_>) -> Vec<BarAction> {
    let t = Tokens::get(ui.ctx());
    let mut actions = Vec::new();
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 8.0;
        ui.add_space(4.0);
        ui.add_enabled_ui(input.has_selection, |ui| {
            if icon_button(ui, "rotate-ccw", 24.0, false, |p, r, c| {
                rotate_icon(p, r, c, false)
            })
            .on_hover_text("Rotate left (⌘[)")
            .clicked()
            {
                actions.push(BarAction::RotateCcw);
            }
            if icon_button(ui, "rotate-cw", 24.0, false, |p, r, c| {
                rotate_icon(p, r, c, true)
            })
            .on_hover_text("Rotate right (⌘])")
            .clicked()
            {
                actions.push(BarAction::RotateCw);
            }
            if icon_button(ui, "flip", 24.0, false, flip_icon)
                .on_hover_text("Flip horizontal (⇧⌘H)")
                .clicked()
            {
                actions.push(BarAction::Flip);
            }
            ui.add_space(4.0);
            if hq_badge(ui, input.hq_mode, &t)
                .on_hover_text(match input.hq_mode {
                    HqMode::Off => "HQ preview off — the 1536 px proxy (click: auto)",
                    HqMode::Auto => "HQ preview auto — sharper tier once magnified 2× (click: on)",
                    HqMode::On => "HQ preview on — full resolution, export truth (click: off)",
                })
                .clicked()
            {
                actions.push(BarAction::CycleHq);
            }
            let zones = badge(ui, "Zones", input.zones_on, true, &t);
            if zones
                .on_hover_text(
                    "Zone system overlay (⇧Z): Adams zones over the preview, same ruler as the densitometer",
                )
                .clicked()
            {
                actions.push(BarAction::ToggleZones);
            }
        });
        // centre: densitometer slot
        let remaining = ui.available_width();
        let right_w = 110.0;
        let centre_w = (remaining - right_w).max(0.0);
        ui.allocate_ui_with_layout(
            vec2(centre_w, 24.0),
            egui::Layout::centered_and_justified(egui::Direction::LeftToRight),
            |ui| {
                if input.has_selection
                    && let Some(z) = crate::zones::readout_ui(ui, input.readout)
                {
                    actions.push(BarAction::ArmZone(z));
                }
            },
        );
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.add_space(6.0);
            for c in CanvasColor::ALL.iter().rev() {
                let (rect, r) = ui.allocate_exact_size(Vec2::splat(18.0), Sense::click());
                let selected = *c == input.canvas_color;
                let p = ui.painter();
                p.circle_filled(rect.center(), 8.0, c.color());
                p.circle_stroke(
                    rect.center(),
                    8.0,
                    if selected {
                        Stroke::new(2.0, t.accent)
                    } else {
                        Stroke::new(1.0, t.text_secondary.gamma_multiply(0.4))
                    },
                );
                if r.on_hover_text(c.label()).clicked() {
                    actions.push(BarAction::CanvasColor(*c));
                }
            }
            ui.label(
                egui::RichText::new("Canvas")
                    .size(10.5)
                    .color(t.text_secondary),
            );
        });
    });
    actions
}

/// HQ badge with the spec's fill/stroke/text per state.
fn hq_badge(ui: &mut egui::Ui, mode: HqMode, t: &Tokens) -> egui::Response {
    let (fill, stroke, text) = match mode {
        HqMode::Off => (
            t.text_alpha(0.06),
            t.text_secondary.gamma_multiply(0.4),
            t.text,
        ),
        HqMode::Auto => (t.accent_alpha(0.18), t.accent_alpha(0.55), t.text),
        HqMode::On => (t.accent_alpha(0.85), t.accent, Color32::WHITE),
    };
    let label = match mode {
        HqMode::Off => "HQ",
        HqMode::Auto => "HQ auto",
        HqMode::On => "HQ on",
    };
    chip(ui, label, fill, stroke, text, t)
}

fn badge(ui: &mut egui::Ui, label: &str, on: bool, enabled: bool, t: &Tokens) -> egui::Response {
    let (fill, stroke) = if on {
        (t.accent_alpha(0.3), t.accent)
    } else {
        (t.text_alpha(0.06), t.text_secondary.gamma_multiply(0.4))
    };
    ui.add_enabled_ui(enabled, |ui| chip(ui, label, fill, stroke, t.text, t))
        .inner
}

fn chip(
    ui: &mut egui::Ui,
    label: &str,
    fill: Color32,
    stroke: Color32,
    text: Color32,
    t: &Tokens,
) -> egui::Response {
    let galley = ui
        .painter()
        .layout_no_wrap(label.to_owned(), t.font(10.5), text);
    let size = galley.size() + vec2(14.0, 6.0);
    let (rect, resp) = ui.allocate_exact_size(size, Sense::click());
    let enabled = ui.is_enabled();
    let alpha = if enabled { 1.0 } else { 0.4 };
    let p = ui.painter();
    p.rect_filled(rect, size.y / 2.0, fill.gamma_multiply(alpha));
    p.rect_stroke(
        rect,
        size.y / 2.0,
        Stroke::new(1.0, stroke.gamma_multiply(alpha)),
        StrokeKind::Inside,
    );
    p.galley(
        rect.min + vec2(7.0, 3.0),
        galley,
        text.gamma_multiply(alpha),
    );
    resp
}

/// A square icon button drawn in code.
pub fn icon_button(
    ui: &mut egui::Ui,
    id: &str,
    size: f32,
    active: bool,
    paint: impl FnOnce(&egui::Painter, Rect, Color32),
) -> egui::Response {
    let t = Tokens::get(ui.ctx());
    let style = craft_ui::buttons::IconButtonStyle {
        normal: craft_ui::buttons::ButtonVisuals {
            fill: Color32::TRANSPARENT,
            stroke: Stroke::NONE,
            icon: if ui.is_enabled() {
                t.text_secondary
            } else {
                t.text_disabled
            },
        },
        hovered: craft_ui::buttons::ButtonVisuals {
            fill: t.hover,
            stroke: Stroke::NONE,
            icon: t.text,
        },
        pressed: Some(craft_ui::buttons::ButtonVisuals {
            fill: t.pressed,
            stroke: Stroke::NONE,
            icon: t.text,
        }),
        selected: craft_ui::buttons::ButtonVisuals {
            fill: t.accent_alpha(0.35),
            stroke: Stroke::new(1.0, t.accent),
            icon: t.text,
        },
        corner_radius: egui::CornerRadius::same(4),
        focus_stroke: Stroke::new(1.0, t.accent),
    };
    ui.push_id(id, |ui| {
        craft_ui::buttons::IconButton::new(id, Vec2::splat(size), &style)
            .selected(active)
            .show(ui, |ui, rect, color| paint(ui.painter(), rect, color))
    })
    .inner
}

/// ⟲ / ⟳: a three-quarter arc with an arrow head.
pub fn rotate_icon(p: &egui::Painter, rect: Rect, color: Color32, clockwise: bool) {
    let c = rect.center();
    let r = rect.width() * 0.3;
    let n = 24;
    let (start, end) = (0.25 * std::f32::consts::TAU, 1.0 * std::f32::consts::TAU);
    let pts: Vec<Pos2> = (0..=n)
        .map(|i| {
            let a = start + (end - start) * i as f32 / n as f32;
            let a = if clockwise {
                a
            } else {
                std::f32::consts::PI - a
            };
            c + vec2(a.cos() * r, a.sin() * r)
        })
        .collect();
    let tip = pts.last().copied().unwrap_or(c);
    p.add(egui::Shape::line(pts, Stroke::new(1.5, color)));
    let d = if clockwise { 1.0 } else { -1.0 };
    p.add(egui::Shape::convex_polygon(
        vec![
            tip + vec2(-3.0 * d, -3.5),
            tip + vec2(3.5 * d, 0.0),
            tip + vec2(-3.0 * d, 3.5),
        ],
        color,
        Stroke::NONE,
    ));
}

/// Flip horizontal: two mirrored triangles about a dashed axis.
pub fn flip_icon(p: &egui::Painter, rect: Rect, color: Color32) {
    let c = rect.center();
    let h = rect.height() * 0.28;
    let w = rect.width() * 0.3;
    p.add(egui::Shape::convex_polygon(
        vec![c + vec2(-2.0, -h), c + vec2(-2.0, h), c + vec2(-w, h)],
        color,
        Stroke::NONE,
    ));
    p.add(egui::Shape::closed_line(
        vec![c + vec2(2.0, -h), c + vec2(2.0, h), c + vec2(w, h)],
        Stroke::new(1.2, color),
    ));
    p.line_segment(
        [c + vec2(0.0, -h - 2.0), c + vec2(0.0, h + 2.0)],
        Stroke::new(1.0, color.gamma_multiply(0.6)),
    );
}
