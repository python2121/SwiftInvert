//! `--selftest out.png [folder]` — the whole shell rendered offscreen through `egui_kittest`
//! (egui-wgpu into a texture, read back as a PNG): open the folder, let the first RAW decode,
//! analyse and render, then screenshot. Sidecars are never written here (the Qt shell's rule).
//! This is how the shell is verified without a window: run it, then LOOK at the PNG.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use crate::app::{AppOptions, NegApp};

/// Offscreen window size (points) and scale.
pub const SELFTEST_SIZE: [f32; 2] = [1440.0, 900.0];

/// Run the selftest: returns the written PNG's pixel size.
pub fn run_selftest(out: &Path, folder: PathBuf, timeout: Duration) -> Result<[u32; 2], String> {
    if !folder.is_dir() {
        return Err(format!("{} is not a folder", folder.display()));
    }
    let options = AppOptions {
        initial_folder: Some(folder),
        autosave: false,
        persist: false,
        renderer: None,
        presenter: None,
    };
    let mut harness = egui_kittest::Harness::builder()
        .with_size(egui::vec2(SELFTEST_SIZE[0], SELFTEST_SIZE[1]))
        .with_pixels_per_point(1.0)
        .with_max_steps(100_000)
        .build_eframe(|cc| NegApp::new(cc, options));
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
    Ok(size)
}

/// `<stem><suffix>.<ext>` next to `path`.
fn sibling(path: &Path, suffix: &str) -> PathBuf {
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
