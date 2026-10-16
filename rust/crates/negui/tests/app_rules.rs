//! Review-8 fixes, exercised through the real app and engine over a FAKE decoder (a small
//! synthetic negative, no RAW file needed) and the CPU renderer: sidecar flush on folder change,
//! named-action labels, worker panics, a failed decode reported once, modal key discipline,
//! the drag-counter release, the texture-slot guarantee, split prefs, the zone overlay while
//! View Original is held, and the off-thread folder browser.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use eframe::Storage as _;
use egui_kittest::kittest::Queryable;
use negkit::{ExposureSettings, RgbImage};
use negraw::{CfaKind, Decoded, NegRawError, RawBackend, RawInfoLite};
use negsession::renderer::{CpuRenderer, Renderer};
use negsession::{RenderRequest, SidecarStore, ToolMode};
use negui::app::Command;
use negui::engine::{Cmd, Engine, Evt};
use negui::{AppOptions, NegApp};

/// A decoder over a canned 96×64 negative-ish gradient. Paths whose name contains `boom` panic,
/// `bad` fail; every preview decode is counted.
#[derive(Default)]
struct FakeBackend {
    decodes: AtomicUsize,
}

fn synthetic() -> RgbImage {
    let (w, h) = (96usize, 64usize);
    let mut data = Vec::with_capacity(w * h * 3);
    for y in 0..h {
        for x in 0..w {
            let t = (x + y) as f32 / (w + h) as f32;
            data.extend_from_slice(&[0.55 + 0.3 * t, 0.35 + 0.25 * t, 0.2 + 0.2 * t]);
        }
    }
    RgbImage::from_vec(w, h, data).unwrap()
}

impl RawBackend for FakeBackend {
    fn name(&self) -> &'static str {
        "fake"
    }
    fn decode_preview(&self, path: &Path) -> negraw::Result<Decoded> {
        self.decodes.fetch_add(1, Ordering::SeqCst);
        let name = path.to_string_lossy();
        if name.contains("boom") {
            panic!("fake decoder exploded on {name}");
        }
        if name.contains("bad") {
            return Err(NegRawError::Corrupt("truncated".into()));
        }
        Ok(Decoded {
            image: synthetic(),
            exif_orientation_applied: true,
            is_xtrans: false,
            flip: 0,
        })
    }
    fn decode_full(&self, path: &Path) -> negraw::Result<Decoded> {
        self.decode_preview(path)
    }
    fn thumbnail(&self, _path: &Path) -> negraw::Result<Option<Vec<u8>>> {
        Ok(None)
    }
    fn probe(&self, _path: &Path) -> negraw::Result<RawInfoLite> {
        Ok(RawInfoLite {
            width: 96,
            height: 64,
            cfa: CfaKind::Bayer,
            flip: 0,
            make: None,
            model: None,
        })
    }
}

fn cpu() -> Arc<dyn Renderer> {
    Arc::new(CpuRenderer)
}

/// A fresh folder under the test tmp dir holding empty `.cr3` files with these stems.
fn folder(name: &str, stems: &[&str]) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join(name);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    for s in stems {
        std::fs::write(dir.join(format!("{s}.cr3")), b"not really a raw").unwrap();
    }
    dir
}

type H = egui_kittest::Harness<'static, NegApp>;

fn app(dir: PathBuf, backend: Arc<FakeBackend>, autosave: bool) -> H {
    let options = AppOptions {
        initial_folder: Some(dir),
        autosave,
        persist: false,
        renderer: Some(cpu()),
        presenter: None,
        backend: Some(backend),
    };
    egui_kittest::Harness::builder()
        .with_size(egui::vec2(1280.0, 800.0))
        .with_pixels_per_point(1.0)
        .with_max_steps(100_000)
        .build_eframe(|cc| NegApp::new(cc, options))
}

fn until(h: &mut H, what: &str, done: impl Fn(&NegApp) -> bool) {
    let t0 = Instant::now();
    while !done(h.state()) {
        assert!(t0.elapsed() < Duration::from_secs(60), "timed out: {what}");
        h.step();
        std::thread::sleep(Duration::from_millis(3));
    }
}

fn ready(h: &mut H) {
    until(h, "first frame", |a| a.selftest_ready());
    h.step();
}

// ───────────────────────────── [1] sidecar flush ─────────────────────────────

#[test]
fn opening_another_folder_flushes_the_pending_sidecar() {
    let a = folder("rules-flush-a", &["one"]);
    let b = folder("rules-flush-b", &["two"]);
    let mut h = app(a.clone(), Arc::default(), true);
    ready(&mut h);
    h.state_mut().settings_mut().exposure_stops = 0.5;
    h.step(); // settings didSet → save armed for +1 s on the app's clock (frozen here)
    let sidecar = a.join("one.cr3");
    assert!(
        SidecarStore::load(&sidecar).is_none(),
        "not saved yet (debounce)"
    );
    let ctx = h.ctx.clone();
    h.state_mut().open_folder_path(&ctx, b);
    let saved = SidecarStore::load(&sidecar).expect("flushed before the folder changed");
    assert_eq!(saved.exposure_stops, 0.5);
}

// ───────────────────────────── [2] named-action labels ─────────────────────────────

#[test]
fn no_op_named_actions_leave_no_label_armed() {
    let dir = folder("rules-labels", &["one"]);
    let mut h = app(dir, Arc::default(), false);
    ready(&mut h);
    assert_eq!(h.state().settings(), &ExposureSettings::default());
    // nothing to reset / clear: none of these may arm its label
    for cmd in [
        Command::ResetAll,
        Command::StartFromScratch,
        Command::ClearCropAndStraighten,
        Command::ClearAnalysis,
    ] {
        h.state_mut().run(cmd);
        h.step();
    }
    let before = h.state().history_len();
    h.state_mut().settings_mut().exposure_stops = 0.3;
    h.step();
    h.state_mut().advance_clock(Duration::from_secs(1));
    h.step();
    assert_eq!(h.state().history_len(), before + 1);
    assert_eq!(
        h.state().current_history_label().as_deref(),
        Some("Exposure +0.30"),
        "the slider's own label, not \"Reset all\""
    );
    // a Reset All that does change something keeps its label
    h.state_mut().run(Command::ResetAll);
    h.step();
    h.state_mut().advance_clock(Duration::from_secs(1));
    h.step();
    assert_eq!(
        h.state().current_history_label().as_deref(),
        Some("Reset all")
    );
}

// ───────────────────────────── [6] drag counter ─────────────────────────────

#[test]
fn a_lost_drag_end_is_released_with_the_pointer() {
    let dir = folder("rules-drag", &["one"]);
    let mut h = app(dir, Arc::default(), false);
    ready(&mut h);
    let before = h.state().history_len();
    // DragBegin whose DragEnd never comes (the row was disabled mid-drag)
    h.state_mut().control_editing(true);
    h.step(); // no button is down: the session ends here
    h.state_mut().settings_mut().grade = 95.0;
    h.step();
    h.state_mut().advance_clock(Duration::from_millis(800));
    h.step();
    assert_eq!(
        h.state().history_len(),
        before + 1,
        "the 0.7 s commit still fires"
    );
    assert_eq!(
        h.state().current_history_label().as_deref(),
        Some("Grade 95")
    );
}

// ───────────────────────────── [4][5] modal key discipline ─────────────────────────────

#[test]
fn escape_closes_the_dialog_first_and_keys_wait_for_it() {
    let dir = folder("rules-keys", &["a", "b"]);
    let mut h = app(dir.clone(), Arc::default(), false);
    ready(&mut h);
    let first = h.state().selection().map(Path::to_path_buf);
    assert_eq!(first.as_deref(), Some(dir.join("a.cr3").as_path()));
    h.state_mut().arm_placement_zone(5.0);
    assert!(h.state().zone_placement_active());
    h.state_mut().run(Command::Export);
    h.step();
    assert!(h.state().modal_open());

    // blocked while the sheet is up
    h.key_press(egui::Key::ArrowRight);
    h.step();
    h.key_press_modifiers(egui::Modifiers::SHIFT, egui::Key::Z);
    h.step();
    h.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::K);
    h.step();
    h.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::CloseBracket);
    h.step();
    assert_eq!(h.state().selection(), first.as_deref(), "arrows blocked");
    assert!(!h.state().zone_overlay_on(), "⇧Z blocked");
    assert_eq!(h.state().tool_mode(), ToolMode::None, "⌘K blocked");
    assert_eq!(h.state().settings().rotation, 0, "⌘] blocked");
    assert!(h.state().modal_open());

    // Escape closes the sheet, not the placement behind it
    h.key_press(egui::Key::Escape);
    h.step();
    assert!(!h.state().modal_open());
    assert!(h.state().zone_placement_active());
    // the next Escape reaches the canvas layer
    h.key_press(egui::Key::Escape);
    h.step();
    assert!(!h.state().zone_placement_active());
    // and keys act again
    h.key_press(egui::Key::ArrowRight);
    h.step();
    assert_eq!(h.state().selection(), Some(dir.join("b.cr3").as_path()));
}

// ───────────────────────────── [12] zone overlay vs View Original ─────────────────────────────

#[test]
fn the_zone_overlay_hides_while_view_original_is_held() {
    let dir = folder("rules-zones", &["one"]);
    let mut h = app(dir, Arc::default(), false);
    ready(&mut h);
    h.state_mut().run(Command::ToggleZones);
    h.step();
    assert!(h.state().zone_overlay_shown());
    h.state_mut().view_original(true);
    assert!(!h.state().zone_overlay_shown());
    h.state_mut().view_original(false);
    assert!(h.state().zone_overlay_shown());
}

// ───────────────────────────── [15] failed decode reported once ─────────────────────────────

#[test]
fn a_raw_that_fails_to_decode_is_reported_once() {
    let dir = folder("rules-bad", &["bad"]);
    let backend = Arc::new(FakeBackend::default());
    let mut h = app(dir, Arc::clone(&backend), false);
    until(&mut h, "error pill", |a| a.render_error().is_some());
    for _ in 0..20 {
        h.step();
        std::thread::sleep(Duration::from_millis(5));
    }
    let pill = h.state().render_error().unwrap_or_default().to_owned();
    assert!(
        pill.starts_with("Couldn't open bad.cr3: ") && pill.contains("truncated"),
        "{pill}"
    );
    assert!(!h.state().busy(), "no Developing… forever");
    assert_eq!(backend.decodes.load(Ordering::SeqCst), 1, "decoded once");
}

// ───────────────────────────── [3] worker panics ─────────────────────────────

fn render_cmd(path: &Path, generation: u64) -> Cmd {
    Cmd::Render {
        path: path.to_path_buf(),
        settings: ExposureSettings::default(),
        variant: None,
        tool: ToolMode::None,
        request: RenderRequest {
            srgb: true,
            ..RenderRequest::default()
        },
        generation,
        requested_at: Instant::now(),
    }
}

fn open_cmd(path: &Path, generation: u64) -> Cmd {
    Cmd::Open {
        path: path.to_path_buf(),
        settings: ExposureSettings::default(),
        generation,
    }
}

fn wait_for(engine: &mut Engine, mut done: impl FnMut(&Evt) -> bool) -> Vec<Evt> {
    let t0 = Instant::now();
    let mut seen = Vec::new();
    loop {
        for e in engine.poll() {
            let hit = done(&e);
            seen.push(e);
            if hit {
                return seen;
            }
        }
        assert!(t0.elapsed() < Duration::from_secs(30), "no event: {seen:?}");
        std::thread::sleep(Duration::from_millis(2));
    }
}

#[test]
fn a_panicking_command_becomes_an_error_and_the_worker_keeps_serving() {
    let backend = Arc::new(FakeBackend::default());
    let mut engine = Engine::spawn(backend, cpu(), || {});
    let boom = PathBuf::from("/nowhere/boom.cr3");
    engine.send(open_cmd(&boom, 1));
    let seen = wait_for(&mut engine, |e| matches!(e, Evt::Opened { .. }));
    match seen.last() {
        Some(Evt::Opened { result: Err(e), .. }) => {
            assert!(e.contains("engine error") && e.contains("exploded"), "{e}")
        }
        other => panic!("expected a failed open, got {other:?}"),
    }
    engine.send(render_cmd(&boom, 2));
    let seen = wait_for(&mut engine, |e| {
        matches!(e, Evt::RenderFailed { .. } | Evt::Rendered { .. })
    });
    assert!(matches!(seen.last(), Some(Evt::RenderFailed { .. })));
    // the next image still renders
    let good = PathBuf::from("/nowhere/good.cr3");
    engine.send(open_cmd(&good, 3));
    engine.send(render_cmd(&good, 4));
    let seen = wait_for(&mut engine, |e| {
        matches!(e, Evt::RenderFailed { .. } | Evt::Rendered { .. })
    });
    match seen.last() {
        Some(Evt::Rendered { output, .. }) => assert!(output.width > 0),
        other => panic!("expected a frame, got {other:?}"),
    }
    assert!(engine.stopped().is_none());
}

#[test]
fn a_failed_open_is_not_decoded_again_by_its_render() {
    let backend = Arc::new(FakeBackend::default());
    let mut engine = Engine::spawn(Arc::clone(&backend) as Arc<dyn RawBackend>, cpu(), || {});
    let bad = PathBuf::from("/nowhere/bad.cr3");
    engine.send(open_cmd(&bad, 1));
    engine.send(render_cmd(&bad, 2));
    let seen = wait_for(&mut engine, |e| matches!(e, Evt::RenderFailed { .. }));
    assert!(
        seen.iter()
            .any(|e| matches!(e, Evt::Opened { result: Err(_), .. }))
    );
    assert_eq!(backend.decodes.load(Ordering::SeqCst), 1);
    // a new Open retries
    engine.send(open_cmd(&bad, 3));
    wait_for(&mut engine, |e| matches!(e, Evt::Opened { .. }));
    assert_eq!(backend.decodes.load(Ordering::SeqCst), 2);
}

// ───────────────────────────── [7] texture slots ─────────────────────────────

/// The worker leases, the UI presents (keeping the shown frame's lease) — however far the UI
/// lags, the worker is never handed the slot on screen or one still in flight.
#[test]
fn the_worker_never_writes_a_displayed_or_queued_slot() {
    use negui::presenter_gpu::{RING, SlotLease, SlotPool};
    let pool = SlotPool::new();
    let mut displayed: Option<SlotLease> = None;
    let mut in_flight: std::collections::VecDeque<SlotLease> = Default::default();
    let mut fallbacks = 0;
    // a deterministic schedule of renders and UI polls (the UI sometimes lags by several)
    for step in 0..400u32 {
        let renders = 1 + (step * 7 % 3);
        for _ in 0..renders {
            match pool.acquire() {
                Some(lease) => {
                    let slot = lease.slot();
                    assert!(slot < RING);
                    assert_ne!(Some(slot), displayed.as_ref().map(SlotLease::slot));
                    assert!(in_flight.iter().all(|l| l.slot() != slot));
                    in_flight.push_back(lease);
                }
                None => fallbacks += 1, // readback path, never a busy slot
            }
        }
        let polls = step * 5 % 4;
        for _ in 0..polls {
            if let Some(next) = in_flight.pop_front() {
                // stale frames are sometimes discarded instead of shown
                if step % 9 == 0 {
                    drop(next);
                } else {
                    displayed = Some(next); // the previous lease drops: its slot frees
                }
            }
        }
        assert!(
            displayed
                .iter()
                .chain(in_flight.iter())
                .all(|l| pool.is_busy(l.slot()))
        );
    }
    assert!(fallbacks > 0, "the schedule exercised a full ring");
    drop(in_flight);
    let shown = displayed.as_ref().map(SlotLease::slot);
    for s in 0..RING {
        assert_eq!(pool.is_busy(s), Some(s) == shown);
    }
}

// ───────────────────────────── [11] split prefs ─────────────────────────────

#[derive(Default)]
struct MemStorage(std::collections::HashMap<String, String>);

impl eframe::Storage for MemStorage {
    fn get_string(&self, key: &str) -> Option<String> {
        self.0.get(key).cloned()
    }
    fn set_string(&mut self, key: &str, value: String) {
        self.0.insert(key.to_owned(), value);
    }
    fn remove_string(&mut self, key: &str) {
        self.0.remove(key);
    }
    fn flush(&mut self) {}
}

#[test]
fn a_corrupt_ui_prefs_blob_leaves_profiles_and_export_intact() {
    use negsession::{ExportFormat, ProfileStore};
    let mut store = ProfileStore::new();
    let id = negui::dialogs::profiles::create_profile(
        &mut store,
        "Portra",
        &ExposureSettings {
            exposure_stops: 0.7,
            ..ExposureSettings::default()
        },
    )
    .unwrap();
    store.set_active(&id);
    let mut prefs = negui::AppPrefs {
        profiles: store.to_json().unwrap(),
        library_visible: false,
        ..Default::default()
    };
    prefs.export.format = ExportFormat::Tiff16;
    let mut storage = MemStorage::default();
    prefs.store(&mut storage);
    // one bad field in the UI blob
    storage.set_string(
        negui::app::PREFS_KEY,
        "(library_visible: false, library_width: \"wide\")".into(),
    );
    let back = negui::AppPrefs::load(&storage);
    assert!(back.library_visible, "the UI group fell back to defaults");
    let restored = ProfileStore::from_json(&back.profiles);
    assert_eq!(restored.active_id(), id, "profiles survived");
    assert_eq!(back.export.format, ExportFormat::Tiff16, "export survived");

    // a store written before the split: export + profiles inside the UI blob
    #[derive(serde::Serialize)]
    struct OldPrefs {
        library_visible: bool,
        export: negsession::ExportOptions,
        profiles: String,
    }
    let mut legacy = MemStorage::default();
    eframe::set_value(
        &mut legacy,
        negui::app::PREFS_KEY,
        &OldPrefs {
            library_visible: false,
            export: prefs.export.clone(),
            profiles: store.to_json().unwrap(),
        },
    );
    let back = negui::AppPrefs::load(&legacy);
    assert!(!back.library_visible);
    assert_eq!(back.export.format, ExportFormat::Tiff16);
    assert_eq!(ProfileStore::from_json(&back.profiles).active_id(), id);
}

// ───────────────────────────── [9] folder browser ─────────────────────────────

#[test]
fn the_folder_browser_lists_off_the_ui_thread_once_per_path() {
    let root = folder("rules-browser", &[]);
    std::fs::create_dir_all(root.join("film-02")).unwrap();
    std::fs::create_dir_all(root.join("film-10")).unwrap();
    std::fs::create_dir_all(root.join(".hidden")).unwrap();
    let browser = negui::dialogs::FolderBrowser::new(Some(&root));
    let mut harness = egui_kittest::Harness::builder()
        .with_size(egui::vec2(500.0, 400.0))
        .build_ui_state(
            |ui, b: &mut negui::dialogs::FolderBrowser| {
                let _ = b.ui(ui, "Use This Folder");
            },
            browser,
        );
    let t0 = Instant::now();
    while !harness.state().listing_ready() {
        assert!(t0.elapsed() < Duration::from_secs(10));
        harness.step();
        std::thread::sleep(Duration::from_millis(2));
    }
    harness.run();
    let _ = harness.get_by_label("▸ film-02");
    let _ = harness.get_by_label("▸ film-10");
    assert!(harness.query_by_label("▸ .hidden").is_none());
    // navigating re-lists; the old listing is not reused
    harness.state_mut().text = root.join("film-02").display().to_string();
    harness.step();
    let t0 = Instant::now();
    while !harness.state().listing_ready() {
        assert!(t0.elapsed() < Duration::from_secs(10));
        harness.step();
        std::thread::sleep(Duration::from_millis(2));
    }
    harness.run();
    assert!(harness.query_by_label("▸ film-10").is_none());
    assert_eq!(harness.state().current(), root.join("film-02"));
}
