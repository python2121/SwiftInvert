//! `NegApp` — the `AppModel` + window glue (session-spec §6, controls-spec §8–§14): selection
//! and multi-selection, the settings mirror (a change → coalesced latest-wins render + 1 s
//! debounced sidecar save + history commit rules), tool-mode enum (the tools themselves are a
//! later wave), baseline hold, HQ mode with the 2× zoom threshold and 350 ms upward debounce,
//! per-image history with drag-commits-on-release and the 0.7 s coalesce, sticky prefs through
//! eframe storage, the key map and the menu bar.
//!
//! Seams for later waves: [`ToolMode`] is carried (renders go through
//! `Session::render_tool_preview`, so entering a tool already widens the render) but no overlay
//! draws a box yet; `Command::Export`, `ChooseDefaults`, `TestStrip` and `ToggleZones` set a
//! status line; the control channel would drive [`NegApp::run`] with [`Command`]s.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use egui::{Key, KeyboardShortcut, Modifiers, Rect, pos2, vec2};
use negkit::ExposureSettings;
use negsession::history::{PendingCommit, labels};
use negsession::hq::{HQ_AUTO_ZOOM_THRESHOLD, HQ_UPWARD_DEBOUNCE_MS};
use negsession::renderer::{GpuRenderer, Renderer, default_renderer};
use negsession::session::baseline_settings;
use negsession::{
    History, HqMode, ProfileStore, RawBackend, RenderRequest, RenderTier, SessionInfo,
    SettingsGeometry, SidecarStore, ToolMode,
};

use crate::controls::{SidebarAction, SidebarInput, SidebarPrefs, SidebarState, controls_sidebar};
use crate::engine::{Cmd, Engine, Evt};
use crate::library::{LibraryAction, LibraryState, library_panel};
use crate::loupe::{
    BarAction, BarInput, CanvasAction, CanvasColor, CanvasInput, CpuPresenter, FramePresenter,
    LoupeState, canvas, control_bar,
};
use crate::theme::Tokens;
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
/// eframe storage key for [`AppPrefs`].
pub const PREFS_KEY: &str = "swiftinvert.prefs";
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
            // --- U3: darkroom ---
            show_zone_overlay: false,
            // --- end U3 ---
        }
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
}

impl Default for AppOptions {
    fn default() -> Self {
        Self {
            initial_folder: None,
            autosave: true,
            persist: true,
            renderer: None,
            presenter: None,
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
    save_due: Option<Instant>,
    status_message: Option<String>,
    copied: Option<ExposureSettings>,
    hover_uv: Option<(f32, f32)>,

    folder_dialog: Option<FolderDialog>,
    perf: Perf,
    frames_rendered: u64,
    open_failed: Option<String>,

    // --- U3: darkroom ---
    zones: ZonesState,
    strip: TestStripController,
    placement: ZonePlacementController,
    levels: LevelsWindow,
    // --- end U3 ---
}

struct FolderDialog {
    text: String,
}

impl NegApp {
    /// Build the app (theme, prefs, worker threads, initial folder).
    pub fn new(cc: &eframe::CreationContext<'_>, options: AppOptions) -> Self {
        crate::theme::install(&cc.egui_ctx);
        let prefs: AppPrefs = if options.persist {
            cc.storage
                .and_then(|s| eframe::get_value::<AppPrefs>(s, PREFS_KEY))
                .unwrap_or_default()
        } else {
            AppPrefs::default()
        };
        let backend: Arc<dyn RawBackend> = Arc::from(negsession::default_backend());
        let renderer = options
            .renderer
            .unwrap_or_else(|| shared_or_default_renderer(cc));
        let ctx = cc.egui_ctx.clone();
        let engine = Engine::spawn(Arc::clone(&backend), renderer, move || {
            ctx.request_repaint()
        });
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
            presenter: options
                .presenter
                .unwrap_or_else(|| Box::new(CpuPresenter::default())),
            prefs,
            profiles: ProfileStore::new(),
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
            save_due: None,
            status_message: None,
            copied: None,
            hover_uv: None,
            folder_dialog: None,
            perf: Perf::default(),
            frames_rendered: 0,
            open_failed: None,
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

    // ───────────────────────────── folder / selection ─────────────────────────────

    fn open_folder(&mut self, ctx: &egui::Context, root: PathBuf, remember: bool) {
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
                self.save_due = Some(Instant::now() + SAVE_DEBOUNCE);
            }
            if !self.is_navigating_history {
                self.pending.schedule(Instant::now());
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
        if self
            .hq
            .on_zoom(f64::from(zoom), has_sel, tool, Instant::now())
        {
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
                let text = self
                    .library
                    .root
                    .clone()
                    .or_else(|| std::env::var_os("HOME").map(PathBuf::from))
                    .map(|p| p.display().to_string())
                    .unwrap_or_default();
                self.folder_dialog = Some(FolderDialog { text });
            }
            Command::ChooseDefaults => {
                self.status_message =
                    Some("Default-settings profiles arrive in a later wave".into())
            }
            Command::Export => {
                if has_sel {
                    self.status_message = Some("Export arrives in a later wave".into());
                }
            }
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
                    self.arm_label(labels::RESET_ALL);
                    self.settings = next;
                }
            }
            Command::StartFromScratch => {
                if has_sel {
                    self.arm_label(labels::START_FROM_SCRATCH);
                    self.settings = ExposureSettings::default();
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
                    self.arm_label(labels::ROTATE_CCW);
                    self.settings.rotation = (self.settings.rotation - 90).rem_euclid(360);
                }
            }
            Command::RotateRight => {
                if has_sel {
                    self.arm_label(labels::ROTATE_CW);
                    self.settings.rotation = (self.settings.rotation + 90).rem_euclid(360);
                }
            }
            Command::Flip => {
                if has_sel {
                    self.arm_label(labels::FLIP_HORIZONTAL);
                    self.settings.flip_horizontal = !self.settings.flip_horizontal;
                }
            }
            Command::ToggleCrop | Command::ToggleAnalysis => {
                self.clear_darkroom_layers(); // --- U3: darkroom ---
                self.status_message = Some("Canvas tools arrive in a later wave".into());
            }
            Command::ClearCropAndStraighten => {
                if has_sel {
                    let had_crop = self.settings.crop_rect.is_some();
                    let had_angle = self.settings.fine_rotation.abs() > 1e-9;
                    let label = match (had_crop, had_angle) {
                        (true, true) => labels::CROP_AND_STRAIGHTEN_CLEARED,
                        (true, false) => labels::CROP_CLEARED,
                        _ => labels::STRAIGHTEN_CLEARED,
                    };
                    self.arm_label(label);
                    self.settings.crop_rect = None;
                    self.settings.fine_rotation = 0.0;
                }
            }
            Command::ClearAnalysis => {
                if has_sel && self.settings.analysis_rect.is_some() {
                    self.arm_label(labels::ANALYSIS_REGION_CLEARED);
                    self.settings.analysis_rect = None;
                    self.settings.analysis_rect_fine_rotation = 0.0;
                }
            }
            // --- U3: darkroom --- Escape clears the strip first, then the placement
            Command::Escape if self.strip.owns_canvas() || self.placement.is_active() => {
                if self.strip.owns_canvas() {
                    self.clear_test_strip();
                } else {
                    self.clear_zone_placement();
                }
            }
            // --- end U3 ---
            Command::Escape => {
                if self.tool_mode != ToolMode::None {
                    self.tool_mode = ToolMode::None;
                    self.hq.refresh(f64::from(self.canvas_zoom), true, false);
                    self.schedule_render();
                } else {
                    self.folder_dialog = None;
                }
            }
            Command::TestStrip => self.toggle_test_strip(), // --- U3: darkroom ---
        }
    }

    // ───────────────────────────── per-frame ─────────────────────────────

    fn handle_events(&mut self, ctx: &egui::Context) {
        for evt in self.engine.poll() {
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
                        self.open_failed = Some(format!("Couldn't develop this image: {e}"));
                        self.render_error = Some("Couldn't develop this image".into());
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
                } => {
                    if Some(path.as_path()) != self.selection.as_deref()
                        || generation < self.open_generation
                    {
                        continue;
                    }
                    let frame = self.presenter.present(ctx, &output);
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
                    self.zones.adopt(*output, self.prefs.show_zone_overlay);
                    // --- end U3 ---
                }
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
                    if Some(path.as_path()) == self.selection.as_deref()
                        && generation >= self.open_generation
                    {
                        self.developing = false;
                        self.analyzing = false;
                        self.render_error = Some(format!("Render failed: {error}"));
                    }
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
                self.selection = None;
                self.multi.clear();
            }
        }
    }

    fn tick_timers(&mut self, ctx: &egui::Context) {
        let now = Instant::now();
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

    fn handle_keys(&mut self, ctx: &egui::Context) -> Vec<Command> {
        let mut cmds = Vec::new();
        let text_focus = ctx.egui_wants_keyboard_input();
        ctx.input_mut(|i| {
            for (s, c) in SHORTCUTS {
                if i.consume_shortcut(s) {
                    cmds.push(*c);
                }
            }
            if !text_focus {
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
            if i.consume_key(Modifiers::NONE, Key::Escape) {
                cmds.push(Command::Escape);
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
                ui.horizontal(|ui| {
                    ui.label("Folder");
                    let te =
                        egui::TextEdit::singleline(&mut dialog.text).desired_width(f32::INFINITY);
                    let r = ui.add(te);
                    if r.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)) {
                        chosen = Some(PathBuf::from(dialog.text.trim()));
                    }
                });
                let current = PathBuf::from(dialog.text.trim());
                ui.horizontal(|ui| {
                    if ui.button("↑ Parent").clicked()
                        && let Some(parent) = current.parent()
                    {
                        dialog.text = parent.display().to_string();
                    }
                    if ui
                        .add_enabled(current.is_dir(), egui::Button::new("Open"))
                        .clicked()
                    {
                        chosen = Some(current.clone());
                    }
                    if ui.button("Cancel").clicked() {
                        close = true;
                    }
                });
                ui.separator();
                let mut dirs: Vec<PathBuf> = std::fs::read_dir(&current)
                    .map(|rd| {
                        rd.flatten()
                            .map(|e| e.path())
                            .filter(|p| {
                                p.is_dir()
                                    && !p
                                        .file_name()
                                        .is_some_and(|n| n.to_string_lossy().starts_with('.'))
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                dirs.sort_by(|a, b| {
                    negsession::library::natural_cmp(
                        &a.file_name()
                            .map(|n| n.to_string_lossy().into_owned())
                            .unwrap_or_default(),
                        &b.file_name()
                            .map(|n| n.to_string_lossy().into_owned())
                            .unwrap_or_default(),
                    )
                });
                egui::ScrollArea::vertical()
                    .max_height(300.0)
                    .show(ui, |ui| {
                        for d in dirs.iter().take(500) {
                            let name = d
                                .file_name()
                                .map(|n| n.to_string_lossy().into_owned())
                                .unwrap_or_default();
                            let r = ui.selectable_label(false, format!("▸ {name}"));
                            if r.double_clicked() {
                                chosen = Some(d.clone());
                            } else if r.clicked() {
                                dialog.text = d.display().to_string();
                            }
                        }
                    });
            });
        if let Some(c) = chosen {
            if c.is_dir() {
                self.folder_dialog = None;
                self.open_folder(ctx, c, true);
            } else {
                self.status_message = Some("Not a folder".into());
            }
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
                    is_exporting: false,
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
                let mut zone_layer = self
                    .zones
                    .grid
                    .as_ref()
                    .filter(|_| self.prefs.show_zone_overlay && plain)
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
                        zones_on: false,
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
            eframe::set_value(storage, PREFS_KEY, &self.prefs);
        }
    }

    fn on_exit(&mut self) {
        self.flush_save();
    }
}

/// Share eframe's wgpu device with the engine when there is one (zero-copy later; today the
/// readback still crosses to the CPU), else the process default (headless GPU or CPU).
fn shared_or_default_renderer(cc: &eframe::CreationContext<'_>) -> Arc<dyn Renderer> {
    if std::env::var(neggpu_env_key())
        .ok()
        .is_some_and(|v| v.eq_ignore_ascii_case("off"))
    {
        return default_renderer();
    }
    if let Some(rs) = &cc.wgpu_render_state {
        match GpuRenderer::from_existing(rs.device.clone(), rs.queue.clone(), rs.adapter.get_info())
        {
            Ok(gpu) => {
                log::info!("renderer: {} (sharing the window's device)", gpu.name());
                return Arc::new(gpu);
            }
            Err(e) => log::warn!("renderer: shared device refused ({e}); falling back"),
        }
    }
    default_renderer()
}

fn neggpu_env_key() -> &'static str {
    "NEG_GPU"
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
