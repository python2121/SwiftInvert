//! `--selftest out.png [folder]` — the whole shell rendered offscreen through `egui_kittest`
//! (egui-wgpu into a texture, read back as a PNG): open the folder, let the first RAW decode,
//! analyse and render, then screenshot. Sidecars are never written here (the Qt shell's rule).
//! This is how the shell is verified without a window: run it, then LOOK at the PNG.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use crate::app::{AppOptions, NegApp};

/// Offscreen window size (points) and scale.
pub const SELFTEST_SIZE: [f32; 2] = [1440.0, 900.0];

// --- U3: darkroom ---
/// Optional darkroom captures (`--teststrip`, `--zones`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct DarkroomCaptures {
    /// `<out>_teststrip.png` (hovered patch) and `<out>_teststrip_preview.png` (one patch
    /// previewed full-frame).
    pub test_strip: bool,
    /// `<out>_zones.png` (overlay + a densitometer reading), `<out>_placement.png` (two pins
    /// solved) and `<out>_levels.png` (the Interactive Histogram with a planted anchor).
    pub zones: bool,
}
// --- end U3 ---

/// Run the selftest: returns the written PNG's pixel size.
pub fn run_selftest(out: &Path, folder: PathBuf, timeout: Duration) -> Result<[u32; 2], String> {
    run_selftest_with(out, folder, timeout, SelftestOptions::default())
}

// --- U2: tools & dialogs ---
/// Extra selftest passes (`--crop`, `--export <dir>`, `--dialogs`, `--latency`) and the wgpu
/// setup the harness renders with (the app's own, so the engine can share the device and the
/// zero-copy presenter is the one exercised; `None` = kittest's default lazy renderer).
#[derive(Default)]
pub struct SelftestOptions {
    pub crop: bool,
    pub export_dir: Option<PathBuf>,
    pub dialogs: bool,
    pub latency: bool,
    pub wgpu_setup: Option<eframe::egui_wgpu::WgpuSetup>,
    /// The darkroom captures (`--teststrip`, `--zones`; U3).
    pub darkroom: DarkroomCaptures,
}
// --- end U2 ---

/// [`run_selftest`] with the extra passes.
pub fn run_selftest_with(
    out: &Path,
    folder: PathBuf,
    timeout: Duration,
    extra: SelftestOptions,
) -> Result<[u32; 2], String> {
    if !folder.is_dir() {
        return Err(format!("{} is not a folder", folder.display()));
    }
    let options = AppOptions {
        initial_folder: Some(folder),
        autosave: false,
        persist: false,
        renderer: None,
        presenter: None,
        backend: None,
        // headless: no AppKit menu bar; the in-window bar is what the screenshots show
        native_menu: false,
    };
    let mut builder = egui_kittest::Harness::builder()
        .with_size(egui::vec2(SELFTEST_SIZE[0], SELFTEST_SIZE[1]))
        .with_pixels_per_point(1.0)
        .with_max_steps(100_000);
    // --- U2 ---
    let SelftestOptions {
        crop,
        export_dir,
        dialogs,
        latency,
        wgpu_setup,
        darkroom,
    } = extra;
    if let Some(setup) = wgpu_setup {
        builder = builder.renderer(egui_kittest::wgpu::WgpuTestRenderer::from_setup(setup));
    }
    // --- end U2 ---
    let mut harness = builder.build_eframe(|cc| NegApp::new(cc, options));
    let start = Instant::now();
    loop {
        harness.step();
        if harness.state().selftest_ready() {
            break;
        }
        if let Some(e) = harness.state().selftest_failure() {
            return Err(e);
        }
        if start.elapsed() > timeout {
            return Err(format!("selftest timed out after {:?}", timeout));
        }
        std::thread::sleep(Duration::from_millis(15));
    }
    // a few settle frames (texture uploads, animations)
    for _ in 0..8 {
        harness.step();
        std::thread::sleep(Duration::from_millis(16));
    }
    let image = harness.render()?;
    let perf = harness.state().perf();
    log::info!(
        "selftest: ready in {:.1} s; first render {:.1} ms, latency {:.1} ms, ui {:.2} ms",
        start.elapsed().as_secs_f32(),
        perf.render_ms,
        perf.latency_ms,
        perf.ui_ms
    );
    if let Some(parent) = out.parent()
        && !parent.as_os_str().is_empty()
    {
        std::fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
    }
    image
        .save(out)
        .map_err(|e| format!("{}: {e}", out.display()))?;
    let size = [image.width(), image.height()];

    // Slider → frame: one Exposure notch, time until the next frame is presented.
    let before = harness.state().frames_rendered();
    let t0 = Instant::now();
    harness.state_mut().bench_nudge();
    let mut nudged_ms = None;
    while t0.elapsed() < Duration::from_secs(10) {
        harness.step();
        if harness.state().frames_rendered() > before {
            nudged_ms = Some(t0.elapsed().as_secs_f32() * 1000.0);
            break;
        }
        std::thread::sleep(Duration::from_millis(1));
    }
    let perf = harness.state().perf();
    match nudged_ms {
        Some(ms) => log::info!(
            "selftest: adjustment → new frame {ms:.1} ms (engine render {:.1} ms, request latency {:.1} ms)",
            perf.render_ms,
            perf.latency_ms
        ),
        None => log::warn!("selftest: no frame followed the adjustment within 10 s"),
    }

    // Second capture: the controls column scrolled to its end (mixer/grading/tone rows).
    let sidebar = egui::pos2(SELFTEST_SIZE[0] - 145.0, SELFTEST_SIZE[1] * 0.5);
    harness.hover_at(sidebar);
    for (notches, suffix) in [(3, "_mid"), (40, "_controls")] {
        for _ in 0..notches {
            harness.event(egui::Event::MouseWheel {
                unit: egui::MouseWheelUnit::Line,
                delta: egui::vec2(0.0, -6.0),
                modifiers: egui::Modifiers::NONE,
                phase: egui::TouchPhase::Move,
            });
            harness.step();
        }
        for _ in 0..4 {
            harness.step();
        }
        let capture = harness.render()?;
        let path = sibling(out, suffix);
        capture
            .save(&path)
            .map_err(|e| format!("{}: {e}", path.display()))?;
        log::info!("selftest: wrote {}", path.display());
    }
    harness.remove_cursor();
    // --- U2: tools & dialogs ---
    let passes = crate::tools::selftest::Passes {
        crop,
        export_dir,
        dialogs,
        latency,
    };
    crate::tools::selftest::run(&mut harness, out, &passes)?;
    // --- end U2 ---
    // --- U3: darkroom ---
    if darkroom.zones {
        darkroom_zones(&mut harness, out)?;
    }
    if darkroom.test_strip {
        darkroom_test_strip(&mut harness, out)?;
    }
    // --- end U3 ---
    Ok(size)
}

// --- U3: darkroom ---
type AppHarness<'a> = egui_kittest::Harness<'a, NegApp>;

/// Step until `done` holds (or `limit` elapses).
fn step_until(
    harness: &mut AppHarness<'_>,
    limit: Duration,
    done: impl Fn(&NegApp) -> bool,
) -> Result<(), String> {
    let t0 = Instant::now();
    while !done(harness.state()) {
        if t0.elapsed() > limit {
            return Err("darkroom selftest: timed out waiting for the layer".into());
        }
        harness.step();
        std::thread::sleep(Duration::from_millis(5));
    }
    Ok(())
}

fn settle(harness: &mut AppHarness<'_>, frames: usize) {
    for _ in 0..frames {
        harness.step();
        std::thread::sleep(Duration::from_millis(4));
    }
}

fn capture(harness: &mut AppHarness<'_>, out: &Path, suffix: &str) -> Result<(), String> {
    let image = harness.render()?;
    let path = sibling(out, suffix);
    image
        .save(&path)
        .map_err(|e| format!("{}: {e}", path.display()))?;
    log::info!("selftest: wrote {}", path.display());
    Ok(())
}

fn click(harness: &mut AppHarness<'_>, at: egui::Pos2) {
    harness.hover_at(at);
    harness.drag_at(at);
    harness.event(egui::Event::PointerButton {
        pos: at,
        button: egui::PointerButton::Primary,
        pressed: false,
        modifiers: egui::Modifiers::NONE,
    });
    harness.step();
}

fn frame_point(harness: &AppHarness<'_>, u: f32, v: f32) -> Result<egui::Pos2, String> {
    let r = harness
        .state()
        .canvas_frame_rect()
        .ok_or("darkroom selftest: no frame on the canvas")?;
    Ok(egui::pos2(
        r.left() + u * r.width(),
        r.top() + v * r.height(),
    ))
}

/// Zone overlay + densitometer, then a two-pin zone placement, then the Interactive Histogram.
fn darkroom_zones(harness: &mut AppHarness<'_>, out: &Path) -> Result<(), String> {
    let limit = Duration::from_secs(30);
    harness.state_mut().run(crate::app::Command::ToggleZones);
    step_until(harness, limit, |a| a.zone_grid_ready())?;
    let probe = frame_point(harness, 0.5, 0.5)?;
    harness.hover_at(probe);
    settle(harness, 6);
    log::info!(
        "selftest: zone overlay computed in {:.1} ms; read-out \"{}\"",
        harness.state().zone_grid_ms(),
        harness.state().densitometer_readout().text()
    );
    capture(harness, out, "_zones")?;
    harness.state_mut().run(crate::app::Command::ToggleZones);

    // Zone placement: Zone V at the centre, Zone VII at a third of the frame.
    for (zone, (u, v)) in [(5.0, (0.5, 0.5)), (7.0, (0.3, 0.35))] {
        harness.state_mut().arm_placement_zone(zone);
        settle(harness, 2);
        let p = frame_point(harness, u, v)?;
        click(harness, p);
        let want = if zone > 5.0 { 2 } else { 1 };
        step_until(harness, limit, |a| {
            a.placement_status()
                .is_some_and(|(n, solved, solving)| n == want && solved && !solving)
        })?;
    }
    harness.remove_cursor();
    settle(harness, 6);
    capture(harness, out, "_placement")?;
    harness.state_mut().run(crate::app::Command::Escape);
    settle(harness, 2);

    // Interactive Histogram: open it and plant one anchor by dragging in the plot.
    harness
        .state_mut()
        .run(crate::app::Command::InteractiveHistogram);
    settle(harness, 4);
    let window = harness
        .ctx
        .memory(|m| m.area_rect(egui::Id::new("interactive-histogram")))
        .ok_or("darkroom selftest: the Interactive Histogram window has no rect")?;
    let start = egui::pos2(window.center().x - 40.0, window.center().y);
    let end = start + egui::vec2(60.0, 0.0);
    harness.hover_at(start);
    harness.drag_at(start);
    for i in 1..=6 {
        harness.hover_at(start + (end - start) * (i as f32 / 6.0));
    }
    harness.drop_at(end);
    settle(harness, 10);
    step_until(harness, limit, |a| !a.settings().levels_red.is_empty())?;
    settle(harness, 30);
    capture(harness, out, "_levels")?;
    harness.state_mut().close_interactive_histogram();
    settle(harness, 2);
    Ok(())
}

/// The test strip with a hovered patch, then one patch previewed.
fn darkroom_test_strip(harness: &mut AppHarness<'_>, out: &Path) -> Result<(), String> {
    let limit = Duration::from_secs(60);
    harness.state_mut().run(crate::app::Command::TestStrip);
    step_until(harness, limit, |a| a.test_strip_up())?;
    log::info!(
        "selftest: test strip built in {:.0} ms",
        harness.state().test_strip_build_ms()
    );
    // hover the patch at row 1, col 3
    let p = frame_point(harness, 0.7, 0.3)?;
    harness.hover_at(p);
    settle(harness, 6);
    capture(harness, out, "_teststrip")?;
    // single click → held for the double-click interval → preview
    click(harness, p);
    settle(harness, 40);
    harness.remove_cursor();
    settle(harness, 20);
    capture(harness, out, "_teststrip_preview")?;
    harness.state_mut().run(crate::app::Command::Escape);
    settle(harness, 2);
    Ok(())
}
// --- end U3 ---

/// `<stem><suffix>.<ext>` next to `path`.
pub fn sibling(path: &Path, suffix: &str) -> PathBuf {
    let stem = path
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    let ext = path
        .extension()
        .map(|e| e.to_string_lossy().into_owned())
        .unwrap_or_else(|| "png".into());
    path.with_file_name(format!("{stem}{suffix}.{ext}"))
}
