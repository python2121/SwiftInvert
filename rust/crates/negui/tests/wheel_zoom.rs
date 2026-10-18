//! Mouse-wheel zoom on the canvas: a notch zooms once and leaves no scroll behind.
//!
//! The bug (Linux mouse wheels): egui parks a line/page wheel scroll and releases it over the
//! following frames as `smooth_scroll_delta`; the canvas zoomed on the notch's frame, then read
//! that released tail as a trackpad pan and slid the view. `wheel_notches_to_zoom` rewrites the
//! notch into a zoom event before egui sees it. These tests drive egui itself.

use egui::{Event, Modifiers, MouseWheelUnit, Pos2, RawInput, Rect, Vec2, pos2, vec2};
use negui::loupe::{WHEEL_NOTCH_FACTOR, wheel_notches_to_zoom};

const CANVAS: Rect = Rect::from_min_max(pos2(300.0, 30.0), pos2(1100.0, 840.0));

fn wheel(unit: MouseWheelUnit, delta: Vec2, modifiers: Modifiers) -> Event {
    Event::MouseWheel {
        unit,
        delta,
        modifiers,
        phase: egui::TouchPhase::Move,
    }
}

/// Run `frames` egui frames at 60 fps, the first carrying `first`; returns per frame
/// (zoom_delta, smooth_scroll_delta). `rewrite` applies the app's raw-input hook.
fn run(first: Vec<Event>, frames: usize, rewrite: bool) -> Vec<(f32, Vec2)> {
    let ctx = egui::Context::default();
    let pointer = pos2(700.0, 400.0);
    let mut out = Vec::new();
    for f in 0..frames {
        let mut raw = RawInput {
            time: Some(f as f64 / 60.0),
            predicted_dt: 1.0 / 60.0,
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(1440.0, 900.0))),
            ..Default::default()
        };
        raw.events.push(Event::PointerMoved(pointer));
        if f == 0 {
            raw.events.extend(first.iter().cloned());
        }
        if rewrite {
            wheel_notches_to_zoom(&mut raw, CANVAS, Some(pointer), |_| true);
        }
        let mut sample = (1.0, Vec2::ZERO);
        let mut full = ctx.run_ui(raw, |ui| {
            sample = ui.input(|i| (i.zoom_delta(), i.smooth_scroll_delta));
        });
        full.textures_delta.clear();
        out.push(sample);
    }
    out
}

fn tail_after_first(frames: &[(f32, Vec2)]) -> f32 {
    frames.iter().skip(1).map(|(_, s)| s.length()).sum()
}

#[test]
fn unrewritten_wheel_notch_leaves_a_scroll_tail_for_later_frames() {
    // documents the mechanism the fix answers: the notch's scroll arrives AFTER its frame
    let frames = run(
        vec![wheel(MouseWheelUnit::Line, vec2(0.0, 1.0), Modifiers::NONE)],
        30,
        false,
    );
    assert!(
        tail_after_first(&frames) > 5.0,
        "egui should release the parked wheel scroll over later frames: {frames:?}"
    );
}

#[test]
fn a_wheel_notch_over_the_canvas_zooms_once_and_never_scrolls() {
    for (unit, dy) in [
        (MouseWheelUnit::Line, 1.0),
        (MouseWheelUnit::Line, -2.0),
        (MouseWheelUnit::Page, 1.0),
    ] {
        let frames = run(vec![wheel(unit, vec2(0.0, dy), Modifiers::NONE)], 30, true);
        let expect = WHEEL_NOTCH_FACTOR.powf(dy);
        assert!(
            (frames[0].0 - expect).abs() < 1e-5,
            "{unit:?} {dy}: zoom {} ≠ {expect}",
            frames[0].0
        );
        assert!(
            frames
                .iter()
                .all(|(z, s)| *s == Vec2::ZERO && (frames[0].0 == *z || *z == 1.0)),
            "{unit:?} {dy}: scroll or extra zoom after the notch: {frames:?}"
        );
        assert!(frames.iter().skip(1).all(|(z, _)| *z == 1.0));
    }
}

#[test]
fn trackpad_scrolling_and_egui_zoom_chords_pass_through() {
    let pointer = Some(pos2(700.0, 400.0));
    for event in [
        // trackpad two-finger scroll (points): still a pan
        wheel(MouseWheelUnit::Point, vec2(3.0, -4.0), Modifiers::NONE),
        // ⌘/Ctrl + wheel: egui's own zoom chord
        wheel(MouseWheelUnit::Line, vec2(0.0, 1.0), Modifiers::COMMAND),
        wheel(MouseWheelUnit::Line, vec2(0.0, 1.0), Modifiers::CTRL),
        // a horizontal-only tilt-wheel step: a pan, not a zoom
        wheel(MouseWheelUnit::Line, vec2(1.0, 0.0), Modifiers::NONE),
    ] {
        let mut raw = RawInput::default();
        raw.events.push(event.clone());
        wheel_notches_to_zoom(&mut raw, CANVAS, pointer, |_| true);
        assert_eq!(raw.events, vec![event]);
    }
}

#[test]
fn only_wheel_events_over_the_canvas_layer_become_zoom() {
    let notch = wheel(MouseWheelUnit::Line, vec2(0.0, 1.0), Modifiers::NONE);
    // over the library (outside the canvas): keeps scrolling the library
    let mut raw = RawInput::default();
    raw.events.push(notch.clone());
    wheel_notches_to_zoom(&mut raw, CANVAS, Some(pos2(100.0, 400.0)), |_| true);
    assert_eq!(raw.events, vec![notch.clone()]);
    // over the canvas but under a floating window: scrolls the window
    let mut raw = RawInput::default();
    raw.events.push(notch.clone());
    wheel_notches_to_zoom(&mut raw, CANVAS, Some(pos2(700.0, 400.0)), |_| false);
    assert_eq!(raw.events, vec![notch.clone()]);
    // the pointer arrives over the canvas within the same batch: the notch follows it
    let mut raw = RawInput::default();
    raw.events.push(Event::PointerMoved(pos2(700.0, 400.0)));
    raw.events.push(notch);
    wheel_notches_to_zoom(&mut raw, CANVAS, Some(pos2(100.0, 400.0)), |_| true);
    assert_eq!(raw.events.get(1), Some(&Event::Zoom(WHEEL_NOTCH_FACTOR)));
}
