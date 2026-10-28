//! Zone placement (wave 4 / U3) — `ZonePlacementLayer` (`ZonePlacementView.swift`) + the
//! `AppModel` placement state (session-spec §8, NegPy 5a095f3 / 9dff124): click a zone-strip
//! cell, click that spot on the photo, and Brightness (1 pin) / + Grade (2) / + one knee
//! control (3) solve so the tone prints there. The solved-look preview is drawn over the frame,
//! pins drag (re-sample; an untargeted pin re-snaps to its new reading, the solve waits for the
//! release), Apply = ONE history entry ("Zone placement"), Escape / any edit / tool / baseline /
//! HQ / navigation clears. Sampling, the forward model and the solve run on the engine worker
//! (`Session::{sample_zone_pin, predicted_zone, solve_zone_placement}`); one generation counter
//! fences stale results.

use egui::{Align2, Color32, Pos2, Rect, Stroke, StrokeKind, TextureHandle, pos2, vec2};
use negkit::zone_placement::{MAX_PINS, Pin, Solution};
use negsession::RenderOutput;
use negsession::densitometer::zone_roman;

use crate::engine::PinOp;
use crate::loupe::{CanvasOverlay, LoupeState, content_rect, pill};
use crate::theme::Tokens;

/// Pin dot radius / grab radius (points).
pub const PIN_RADIUS: f32 = 7.0;
pub const PIN_GRAB: f32 = 12.0;

/// What the layer asks the app to do.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum PlacementAction {
    AddPin {
        u: f64,
        v: f64,
    },
    MovePin {
        index: usize,
        u: f64,
        v: f64,
        last: bool,
    },
    Apply,
    Clear,
}

/// `ZonePlacementState`.
#[derive(Default)]
pub struct PlacementState {
    pub pins: Vec<Pin>,
    /// What each pin prints at under the CURRENT settings (roman), refreshed per solve.
    pub labels: Vec<String>,
    pub solution: Option<Solution>,
    /// The solved-look render and its content window.
    pub preview: Option<(TextureHandle, negkit::NormalizedRect)>,
    /// Zone picked on the strip, consumed by the next canvas click.
    pub armed: Option<f64>,
    pub solving: bool,
}

/// The app's placement controller.
#[derive(Default)]
pub struct ZonePlacementController {
    pub generation: u64,
    pub state: Option<PlacementState>,
    /// Pin being dragged + its live frame-normalized position.
    drag: Option<(usize, [f64; 2])>,
    actions: Vec<PlacementAction>,
}

impl std::fmt::Debug for ZonePlacementController {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ZonePlacementController")
            .field("generation", &self.generation)
            .field("active", &self.state.is_some())
            .finish()
    }
}

/// Add `pin` (append below [`MAX_PINS`], else replace the pin nearest it).
pub fn insert_pin(pins: &mut Vec<Pin>, pin: Pin) {
    if pins.len() < MAX_PINS {
        pins.push(pin);
        return;
    }
    let d = |p: &Pin| (p.nx - pin.nx).powi(2) + (p.ny - pin.ny).powi(2);
    let nearest = pins
        .iter()
        .enumerate()
        .min_by(|a, b| d(a.1).total_cmp(&d(b.1)))
        .map(|(i, _)| i);
    if let Some(slot) = nearest.and_then(|i| pins.get_mut(i)) {
        *slot = pin;
    }
}

/// A moved pin: position + sample from `sampled`; an untargeted pin takes the re-snapped
/// reading, a retargeted one keeps its zone.
pub fn moved_pin(old: &Pin, sampled: &Pin) -> Pin {
    Pin {
        nx: sampled.nx,
        ny: sampled.ny,
        val_rgb: sampled.val_rgb,
        val_luma: sampled.val_luma,
        mask_val: sampled.mask_val,
        target_zone: if old.retargeted {
            old.target_zone
        } else {
            sampled.target_zone
        },
        retargeted: old.retargeted,
    }
}

/// The panel summary (`ZonePlacementLayer.solutionSummary`).
pub fn solution_summary(solution: &Solution, pins: usize) -> String {
    let mut parts = vec![format!("Brightness {:.2}", 2.0 - solution.settings.density)];
    if pins >= 2 {
        parts.push(format!("Grade R{:.0}", solution.settings.grade));
    }
    if let Some(k) = solution.knee_label {
        parts.push(k.to_owned());
    }
    parts.join(" · ")
}

impl ZonePlacementController {
    pub fn is_active(&self) -> bool {
        self.state.is_some()
    }

    pub fn armed(&self) -> Option<f64> {
        self.state.as_ref().and_then(|s| s.armed)
    }

    /// `armZonePlacementTarget`: toggle `zone` as the armed target (creating the state).
    pub fn arm(&mut self, zone: f64) {
        let st = self.state.get_or_insert_with(PlacementState::default);
        st.armed = if st.armed == Some(zone) {
            None
        } else {
            Some(zone)
        };
    }

    /// Bump and return the generation (a sample or solve is being issued).
    pub fn next_generation(&mut self) -> u64 {
        self.generation += 1;
        self.generation
    }

    /// `clearZonePlacement`. Returns whether it owned the canvas.
    pub fn clear(&mut self) -> bool {
        let owned = self.state.is_some();
        self.generation += 1;
        self.state = None;
        self.drag = None;
        self.actions.clear();
        owned
    }

    /// Apply a sample result. Returns true when a re-solve should follow.
    pub fn apply_sample(&mut self, op: PinOp, pin: Pin) -> bool {
        let Some(st) = &mut self.state else {
            return false;
        };
        match op {
            PinOp::Add => {
                insert_pin(&mut st.pins, pin);
                st.armed = None;
                true
            }
            PinOp::Move { index, last } => {
                let Some(old) = st.pins.get_mut(index) else {
                    return false;
                };
                *old = moved_pin(old, &pin);
                last
            }
        }
    }

    /// Adopt a solve result.
    pub fn adopt_solve(
        &mut self,
        ctx: &egui::Context,
        labels: Vec<String>,
        solution: Option<Solution>,
        preview: Option<&RenderOutput>,
    ) {
        let Some(st) = &mut self.state else {
            return;
        };
        st.labels = labels;
        st.solution = solution;
        st.solving = false;
        st.preview = preview.and_then(|out| {
            let bytes = out.frame.rgba8()?;
            if out.width == 0 || bytes.len() != out.width * out.height * 4 {
                return None;
            }
            let img = egui::ColorImage::from_rgba_unmultiplied([out.width, out.height], bytes);
            Some((
                ctx.load_texture("zone-placement", img, egui::TextureOptions::LINEAR),
                out.content_window,
            ))
        });
    }

    pub fn take_actions(&mut self) -> Vec<PlacementAction> {
        std::mem::take(&mut self.actions)
    }

    fn pin_pos(&self, i: usize, pin: &Pin, frame_rect: Rect) -> Pos2 {
        let (u, v) = match self.drag {
            Some((d, p)) if d == i => (p[0], p[1]),
            _ => (pin.nx, pin.ny),
        };
        pos2(
            frame_rect.left() + u as f32 * frame_rect.width(),
            frame_rect.top() + v as f32 * frame_rect.height(),
        )
    }
}

fn normalized(p: Pos2, frame_rect: Rect) -> (f64, f64) {
    (
        f64::from((p.x - frame_rect.left()) / frame_rect.width().max(1.0)).clamp(0.0, 1.0),
        f64::from((p.y - frame_rect.top()) / frame_rect.height().max(1.0)).clamp(0.0, 1.0),
    )
}

impl CanvasOverlay for ZonePlacementController {
    fn paint(&mut self, ui: &mut egui::Ui, frame_rect: Rect, _zoom_state: &LoupeState) {
        let t = Tokens::get(ui.ctx());
        let clip = ui.clip_rect();
        let Some(st) = &self.state else {
            return;
        };
        let p = ui.painter().clone();
        if let Some((tex, window)) = &st.preview {
            p.image(
                tex.id(),
                content_rect(frame_rect, window),
                Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)),
                Color32::WHITE,
            );
        }
        if let (Some(z), true) = (st.armed, st.pins.is_empty()) {
            pill(
                &p,
                pos2(clip.center().x, clip.top() + 10.0),
                Align2::CENTER_TOP,
                &format!(
                    "Click the tone to place on Zone {} — Esc cancels",
                    zone_roman(z)
                ),
                &t,
            );
        }
        let mono = egui::FontId::monospace(10.0);
        for (i, pin) in st.pins.iter().enumerate() {
            let c = self.pin_pos(i, pin, frame_rect);
            let measured = st.labels.get(i).map_or("–", String::as_str);
            let label = format!("{measured} → {}", zone_roman(pin.target_zone));
            let galley = p.layout_no_wrap(label, mono.clone(), Color32::WHITE);
            let size = galley.size() + vec2(10.0, 4.0);
            let r = Rect::from_center_size(c - vec2(0.0, PIN_RADIUS + 4.0 + size.y / 2.0), size);
            p.rect_filled(r, size.y / 2.0, Color32::from_black_alpha(180));
            p.galley(r.min + vec2(5.0, 2.0), galley, Color32::WHITE);
            p.circle_filled(c, PIN_RADIUS, Color32::from_black_alpha(140));
            p.circle_stroke(c, PIN_RADIUS - 0.75, Stroke::new(1.5, Color32::WHITE));
            p.circle_filled(c, 1.5, Color32::WHITE);
        }
        if st.pins.is_empty() {
            return;
        }
        // Bottom panel: summary + Apply / Clear.
        let (summary, warn) = if st.solving {
            ("solving…".to_owned(), false)
        } else if let Some(sol) = &st.solution {
            (solution_summary(sol, st.pins.len()), sol.clamped)
        } else {
            ("no solution — move the pins apart".to_owned(), false)
        };
        let panel_w = 460.0f32.min(clip.width() - 16.0).max(200.0);
        let panel = Rect::from_center_size(
            pos2(clip.center().x, clip.bottom() - 12.0 - 20.0),
            vec2(panel_w, if warn { 40.0 } else { 30.0 }),
        );
        p.rect_filled(panel, 15.0, t.chrome.gamma_multiply(0.95));
        p.rect_stroke(
            panel,
            15.0,
            Stroke::new(1.0, t.text_secondary.gamma_multiply(0.3)),
            StrokeKind::Inside,
        );
        let mut child = ui.new_child(
            egui::UiBuilder::new()
                .max_rect(panel.shrink2(vec2(14.0, 4.0)))
                .layout(egui::Layout::right_to_left(egui::Align::Center)),
        );
        if child
            .button("Clear")
            .on_hover_text("Drop the pins without changing anything (Esc)")
            .clicked()
        {
            self.actions.push(PlacementAction::Clear);
        }
        let can_apply = st.solution.is_some() && !st.solving;
        if child
            .add_enabled(can_apply, egui::Button::new("Apply"))
            .on_hover_text("Commit the solved settings as one history entry")
            .clicked()
        {
            self.actions.push(PlacementAction::Apply);
        }
        child.with_layout(egui::Layout::top_down(egui::Align::Min), |ui| {
            ui.spacing_mut().item_spacing.y = 1.0;
            ui.label(
                egui::RichText::new(summary)
                    .font(egui::FontId::monospace(10.0))
                    .color(t.text),
            );
            if warn {
                ui.label(
                    egui::RichText::new("some targets are out of reach — closest shown")
                        .size(9.0)
                        .color(Color32::from_rgb(255, 149, 0)),
                );
            }
        });
    }

    fn handle(&mut self, response: &egui::Response, frame_rect: Rect) -> bool {
        let Some(st) = &self.state else {
            return false;
        };
        let pointer = response.interact_pointer_pos();
        if response.drag_started()
            && let Some(p) = pointer
        {
            let hit = st
                .pins
                .iter()
                .enumerate()
                .map(|(i, pin)| (i, self.pin_pos(i, pin, frame_rect).distance(p)))
                .filter(|(_, d)| *d <= PIN_GRAB)
                .min_by(|a, b| a.1.total_cmp(&b.1))
                .map(|(i, _)| i);
            if let Some(i) = hit {
                let (u, v) = normalized(p, frame_rect);
                self.drag = Some((i, [u, v]));
            }
        }
        if let Some((i, _)) = self.drag {
            if let Some(p) = pointer {
                let (u, v) = normalized(p, frame_rect);
                let moved = self.drag.is_some_and(|(_, q)| q != [u, v]);
                self.drag = Some((i, [u, v]));
                if response.drag_stopped() {
                    self.actions.push(PlacementAction::MovePin {
                        index: i,
                        u,
                        v,
                        last: true,
                    });
                    self.drag = None;
                } else if moved {
                    self.actions.push(PlacementAction::MovePin {
                        index: i,
                        u,
                        v,
                        last: false,
                    });
                }
            } else {
                // Pointer gone mid-drag: settle where it was.
                if let Some((i, q)) = self.drag.take() {
                    self.actions.push(PlacementAction::MovePin {
                        index: i,
                        u: q[0],
                        v: q[1],
                        last: true,
                    });
                }
            }
            return true;
        }
        if response.double_clicked() {
            return true;
        }
        if response.clicked()
            && let Some(p) = pointer
        {
            let on_pin = st
                .pins
                .iter()
                .enumerate()
                .any(|(i, pin)| self.pin_pos(i, pin, frame_rect).distance(p) <= PIN_GRAB);
            if frame_rect.contains(p) && !on_pin {
                let (u, v) = normalized(p, frame_rect);
                self.actions.push(PlacementAction::AddPin { u, v });
            }
            return true;
        }
        false
    }
}
