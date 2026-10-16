//! `NegApp` — the `AppModel` + window glue (session-spec §6, controls-spec §8–§14): selection
//! and multi-selection, the settings mirror (a change → coalesced latest-wins render + 1 s
//! debounced sidecar save + history commit rules), tool-mode enum (the tools themselves are a
//! later wave), baseline hold, HQ mode with the 2× zoom threshold and 350 ms upward debounce,
//! per-image history with drag-commits-on-release and the 0.7 s coalesce, sticky prefs through
//! eframe storage, the key map and the menu bar.
//!
//! Seams for later waves: [`ToolMode`] is carried (renders go through
//! `Session::render_tool_preview`, so entering a tool already widens the render) but no overlay
//! draws a box yet; `Command::Export` and `ChooseDefaults` set a status line; the control
//! channel would drive [`NegApp::run`] with [`Command`]s. The darkroom layers (test strip, zone
//! placement, densitometer + zone overlay, Interactive Histogram) are wired in the `U3` block.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use egui::{Key, KeyboardShortcut, Modifiers, Rect, pos2, vec2};
use negkit::ExposureSettings;
use negsession::history::{PendingCommit, labels};
use negsession::hq::{HQ_AUTO_ZOOM_THRESHOLD, HQ_UPWARD_DEBOUNCE_MS};
use negsession::renderer::Renderer;
use negsession::session::baseline_settings;
use negsession::{
    History, HqMode, ProfileStore, RawBackend, RenderRequest, RenderTier, SessionInfo,
    SettingsGeometry, SidecarStore, ToolMode,
};

use crate::controls::{SidebarAction, SidebarInput, SidebarPrefs, SidebarState, controls_sidebar};
use crate::engine::{Cmd, Engine, Evt};
use crate::library::{LibraryAction, LibraryState, library_panel};
use crate::loupe::{
    BarAction, BarInput, CanvasAction, CanvasColor, CanvasInput, FramePresenter, LoupeState,
    canvas, control_bar,
};
use crate::theme::Tokens;
// --- U2: tools & dialogs ---
use crate::dialogs::{DialogAction, DialogState};
use crate::engine::ExportItem;
use crate::tools::{self, AnalysisTool, CropTool, ToolAction, ToolState};
use negsession::ExportOptions;
// --- end U2 ---
// --- U3: darkroom ---
use crate::engine::{DarkroomCmd, DarkroomEvt, PinOp};
use crate::interactive_histogram::{LevelsAction, LevelsWindow};
use crate::loupe::CanvasOverlay;
use crate::teststrip::{StripAction, TestStripController};
use crate::zone_placement::{PlacementAction, ZonePlacementController};
use crate::zones::{Readout, ZoneOverlayLayer, ZonesState};
// --- end U3 ---

/// Sidecar save debounce (AppModel.swift:928).
pub const SAVE_DEBOUNCE: Duration = Duration::from_secs(1);
/// eframe storage key for the UI part of [`AppPrefs`] (RON).
pub const PREFS_KEY: &str = "swiftinvert.prefs";
/// eframe storage key for the sticky export options (RON), decoded on its own.
pub const EXPORT_KEY: &str = "swiftinvert.export";
/// eframe storage key for the profile store (its JSON blob, stored verbatim).
pub const PROFILES_KEY: &str = "swiftinvert.profiles";
/// Library splitter range (SwiftInvertApp.swift:332-352).
pub const LIBRARY_WIDTH_RANGE: std::ops::RangeInclusive<f32> = 200.0..=560.0;

/// Persisted preferences (controls-spec §13).
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct AppPrefs {
    pub library_folder: Option<PathBuf>,
    pub canvas_color: CanvasColor,
    pub library_visible: bool,
    pub library_width: f32,
    pub sidebar: SidebarPrefs,
    /// Collapsed folder identities per root (Qt: `collapsed/<root>`).
    pub collapsed: HashMap<String, Vec<String>>,
    // --- U2: tools & dialogs ---
    /// Sticky export sheet (`UserDefaults["exportOptions"]`, Swift field semantics). Stored
    /// under [`EXPORT_KEY`], not in the UI blob.
    #[serde(skip)]
    pub export: ExportOptions,
    /// User profiles + the active id, as the engine's JSON blob (`settingsProfiles` /
    /// `activeProfileID`; parsed leniently by `ProfileStore::from_json`). A JSON string rather
    /// than nested RON: `ExposureSettings`' lenient decoder reads through `serde_json::Value`,
    /// which RON's struct notation does not round-trip. Stored under [`PROFILES_KEY`].
    #[serde(skip)]
    pub profiles: String,
    // --- end U2 ---
    // --- U3: darkroom ---
    /// `@AppStorage("showZoneOverlay")` (default off).
    pub show_zone_overlay: bool,
    // --- end U3 ---
}

impl Default for AppPrefs {
    fn default() -> Self {
        Self {
            library_folder: None,
            canvas_color: CanvasColor::VeryDarkGray,
            library_visible: true,
            library_width: 320.0,
            sidebar: SidebarPrefs::default(),
            collapsed: HashMap::new(),
            // --- U2: tools & dialogs ---
            export: crate::dialogs::export::default_export_options(),
            profiles: String::new(),
            // --- U3: darkroom ---
            show_zone_overlay: false,
            // --- end U3 ---
        }
    }
}

/// The export options and profiles as an older build stored them: inside the UI blob.
/// (Fields read bare, as they were written — RON spells an `Option` field `Some(..)`.)
#[derive(Default, serde::Deserialize)]
#[serde(default)]
struct LegacyPrefs {
    #[serde(deserialize_with = "present")]
    export: Option<ExportOptions>,
    #[serde(deserialize_with = "present")]
    profiles: Option<String>,
}

fn present<'de, D, T>(d: D) -> Result<Option<T>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: serde::Deserialize<'de>,
{
    T::deserialize(d).map(Some)
}

impl AppPrefs {
    /// Read the three groups (UI prefs, export options, profiles) from their own keys, each
    /// decoded independently with its own fallback — a bad UI field can't reset the profile
    /// list or the export sheet, and the next save can't overwrite them with defaults. A store
    /// written before the split falls back to the export/profiles fields of the old blob.
    pub fn load(storage: &dyn eframe::Storage) -> AppPrefs {
        let mut prefs: AppPrefs = eframe::get_value(storage, PREFS_KEY).unwrap_or_default();
        let legacy: LegacyPrefs = eframe::get_value(storage, PREFS_KEY).unwrap_or_default();
        prefs.export = eframe::get_value::<ExportOptions>(storage, EXPORT_KEY)
            .or(legacy.export)
            .map(crate::dialogs::export::sanitize)
            .unwrap_or_else(crate::dialogs::export::default_export_options);
        prefs.profiles = storage
            .get_string(PROFILES_KEY)
            .or(legacy.profiles)
            .unwrap_or_default();
        prefs
    }

    /// Write the three groups under their own keys.
    pub fn store(&self, storage: &mut dyn eframe::Storage) {
        eframe::set_value(storage, PREFS_KEY, self);
        eframe::set_value(storage, EXPORT_KEY, &self.export);
        storage.set_string(PROFILES_KEY, self.profiles.clone());
    }
}

/// Construction options.
pub struct AppOptions {
    /// Folder to open at launch (else the sticky folder).
    pub initial_folder: Option<PathBuf>,
    /// Write sidecars (off under `--selftest`, like the Qt shell).
    pub autosave: bool,
    /// Read/write eframe storage.
    pub persist: bool,
    /// Renderer override; `None` shares eframe's wgpu device when there is one, else the default.
    pub renderer: Option<Arc<dyn Renderer>>,
    /// Frame presenter override; `None` = the CPU/readback presenter.
    pub presenter: Option<Box<dyn FramePresenter>>,
    /// RAW decoder override (tests); `None` = the build's default backend.
    pub backend: Option<Arc<dyn RawBackend>>,
}

impl Default for AppOptions {
    fn default() -> Self {
        Self {
            initial_folder: None,
            autosave: true,
            persist: true,
            renderer: None,
            presenter: None,
            backend: None,
        }
    }
}

/// Every action the menus, keys and (later) the control channel can issue.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Command {
    OpenFolder,
    ChooseDefaults,
    Export,
    ShowInFileManager,
    Undo,
    Redo,
    CopyAdjustments,
    PasteAdjustments,
    PasteToSelection,
    ResetAll,
    StartFromScratch,
    ToggleLibrary,
    ToggleGrid,
    ToggleZones,
    SetHq(HqMode),
    CycleHq,
    Previous,
    Next,
    RotateLeft,
    RotateRight,
    Flip,
    ToggleCrop,
    ClearCropAndStraighten,
    ToggleAnalysis,
    ClearAnalysis,
    Escape,
    TestStrip,
    // --- U2: tools & dialogs ---
    /// Commit the open tool (crop: box + angle; analysis: leave) — Return.
    ApplyTool,
    /// Leave the open tool discarding it (crop: restore the entry snapshot).
    CancelTool,
    // --- U3: darkroom ---
    /// Open (or raise) the Interactive Histogram window.
    InteractiveHistogram,
    // --- end U3 ---
}

const fn sc(m: Modifiers, k: Key) -> KeyboardShortcut {
    KeyboardShortcut::new(m, k)
}

const CMD_SHIFT: Modifiers = Modifiers {
    alt: false,
    ctrl: false,
    shift: true,
    mac_cmd: false,
    command: true,
};
const CMD_ALT: Modifiers = Modifiers {
    alt: true,
    ctrl: false,
    shift: false,
    mac_cmd: false,
    command: true,
};
const CMD_ALT_SHIFT: Modifiers = Modifiers {
    alt: true,
    ctrl: false,
    shift: true,
    mac_cmd: false,
    command: true,
};

/// Menu shortcuts (controls-spec §8); checked most-modifiers-first so ⌥⇧⌘V beats ⇧⌘V.
pub const SHORTCUTS: &[(KeyboardShortcut, Command)] = &[
    (sc(CMD_ALT_SHIFT, Key::V), Command::PasteToSelection),
    (sc(CMD_SHIFT, Key::Z), Command::Redo),
    (sc(CMD_SHIFT, Key::C), Command::CopyAdjustments),
    (sc(CMD_SHIFT, Key::V), Command::PasteAdjustments),
    (sc(CMD_SHIFT, Key::R), Command::ShowInFileManager),
    (sc(CMD_SHIFT, Key::L), Command::ToggleLibrary),
    (sc(CMD_SHIFT, Key::G), Command::ToggleGrid),
    (sc(CMD_SHIFT, Key::P), Command::CycleHq),
    (sc(CMD_SHIFT, Key::H), Command::Flip),
    (sc(CMD_SHIFT, Key::K), Command::ToggleAnalysis),
    (sc(CMD_ALT, Key::R), Command::ResetAll),
    (sc(Modifiers::COMMAND, Key::O), Command::OpenFolder),
    (sc(Modifiers::COMMAND, Key::E), Command::Export),
    (sc(Modifiers::COMMAND, Key::Z), Command::Undo),
    (
        sc(Modifiers::COMMAND, Key::OpenBracket),
        Command::RotateLeft,
    ),
    (
        sc(Modifiers::COMMAND, Key::CloseBracket),
        Command::RotateRight,
    ),
    (sc(Modifiers::COMMAND, Key::K), Command::ToggleCrop),
];

fn shortcut_for(cmd: Command) -> Option<KeyboardShortcut> {
    SHORTCUTS.iter().find(|(_, c)| *c == cmd).map(|(s, _)| *s)
}

/// `AppModel`'s HQ state machine (session-spec §5): the mode, whether HQ is ACTIVE, and the
/// 350 ms upward debounce (downward swaps are instant). Pure: the caller supplies `now`.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct HqController {
    pub mode: HqMode,
    pub active: bool,
    due: Option<Instant>,
    // --- U3: darkroom ---
    /// `canvasOwnedByProxyTool`: a test strip or zone placement owns the canvas (proxy renders
    /// only — HQ is suppressed).
    pub canvas_owned: bool,
    // --- end U3 ---
}

impl HqController {
    fn resolve(&self, zoom: f64, has_selection: bool, tool_active: bool) -> bool {
        self.mode.resolve(
            zoom,
            HQ_AUTO_ZOOM_THRESHOLD,
            has_selection,
            self.canvas_owned,
            tool_active,
        )
    }

    /// The tier a render should request now.
    pub fn tier(&self) -> RenderTier {
        self.mode.render_tier(self.active)
    }

    /// Re-resolve without debounce (selection / tool / mode change). Returns whether `active`
    /// changed.
    pub fn refresh(&mut self, zoom: f64, has_selection: bool, tool_active: bool) -> bool {
        self.due = None;
        let want = self.resolve(zoom, has_selection, tool_active);
        let changed = want != self.active;
        self.active = want;
        changed
    }

    /// `hqMode` didSet: set the mode and re-resolve immediately.
    pub fn set_mode(&mut self, mode: HqMode, zoom: f64, has_selection: bool, tool_active: bool) {
        self.mode = mode;
        self.refresh(zoom, has_selection, tool_active);
    }

    /// `canvasZoom` didSet: acts only in Auto and only on a threshold CROSSING — upward swaps
    /// are armed for [`HQ_UPWARD_DEBOUNCE_MS`], downward swaps happen now. Returns true when a
    /// render is needed immediately.
    pub fn on_zoom(
        &mut self,
        zoom: f64,
        has_selection: bool,
        tool_active: bool,
        now: Instant,
    ) -> bool {
        if self.mode != HqMode::Auto {
            return false;
        }
        let want = self.resolve(zoom, has_selection, tool_active);
        if want && !self.active {
            if self.due.is_none() {
                self.due = Some(now + Duration::from_millis(HQ_UPWARD_DEBOUNCE_MS));
            }
            false
        } else if !want {
            self.due = None;
            if self.active {
                self.active = false;
                true
            } else {
                false
            }
        } else {
            false
        }
    }

    /// Timer poll: fires the armed upward swap once due (and still wanted). Returns true when a
    /// render is needed.
    pub fn tick(
        &mut self,
        now: Instant,
        zoom: f64,
        has_selection: bool,
        tool_active: bool,
    ) -> bool {
        if self.due.is_some_and(|d| now >= d) {
            self.due = None;
            if self.resolve(zoom, has_selection, tool_active) && !self.active {
                self.active = true;
                return true;
            }
        }
        false
    }

    /// When the armed swap comes due.
    pub fn next_due(&self) -> Option<Instant> {
        self.due
    }
}

/// Rolling performance read-outs (status bar + report).
#[derive(Clone, Copy, Debug, Default)]
pub struct Perf {
    /// Exponential moving average of `App::ui` wall time.
    pub ui_ms: f32,
    /// Last engine render time (decode/analysis included when they ran).
    pub render_ms: f32,
    /// Last request → result latency.
    pub latency_ms: f32,
    pub frames: u64,
}

/// The app.
pub struct NegApp {
    engine: Engine,
    library: LibraryState,
    sidebar: SidebarState,
    loupe: LoupeState,
    presenter: Box<dyn FramePresenter>,
    prefs: AppPrefs,
    profiles: ProfileStore,
    autosave: bool,
    persist: bool,

    selection: Option<PathBuf>,
    multi: HashSet<PathBuf>,
    settings: ExposureSettings,
    last_settings: ExposureSettings,
    is_restoring_settings: bool,
    is_navigating_history: bool,
    tool_mode: ToolMode,
    baseline_held: bool,
    hq: HqController,
    canvas_zoom: f32,

    generation: u64,
    open_generation: u64,
    developing: bool,
    analyzing: bool,
    render_error: Option<String>,
    histogram: Option<Box<negkit::reference::Histogram256>>,
    density_range: f64,
    info: Option<SessionInfo>,

    histories: HashMap<PathBuf, History>,
    pending: PendingCommit,
    /// Added to `Instant::now()` for every timer (sidecar debounce, history coalesce, HQ
    /// debounce) — tests advance it instead of sleeping.
    clock_offset: Duration,
    save_due: Option<Instant>,
    status_message: Option<String>,
    copied: Option<ExposureSettings>,
    hover_uv: Option<(f32, f32)>,

    folder_dialog: Option<FolderDialog>,
    perf: Perf,
    frames_rendered: u64,
    open_failed: Option<String>,
    // --- U2: tools & dialogs ---
    tools: ToolState,
    dialogs: DialogState,
    export_generation: u64,
    /// The proxy readback that accompanies a zero-copy frame (the densitometer's bytes).
    probe: Option<Box<negsession::RenderOutput>>,

    // --- U3: darkroom ---
    zones: ZonesState,
    strip: TestStripController,
    placement: ZonePlacementController,
    levels: LevelsWindow,
    // --- end U3 ---
}

struct FolderDialog {
    browser: crate::dialogs::FolderBrowser,
}

impl NegApp {
    /// Build the app (theme, prefs, worker threads, initial folder).
    pub fn new(cc: &eframe::CreationContext<'_>, options: AppOptions) -> Self {
        crate::theme::install(&cc.egui_ctx);
        let prefs: AppPrefs = if options.persist {
            cc.storage.map(AppPrefs::load).unwrap_or_default()
        } else {
            AppPrefs::default()
        };
        let backend: Arc<dyn RawBackend> = options
            .backend
            .unwrap_or_else(|| Arc::from(negsession::default_backend()));
        let renderer = options.renderer.unwrap_or_else(|| {
            crate::presenter_gpu::shared_or_default_renderer(cc.wgpu_render_state.as_ref())
        });
        // --- U2: tools & dialogs --- the zero-copy presenter when the device is shared
        let (presenter, texture_sink) = match options.presenter {
            Some(p) => (p, None),
            None => {
                crate::presenter_gpu::choose_presenter(cc.wgpu_render_state.as_ref(), &renderer)
            }
        };
        // --- end U2 ---
        let ctx = cc.egui_ctx.clone();
        let engine = Engine::spawn(Arc::clone(&backend), renderer, move || {
            ctx.request_repaint()
        });
        // --- U2 ---
        if let Some(sink) = texture_sink {
            engine.send(Cmd::UseTextureTarget(sink));
        }
        // --- end U2 ---
        let ctx = cc.egui_ctx.clone();
        let library = LibraryState::new(Arc::clone(&backend), move || ctx.request_repaint());
        let mut app = NegApp {
            engine,
            library,
            sidebar: SidebarState {
                prefs: prefs.sidebar.clone(),
                ..Default::default()
            },
            loupe: LoupeState::default(),
            presenter,
            profiles: ProfileStore::from_json(&prefs.profiles),
            prefs,
            autosave: options.autosave,
            persist: options.persist,
            selection: None,
            multi: HashSet::new(),
            settings: ExposureSettings::default(),
            last_settings: ExposureSettings::default(),
            is_restoring_settings: false,
            is_navigating_history: false,
            tool_mode: ToolMode::None,
            baseline_held: false,
            hq: HqController::default(),
            canvas_zoom: 1.0,
            generation: 0,
            open_generation: 0,
            developing: false,
            analyzing: false,
            render_error: None,
            histogram: None,
            density_range: 0.0,
            info: None,
            histories: HashMap::new(),
            pending: PendingCommit::default(),
            clock_offset: Duration::ZERO,
            save_due: None,
            status_message: None,
            copied: None,
            hover_uv: None,
            folder_dialog: None,
            perf: Perf::default(),
            frames_rendered: 0,
            open_failed: None,
            tools: ToolState::default(),
            dialogs: DialogState::default(),
            export_generation: 0,
            probe: None,
            // --- U3: darkroom ---
            zones: ZonesState::default(),
            strip: TestStripController::default(),
            placement: ZonePlacementController::default(),
            levels: LevelsWindow::default(),
            // --- end U3 ---
        };
        let folder = options
            .initial_folder
            .or_else(|| app.prefs.library_folder.clone())
            .filter(|p| p.is_dir());
        if let Some(f) = folder {
            app.open_folder(&cc.egui_ctx, f, options.persist);
        }
        app
    }

    // ───────────────────────────── read-outs ─────────────────────────────

    /// A frame has been rendered, the scan is done and the selected thumbnail resolved — what
    /// `--selftest` waits for.
    pub fn selftest_ready(&self) -> bool {
        self.frames_rendered > 0
            && !self.library.scanning
            && self
                .selection
                .as_deref()
                .is_some_and(|p| self.library.thumbnail_settled(p))
    }

    /// Why a selftest cannot succeed (decode or render failure, nothing to open).
    pub fn selftest_failure(&self) -> Option<String> {
        if let Some(e) = &self.open_failed {
            return Some(e.clone());
        }
        if let Some(e) = &self.render_error {
            return Some(e.clone());
        }
        if !self.library.scanning && self.library.tree.is_none() && self.library.root.is_some() {
            return Some("no RAW files in the folder".to_owned());
        }
        None
    }

    pub fn perf(&self) -> Perf {
        self.perf
    }

    /// Frames presented so far.
    pub fn frames_rendered(&self) -> u64 {
        self.frames_rendered
    }

    /// Selftest/bench hook: a one-notch Exposure edit, as a slider tick would make it.
    pub fn bench_nudge(&mut self) {
        if self.selection.is_some() {
            self.settings.exposure_stops = (self.settings.exposure_stops + 0.1).min(2.0);
        }
    }

    pub fn settings(&self) -> &ExposureSettings {
        &self.settings
    }

    pub fn selection(&self) -> Option<&Path> {
        self.selection.as_deref()
    }

    pub fn hq_mode(&self) -> HqMode {
        self.hq.mode
    }

    /// Whether HQ is active right now.
    pub fn hq_active(&self) -> bool {
        self.hq.active
    }

    /// The app's clock (`Instant::now()` + the test offset).
    fn now(&self) -> Instant {
        Instant::now() + self.clock_offset
    }

    /// Test hook: move the app's clock forward (timers fire on the next frame).
    #[doc(hidden)]
    pub fn advance_clock(&mut self, by: Duration) {
        self.clock_offset += by;
    }

    /// Test hook: open `root` as the library folder (the Open Folder dialog's confirm).
    #[doc(hidden)]
    pub fn open_folder_path(&mut self, ctx: &egui::Context, root: PathBuf) {
        self.open_folder(ctx, root, false);
    }

    /// Test hook: a draggable control reports begin/end (`setControlEditing`).
    #[doc(hidden)]
    pub fn control_editing(&mut self, editing: bool) {
        self.set_control_editing(editing);
    }

    /// Test hook: the settings mirror (an edit through it behaves like a control change).
    #[doc(hidden)]
    pub fn settings_mut(&mut self) -> &mut ExposureSettings {
        &mut self.settings
    }

    /// The error pill text on the canvas, if any.
    pub fn render_error(&self) -> Option<&str> {
        self.render_error.as_deref()
    }

    /// Whether a modal dialog (Export, Profiles, Open Folder) is up.
    pub fn modal_open(&self) -> bool {
        self.dialogs.is_modal_open() || self.folder_dialog.is_some()
    }

    // ───────────────────────────── folder / selection ─────────────────────────────

    fn open_folder(&mut self, ctx: &egui::Context, root: PathBuf, remember: bool) {
        // The open frame's pending sidecar save belongs to it: write it before the selection
        // goes away (`save_now` writes nothing once `selection` is `None`).
        self.flush_save();
        let key = root.to_string_lossy().into_owned();
        let collapsed: HashSet<String> = self
            .prefs
            .collapsed
            .get(&key)
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .collect();
        let c = ctx.clone();
        self.library
            .open_folder(root.clone(), collapsed, move || c.request_repaint());
        self.engine.send(Cmd::ClearSessions);
        self.histories.clear();
        if remember {
            self.prefs.library_folder = Some(root);
        }
        self.selection = None;
        self.multi.clear();
        self.loupe.clear();
        self.presenter.clear();
        // --- U3: darkroom ---
        self.clear_darkroom_layers();
        self.zones.clear();
        // --- end U3 ---
    }

    fn select(&mut self, path: PathBuf, additive: bool, range: bool) {
        if range && let Some(cur) = self.selection.clone() {
            self.multi = self.library.range(&cur, &path).into_iter().collect();
            self.multi.insert(path.clone());
        } else if additive {
            if self.multi.len() > 1 && self.multi.contains(&path) {
                self.multi.remove(&path);
                if self.selection.as_deref() == Some(path.as_path())
                    && let Some(next) = self.multi.iter().next().cloned()
                {
                    self.open_selection(next);
                }
                return;
            }
            self.multi.insert(path.clone());
        } else {
            self.multi.clear();
            self.multi.insert(path.clone());
        }
        if self.selection.as_deref() != Some(path.as_path()) {
            self.open_selection(path);
        }
    }

    fn select_adjacent(&mut self, delta: isize) {
        if let Some(next) = self.library.adjacent(self.selection.as_deref(), delta) {
            self.select(next, false, false);
        }
    }

    /// `openSelection` (session-spec §6).
    fn open_selection(&mut self, path: PathBuf) {
        self.flush_save();
        self.generation += 1;
        self.open_generation = self.generation;
        self.tool_mode = ToolMode::None;
        self.baseline_held = false;
        self.loupe.clear();
        self.loupe.reset_zoom();
        self.canvas_zoom = 1.0;
        self.histogram = None;
        self.density_range = 0.0;
        self.info = None;
        self.render_error = None;
        self.open_failed = None;
        self.analyzing = false;
        self.hover_uv = None;
        // --- U3: darkroom ---
        self.clear_darkroom_layers();
        self.zones.clear();
        // --- end U3 ---
        self.multi.insert(path.clone());
        let restored =
            SidecarStore::load(&path).unwrap_or_else(|| self.profiles.default_settings());
        self.is_restoring_settings = true;
        self.settings = restored.clone();
        self.last_settings = restored.clone();
        self.histories
            .entry(path.clone())
            .or_insert_with(|| History::new(restored.clone()));
        self.pending.cancel();
        self.selection = Some(path.clone());
        self.hq.refresh(1.0, true, false);
        self.developing = true;
        self.engine.send(Cmd::Open {
            path: path.clone(),
            settings: restored,
            generation: self.generation,
        });
        self.schedule_render();
        self.is_restoring_settings = false;
        self.library.reveal(&path);
    }

    // ───────────────────────────── render / save / history plumbing ─────────────────────────────

    fn tool_active(&self) -> bool {
        self.tool_mode != ToolMode::None
    }

    fn render_tier(&self) -> RenderTier {
        self.hq.tier()
    }

    /// `scheduleRender`: one latest-wins request with the current state.
    fn schedule_render(&mut self) {
        let Some(path) = self.selection.clone() else {
            return;
        };
        self.generation += 1;
        let variant = self
            .baseline_held
            .then(|| baseline_settings(&self.settings));
        self.engine.send(Cmd::Render {
            path,
            settings: self.settings.clone(),
            variant,
            tool: self.tool_mode,
            request: RenderRequest {
                tier: self.render_tier(),
                uncropped: self.tool_mode != ToolMode::None,
                retain_full: self.hq.mode != HqMode::Off,
                srgb: true,
                ..RenderRequest::default()
            },
            generation: self.generation,
            requested_at: Instant::now(),
        });
    }

    /// `settings` didSet (session-spec §6).
    fn settings_changed(&mut self) {
        self.last_settings = self.settings.clone();
        // --- U3: darkroom --- any real edit clears the strip and the placement
        self.clear_darkroom_layers();
        // --- end U3 ---
        self.schedule_render();
        if !self.is_restoring_settings {
            if self.autosave {
                self.save_due = Some(self.now() + SAVE_DEBOUNCE);
            }
            if !self.is_navigating_history {
                self.pending.schedule(self.now());
            }
        }
    }

    fn flush_save(&mut self) {
        if self.save_due.take().is_some() {
            self.save_now();
        }
    }

    fn save_now(&mut self) {
        if !self.autosave {
            return;
        }
        if let Some(path) = &self.selection
            && let Err(e) = SidecarStore::save(&self.settings, path)
        {
            self.status_message = Some(format!("Couldn't save settings: {e}"));
        }
    }

    fn history(&mut self) -> Option<&mut History> {
        let path = self.selection.clone()?;
        self.histories.get_mut(&path)
    }

    fn commit_history(&mut self) {
        let settings = self.settings.clone();
        if let Some(h) = self.history() {
            h.commit(&settings);
        }
    }

    fn apply_navigated(&mut self, settings: ExposureSettings) {
        self.is_navigating_history = true;
        self.settings = settings;
        self.settings_changed();
        self.is_navigating_history = false;
    }

    fn undo(&mut self) {
        self.pending.cancel();
        let current = self.settings.clone();
        let next = self.history().and_then(|h| h.undo(&current));
        if let Some(s) = next {
            self.apply_navigated(s);
        }
    }

    fn redo(&mut self) {
        self.pending.cancel();
        let current = self.settings.clone();
        let next = self.history().and_then(|h| h.redo(&current));
        if let Some(s) = next {
            self.apply_navigated(s);
        }
    }

    fn jump_history(&mut self, index: usize) {
        self.pending.cancel();
        let next = self.history().and_then(|h| h.jump(index));
        if let Some(s) = next {
            self.apply_navigated(s);
        }
    }

    /// A named action: set `next` with `label` armed for its history entry — only when it
    /// changes something (an unchanged action must not leave the label armed for the NEXT,
    /// unrelated edit: "disarmed if nothing changed").
    fn set_labeled(&mut self, next: ExposureSettings, label: &str) {
        if next != self.settings {
            self.arm_label(label);
            self.settings = next;
        }
    }

    fn arm_label(&mut self, label: &str) {
        let label = label.to_owned();
        if let Some(h) = self.history() {
            h.set_pending_label(Some(label));
        }
    }

    fn set_control_editing(&mut self, editing: bool) {
        if self.pending.set_control_editing(editing) {
            self.commit_history();
        }
    }

    /// End every open drag session at once (pointer released) and commit like the last end.
    fn release_control_editing(&mut self) {
        let mut ended = false;
        while self.pending.is_dragging() {
            ended = self.pending.set_control_editing(false);
        }
        if ended {
            self.commit_history();
        }
    }

    fn set_hq_mode(&mut self, mode: HqMode) {
        let (zoom, has_sel, tool) = (
            f64::from(self.canvas_zoom),
            self.selection.is_some(),
            self.tool_active(),
        );
        // --- U3: darkroom --- (`hqMode` didSet clears strip + placement)
        self.clear_darkroom_layers();
        // --- end U3 ---
        self.hq.set_mode(mode, zoom, has_sel, tool);
        if mode == HqMode::Off
            && let Some(p) = self.selection.clone()
        {
            self.engine.send(Cmd::ReleaseEnhanced { path: p });
        }
        self.schedule_render();
    }

    fn on_zoom(&mut self, zoom: f32) {
        self.canvas_zoom = zoom;
        let (has_sel, tool) = (self.selection.is_some(), self.tool_active());
        if self.hq.on_zoom(f64::from(zoom), has_sel, tool, self.now()) {
            self.schedule_render();
        }
    }

    fn set_baseline(&mut self, on: bool) {
        if self.baseline_held != on {
            // --- U3: darkroom ---
            if on {
                self.clear_darkroom_layers();
            }
            // --- end U3 ---
            self.baseline_held = on;
            self.schedule_render();
        }
    }

    // ───────────────────────────── commands ─────────────────────────────

    /// Execute a command (menus, keys, control bar; the control channel later).
    pub fn run(&mut self, cmd: Command) {
        let has_sel = self.selection.is_some();
        match cmd {
            Command::OpenFolder => {
                self.folder_dialog = Some(FolderDialog {
                    browser: crate::dialogs::FolderBrowser::new(self.library.root.as_deref()),
                });
            }
            // --- U2: tools & dialogs ---
            Command::ChooseDefaults => self.dialogs.open_profiles(&self.profiles),
            Command::Export => {
                if has_sel {
                    self.open_export_sheet();
                }
            }
            Command::ApplyTool => self.apply_tool(),
            Command::CancelTool => self.cancel_tool(),
            // --- end U2 ---
            Command::ShowInFileManager => {
                if let Some(p) = self.selection.clone() {
                    show_in_file_manager(&p);
                }
            }
            Command::Undo => self.undo(),
            Command::Redo => self.redo(),
            Command::CopyAdjustments => {
                if has_sel {
                    self.copied = Some(self.settings.clone());
                    self.status_message = Some("Adjustments copied".into());
                }
            }
            Command::PasteAdjustments => {
                if let Some(src) = self.copied.clone()
                    && has_sel
                {
                    let next = src.keeping_geometry(&self.settings);
                    if next != self.settings {
                        self.arm_label(labels::PASTE_ADJUSTMENTS);
                        self.settings = next;
                    }
                }
            }
            Command::PasteToSelection => {
                if let Some(src) = self.copied.clone() {
                    let others: Vec<PathBuf> = self
                        .multi
                        .iter()
                        .filter(|p| Some(p.as_path()) != self.selection.as_deref())
                        .cloned()
                        .collect();
                    let mut n = 0;
                    for p in &others {
                        let target = SidecarStore::load(p)
                            .unwrap_or_else(|| self.profiles.default_settings());
                        if self.autosave
                            && SidecarStore::save(&src.keeping_geometry(&target), p).is_ok()
                        {
                            n += 1;
                        }
                    }
                    self.run(Command::PasteAdjustments);
                    self.status_message = Some(format!(
                        "Pasted adjustments to {} image(s)",
                        n + usize::from(has_sel)
                    ));
                }
            }
            Command::ResetAll => {
                if has_sel {
                    let next = self.profiles.default_settings();
                    self.set_labeled(next, labels::RESET_ALL);
                }
            }
            Command::StartFromScratch => {
                if has_sel {
                    self.set_labeled(ExposureSettings::default(), labels::START_FROM_SCRATCH);
                }
            }
            Command::ToggleLibrary => self.prefs.library_visible = !self.prefs.library_visible,
            Command::ToggleGrid => {
                self.sidebar.prefs.show_grid_lines = !self.sidebar.prefs.show_grid_lines
            }
            // --- U3: darkroom ---
            Command::ToggleZones => {
                if has_sel {
                    self.prefs.show_zone_overlay = !self.prefs.show_zone_overlay;
                    self.zones.refresh_grid(self.prefs.show_zone_overlay);
                }
            }
            Command::InteractiveHistogram => {
                if has_sel {
                    self.levels.open = true;
                }
            }
            // --- end U3 ---
            Command::SetHq(m) => {
                if has_sel {
                    self.set_hq_mode(m);
                }
            }
            Command::CycleHq => {
                if has_sel {
                    self.set_hq_mode(self.hq.mode.next());
                }
            }
            Command::Previous => self.select_adjacent(-1),
            Command::Next => self.select_adjacent(1),
            // --- U3: darkroom --- while a strip is up the rotate commands turn the LADDER
            Command::RotateLeft | Command::RotateRight if self.strip.owns_canvas() => {
                self.rotate_ladder(cmd == Command::RotateRight);
            }
            // --- end U3 ---
            Command::RotateLeft => {
                if has_sel {
                    let mut next = self.settings.clone();
                    next.rotation = (next.rotation - 90).rem_euclid(360);
                    self.set_labeled(next, labels::ROTATE_CCW);
                }
            }
            Command::RotateRight => {
                if has_sel {
                    let mut next = self.settings.clone();
                    next.rotation = (next.rotation + 90).rem_euclid(360);
                    self.set_labeled(next, labels::ROTATE_CW);
                }
            }
            Command::Flip => {
                if has_sel {
                    let mut next = self.settings.clone();
                    next.flip_horizontal = !next.flip_horizontal;
                    self.set_labeled(next, labels::FLIP_HORIZONTAL);
                }
            }
            // --- U2: tools & dialogs ---
            Command::ToggleCrop => {
                self.clear_darkroom_layers(); // --- U3: darkroom ---
                if has_sel {
                    self.toggle_crop_tool();
                }
            }
            Command::ToggleAnalysis => {
                self.clear_darkroom_layers(); // --- U3: darkroom ---
                if has_sel {
                    self.toggle_analysis_tool();
                }
            }
            // --- end U2 ---
            Command::ClearCropAndStraighten => {
                if has_sel {
                    let had_crop = self.settings.crop_rect.is_some();
                    let had_angle = self.settings.fine_rotation != 0.0;
                    let label = match (had_crop, had_angle) {
                        (true, true) => labels::CROP_AND_STRAIGHTEN_CLEARED,
                        (true, false) => labels::CROP_CLEARED,
                        _ => labels::STRAIGHTEN_CLEARED,
                    };
                    let mut next = self.settings.clone();
                    next.crop_rect = None;
                    next.fine_rotation = 0.0;
                    self.set_labeled(next, label);
                }
            }
            Command::ClearAnalysis => {
                if has_sel {
                    let mut next = self.settings.clone();
                    next.analysis_rect = None;
                    next.analysis_rect_fine_rotation = 0.0;
                    self.set_labeled(next, labels::ANALYSIS_REGION_CLEARED);
                }
            }
            Command::Escape => {
                // Modal dialogs first (the key monitor stands down while one is up), then the
                // test strip, the zone placement, and the tools.
                if self.dialogs.escape() {
                    return;
                }
                if self.folder_dialog.take().is_some() {
                    return;
                }
                // --- U3: darkroom --- the strip first, then the placement
                if self.strip.owns_canvas() {
                    self.clear_test_strip();
                    return;
                }
                if self.placement.is_active() {
                    self.clear_zone_placement();
                    return;
                }
                // --- end U3 ---
                // --- U2: tools & dialogs --- the crop tool cancels
                if self.tool_mode == ToolMode::Crop {
                    self.cancel_tool();
                    return;
                }
                // --- end U2 ---
                if self.tool_mode != ToolMode::None {
                    self.tool_mode = ToolMode::None;
                    self.hq.refresh(f64::from(self.canvas_zoom), true, false);
                    self.schedule_render();
                }
            }
            Command::TestStrip => self.toggle_test_strip(), // --- U3: darkroom ---
        }
    }

    // ───────────────────────────── per-frame ─────────────────────────────

    fn handle_events(&mut self, ctx: &egui::Context) {
        let events = self.engine.poll();
        if let Some(reason) = self.engine.stopped() {
            // Fatal: nothing renders any more — say so instead of "Developing…" forever.
            let pill = format!("engine stopped: {reason}");
            if self.render_error.as_deref() != Some(pill.as_str()) {
                self.render_error = Some(pill);
            }
            self.developing = false;
            self.analyzing = false;
        }
        for evt in events {
            match evt {
                Evt::Opened {
                    path,
                    generation,
                    result,
                } => {
                    if Some(path.as_path()) != self.selection.as_deref()
                        || generation < self.open_generation
                    {
                        continue;
                    }
                    if let Err(e) = result {
                        self.developing = false;
                        self.analyzing = false;
                        let name = path
                            .file_name()
                            .map(|n| n.to_string_lossy().into_owned())
                            .unwrap_or_else(|| path.display().to_string());
                        let message = format!("Couldn't open {name}: {e}");
                        log::warn!("{message}");
                        self.open_failed = Some(message.clone());
                        self.render_error = Some(message);
                    }
                }
                Evt::Analyzing { path, generation } => {
                    if Some(path.as_path()) == self.selection.as_deref()
                        && generation >= self.open_generation
                    {
                        self.analyzing = true;
                    }
                }
                Evt::Rendered {
                    path,
                    generation,
                    output,
                    info,
                    render_time,
                    latency,
                    gpu,
                } => {
                    if Some(path.as_path()) != self.selection.as_deref()
                        || generation < self.open_generation
                    {
                        continue;
                    }
                    let frame = self.presenter.present_frame(ctx, &output, gpu);
                    self.loupe.adopt(&output, frame);
                    self.histogram = output.histogram.map(Box::new);
                    self.density_range = output.density_range;
                    self.info = info;
                    self.developing = false;
                    self.analyzing = false;
                    self.render_error = None;
                    self.frames_rendered += 1;
                    self.perf.render_ms = render_time.as_secs_f32() * 1000.0;
                    self.perf.latency_ms = latency.as_secs_f32() * 1000.0;
                    log::debug!(
                        "render {:?} {}×{} in {:.1} ms (latency {:.1} ms)",
                        output.tier,
                        output.width,
                        output.height,
                        self.perf.render_ms,
                        self.perf.latency_ms
                    );
                    // --- U3: darkroom --- the densitometer adopts the bytes (moved, not copied)
                    // (U2: a zero-copy texture frame carries none — its proxy probe stands in)
                    let output = self.probe_for(output);
                    self.zones.adopt(output, self.prefs.show_zone_overlay);
                    // --- end U3 ---
                }
                // --- U2: tools & dialogs ---
                Evt::ProbeFrame {
                    path,
                    generation,
                    output,
                } => {
                    if Some(path.as_path()) == self.selection.as_deref()
                        && generation >= self.open_generation
                    {
                        self.probe = Some(output);
                    }
                }
                evt @ (Evt::ExportProgress { .. } | Evt::ExportFinished { .. }) => {
                    if let Some(status) = self.dialogs.on_engine_event(&evt) {
                        self.status_message = Some(status);
                    }
                }
                // --- end U2 ---
                // --- U3: darkroom ---
                Evt::Darkroom {
                    path,
                    generation,
                    event,
                } => {
                    if Some(path.as_path()) == self.selection.as_deref() {
                        self.on_darkroom_event(ctx, generation, event);
                    }
                }
                // --- end U3 ---
                Evt::RenderFailed {
                    path,
                    generation,
                    error,
                } => {
                    // After a failed open the pill already says why: the render of that
                    // frame reports the same failure and must not replace it.
                    if Some(path.as_path()) == self.selection.as_deref()
                        && generation >= self.open_generation
                        && self.open_failed.is_none()
                    {
                        self.developing = false;
                        self.analyzing = false;
                        self.render_error = Some(format!("Render failed: {error}"));
                    }
                }
                Evt::EngineError { message } => {
                    self.clear_darkroom_layers();
                    self.status_message = Some(message);
                }
            }
        }
        if self.library.poll(ctx) {
            let keep = self
                .selection
                .as_deref()
                .is_some_and(|p| self.library.contains(p));
            if !keep && let Some(first) = self.library.flat.first().cloned() {
                self.select(first, false, false);
            } else if !keep {
                self.flush_save();
                self.selection = None;
                self.multi.clear();
            }
        }
    }

    fn tick_timers(&mut self, ctx: &egui::Context) {
        // A drag session can't outlive the pointer: if a control lost its drag-end (its row was
        // disabled or rebuilt mid-drag), the release still ends the session and commits.
        if self.pending.is_dragging() && !ctx.input(|i| i.pointer.any_down()) {
            self.release_control_editing();
        }
        let now = self.now();
        if self.pending.is_due(now) {
            self.pending.cancel();
            self.commit_history();
        }
        if self.save_due.is_some_and(|d| now >= d) {
            self.save_due = None;
            self.save_now();
        }
        let (zoom, has_sel, tool) = (
            f64::from(self.canvas_zoom),
            self.selection.is_some(),
            self.tool_active(),
        );
        if self.hq.tick(now, zoom, has_sel, tool) {
            self.schedule_render();
        }
        let mut next: Option<Instant> = None;
        for d in [self.save_due, self.hq.next_due()].into_iter().flatten() {
            next = Some(next.map_or(d, |n: Instant| n.min(d)));
        }
        if self.pending.is_armed() {
            // PendingCommit keeps the due instant private; poll within the coalesce window.
            let d = now + Duration::from_millis(50);
            next = Some(next.map_or(d, |n| n.min(d)));
        }
        if let Some(n) = next {
            ctx.request_repaint_after(n.saturating_duration_since(now));
        }
    }

    /// The window key monitor. It stands down entirely while a text field has keyboard focus
    /// (⌘Z there edits the text, not the image) — the field's own Return/Escape act — and,
    /// while a modal dialog is up, it only routes Escape (which closes the dialog first).
    fn handle_keys(&mut self, ctx: &egui::Context) -> Vec<Command> {
        let mut cmds = Vec::new();
        if ctx.egui_wants_keyboard_input() {
            return cmds;
        }
        let modal = self.modal_open();
        ctx.input_mut(|i| {
            if i.consume_key(Modifiers::NONE, Key::Escape) {
                cmds.push(Command::Escape);
            }
            if modal {
                return;
            }
            for (s, c) in SHORTCUTS {
                if i.consume_shortcut(s) {
                    cmds.push(*c);
                }
            }
            {
                if i.consume_key(Modifiers::NONE, Key::ArrowLeft)
                    || i.consume_key(Modifiers::NONE, Key::ArrowUp)
                {
                    cmds.push(Command::Previous);
                }
                if i.consume_key(Modifiers::NONE, Key::ArrowRight)
                    || i.consume_key(Modifiers::NONE, Key::ArrowDown)
                {
                    cmds.push(Command::Next);
                }
                if i.consume_key(Modifiers::SHIFT, Key::Z) {
                    cmds.push(Command::ToggleZones);
                }
                if i.consume_key(Modifiers::SHIFT, Key::T) {
                    cmds.push(Command::TestStrip);
                }
            }
        });
        cmds
    }

    fn menu_bar(&mut self, ui: &mut egui::Ui) -> Vec<Command> {
        let mut cmds = Vec::new();
        let has_sel = self.selection.is_some();
        let has_files = !self.library.flat.is_empty();
        let (can_undo, can_redo, uncommitted) = self
            .selection
            .as_ref()
            .and_then(|p| self.histories.get(p))
            .map(|h| {
                (
                    h.can_undo(),
                    h.can_redo(),
                    h.has_uncommitted_edit(&self.settings),
                )
            })
            .unwrap_or((false, false, false));
        let item =
            |ui: &mut egui::Ui, label: &str, cmd: Command, enabled: bool, checked: bool| -> bool {
                let mut b = egui::Button::new(if checked {
                    format!("✓ {label}")
                } else {
                    label.to_owned()
                });
                if let Some(s) = shortcut_for(cmd) {
                    b = b.shortcut_text(ui.ctx().format_shortcut(&s));
                }
                let r = ui.add_enabled(enabled, b);
                if r.clicked() {
                    ui.close();
                    true
                } else {
                    false
                }
            };
        egui::MenuBar::new().ui(ui, |ui| {
            ui.menu_button("File", |ui| {
                if item(ui, "Open Folder…", Command::OpenFolder, true, false) {
                    cmds.push(Command::OpenFolder);
                }
                if item(
                    ui,
                    "Choose Default Settings…",
                    Command::ChooseDefaults,
                    true,
                    false,
                ) {
                    cmds.push(Command::ChooseDefaults);
                }
                ui.separator();
                if item(ui, "Export…", Command::Export, has_sel, false) {
                    cmds.push(Command::Export);
                }
                if item(
                    ui,
                    "Show in File Manager",
                    Command::ShowInFileManager,
                    has_sel,
                    false,
                ) {
                    cmds.push(Command::ShowInFileManager);
                }
            });
            ui.menu_button("Edit", |ui| {
                if item(
                    ui,
                    "Undo Edit",
                    Command::Undo,
                    can_undo || uncommitted,
                    false,
                ) {
                    cmds.push(Command::Undo);
                }
                if item(ui, "Redo Edit", Command::Redo, can_redo, false) {
                    cmds.push(Command::Redo);
                }
                ui.separator();
                if item(
                    ui,
                    "Copy Adjustments",
                    Command::CopyAdjustments,
                    has_sel,
                    false,
                ) {
                    cmds.push(Command::CopyAdjustments);
                }
                if item(
                    ui,
                    "Paste Adjustments",
                    Command::PasteAdjustments,
                    has_sel && self.copied.is_some(),
                    false,
                ) {
                    cmds.push(Command::PasteAdjustments);
                }
                if item(
                    ui,
                    "Paste Adjustments to Selection",
                    Command::PasteToSelection,
                    self.multi.len() > 1 && self.copied.is_some(),
                    false,
                ) {
                    cmds.push(Command::PasteToSelection);
                }
                ui.separator();
                if item(ui, "Reset All", Command::ResetAll, has_sel, false) {
                    cmds.push(Command::ResetAll);
                }
            });
            ui.menu_button("View", |ui| {
                if item(
                    ui,
                    "Show Library",
                    Command::ToggleLibrary,
                    true,
                    self.prefs.library_visible,
                ) {
                    cmds.push(Command::ToggleLibrary);
                }
                if item(
                    ui,
                    "Show Grid Lines",
                    Command::ToggleGrid,
                    true,
                    self.sidebar.prefs.show_grid_lines,
                ) {
                    cmds.push(Command::ToggleGrid);
                }
                if item(
                    ui,
                    "Zone Overlay",
                    Command::ToggleZones,
                    has_sel,
                    self.prefs.show_zone_overlay, // --- U3: darkroom ---
                ) {
                    cmds.push(Command::ToggleZones);
                }
                ui.separator();
                ui.label(egui::RichText::new("HQ Preview").weak());
                for (m, label) in [
                    (HqMode::Off, "Off"),
                    (HqMode::Auto, "Auto"),
                    (HqMode::On, "On"),
                ] {
                    if item(
                        ui,
                        &format!("    {label}"),
                        Command::SetHq(m),
                        has_sel,
                        self.hq.mode == m,
                    ) {
                        cmds.push(Command::SetHq(m));
                    }
                }
                if item(ui, "Cycle HQ Preview", Command::CycleHq, has_sel, false) {
                    cmds.push(Command::CycleHq);
                }
            });
            ui.menu_button("Image", |ui| {
                if item(ui, "Previous Image", Command::Previous, has_files, false) {
                    cmds.push(Command::Previous);
                }
                if item(ui, "Next Image", Command::Next, has_files, false) {
                    cmds.push(Command::Next);
                }
                ui.separator();
                if item(ui, "Rotate Left", Command::RotateLeft, has_sel, false) {
                    cmds.push(Command::RotateLeft);
                }
                if item(ui, "Rotate Right", Command::RotateRight, has_sel, false) {
                    cmds.push(Command::RotateRight);
                }
                if item(ui, "Flip Horizontal", Command::Flip, has_sel, false) {
                    cmds.push(Command::Flip);
                }
                ui.separator();
                if item(
                    ui,
                    "Crop or Straighten",
                    Command::ToggleCrop,
                    has_sel,
                    self.tool_mode == ToolMode::Crop,
                ) {
                    cmds.push(Command::ToggleCrop);
                }
                let geometry =
                    self.settings.crop_rect.is_some() || self.settings.fine_rotation.abs() > 1e-9;
                if item(
                    ui,
                    "Clear Crop & Straighten",
                    Command::ClearCropAndStraighten,
                    has_sel && geometry,
                    false,
                ) {
                    cmds.push(Command::ClearCropAndStraighten);
                }
                if item(
                    ui,
                    "Crop for Analysis",
                    Command::ToggleAnalysis,
                    has_sel,
                    self.tool_mode == ToolMode::AnalysisRegion,
                ) {
                    cmds.push(Command::ToggleAnalysis);
                }
                if item(
                    ui,
                    "Clear Analysis Region",
                    Command::ClearAnalysis,
                    has_sel && self.settings.analysis_rect.is_some(),
                    false,
                ) {
                    cmds.push(Command::ClearAnalysis);
                }
            });
        });
        cmds
    }

    fn folder_dialog_ui(&mut self, ctx: &egui::Context) {
        let Some(dialog) = &mut self.folder_dialog else {
            return;
        };
        let mut open = true;
        let mut chosen: Option<PathBuf> = None;
        let mut close = false;
        egui::Window::new("Open Folder")
            .open(&mut open)
            .collapsible(false)
            .resizable(true)
            .default_size(vec2(520.0, 420.0))
            .anchor(egui::Align2::CENTER_CENTER, vec2(0.0, 0.0))
            .show(ctx, |ui| {
                chosen = dialog.browser.ui(ui, "Open");
                ui.separator();
                if ui.button("Cancel").clicked() {
                    close = true;
                }
            });
        if let Some(c) = chosen {
            self.folder_dialog = None;
            self.open_folder(ctx, c, true);
        } else if close || !open {
            self.folder_dialog = None;
        }
    }

    fn persist_collapsed(&mut self) {
        if let Some(root) = &self.library.root {
            let key = root.to_string_lossy().into_owned();
            let mut v: Vec<String> = self.library.collapsed.iter().cloned().collect();
            v.sort();
            self.prefs.collapsed.insert(key, v);
        }
    }
}

// --- U3: darkroom ---
/// The darkroom layers (test strip, zone placement, densitometer + zone overlay, Interactive
/// Histogram) — `AppModel` session-spec §7–§9. The layer state lives in `teststrip`,
/// `zone_placement`, `zones` and `interactive_histogram`; this block is the app glue.
impl NegApp {
    /// Whether the strip or the placement owns the canvas (HQ suppressed, probe off).
    fn darkroom_owns_canvas(&self) -> bool {
        self.strip.owns_canvas() || self.placement.is_active()
    }

    /// Re-resolve HQ after the canvas owner changed (`refreshHQActive(debounced: false)`).
    fn refresh_canvas_owner(&mut self) {
        self.hq.canvas_owned = self.darkroom_owns_canvas();
        let (zoom, has_sel, tool) = (
            f64::from(self.canvas_zoom),
            self.selection.is_some(),
            self.tool_active(),
        );
        if self.hq.refresh(zoom, has_sel, tool) {
            self.schedule_render();
        }
    }

    /// Clear both canvas-owning layers.
    fn clear_darkroom_layers(&mut self) {
        let a = self.strip.clear();
        let b = self.placement.clear();
        if a || b {
            self.refresh_canvas_owner();
        }
    }

    fn clear_test_strip(&mut self) {
        if self.strip.clear() {
            self.refresh_canvas_owner();
        }
    }

    fn clear_zone_placement(&mut self) {
        if self.placement.clear() {
            self.refresh_canvas_owner();
        }
    }

    fn send_darkroom(&mut self, cmd: DarkroomCmd, generation: u64) {
        let Some(path) = self.selection.clone() else {
            return;
        };
        self.engine.send(Cmd::Darkroom {
            path,
            settings: self.settings.clone(),
            cmd,
            generation,
        });
    }

    /// Strip and placement are proxy renders: an explicit HQ On is demoted to Off first.
    fn demote_hq_on(&mut self) {
        if self.hq.mode == HqMode::On {
            self.set_hq_mode(HqMode::Off);
        }
    }

    /// `toggleTestStrip()`.
    fn toggle_test_strip(&mut self) {
        if self.strip.owns_canvas() {
            self.clear_test_strip();
            return;
        }
        if self.selection.is_none() || self.tool_active() || self.baseline_held {
            return;
        }
        self.clear_zone_placement();
        self.demote_hq_on();
        let generation = self.strip.begin_build(self.settings.clone());
        self.refresh_canvas_owner();
        let orientation = self.strip.orientation;
        self.send_darkroom(DarkroomCmd::TestStrip { orientation }, generation);
    }

    /// `rotateTestStripLadder`.
    fn rotate_ladder(&mut self, clockwise: bool) {
        self.strip.orientation = (self.strip.orientation + if clockwise { 1 } else { 3 }) % 4;
        self.clear_test_strip();
        self.toggle_test_strip();
    }

    /// `armZonePlacementTarget`.
    fn arm_zone(&mut self, zone: f64) {
        if self.selection.is_none() || self.tool_active() || self.baseline_held {
            return;
        }
        self.clear_test_strip();
        self.demote_hq_on();
        self.placement.arm(zone);
        self.refresh_canvas_owner();
    }

    fn resolve_zone_placement(&mut self) {
        let Some(st) = &mut self.placement.state else {
            return;
        };
        if st.pins.is_empty() {
            return;
        }
        st.solving = true;
        let pins = st.pins.clone();
        let generation = self.placement.next_generation();
        self.send_darkroom(DarkroomCmd::ZoneSolve { pins }, generation);
    }

    /// The probe meters the plain presentation only (no tool, no strip, no placement).
    fn probe_densitometer(&mut self, uv: Option<(f32, f32)>) {
        let active = !self.tool_active() && !self.baseline_held && !self.darkroom_owns_canvas();
        let uv = uv.filter(|_| active);
        self.zones.probe(uv, &self.loupe.content_window);
    }

    fn on_darkroom_event(&mut self, ctx: &egui::Context, generation: u64, event: DarkroomEvt) {
        match event {
            DarkroomEvt::TestStrip { result, build_time } => {
                if generation != self.strip.generation {
                    return;
                }
                self.strip.build_ms = build_time.as_secs_f32() * 1000.0;
                let snapshot = self.strip.snapshot.clone();
                match result {
                    Ok(out)
                        if snapshot.as_ref() == Some(&self.settings)
                            && !self.tool_active()
                            && !self.baseline_held =>
                    {
                        log::info!(
                            "test strip {}×{} built in {:.0} ms",
                            out.width,
                            out.height,
                            self.strip.build_ms
                        );
                        self.strip.adopt(ctx, &out, self.settings.clone());
                    }
                    Ok(_) => self.clear_test_strip(),
                    Err(e) => {
                        self.status_message = Some(format!("Test strip failed: {e}"));
                        self.clear_test_strip();
                    }
                }
            }
            DarkroomEvt::Preview { result } => {
                if generation != self.strip.generation {
                    return;
                }
                if let (Ok(out), Some((row, col))) = (result, self.strip.requested_preview.take()) {
                    self.strip.adopt_preview(ctx, row, col, &out);
                }
            }
            DarkroomEvt::ZoneSampled { op, result } => {
                if generation != self.placement.generation {
                    return;
                }
                if let Ok(pin) = result
                    && self.placement.apply_sample(op, pin)
                {
                    self.resolve_zone_placement();
                }
            }
            DarkroomEvt::ZoneSolved {
                labels,
                solution,
                preview,
            } => {
                if generation != self.placement.generation {
                    return;
                }
                self.placement
                    .adopt_solve(ctx, labels, solution.map(|s| *s), preview.as_deref());
            }
        }
    }

    /// Per-frame: the Interactive Histogram window and the layers' queued actions.
    fn darkroom_frame(&mut self, ctx: &egui::Context) {
        let enabled = self.selection.is_some();
        let actions = crate::interactive_histogram::show(
            ctx,
            &mut self.levels,
            &mut self.settings,
            self.histogram.as_deref(),
            enabled,
        );
        for a in actions {
            match a {
                LevelsAction::DragBegin => self.set_control_editing(true),
                LevelsAction::DragEnd => self.set_control_editing(false),
                LevelsAction::Label(l) => self.arm_label(&l),
            }
        }
        for a in self.strip.take_actions(ctx) {
            match a {
                StripAction::Preview { row, col } => {
                    let Some(strip) = &self.strip.strip else {
                        continue;
                    };
                    let Some(s) =
                        crate::teststrip::picked_settings(&strip.base, row, col, strip.orientation)
                    else {
                        continue;
                    };
                    self.strip.requested_preview = Some((row, col));
                    let generation = self.strip.generation;
                    if let Some(path) = self.selection.clone() {
                        self.engine.send(Cmd::Darkroom {
                            path,
                            settings: s,
                            cmd: DarkroomCmd::Preview,
                            generation,
                        });
                    }
                }
                StripAction::BackToGrid => self.strip.back_to_grid(),
                StripAction::Confirm { row, col } => {
                    let orientation = self.strip.strip.as_ref().map(|s| s.orientation);
                    if let Some(next) = orientation.and_then(|o| {
                        crate::teststrip::picked_settings(&self.settings, row, col, o)
                    }) {
                        if next != self.settings {
                            self.arm_label(labels::TEST_STRIP_PICK);
                            self.settings = next;
                        }
                        self.clear_test_strip();
                    }
                }
            }
        }
        for a in self.placement.take_actions() {
            match a {
                PlacementAction::AddPin { u, v } => {
                    let armed = self.placement.armed();
                    let generation = self.placement.next_generation();
                    self.send_darkroom(
                        DarkroomCmd::ZoneSample {
                            u,
                            v,
                            armed,
                            op: PinOp::Add,
                        },
                        generation,
                    );
                }
                PlacementAction::MovePin { index, u, v, last } => {
                    let generation = self.placement.next_generation();
                    self.send_darkroom(
                        DarkroomCmd::ZoneSample {
                            u,
                            v,
                            armed: None,
                            op: PinOp::Move { index, last },
                        },
                        generation,
                    );
                }
                PlacementAction::Apply => {
                    let next = self
                        .placement
                        .state
                        .as_ref()
                        .and_then(|s| s.solution.as_ref())
                        .map(|s| s.settings.clone());
                    if let Some(next) = next {
                        if next != self.settings {
                            self.arm_label(labels::ZONE_PLACEMENT);
                            self.settings = next;
                        }
                        self.clear_zone_placement();
                    }
                }
                PlacementAction::Clear => self.clear_zone_placement(),
            }
        }
    }

    // ── read-outs and hooks for the selftest / tests ──

    /// Whether the zone overlay is drawn: the preference, on the plain presentation only — no
    /// tool, and (like the probe) not while View Original shows the stock look.
    pub fn zone_overlay_shown(&self) -> bool {
        self.prefs.show_zone_overlay && self.tool_mode == ToolMode::None && !self.baseline_held
    }

    /// Test hook: hold / release View Original.
    #[doc(hidden)]
    pub fn view_original(&mut self, on: bool) {
        self.set_baseline(on);
    }

    /// The open image's history length.
    pub fn history_len(&self) -> usize {
        self.selection
            .as_ref()
            .and_then(|p| self.histories.get(p))
            .map_or(0, |h| h.entries().len())
    }

    /// Whether the zone overlay preference is on.
    pub fn zone_overlay_on(&self) -> bool {
        self.prefs.show_zone_overlay
    }

    /// The overlay grid is computed (the overlay is drawn on the next frame).
    pub fn zone_grid_ready(&self) -> bool {
        self.zones.grid.is_some()
    }

    /// Last zone-grid computation time (ms).
    pub fn zone_grid_ms(&self) -> f32 {
        self.zones.grid_ms
    }

    /// The current densitometer read-out.
    pub fn densitometer_readout(&self) -> Readout {
        Readout {
            reading: self.zones.densitometer.reading(),
            armed: self.placement.armed(),
        }
    }

    /// A test strip is on screen (not merely building).
    pub fn test_strip_up(&self) -> bool {
        self.strip.strip.is_some()
    }

    /// A test strip is showing or building.
    pub fn test_strip_owns_canvas(&self) -> bool {
        self.strip.owns_canvas()
    }

    /// Last test-strip build time (ms, worker side).
    pub fn test_strip_build_ms(&self) -> f32 {
        self.strip.build_ms
    }

    /// The ladder orientation (0–3).
    pub fn test_strip_orientation(&self) -> i32 {
        self.strip.orientation
    }

    /// Arm a zone for placement (a zone-strip cell click).
    pub fn arm_placement_zone(&mut self, zone: f64) {
        self.arm_zone(zone);
    }

    /// Placement pins and whether a solution is in.
    pub fn placement_status(&self) -> Option<(usize, bool, bool)> {
        self.placement
            .state
            .as_ref()
            .map(|s| (s.pins.len(), s.solution.is_some(), s.solving))
    }

    /// The canvas' ideal frame rect of the last layout (screen points).
    pub fn canvas_frame_rect(&self) -> Option<Rect> {
        self.loupe.last_frame_rect
    }

    /// Whether the Interactive Histogram window is open.
    pub fn interactive_histogram_open(&self) -> bool {
        self.levels.open
    }

    /// The current history entry's label for the open image (tests).
    pub fn current_history_label(&self) -> Option<String> {
        let h = self
            .selection
            .as_ref()
            .and_then(|p| self.histories.get(p))?;
        h.entries().get(h.index()).map(|e| e.label.clone())
    }

    /// Whether zone placement is up.
    pub fn zone_placement_active(&self) -> bool {
        self.placement.is_active()
    }

    /// Close the Interactive Histogram window.
    pub fn close_interactive_histogram(&mut self) {
        if self.levels.drag_index.take().is_some() {
            self.set_control_editing(false);
        }
        self.levels.open = false;
    }
}
// --- end U3 ---

impl eframe::App for NegApp {
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.handle_events(ctx);
        self.tick_timers(ctx);
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let t0 = Instant::now();
        let ctx = ui.ctx().clone();
        let t = Tokens::get(&ctx);
        let mut cmds = self.handle_keys(&ctx);

        egui::Panel::top("menu-bar")
            .exact_size(t.menu_bar_h)
            .frame(
                egui::Frame::new()
                    .fill(t.chrome)
                    .inner_margin(egui::Margin::symmetric(6, 2)),
            )
            .show(ui, |ui| {
                cmds.extend(self.menu_bar(ui));
            });

        // controls sidebar (right)
        let mut sidebar_actions = Vec::new();
        egui::Panel::right("controls")
            .exact_size(t.sidebar_w)
            .resizable(false)
            .frame(egui::Frame::new().fill(t.chrome))
            .show(ui, |ui| {
                let history = self.selection.as_ref().and_then(|p| self.histories.get(p));
                let (can_undo, can_redo) = history
                    .map(|h| (h.can_undo(), h.can_redo()))
                    .unwrap_or((false, false));
                let input = SidebarInput {
                    has_selection: self.selection.is_some(),
                    bins: self.histogram.as_deref(),
                    density_range: self.density_range,
                    character: self
                        .info
                        .as_ref()
                        .and_then(|i| i.character)
                        .map(|c| c.label()),
                    status_message: self.status_message.as_deref(),
                    history,
                    tool_mode: self.tool_mode,
                    baseline_held: self.baseline_held,
                    can_undo,
                    can_redo,
                    is_exporting: self.dialogs.exporting(),
                    test_strip_active: self.strip.owns_canvas(), // --- U3: darkroom ---
                };
                let mut sidebar = std::mem::take(&mut self.sidebar);
                sidebar_actions = controls_sidebar(ui, &mut sidebar, &mut self.settings, input);
                self.sidebar = sidebar;
            });

        // library (left)
        let mut lib_actions = Vec::new();
        if self.prefs.library_visible {
            let resp = egui::Panel::left("library")
                .default_size(
                    self.prefs
                        .library_width
                        .clamp(*LIBRARY_WIDTH_RANGE.start(), *LIBRARY_WIDTH_RANGE.end()),
                )
                .size_range(LIBRARY_WIDTH_RANGE)
                .resizable(true)
                .frame(
                    egui::Frame::new()
                        .fill(t.chrome)
                        .inner_margin(egui::Margin::symmetric(8, 6)),
                )
                .show(ui, |ui| {
                    lib_actions = library_panel(
                        ui,
                        &mut self.library,
                        self.selection.as_deref(),
                        &self.multi,
                    );
                });
            let w = resp.response.rect.width();
            if (w - self.prefs.library_width).abs() > 0.5 {
                self.prefs.library_width = w;
            }
        }

        // canvas + control bar + status
        let mut canvas_actions = Vec::new();
        let mut bar_actions = Vec::new();
        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(t.chrome))
            .show(ui, |ui| {
                let full = ui.available_rect_before_wrap();
                let status_h = 18.0;
                let canvas_rect = Rect::from_min_max(
                    full.min,
                    pos2(full.right(), full.bottom() - t.control_bar_h - status_h),
                );
                let bar_rect = Rect::from_min_max(
                    pos2(full.left(), canvas_rect.bottom()),
                    pos2(full.right(), canvas_rect.bottom() + t.control_bar_h),
                );
                let status_rect =
                    Rect::from_min_max(pos2(full.left(), bar_rect.bottom()), full.max);
                let input = CanvasInput {
                    has_selection: self.selection.is_some(),
                    developing: self.developing,
                    analyzing: self.analyzing,
                    baseline: self.baseline_held,
                    error: self.render_error.as_deref(),
                    show_grid: self.sidebar.prefs.show_grid_lines,
                    grid: self.sidebar.prefs.grid_line_type,
                    canvas_color: self.prefs.canvas_color,
                    tool_mode: self.tool_mode,
                    frame_label: None,
                };
                // --- U3: darkroom --- overlay layers (zone overlay, test strip, placement)
                let plain = self.tool_mode == ToolMode::None;
                let shown = self.zone_overlay_shown();
                let mut zone_layer = self
                    .zones
                    .grid
                    .as_ref()
                    .filter(|_| shown)
                    .map(|grid| ZoneOverlayLayer { grid });
                let mut overlays: Vec<&mut dyn CanvasOverlay> = Vec::new();
                if let Some(z) = zone_layer.as_mut() {
                    overlays.push(z);
                }
                if plain {
                    overlays.push(&mut self.strip);
                    overlays.push(&mut self.placement);
                }
                // --- end U3 ---
                // --- U2: tools & dialogs --- the crop box / analysis rect while a tool is up
                overlays.extend(self.tools.overlays());
                // --- end U2 ---
                canvas_actions = canvas(ui, canvas_rect, &mut self.loupe, &input, &mut overlays);
                let mut bar_ui = ui.new_child(
                    egui::UiBuilder::new()
                        .max_rect(bar_rect)
                        .layout(egui::Layout::left_to_right(egui::Align::Center)),
                );
                bar_ui.painter().rect_filled(bar_rect, 0.0, t.chrome);
                // --- U3: darkroom ---
                let readout = Readout {
                    reading: self.zones.densitometer.reading(),
                    armed: self.placement.armed(),
                };
                // --- end U3 ---
                bar_actions = control_bar(
                    &mut bar_ui,
                    &BarInput {
                        has_selection: self.selection.is_some(),
                        hq_mode: self.hq.mode,
                        hq_active: self.hq.active,
                        zones_on: self.prefs.show_zone_overlay, // --- U3: darkroom ---
                        canvas_color: self.prefs.canvas_color,
                        readout: &readout,
                    },
                );
                // status line
                let p = ui.painter_at(status_rect);
                p.rect_filled(status_rect, 0.0, t.inset);
                let tier = match self.loupe.tier {
                    RenderTier::Proxy => "proxy",
                    RenderTier::MediumIfFree => "medium",
                    RenderTier::Full => "full",
                };
                let left = format!(
                    "{} · {} · render {:.1} ms · latency {:.1} ms · ui {:.2} ms",
                    self.engine.renderer_name,
                    tier,
                    self.perf.render_ms,
                    self.perf.latency_ms,
                    self.perf.ui_ms
                );
                p.text(
                    pos2(status_rect.left() + 8.0, status_rect.center().y),
                    egui::Align2::LEFT_CENTER,
                    left,
                    t.mono(9.5),
                    t.text_tertiary,
                );
                if let Some(sel) = &self.selection {
                    let idx = self.library.index_of(sel).unwrap_or(0);
                    let name = sel
                        .file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_default();
                    p.text(
                        pos2(status_rect.right() - 8.0, status_rect.center().y),
                        egui::Align2::RIGHT_CENTER,
                        format!("{name} · {idx} of {}", self.library.flat.len()),
                        t.mono(9.5),
                        t.text_tertiary,
                    );
                }
            });

        // ── apply actions ──
        for a in sidebar_actions {
            match a {
                SidebarAction::SettingsEdited => {}
                SidebarAction::DragBegin => self.set_control_editing(true),
                SidebarAction::DragEnd => self.set_control_editing(false),
                SidebarAction::ResetAll => cmds.push(Command::ResetAll),
                SidebarAction::StartFromScratch => cmds.push(Command::StartFromScratch),
                SidebarAction::ViewOriginal(on) => self.set_baseline(on),
                SidebarAction::ToggleAnalysisTool => cmds.push(Command::ToggleAnalysis),
                SidebarAction::ClearAnalysisRect => cmds.push(Command::ClearAnalysis),
                SidebarAction::ToggleCropTool => cmds.push(Command::ToggleCrop),
                SidebarAction::ClearCropAndStraighten => cmds.push(Command::ClearCropAndStraighten),
                SidebarAction::TestStrip => cmds.push(Command::TestStrip),
                SidebarAction::Export => cmds.push(Command::Export),
                SidebarAction::Undo => cmds.push(Command::Undo),
                SidebarAction::Redo => cmds.push(Command::Redo),
                SidebarAction::JumpHistory(i) => self.jump_history(i),
                SidebarAction::PrefsChanged => {}
                // --- U3: darkroom ---
                SidebarAction::OpenInteractiveHistogram => cmds.push(Command::InteractiveHistogram),
                // --- end U3 ---
            }
        }
        for a in lib_actions {
            match a {
                LibraryAction::Select {
                    path,
                    additive,
                    range,
                } => self.select(path, additive, range),
                LibraryAction::ChooseFolder => cmds.push(Command::OpenFolder),
                LibraryAction::ToggleLibrary => cmds.push(Command::ToggleLibrary),
                LibraryAction::CollapsedChanged => self.persist_collapsed(),
            }
        }
        for a in canvas_actions {
            match a {
                CanvasAction::ZoomChanged(z) => self.on_zoom(z),
                CanvasAction::Hover(uv) => {
                    self.hover_uv = uv;
                    self.probe_densitometer(uv); // --- U3: darkroom ---
                }
            }
        }
        for a in bar_actions {
            match a {
                BarAction::RotateCcw => cmds.push(Command::RotateLeft),
                BarAction::RotateCw => cmds.push(Command::RotateRight),
                BarAction::Flip => cmds.push(Command::Flip),
                BarAction::CycleHq => cmds.push(Command::CycleHq),
                BarAction::ToggleZones => cmds.push(Command::ToggleZones),
                BarAction::CanvasColor(c) => self.prefs.canvas_color = c,
                BarAction::ArmZone(z) => self.arm_zone(z), // --- U3: darkroom ---
            }
        }
        // --- U2: tools & dialogs ---
        cmds.extend(self.tools_and_dialogs_frame(&ctx));
        // --- end U2 ---
        for c in cmds {
            self.run(c);
        }
        self.folder_dialog_ui(&ctx);
        self.darkroom_frame(&ctx); // --- U3: darkroom ---

        // settings didSet
        if self.settings != self.last_settings {
            self.settings_changed();
        }
        self.prefs.sidebar = self.sidebar.prefs.clone();

        let dt = t0.elapsed().as_secs_f32() * 1000.0;
        self.perf.ui_ms = if self.perf.frames == 0 {
            dt
        } else {
            self.perf.ui_ms * 0.9 + dt * 0.1
        };
        self.perf.frames += 1;
    }

    fn save(&mut self, storage: &mut dyn eframe::Storage) {
        if self.persist {
            self.prefs.store(storage);
        }
    }

    fn on_exit(&mut self) {
        self.flush_save();
        // A running batch stops after the frame in progress instead of finishing the batch
        // while the window waits to close.
        self.dialogs.cancel_export();
    }
}

/// Reveal `path` in the platform file manager.
fn show_in_file_manager(path: &Path) {
    #[cfg(target_os = "macos")]
    let spawned = std::process::Command::new("open")
        .arg("-R")
        .arg(path)
        .spawn();
    #[cfg(target_os = "windows")]
    let spawned = std::process::Command::new("explorer")
        .arg(format!("/select,{}", path.display()))
        .spawn();
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    let spawned = std::process::Command::new("xdg-open")
        .arg(path.parent().unwrap_or(path))
        .spawn();
    if let Err(e) = spawned {
        log::warn!("show in file manager: {e}");
    }
}

// --- U2: tools & dialogs ---
impl NegApp {
    /// The orientation-only proxy frame dims of the last render (the crop-geometry base).
    fn frame_v2(&self) -> negkit::V2 {
        let (w, h) = self.loupe.frame_size;
        [w as f64, h as f64]
    }

    /// Enter/leave a tool: re-resolve HQ (tools suppress Auto) and re-render the tool preview.
    fn set_tool_mode(&mut self, mode: ToolMode) {
        if self.set_tool_mode_quiet(mode) {
            self.schedule_render();
        }
    }

    /// [`Self::set_tool_mode`] without the render (a settings commit that renders follows).
    /// Returns whether the mode changed.
    fn set_tool_mode_quiet(&mut self, mode: ToolMode) -> bool {
        if self.tool_mode == mode {
            return false;
        }
        self.tool_mode = mode;
        if mode == ToolMode::Crop {
            // Crop & Straighten pins the canvas to fit.
            self.loupe.reset_zoom();
            self.canvas_zoom = 1.0;
        }
        let (zoom, has_sel) = (f64::from(self.canvas_zoom), self.selection.is_some());
        self.hq.refresh(zoom, has_sel, mode != ToolMode::None);
        true
    }

    /// ⌘K: enter Crop & Straighten (snapshot for Cancel), or commit it when it is up.
    fn toggle_crop_tool(&mut self) {
        if self.tool_mode == ToolMode::Crop {
            self.apply_tool();
            return;
        }
        if self.tool_mode == ToolMode::AnalysisRegion {
            self.leave_tool();
        }
        let frame = self.frame_v2();
        if !(frame[0] > 0.0 && frame[1] > 0.0) {
            return;
        }
        self.tools.crop = Some(CropTool::new(&self.settings, frame));
        self.set_tool_mode(ToolMode::Crop);
    }

    /// ⇧⌘K: enter/leave Crop for Analysis (entering from crop commits the crop first).
    fn toggle_analysis_tool(&mut self) {
        match self.tool_mode {
            ToolMode::AnalysisRegion => self.leave_tool(),
            ToolMode::Crop => {
                self.apply_tool();
                self.enter_analysis_tool();
            }
            ToolMode::None => self.enter_analysis_tool(),
        }
    }

    fn enter_analysis_tool(&mut self) {
        self.tools.analysis = Some(AnalysisTool::new(&self.settings));
        self.set_tool_mode(ToolMode::AnalysisRegion);
    }

    /// Leave whatever tool is up without committing anything.
    fn leave_tool(&mut self) {
        self.tools.clear();
        self.set_tool_mode(ToolMode::None);
    }

    /// Leave the tool and set `next` as the settings with `label` armed, committing ONE history
    /// entry now (tool commits are discrete actions, not slider drags). One render follows.
    fn commit_tool_settings(&mut self, next: ExposureSettings, label: Option<&str>) {
        self.tools.clear();
        if next == self.settings {
            self.set_tool_mode(ToolMode::None);
            return;
        }
        self.set_tool_mode_quiet(ToolMode::None);
        // An in-flight (coalescing) edit gets its own entry first, so Undo of the tool commit
        // undoes only the tool.
        if self.pending.is_armed() {
            self.pending.cancel();
            self.commit_history();
        }
        if let Some(l) = label {
            self.arm_label(l);
        }
        self.settings = next;
        self.settings_changed();
        self.pending.cancel();
        self.commit_history();
    }

    /// Return / Apply: crop commits box + angle (`commitCrop` + `commitFineRotation`); the
    /// analysis tool just leaves.
    fn apply_tool(&mut self) {
        match self.tools.crop.take() {
            Some(mut c) => {
                c.sync(&self.settings, self.frame_v2());
                let mut next = self.settings.clone();
                let label = c.apply(&mut next);
                self.commit_tool_settings(next, label);
            }
            None => self.leave_tool(),
        }
    }

    /// Esc / Cancel: crop restores its entry snapshot ("Cancel crop" if anything moved).
    fn cancel_tool(&mut self) {
        match self.tools.crop.take() {
            Some(c) => {
                let mut next = self.settings.clone();
                c.cancel(&mut next);
                self.commit_tool_settings(next, Some(labels::CANCEL_CROP));
            }
            None => self.leave_tool(),
        }
    }

    /// The analysis tool's release: `commitSelection` (rect + the angle it was drawn at, the
    /// same rule as `Session::commit_analysis_region`), then leave the tool.
    fn commit_analysis(&mut self, rect: negkit::NormalizedRect) {
        let mut next = self.settings.clone();
        tools::commit_analysis_region(&mut next, rect);
        self.commit_tool_settings(next, Some(labels::ANALYSIS_REGION));
    }

    /// ⌘E: the sheet over the multi-selection in library order (the open frame at least).
    fn open_export_sheet(&mut self) {
        if self.dialogs.exporting() {
            self.status_message = Some("An export is already running".into());
            return;
        }
        let mut targets: Vec<PathBuf> = self
            .library
            .flat
            .iter()
            .filter(|p| self.multi.contains(*p))
            .cloned()
            .collect();
        if targets.is_empty()
            && let Some(sel) = self.selection.clone()
        {
            targets.push(sel);
        }
        self.dialogs.open_export(self.prefs.export.clone(), targets);
    }

    /// `performExport`: sticky options saved, the open frame's pending sidecar flushed, then
    /// the engine job (each frame its own session; the open frame with the live settings).
    fn start_export(&mut self, options: ExportOptions, targets: Vec<PathBuf>) {
        self.prefs.export = options.clone();
        self.flush_save();
        self.export_generation += 1;
        let generation = self.export_generation;
        let items: Vec<ExportItem> = targets
            .into_iter()
            .map(|path| ExportItem {
                settings: (Some(path.as_path()) == self.selection.as_deref())
                    .then(|| self.settings.clone()),
                path,
            })
            .collect();
        let cancel = self.dialogs.start_job(generation, items.len());
        self.engine.send(Cmd::Export {
            items,
            default_settings: self.profiles.default_settings(),
            options,
            generation,
            cancel,
        });
    }

    /// Per-frame glue: keep the tool state in step with `tool_mode`, act on overlay/dialog
    /// requests, Return in a tool, keep repainting while an export runs.
    fn tools_and_dialogs_frame(&mut self, ctx: &egui::Context) -> Vec<Command> {
        let mut cmds = Vec::new();
        // a navigation or the existing Escape path may have dropped the mode
        if self.tool_mode == ToolMode::None && self.tools.active() {
            self.tools.clear();
        }
        let frame = self.frame_v2();
        if let Some(c) = &mut self.tools.crop {
            c.sync(&self.settings, frame);
        }
        for a in self.tools.take_actions() {
            match a {
                ToolAction::ApplyCrop => cmds.push(Command::ApplyTool),
                ToolAction::CancelCrop => cmds.push(Command::CancelTool),
                ToolAction::CommitAnalysis(r) => self.commit_analysis(r),
            }
        }
        if self.tool_mode != ToolMode::None
            && !self.dialogs.is_modal_open()
            && !ctx.egui_wants_keyboard_input()
            && ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Enter))
        {
            cmds.push(Command::ApplyTool);
        }
        let current = self.selection.is_some().then(|| self.settings.clone());
        for a in self.dialogs.ui(ctx, &mut self.profiles, current.as_ref()) {
            match a {
                DialogAction::StartExport { options, targets } => {
                    self.start_export(options, targets)
                }
                DialogAction::ProfilesChanged => match self.profiles.to_json() {
                    Ok(json) => self.prefs.profiles = json,
                    Err(e) => self.status_message = Some(format!("Couldn't save profiles: {e}")),
                },
                DialogAction::Status(s) => self.status_message = Some(s),
            }
        }
        if self.dialogs.exporting() {
            ctx.request_repaint_after(Duration::from_millis(100));
        }
        cmds
    }

    /// What the densitometer adopts for `output`: its own bytes, or — for a zero-copy texture
    /// frame, whose RGBA8 frame is empty — the proxy probe the worker sent just before it.
    fn probe_for(&mut self, output: Box<negsession::RenderOutput>) -> negsession::RenderOutput {
        let probe = self.probe.take();
        if output.frame.rgba8().is_some_and(|b| b.is_empty())
            && let Some(p) = probe
        {
            return *p;
        }
        *output
    }

    // ── selftest / bench hooks ──

    /// Developing or analysing (the canvas is about to change).
    pub fn busy(&self) -> bool {
        self.developing || self.analyzing
    }

    /// The tier of the frame on screen.
    pub fn rendered_tier(&self) -> RenderTier {
        self.loupe.tier
    }

    /// Crop tool state (selftest: draw a box / set an angle).
    pub fn crop_tool_mut(&mut self) -> Option<&mut CropTool> {
        self.tools.crop.as_mut()
    }

    /// The current tool mode.
    pub fn tool_mode(&self) -> ToolMode {
        self.tool_mode
    }

    /// Export synchronously-scheduled through the engine with `options` for the open frame
    /// (selftest); returns false when nothing is open or a job runs.
    pub fn start_export_for_selection(&mut self, options: ExportOptions) -> bool {
        let Some(sel) = self.selection.clone() else {
            return false;
        };
        if self.dialogs.exporting() {
            return false;
        }
        self.start_export(options, vec![sel]);
        true
    }

    /// Whether a batch export is running.
    pub fn exporting(&self) -> bool {
        self.dialogs.exporting()
    }

    /// The sidebar status line.
    pub fn status_message(&self) -> Option<&str> {
        self.status_message.as_deref()
    }

    /// The sticky export options.
    pub fn export_options(&self) -> &ExportOptions {
        &self.prefs.export
    }

    /// The persisted preferences.
    pub fn prefs(&self) -> &AppPrefs {
        &self.prefs
    }
}
// --- end U2 ---
