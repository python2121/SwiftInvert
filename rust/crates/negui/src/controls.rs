//! The adjustments sidebar — `ControlsSidebar.swift`, `LabeledSlider`, `GradientSlider`,
//! `ColorMixerSection`, `ColorGradingSection`, `NegativeCharacterRow`, `CropRotationSection`,
//! `HistoryPanel` (controls-spec §1–§4, §6, §7), rendered from [`crate::control_spec`].
//!
//! Widget contract (§2): caption · reset-⨯ · value over the slider; `isChanged` (|v − default|
//! above 1e-9) shows the ⨯ and brightens the value; double-click resets; drags report begin/end so
//! history commits once on release; the wheel NEVER changes a slider — the rows sense only
//! click/drag, so wheel events reach the enclosing scroll area untouched.

use egui::{Align2, Color32, Rect, Sense, Stroke, StrokeKind, Vec2, pos2, vec2};
use negkit::ExposureSettings;
use negkit::reference::Histogram256;
use negsession::{History, ToolMode};

use crate::control_spec::{
    BandTag, ControlSpec, GradingBand, MixerBand, Section, TOGGLES, ToggleSpec, Track, rows_for,
};
use crate::histogram::{HistogramState, histogram_panel, reset_x};
use crate::theme::{Tokens, gray_f, rgb_f};

/// Grid-line overlay styles (`gridLineType`, default thirds).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum GridLineType {
    #[default]
    Thirds,
    Phi,
    Grid,
    Diagonals,
}

impl GridLineType {
    pub const ALL: [GridLineType; 4] = [
        GridLineType::Thirds,
        GridLineType::Phi,
        GridLineType::Grid,
        GridLineType::Diagonals,
    ];

    pub fn label(self) -> &'static str {
        match self {
            GridLineType::Thirds => "Rule of thirds",
            GridLineType::Phi => "Phi (golden ratio)",
            GridLineType::Grid => "Grid",
            GridLineType::Diagonals => "Diagonals",
        }
    }

    /// The persisted string (`@AppStorage("gridLineType")`).
    pub fn key(self) -> &'static str {
        match self {
            GridLineType::Thirds => "thirds",
            GridLineType::Phi => "phi",
            GridLineType::Grid => "grid",
            GridLineType::Diagonals => "diagonals",
        }
    }

    pub fn from_key(key: &str) -> GridLineType {
        Self::ALL
            .into_iter()
            .find(|g| g.key() == key)
            .unwrap_or_default()
    }
}

/// Persisted sidebar state (`@AppStorage` keys in controls-spec §13) plus the band pickers.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct SidebarPrefs {
    pub adjustments_collapsed: bool,
    pub crop_rotation_collapsed: bool,
    pub history_collapsed: bool,
    pub history_height: f32,
    pub show_grid_lines: bool,
    pub grid_line_type: GridLineType,
}

impl Default for SidebarPrefs {
    fn default() -> Self {
        Self {
            adjustments_collapsed: false,
            crop_rotation_collapsed: false,
            history_collapsed: false,
            history_height: 150.0,
            show_grid_lines: false,
            grid_line_type: GridLineType::Thirds,
        }
    }
}

/// Session-only sidebar state.
#[derive(Clone, Debug, Default)]
pub struct SidebarState {
    pub prefs: SidebarPrefs,
    pub mixer_band: MixerBand,
    pub grading_band: GradingBand,
    pub histogram: HistogramState,
}

/// What the sidebar needs to draw.
pub struct SidebarInput<'a> {
    pub has_selection: bool,
    pub bins: Option<&'a Histogram256>,
    /// `RenderOutput.densityRange` (0 until the first render).
    pub density_range: f64,
    pub character: Option<&'static str>,
    pub status_message: Option<&'a str>,
    pub history: Option<&'a History>,
    pub tool_mode: ToolMode,
    pub baseline_held: bool,
    pub can_undo: bool,
    pub can_redo: bool,
    pub is_exporting: bool,
    // --- U3: darkroom ---
    /// A test strip is showing or building (the button's active style).
    pub test_strip_active: bool,
    // --- end U3 ---
}

/// Things the sidebar asked the app to do (settings edits are applied in place).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum SidebarAction {
    /// Settings changed in place (slider/toggle/handle); not a drag frame necessarily.
    SettingsEdited,
    DragBegin,
    DragEnd,
    ResetAll,
    StartFromScratch,
    /// Pressed state of the View Original hold button.
    ViewOriginal(bool),
    ToggleAnalysisTool,
    ClearAnalysisRect,
    ToggleCropTool,
    ClearCropAndStraighten,
    TestStrip,
    Export,
    Undo,
    Redo,
    JumpHistory(usize),
    /// Sidebar prefs changed (persist).
    PrefsChanged,
    // --- U3: darkroom ---
    /// The histogram chart was double-clicked: open the Interactive Histogram window.
    OpenInteractiveHistogram,
    // --- end U3 ---
}

/// Slider result.
#[derive(Clone, Copy, Debug, Default)]
pub struct SliderOut {
    /// New DISPLAY value.
    pub value: Option<f64>,
    pub drag_begin: bool,
    pub drag_end: bool,
}

const ROW_GAP: f32 = 10.0;
const KNOB: f32 = 14.0;

/// One slider row for `spec` showing `shown` (display units).
pub fn labeled_slider(
    ui: &mut egui::Ui,
    spec: &ControlSpec,
    shown: f64,
    enabled: bool,
) -> SliderOut {
    let t = Tokens::get(ui.ctx());
    let mut out = SliderOut::default();
    let changed = (shown - spec.default).abs() > 1e-9;
    let w = ui.available_width();
    let (row, _) = ui.allocate_exact_size(vec2(w, 36.0), Sense::hover());
    let id = ui.id().with(spec.id);
    let text_col = |primary: bool| {
        if !enabled {
            t.text_disabled
        } else if primary {
            t.text
        } else {
            t.text_secondary
        }
    };
    let p = ui.painter();
    // caption row
    let caption_y = row.top() + 8.0;
    p.text(
        pos2(row.left(), caption_y),
        Align2::LEFT_CENTER,
        spec.label,
        t.font(10.5),
        text_col(false),
    );
    let caption_w = p
        .layout_no_wrap(spec.label.to_owned(), t.font(10.5), t.text)
        .size()
        .x;
    let value_text = spec.format(shown);
    p.text(
        pos2(row.right(), caption_y),
        Align2::RIGHT_CENTER,
        &value_text,
        t.mono(10.5),
        text_col(changed),
    );
    if changed && enabled {
        let x_rect = Rect::from_center_size(
            pos2(row.left() + caption_w + 10.0, caption_y),
            Vec2::splat(12.0),
        );
        let mut child = ui.new_child(
            egui::UiBuilder::new()
                .max_rect(x_rect)
                .id_salt(id.with("x")),
        );
        if reset_x(&mut child, &t, spec.id) {
            out.value = Some(spec.default);
        }
    }
    // track
    let track = Rect::from_min_max(
        pos2(row.left() + KNOB / 2.0, row.top() + 20.0),
        pos2(row.right() - KNOB / 2.0, row.top() + 34.0),
    );
    let hit = Rect::from_min_max(
        pos2(row.left(), track.top() - 2.0),
        pos2(row.right(), track.bottom() + 2.0),
    );
    let resp = ui.interact(
        hit,
        id,
        if enabled {
            Sense::click_and_drag()
        } else {
            Sense::hover()
        },
    );
    let to_x = |v: f64| track.left() + spec.position(v) as f32 * track.width();
    let from_x = |x: f32| spec.value_at(f64::from((x - track.left()) / track.width().max(1.0)));
    let mut v = shown;
    if enabled {
        if resp.double_clicked() {
            out.value = Some(spec.default);
            v = spec.default;
        } else {
            if resp.drag_started() {
                out.drag_begin = true;
                ui.data_mut(|d| d.insert_temp(id.with("fine"), spec.position(shown)));
            }
            if resp.dragged() || resp.drag_started() {
                if let Some(ptr) = resp.interact_pointer_pos() {
                    let fine = ui.input(|i| i.modifiers.shift);
                    let nv = if fine {
                        let dx = ui.input(|i| i.pointer.delta().x);
                        let pos = ui
                            .data(|d| d.get_temp::<f64>(id.with("fine")))
                            .unwrap_or_else(|| spec.position(shown));
                        let pos =
                            (pos + f64::from(dx / track.width().max(1.0)) * 0.1).clamp(0.0, 1.0);
                        ui.data_mut(|d| d.insert_temp(id.with("fine"), pos));
                        spec.value_at(pos)
                    } else {
                        let nv = from_x(ptr.x);
                        ui.data_mut(|d| d.insert_temp(id.with("fine"), spec.position(nv)));
                        nv
                    };
                    if (nv - shown).abs() > 1e-12 {
                        out.value = Some(nv);
                        v = nv;
                    }
                }
            } else if resp.clicked()
                && let Some(ptr) = resp.interact_pointer_pos()
            {
                let nv = from_x(ptr.x);
                if (nv - shown).abs() > 1e-12 {
                    out.value = Some(nv);
                    v = nv;
                }
            }
            if resp.drag_stopped() {
                out.drag_end = true;
            }
        }
    }
    // paint groove
    let p = ui.painter();
    let groove_alpha = if enabled { 1.0 } else { 0.4 };
    match spec.track {
        Track::Plain => {
            let groove = Rect::from_center_size(track.center(), vec2(track.width(), 4.0));
            p.rect_filled(groove, 2.0, t.track.gamma_multiply(groove_alpha));
            let zero = spec.position(if spec.min < 0.0 && spec.max > 0.0 {
                0.0
            } else {
                spec.min
            });
            let x0 = track.left() + zero as f32 * track.width();
            let x1 = to_x(v);
            let fill = Rect::from_min_max(
                pos2(x0.min(x1), groove.top()),
                pos2(x0.max(x1), groove.bottom()),
            );
            // the value fill only while the control is live: a disabled slider (e.g. Mask Spacer
            // with Contrast Mask at 0) shows just the grey groove
            if enabled {
                p.rect_filled(fill, 2.0, t.accent.gamma_multiply(0.75));
            }
        }
        Track::Gradient3(stops) => paint_gradient(
            p,
            track,
            &[rgb_f(stops[0]), rgb_f(stops[1]), rgb_f(stops[2])],
            groove_alpha,
        ),
        Track::Gradient2(stops) => {
            paint_gradient(p, track, &[rgb_f(stops[0]), rgb_f(stops[1])], groove_alpha)
        }
    }
    // knob
    let kx = to_x(v);
    let kc = pos2(kx, track.center().y);
    let hovered = resp.hovered() || resp.dragged();
    p.circle_filled(
        kc,
        KNOB / 2.0,
        if enabled {
            t.knob
        } else {
            t.knob.gamma_multiply(0.5)
        },
    );
    p.circle_stroke(
        kc,
        KNOB / 2.0,
        Stroke::new(
            1.0,
            if hovered && enabled {
                t.accent
            } else {
                t.knob_border
            },
        ),
    );
    out
}

/// The `GradientSlider` groove: an 8 pt capsule with a linear gradient leading → trailing.
fn paint_gradient(p: &egui::Painter, track: Rect, stops: &[Color32], alpha: f32) {
    let groove = Rect::from_center_size(track.center(), vec2(track.width(), 8.0));
    let n = 48usize;
    let seg = groove.width() / n as f32;
    let color_at = |u: f32| -> Color32 {
        if stops.len() < 2 {
            return stops.first().copied().unwrap_or(Color32::GRAY);
        }
        let spans = (stops.len() - 1) as f32;
        let x = u.clamp(0.0, 1.0) * spans;
        let i = (x.floor() as usize).min(stops.len() - 2);
        let f = x - i as f32;
        let a = stops.get(i).copied().unwrap_or(Color32::GRAY);
        let b = stops.get(i + 1).copied().unwrap_or(a);
        a.lerp_to_gamma(b, f)
    };
    for i in 0..n {
        let x0 = groove.left() + seg * i as f32;
        let c = color_at((i as f32 + 0.5) / n as f32).gamma_multiply(alpha);
        p.rect_filled(
            Rect::from_min_max(
                pos2(x0, groove.top()),
                pos2(x0 + seg + 0.5, groove.bottom()),
            ),
            0.0,
            c,
        );
    }
    // round the ends
    p.circle_filled(
        pos2(groove.left(), groove.center().y),
        4.0,
        color_at(0.0).gamma_multiply(alpha),
    );
    p.circle_filled(
        pos2(groove.right(), groove.center().y),
        4.0,
        color_at(1.0).gamma_multiply(alpha),
    );
}

/// A switch-style toggle row: label left, switch right. Returns true when flipped.
pub fn toggle_row(ui: &mut egui::Ui, label: &str, value: &mut bool, enabled: bool) -> bool {
    let t = Tokens::get(ui.ctx());
    let w = ui.available_width();
    let (row, _) = ui.allocate_exact_size(vec2(w, 20.0), Sense::hover());
    ui.painter().text(
        pos2(row.left(), row.center().y),
        Align2::LEFT_CENTER,
        label,
        t.font(11.5),
        if enabled { t.text } else { t.text_disabled },
    );
    let sw = Rect::from_min_size(
        pos2(row.right() - 30.0, row.center().y - 8.0),
        vec2(30.0, 16.0),
    );
    let resp = ui.interact(
        sw,
        ui.id().with(label),
        if enabled {
            Sense::click()
        } else {
            Sense::hover()
        },
    );
    let mut flipped = false;
    if enabled && resp.clicked() {
        *value = !*value;
        flipped = true;
    }
    let on = *value;
    let how_on = ui.ctx().animate_bool(resp.id, on);
    let fill = if enabled {
        t.track.lerp_to_gamma(t.accent, how_on)
    } else {
        t.track.gamma_multiply(0.5)
    };
    let p = ui.painter();
    p.rect_filled(sw, 8.0, fill);
    let kx = egui::lerp((sw.left() + 8.0)..=(sw.right() - 8.0), how_on);
    p.circle_filled(
        pos2(kx, sw.center().y),
        6.5,
        if enabled {
            t.knob
        } else {
            t.knob.gamma_multiply(0.5)
        },
    );
    flipped
}

/// A segmented picker; returns the newly selected index.
pub fn segmented(ui: &mut egui::Ui, id: &str, labels: &[&str], selected: usize) -> Option<usize> {
    let t = Tokens::get(ui.ctx());
    let w = ui.available_width();
    let (row, _) = ui.allocate_exact_size(vec2(w, 20.0), Sense::hover());
    let p = ui.painter();
    p.rect_filled(row, 5.0, t.field);
    p.rect_stroke(
        row,
        5.0,
        Stroke::new(1.0, t.button_border),
        StrokeKind::Inside,
    );
    let n = labels.len().max(1) as f32;
    let seg_w = row.width() / n;
    let mut picked = None;
    for (i, label) in labels.iter().enumerate() {
        let r = Rect::from_min_size(
            pos2(row.left() + seg_w * i as f32, row.top()),
            vec2(seg_w, row.height()),
        );
        let resp = ui.interact(r.shrink(1.0), ui.id().with(id).with(i), Sense::click());
        if resp.clicked() && i != selected {
            picked = Some(i);
        }
        let active = i == selected;
        if active {
            ui.painter()
                .rect_filled(r.shrink(2.0), 4.0, t.button.lerp_to_gamma(t.accent, 0.35));
        } else if resp.hovered() {
            ui.painter().rect_filled(r.shrink(2.0), 4.0, t.hover);
        }
        ui.painter().text(
            r.center(),
            Align2::CENTER_CENTER,
            *label,
            t.font(10.5),
            if active { t.text } else { t.text_secondary },
        );
    }
    picked
}

/// Group box: title above an inset frame.
fn group<R>(ui: &mut egui::Ui, title: &str, add: impl FnOnce(&mut egui::Ui) -> R) -> R {
    let t = Tokens::get(ui.ctx());
    ui.label(
        egui::RichText::new(title)
            .size(10.5)
            .color(t.text_secondary)
            .strong(),
    );
    egui::Frame::new()
        .fill(t.inset)
        .corner_radius(6.0)
        .inner_margin(egui::Margin::same(8))
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing.y = ROW_GAP;
            ui.set_width(ui.available_width());
            add(ui)
        })
        .inner
}

/// A disclosure chevron (rotates 0° → 90° when open).
fn chevron(ui: &mut egui::Ui, open: bool, id: &str) -> egui::Response {
    let t = Tokens::get(ui.ctx());
    let (rect, resp) = ui.allocate_exact_size(Vec2::splat(16.0), Sense::click());
    let how_open = ui.ctx().animate_bool(ui.id().with(id), open);
    let c = rect.center();
    let angle = how_open * std::f32::consts::FRAC_PI_2;
    let pts: Vec<egui::Pos2> = [vec2(-2.0, -4.0), vec2(2.0, 0.0), vec2(-2.0, 4.0)]
        .iter()
        .map(|v| {
            let (s, co) = angle.sin_cos();
            c + vec2(v.x * co - v.y * s, v.x * s + v.y * co)
        })
        .collect();
    ui.painter().add(egui::Shape::line(
        pts,
        Stroke::new(
            1.5,
            if resp.hovered() {
                t.text
            } else {
                t.text_secondary
            },
        ),
    ));
    resp
}

fn small_button(ui: &mut egui::Ui, label: &str, enabled: bool, active: bool) -> egui::Response {
    let t = Tokens::get(ui.ctx());
    let mut b = egui::Button::new(egui::RichText::new(label).size(11.0));
    if active {
        b = b.fill(t.accent.gamma_multiply(0.45));
    }
    ui.add_enabled(enabled, b)
}

/// A full-width sidebar action button: icon and label centred together, the icon a little larger
/// than the text with a clear gap between them. `width` is the button's width (pass
/// `ui.available_width()` minus anything sharing the row). Disabled when `enabled` is false or the
/// surrounding ui is disabled; `active` fills it with the accent (a tool that is on).
fn wide_button(
    ui: &mut egui::Ui,
    icon: &str,
    label: &str,
    width: f32,
    enabled: bool,
    active: bool,
) -> egui::Response {
    let t = Tokens::get(ui.ctx());
    let enabled = enabled && ui.is_enabled();
    let size = vec2(width.max(40.0), 26.0);
    let sense = if enabled {
        Sense::click_and_drag()
    } else {
        Sense::hover()
    };
    let (rect, resp) = ui.allocate_exact_size(size, sense);
    if !ui.is_rect_visible(rect) {
        return resp;
    }
    let fill = if active {
        t.accent.gamma_multiply(0.45)
    } else if enabled && resp.is_pointer_button_down_on() {
        t.pressed
    } else if enabled && resp.hovered() {
        t.hover
    } else {
        t.button
    };
    let text_col = if enabled { t.text } else { t.text_disabled };
    let painter = ui.painter();
    painter.rect(
        rect,
        5.0,
        fill,
        Stroke::new(1.0, t.button_border),
        StrokeKind::Inside,
    );
    let icon_g =
        painter.layout_no_wrap(icon.to_owned(), egui::FontId::proportional(15.0), text_col);
    let text_g =
        painter.layout_no_wrap(label.to_owned(), egui::FontId::proportional(11.5), text_col);
    let gap = 7.0;
    let total = icon_g.size().x + gap + text_g.size().x;
    let x0 = rect.center().x - total / 2.0;
    let cy = rect.center().y;
    let text_pos = egui::pos2(x0 + icon_g.size().x + gap, cy - text_g.size().y / 2.0);
    // Optical alignment: centre the icon's INK on the middle of the label's capital letters.
    // Centring layout boxes put symbols from the fallback font (◉, ▦) visibly low, because their
    // glyphs sit low inside a box sized for that font's own ascent/descent.
    let cap = painter.layout_no_wrap("H".to_owned(), egui::FontId::proportional(11.5), text_col);
    let cap_mid = text_pos.y + cap.mesh_bounds.center().y;
    let icon_y = if icon_g.mesh_bounds.is_finite() && icon_g.mesh_bounds.height() > 0.0 {
        cap_mid - icon_g.mesh_bounds.center().y
    } else {
        cy - icon_g.size().y / 2.0
    };
    painter.galley(egui::pos2(x0, icon_y), icon_g.clone(), text_col);
    painter.galley(text_pos, text_g, text_col);
    resp
}

/// A thin full-width drag strip at the top of a bottom panel; dragging it resizes the History
/// panel (both the Crop & Rotation and the History panels carry one). Returns the vertical drag
/// delta this frame (negative = upward = History grows) and whether the drag ended.
fn resize_grip(ui: &mut egui::Ui, id: &str) -> (f32, bool) {
    let t = Tokens::get(ui.ctx());
    let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 7.0), Sense::drag());
    let resp = ui
        .interact(rect, ui.id().with(id), Sense::drag())
        .union(resp);
    let hot = resp.hovered() || resp.dragged();
    if hot {
        ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeVertical);
    }
    let col = if hot { t.text_secondary } else { t.divider };
    let c = rect.center();
    ui.painter()
        .rect_filled(egui::Rect::from_center_size(c, vec2(32.0, 3.0)), 1.5, col);
    (
        if resp.dragged() {
            resp.drag_delta().y
        } else {
            0.0
        },
        resp.drag_stopped(),
    )
}

/// Slider rows of `section` under the current band selections.
fn slider_rows(
    ui: &mut egui::Ui,
    rows: &[&ControlSpec],
    settings: &mut ExposureSettings,
    enabled: bool,
    actions: &mut Vec<SidebarAction>,
) {
    for spec in rows {
        slider_row(ui, spec, settings, enabled, actions);
    }
}

fn slider_row(
    ui: &mut egui::Ui,
    spec: &ControlSpec,
    settings: &mut ExposureSettings,
    enabled: bool,
    actions: &mut Vec<SidebarAction>,
) {
    let row_enabled = enabled && !spec.is_disabled(settings);
    let out = labeled_slider(ui, spec, spec.shown(settings), row_enabled);
    if out.drag_begin {
        actions.push(SidebarAction::DragBegin);
    }
    if let Some(v) = out.value {
        spec.apply(settings, v);
        actions.push(SidebarAction::SettingsEdited);
    }
    if out.drag_end {
        actions.push(SidebarAction::DragEnd);
    }
}

fn toggle_spec_row(
    ui: &mut egui::Ui,
    spec: &ToggleSpec,
    settings: &mut ExposureSettings,
    enabled: bool,
    actions: &mut Vec<SidebarAction>,
) {
    let mut v = spec.toggle.get(settings);
    if toggle_row(ui, spec.label, &mut v, enabled) {
        spec.toggle.set(settings, v);
        actions.push(SidebarAction::SettingsEdited);
    }
}

fn toggle_by_id(id: &str) -> Option<&'static ToggleSpec> {
    TOGGLES.iter().find(|t| t.id == id)
}

fn spec_by_id(id: &str) -> Option<&'static ControlSpec> {
    crate::control_spec::control(id)
}

fn divider(ui: &mut egui::Ui) {
    let t = Tokens::get(ui.ctx());
    let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), 1.0), Sense::hover());
    ui.painter().rect_filled(r, 0.0, t.divider);
}

/// The whole sidebar. Settings edits land in `settings`; everything else comes back as actions.
pub fn controls_sidebar(
    ui: &mut egui::Ui,
    state: &mut SidebarState,
    settings: &mut ExposureSettings,
    input: SidebarInput<'_>,
) -> Vec<SidebarAction> {
    let t = Tokens::get(ui.ctx());
    let mut actions = Vec::new();
    let enabled = input.has_selection;

    // History panel (bottom, resizable) — drawn first so the central content gets the rest.
    let hist_h = if state.prefs.history_collapsed {
        HISTORY_MIN_H
    } else {
        state.prefs.history_height.clamp(HISTORY_MIN_H, 600.0)
    };
    // Both bottom panels carry a drag strip on top; either one resizes History (the Crop &
    // Rotation panel keeps its fixed height and rides on top of History).
    let grip_h = 7.0;
    let mut grip_dy = 0.0;
    let mut grip_done = false;
    let panel_margin = egui::Margin {
        left: 10,
        right: 10,
        top: 0,
        bottom: 4,
    };
    egui::Panel::bottom(ui.id().with("history-panel"))
        .resizable(false)
        .exact_size(hist_h + grip_h)
        .frame(egui::Frame::new().fill(t.chrome).inner_margin(panel_margin))
        .show(ui, |ui| {
            let (dy, done) = resize_grip(ui, "history-grip");
            grip_dy += dy;
            grip_done |= done;
            history_panel(ui, state, &input, &mut actions);
        });

    // Crop & Rotation (bottom, fixed)
    egui::Panel::bottom(ui.id().with("crop-panel"))
        .resizable(false)
        .exact_size(grip_h + crop_panel_height(&state.prefs))
        .frame(egui::Frame::new().fill(t.chrome).inner_margin(panel_margin))
        .show(ui, |ui| {
            let (dy, done) = resize_grip(ui, "crop-grip");
            grip_dy += dy;
            grip_done |= done;
            crop_rotation_section(ui, state, settings, &input, &mut actions);
        });
    if grip_dy != 0.0 {
        // dragging up grows History; a drag on a collapsed History opens it
        if state.prefs.history_collapsed && grip_dy < 0.0 {
            state.prefs.history_collapsed = false;
        }
        if !state.prefs.history_collapsed {
            state.prefs.history_height =
                (state.prefs.history_height - grip_dy).clamp(HISTORY_MIN_H, 600.0);
        }
    }
    if grip_done {
        actions.push(SidebarAction::PrefsChanged);
    }

    // Adjustments
    egui::CentralPanel::default()
        .frame(
            egui::Frame::new()
                .fill(t.chrome)
                .inner_margin(egui::Margin::symmetric(10, 6)),
        )
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                if chevron(ui, !state.prefs.adjustments_collapsed, "adj-chev").clicked() {
                    state.prefs.adjustments_collapsed = !state.prefs.adjustments_collapsed;
                    actions.push(SidebarAction::PrefsChanged);
                }
                ui.label(egui::RichText::new("Adjustments").size(12.5).strong());
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if small_button(ui, "Reset All", enabled, false)
                        .on_hover_text("Reset every adjustment to the default settings")
                        .clicked()
                    {
                        actions.push(SidebarAction::ResetAll);
                    }
                });
            });
            if state.prefs.adjustments_collapsed {
                return;
            }
            ui.add_space(4.0);
            let h = histogram_panel(ui, input.bins, settings, &mut state.histogram);
            if h.drag_begin {
                actions.push(SidebarAction::DragBegin);
            }
            if h.changed {
                actions.push(SidebarAction::SettingsEdited);
            }
            if h.drag_end {
                actions.push(SidebarAction::DragEnd);
            }
            // --- U3: darkroom ---
            if h.double_clicked && enabled {
                actions.push(SidebarAction::OpenInteractiveHistogram);
            }
            // --- end U3 ---
            ui.add_space(6.0);
            egui::ScrollArea::vertical()
                .id_salt("controls-scroll")
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    ui.spacing_mut().item_spacing.y = 6.0;
                    ui.set_width(ui.available_width() - 2.0);
                    groups(ui, state, settings, &input, &mut actions);
                    ui.add_space(16.0);
                });
        });
    actions
}

fn groups(
    ui: &mut egui::Ui,
    state: &mut SidebarState,
    settings: &mut ExposureSettings,
    input: &SidebarInput<'_>,
    actions: &mut Vec<SidebarAction>,
) {
    let t = Tokens::get(ui.ctx());
    let enabled = input.has_selection;
    let gap = 16.0;

    group(ui, Section::PreProcess.label(), |ui| {
        ui.spacing_mut().item_spacing.y = 8.0;
        ui.horizontal(|ui| {
            let active = input.tool_mode == ToolMode::AnalysisRegion;
            let x = if settings.analysis_rect.is_some() {
                20.0
            } else {
                0.0
            };
            // --- U2: tools & dialogs ---
            if wide_button(
                ui,
                "▣",
                "Crop for Analysis",
                ui.available_width() - x,
                enabled,
                active,
            )
            .on_hover_text(crate::app::with_shortcut(
                ui.ctx(),
                "Meter only the region you draw",
                crate::app::Command::ToggleAnalysis,
            ))
            .clicked()
            {
                actions.push(SidebarAction::ToggleAnalysisTool);
            }
            if settings.analysis_rect.is_some() && reset_x(ui, &t, "analysis-x") {
                actions.push(SidebarAction::ClearAnalysisRect);
            }
        });
        if input.tool_mode == ToolMode::AnalysisRegion {
            ui.label(
                egui::RichText::new("Drag on the image to select the area.")
                    .size(10.0)
                    .color(t.text_secondary),
            );
        }
        let hold = wide_button(
            ui,
            "◉",
            "View Original",
            ui.available_width(),
            enabled,
            input.baseline_held,
        )
        .on_hover_text("Hold to compare with the stock conversion");
        let pressed = enabled && hold.is_pointer_button_down_on();
        if pressed != input.baseline_held {
            actions.push(SidebarAction::ViewOriginal(pressed));
        }
        if wide_button(
            ui,
            "↺",
            "Start from Scratch",
            ui.available_width(),
            enabled,
            false,
        )
        .on_hover_text("Stock settings (cancels the default profile)")
        .clicked()
        {
            actions.push(SidebarAction::StartFromScratch);
        }
    });
    ui.add_space(gap);

    group(ui, Section::Print.label(), |ui| {
        if let Some(sp) = toggle_by_id("print.autoExposure") {
            toggle_spec_row(ui, sp, settings, enabled, actions);
        }
        if let Some(sp) = spec_by_id("print.brightness") {
            slider_row(ui, sp, settings, enabled, actions);
        }
        if let Some(sp) = toggle_by_id("print.autoContrast") {
            toggle_spec_row(ui, sp, settings, enabled, actions);
        }
        if let Some(sp) = spec_by_id("print.grade") {
            slider_row(ui, sp, settings, enabled, actions);
        }
        negative_character_row(ui, input.density_range, input.character);
        if let Some(sp) = spec_by_id("print.contrastMask") {
            slider_row(ui, sp, settings, enabled, actions);
        }
        if let Some(sp) = spec_by_id("print.maskSpacer") {
            slider_row(ui, sp, settings, enabled, actions);
        }
        // --- U3: darkroom ---
        if wide_button(
            ui,
            "▦",
            "Test Strip",
            ui.available_width(),
            enabled,
            input.test_strip_active,
        )
        .on_hover_text(crate::app::with_shortcut(
            ui.ctx(),
            "Print the frame as a 5×5 test strip — Brightness across, Grade down",
            crate::app::Command::TestStrip,
        ))
        .clicked()
        {
            actions.push(SidebarAction::TestStrip);
        }
        // --- end U3 ---
    });
    ui.add_space(gap);

    group(ui, Section::Exposure.label(), |ui| {
        let rows = rows_for(Section::Exposure, state.mixer_band, state.grading_band);
        slider_rows(ui, &rows, settings, enabled, actions);
    });
    ui.add_space(gap);

    group(ui, Section::Color.label(), |ui| {
        for spec in rows_for(Section::Color, state.mixer_band, state.grading_band) {
            slider_row(ui, spec, settings, enabled, actions);
            if spec.id == "color.castStrength" || spec.id == "color.hueTrim" {
                divider(ui);
            }
        }
    });
    ui.add_space(gap);

    group(ui, Section::ColorMixer.label(), |ui| {
        let labels: Vec<&str> = MixerBand::ALL.iter().map(|b| b.label()).collect();
        let sel = MixerBand::ALL
            .iter()
            .position(|b| *b == state.mixer_band)
            .unwrap_or(0);
        if let Some(i) = segmented(ui, "mixer-band", &labels, sel)
            && let Some(b) = MixerBand::ALL.get(i)
        {
            state.mixer_band = *b;
        }
        let rows = rows_for(Section::ColorMixer, state.mixer_band, state.grading_band);
        slider_rows(ui, &rows, settings, enabled, actions);
    });
    ui.add_space(gap);

    group(ui, Section::ColorGrading.label(), |ui| {
        let rows = rows_for(Section::ColorGrading, state.mixer_band, state.grading_band);
        let (ungated, gated): (Vec<_>, Vec<_>) = rows.iter().partition(|c| c.band == BandTag::None);
        slider_rows(ui, &ungated, settings, enabled, actions);
        let labels: Vec<&str> = GradingBand::ALL.iter().map(|b| b.label()).collect();
        let sel = GradingBand::ALL
            .iter()
            .position(|b| *b == state.grading_band)
            .unwrap_or(1);
        if let Some(i) = segmented(ui, "grading-band", &labels, sel)
            && let Some(b) = GradingBand::ALL.get(i)
        {
            state.grading_band = *b;
        }
        // re-read under the (possibly new) band
        let rows = rows_for(Section::ColorGrading, state.mixer_band, state.grading_band);
        let gated_now: Vec<_> = rows
            .into_iter()
            .filter(|c| c.band != BandTag::None)
            .collect();
        let _ = gated;
        slider_rows(ui, &gated_now, settings, enabled, actions);
    });
    ui.add_space(gap);

    group(ui, Section::Tone.label(), |ui| {
        for spec in rows_for(Section::Tone, state.mixer_band, state.grading_band) {
            if spec.id == "tone.whitePoint"
                && let Some(sp) = toggle_by_id("tone.trueBlack")
            {
                toggle_spec_row(ui, sp, settings, enabled, actions);
            }
            slider_row(ui, spec, settings, enabled, actions);
        }
    });
    ui.add_space(gap);

    if small_button(ui, "Export…", enabled && !input.is_exporting, false)
        .on_hover_text(crate::app::with_shortcut(
            ui.ctx(),
            "Export the selected frames",
            crate::app::Command::Export,
        ))
        .clicked()
    {
        actions.push(SidebarAction::Export);
    }
    if let Some(msg) = input.status_message {
        ui.label(egui::RichText::new(msg).size(10.5).color(t.text_secondary));
    }
}

/// "Negative" · `"%.2f · %@"` under the Grade slider (NegativeCharacterRow.swift).
pub fn negative_character_row(
    ui: &mut egui::Ui,
    density_range: f64,
    character: Option<&'static str>,
) {
    let t = Tokens::get(ui.ctx());
    let (row, _) = ui.allocate_exact_size(vec2(ui.available_width(), 14.0), Sense::hover());
    let p = ui.painter();
    p.text(
        pos2(row.left(), row.center().y),
        Align2::LEFT_CENTER,
        "Negative",
        t.font(10.0),
        t.text_secondary,
    );
    match character {
        Some(label) if density_range > 0.0 && density_range.is_finite() => {
            p.text(
                pos2(row.right(), row.center().y),
                Align2::RIGHT_CENTER,
                format!("{density_range:.2} · {label}"),
                t.mono(10.0),
                t.text,
            );
        }
        _ => {
            p.text(
                pos2(row.right(), row.center().y),
                Align2::RIGHT_CENTER,
                "—",
                t.mono(10.0),
                t.text_tertiary,
            );
        }
    }
}

/// The smallest History content height: exactly its header row (chevron, title, undo/redo) plus
/// the panel's bottom margin — the header is never clipped, only the list beneath it.
pub const HISTORY_MIN_H: f32 = 26.0;

/// Height of the Crop & Rotation panel's content (without its drag bar): fixed per state —
/// collapsed, open, open with the grid-type picker (shown only while grid lines are on).
pub fn crop_panel_height(prefs: &SidebarPrefs) -> f32 {
    if prefs.crop_rotation_collapsed {
        28.0
    } else if prefs.show_grid_lines {
        126.0
    } else {
        98.0
    }
}

fn crop_rotation_section(
    ui: &mut egui::Ui,
    state: &mut SidebarState,
    settings: &ExposureSettings,
    input: &SidebarInput<'_>,
    actions: &mut Vec<SidebarAction>,
) {
    let t = Tokens::get(ui.ctx());
    ui.horizontal(|ui| {
        if chevron(ui, !state.prefs.crop_rotation_collapsed, "crop-chev").clicked() {
            state.prefs.crop_rotation_collapsed = !state.prefs.crop_rotation_collapsed;
            actions.push(SidebarAction::PrefsChanged);
        }
        ui.label(egui::RichText::new("Crop & Rotation").size(12.5).strong());
    });
    if state.prefs.crop_rotation_collapsed {
        return;
    }
    let enabled = input.has_selection;
    ui.add_enabled_ui(enabled, |ui| {
        ui.horizontal(|ui| {
            let active = input.tool_mode == ToolMode::Crop;
            let geometry_set = settings.crop_rect.is_some() || settings.fine_rotation.abs() > 1e-9;
            let x = if geometry_set { 20.0 } else { 0.0 };
            // --- U2: tools & dialogs ---
            if wide_button(
                ui,
                "⛶",
                "Crop or Straighten",
                ui.available_width() - x,
                true,
                active,
            )
            .on_hover_text(crate::app::with_shortcut(
                ui.ctx(),
                "Crop & Straighten",
                crate::app::Command::ToggleCrop,
            ))
            .clicked()
            {
                actions.push(SidebarAction::ToggleCropTool);
            }
            if geometry_set && reset_x(ui, &t, "crop-x") {
                actions.push(SidebarAction::ClearCropAndStraighten);
            }
        });
        let mut show = state.prefs.show_grid_lines;
        if toggle_row(ui, "Show grid lines", &mut show, true) {
            state.prefs.show_grid_lines = show;
            actions.push(SidebarAction::PrefsChanged);
        }
        // the grid type only matters while grid lines are shown
        if !state.prefs.show_grid_lines {
            return;
        }
        let labels: Vec<&str> = GridLineType::ALL.iter().map(|g| g.label()).collect();
        let sel = GridLineType::ALL
            .iter()
            .position(|g| *g == state.prefs.grid_line_type)
            .unwrap_or(0);
        egui::ComboBox::from_id_salt("grid-type")
            .selected_text(labels.get(sel).copied().unwrap_or(""))
            .width(ui.available_width())
            .show_ui(ui, |ui| {
                for (i, g) in GridLineType::ALL.iter().enumerate() {
                    if ui.selectable_label(i == sel, g.label()).clicked() && i != sel {
                        state.prefs.grid_line_type = *g;
                        actions.push(SidebarAction::PrefsChanged);
                    }
                }
            });
    });
}

fn history_panel(
    ui: &mut egui::Ui,
    state: &mut SidebarState,
    input: &SidebarInput<'_>,
    actions: &mut Vec<SidebarAction>,
) {
    let t = Tokens::get(ui.ctx());
    ui.horizontal(|ui| {
        if chevron(ui, !state.prefs.history_collapsed, "hist-chev").clicked() {
            state.prefs.history_collapsed = !state.prefs.history_collapsed;
            actions.push(SidebarAction::PrefsChanged);
        }
        ui.label(egui::RichText::new("History").size(12.5).strong());
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui
                .add_enabled(
                    input.can_redo,
                    egui::Button::new(egui::RichText::new("↷").size(17.0)).frame(false),
                )
                .on_hover_text(crate::app::with_shortcut(
                    ui.ctx(),
                    "Redo",
                    crate::app::Command::Redo,
                ))
                .clicked()
            {
                actions.push(SidebarAction::Redo);
            }
            if ui
                .add_enabled(
                    input.can_undo,
                    egui::Button::new(egui::RichText::new("↶").size(17.0)).frame(false),
                )
                .on_hover_text(crate::app::with_shortcut(
                    ui.ctx(),
                    "Undo",
                    crate::app::Command::Undo,
                ))
                .clicked()
            {
                actions.push(SidebarAction::Undo);
            }
        });
    });
    if state.prefs.history_collapsed {
        return;
    }
    let Some(history) = input.history else {
        ui.label(egui::RichText::new("—").color(t.text_tertiary));
        return;
    };
    let current = history.index();
    // The header row above is the panel's floor and always stays visible; only the list gives
    // way. A ScrollArea keeps a 64 pt minimum by default, which overflowed a short panel and
    // pushed the header off the top — let it shrink to whatever room is left, even none.
    let room = ui.available_height();
    if room < 1.0 {
        return;
    }
    egui::ScrollArea::vertical()
        .id_salt("history-scroll")
        .auto_shrink([false, false])
        .min_scrolled_height(0.0)
        .max_height(room)
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing.y = 1.0;
            for (i, entry) in history.entries().iter().enumerate().rev() {
                let (row, resp) =
                    ui.allocate_exact_size(vec2(ui.available_width(), 18.0), Sense::click());
                if i == current {
                    ui.painter()
                        .rect_filled(row, 3.0, t.accent.gamma_multiply(0.22));
                } else if resp.hovered() {
                    ui.painter().rect_filled(row, 3.0, t.hover);
                }
                let col = if i > current { t.text_tertiary } else { t.text };
                let p = ui.painter();
                p.text(
                    pos2(row.left() + 4.0, row.center().y),
                    Align2::LEFT_CENTER,
                    i.to_string(),
                    t.mono(9.0),
                    t.text_tertiary,
                );
                p.text(
                    pos2(row.left() + 24.0, row.center().y),
                    Align2::LEFT_CENTER,
                    &entry.label,
                    t.font(10.5),
                    col,
                );
                if resp.clicked() && i != current {
                    actions.push(SidebarAction::JumpHistory(i));
                }
            }
        });
}

/// Colour for a canvas-colour swatch preview (used by the loupe bar too).
pub fn swatch_color(white: f32) -> Color32 {
    gray_f(white)
}
