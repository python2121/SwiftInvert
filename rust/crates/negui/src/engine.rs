//! The engine worker — the sessions live on ONE background thread (the Swift `ImageSession`
//! actors + `AppModel.sessionLRU`), so decode, analysis and render never block the UI. The UI
//! sends [`Cmd`]s and reads [`Evt`]s each frame; the worker coalesces renders latest-wins
//! (`AppModel.scheduleRender`: a request during a render is remembered and the newest state is
//! rendered once).
//!
//! Only RGBA8 frames cross the channel today (the CPU/readback path). A GPU renderer that keeps
//! the frame on the device will still return a [`RenderOutput`]; the loupe's
//! [`crate::loupe::FramePresenter`] is where a native texture would be registered instead of
//! uploaded — nothing here needs to change for it.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use negkit::ExposureSettings;
use negsession::renderer::Renderer;
use negsession::{RawBackend, RenderOutput, RenderRequest, Session, SessionInfo, ToolMode};
// --- U3: darkroom ---
use negkit::zone_placement::{Pin, Solution};
use negsession::{TestStripOutput, TestStripRequest};
// --- end U3 ---

// --- U2: tools & dialogs ---
use std::sync::atomic::{AtomicBool, Ordering};

use negsession::{ExportOptions, Exporter, Frame, SidecarStore};

use crate::presenter_gpu::{GpuFrame, TextureRing, TextureSink};

/// One frame of a batch export: `settings` is the live mirror for the open frame, `None` for
/// the others (their sidecar, else the default profile, is read on the export thread).
#[derive(Clone, Debug)]
pub struct ExportItem {
    pub path: PathBuf,
    pub settings: Option<ExposureSettings>,
}
// --- end U2 ---

/// Sessions kept warm (`AppModel.sessionLRU`): the active one plus the one navigated from.
pub const SESSION_LRU: usize = 2;

/// UI → worker. (Commands are moved once into the channel; the settings payload's size
/// difference between variants is irrelevant there.)
#[derive(Debug)]
#[allow(clippy::large_enum_variant)]
pub enum Cmd {
    /// Open (or revive from the LRU) the session for `path` with these settings; a render
    /// follows as a separate command.
    Open {
        path: PathBuf,
        settings: ExposureSettings,
        generation: u64,
    },
    /// Render `path` with `settings` committed to the session; `variant` (baseline hold) renders
    /// instead of the committed settings without disturbing them.
    Render {
        path: PathBuf,
        settings: ExposureSettings,
        variant: Option<ExposureSettings>,
        tool: ToolMode,
        request: RenderRequest,
        generation: u64,
        requested_at: Instant,
    },
    /// HQ off: drop the medium/full tiers of `path`.
    ReleaseEnhanced {
        path: PathBuf,
    },
    /// Drop every session (folder change).
    ClearSessions,
    // --- U3: darkroom ---
    /// Test strip / patch preview / zone placement work against `path`'s session with
    /// `settings` committed first (the darkroom layers always run on the proxy tier).
    Darkroom {
        path: PathBuf,
        settings: ExposureSettings,
        cmd: DarkroomCmd,
        generation: u64,
    },
    // --- end U3 ---
    Shutdown,
    // --- U2: tools & dialogs ---
    /// Batch export (`AppModel.performExport`) on its own thread so the canvas keeps rendering:
    /// each frame opens its own [`Session`] (its sidecar or `default_settings` unless the item
    /// carries the live settings), renders at full resolution and is written per `options`.
    /// Progress/finish arrive as [`Evt::ExportProgress`] / [`Evt::ExportFinished`]; setting
    /// `cancel` stops before the next frame. Never writes a sidecar.
    Export {
        items: Vec<ExportItem>,
        default_settings: ExposureSettings,
        options: ExportOptions,
        generation: u64,
        cancel: Arc<AtomicBool>,
    },
    /// Present display renders through GPU textures shared with the window (the zero-copy
    /// loupe, [`crate::presenter_gpu`]) instead of RGBA8 readbacks.
    UseTextureTarget(TextureSink),
    // --- end U2 ---
}

// --- U3: darkroom ---
/// The darkroom layers' worker requests (`ImageSession.renderTestStrip`, `render(settings:)`,
/// `sampleZonePin` + `predictedZone`, `solveZonePlacement`).
#[derive(Debug)]
pub enum DarkroomCmd {
    /// Build the 5×5 strip with this ladder orientation.
    TestStrip { orientation: i32 },
    /// One full-frame proxy render of `settings` (a strip patch preview).
    Preview,
    /// Sample a pin at content-normalized `(u, v)`; `armed` = the target zone picked on the
    /// strip, else the metered zone snapped to thirds.
    ZoneSample {
        u: f64,
        v: f64,
        armed: Option<f64>,
        op: PinOp,
    },
    /// Labels + solve + solved-look preview for these pins.
    ZoneSolve { pins: Vec<Pin> },
}

/// What a zone sample is for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PinOp {
    Add,
    Move { index: usize, last: bool },
}

impl DarkroomCmd {
    /// Coalescing key: a request superseded by a later one of the same key is skipped
    /// (latest-wins, like renders). Adds and final moves are never dropped.
    fn coalesce_key(&self) -> Option<u8> {
        match self {
            DarkroomCmd::TestStrip { .. } => Some(0),
            DarkroomCmd::Preview => Some(1),
            DarkroomCmd::ZoneSample {
                op: PinOp::Move { last: false, .. },
                ..
            } => Some(2),
            DarkroomCmd::ZoneSample { .. } => None,
            DarkroomCmd::ZoneSolve { .. } => Some(3),
        }
    }
}

/// The darkroom worker results.
#[derive(Debug)]
pub enum DarkroomEvt {
    TestStrip {
        result: Result<Box<TestStripOutput>, String>,
        build_time: Duration,
    },
    Preview {
        result: Result<Box<RenderOutput>, String>,
    },
    /// The pin carries its frozen sample and its target (armed, else the snapped meter).
    ZoneSampled {
        op: PinOp,
        result: Result<Pin, String>,
    },
    ZoneSolved {
        /// What each pin prints at under the CURRENT settings (roman), pin order.
        labels: Vec<String>,
        solution: Option<Box<Solution>>,
        preview: Option<Box<RenderOutput>>,
    },
}
// --- end U3 ---

/// Worker → UI.
#[derive(Debug)]
pub enum Evt {
    /// `Open` finished (`Err` = decode failed).
    Opened {
        path: PathBuf,
        generation: u64,
        result: Result<(), String>,
    },
    /// A render that will re-run analysis (or decode a tier) is starting — the "Analyzing…" pill.
    Analyzing { path: PathBuf, generation: u64 },
    /// A render finished.
    Rendered {
        path: PathBuf,
        generation: u64,
        output: Box<RenderOutput>,
        info: Option<SessionInfo>,
        /// Time spent in `Session::render` (decode/analysis included when they ran).
        render_time: Duration,
        /// Wall time since the UI issued the request (queueing included).
        latency: Duration,
        /// The zero-copy texture holding the pixels (`output`'s RGBA8 frame is then empty);
        /// dropping it frees its ring slot.
        gpu: Option<GpuFrame>,
    },
    RenderFailed {
        path: PathBuf,
        generation: u64,
        error: String,
    },
    // --- U2: tools & dialogs ---
    /// Export started frame `done + 1` of `total`.
    ExportProgress {
        generation: u64,
        done: usize,
        total: usize,
        current: PathBuf,
    },
    /// The densitometer's bytes for a zero-copy (texture) frame: a proxy-tier RGBA8 render of
    /// the same snapshot, emitted just BEFORE that frame's [`Evt::Rendered`] (whose own RGBA8
    /// frame is empty).
    ProbeFrame {
        path: PathBuf,
        generation: u64,
        output: Box<RenderOutput>,
    },
    /// The batch ended (finished or cancelled).
    ExportFinished {
        generation: u64,
        total: usize,
        written: Vec<PathBuf>,
        failed: Vec<(PathBuf, String)>,
        cancelled: bool,
    },
    // --- end U2 ---
    // --- U3: darkroom ---
    Darkroom {
        path: PathBuf,
        generation: u64,
        event: DarkroomEvt,
    },
    // --- end U3 ---
    /// A command that is neither an open nor a render failed inside the worker (a caught
    /// panic); the worker dropped its sessions and keeps serving.
    EngineError { message: String },
}

/// The UI's handle: a sender, a receiver and the thread.
pub struct Engine {
    tx: Sender<Cmd>,
    rx: Receiver<Evt>,
    thread: Option<JoinHandle<()>>,
    pub renderer_name: String,
    /// Why the worker is gone (its channel disconnected); `None` while it serves.
    stopped: Option<String>,
}

impl Engine {
    /// Spawn the worker. `repaint` is called whenever an event is queued (the UI's
    /// `ctx.request_repaint()`).
    pub fn spawn(
        backend: Arc<dyn RawBackend>,
        renderer: Arc<dyn Renderer>,
        repaint: impl Fn() + Send + 'static,
    ) -> Engine {
        let (tx, cmd_rx) = channel::<Cmd>();
        let (evt_tx, rx) = channel::<Evt>();
        let renderer_name = renderer.name();
        let thread = std::thread::Builder::new()
            .name("negui-engine".into())
            .spawn(move || {
                let mut worker = Worker {
                    backend,
                    renderer,
                    sessions: Vec::new(),
                    evt_tx,
                    repaint: Box::new(repaint),
                    export_jobs: Vec::new(),
                    texture: None,
                    failed_opens: Vec::new(),
                };
                worker.run(cmd_rx);
            })
            .ok();
        let stopped = if thread.is_none() {
            log::error!("engine: could not spawn the worker thread");
            Some("could not start the worker thread".to_owned())
        } else {
            None
        };
        Engine {
            tx,
            rx,
            thread,
            renderer_name,
            stopped,
        }
    }

    /// Queue a command (dropped if the worker is gone — [`Engine::stopped`] then says why).
    pub fn send(&self, cmd: Cmd) {
        if self.tx.send(cmd).is_err() {
            log::warn!("engine: worker gone; command dropped");
        }
    }

    /// Drain pending events. A disconnected channel means the worker thread ended: that is
    /// recorded once (with the panic message when there was one) for [`Engine::stopped`].
    pub fn poll(&mut self) -> Vec<Evt> {
        let mut out = Vec::new();
        loop {
            match self.rx.try_recv() {
                Ok(e) => out.push(e),
                Err(std::sync::mpsc::TryRecvError::Empty) => break,
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    if self.stopped.is_none() {
                        let reason = match self.thread.take().map(JoinHandle::join) {
                            Some(Err(payload)) => panic_message(payload.as_ref()),
                            _ => "the worker thread ended".to_owned(),
                        };
                        log::error!("engine stopped: {reason}");
                        self.stopped = Some(reason);
                    }
                    break;
                }
            }
        }
        out
    }

    /// Why the worker is gone, once it is (fatal: nothing renders any more).
    pub fn stopped(&self) -> Option<&str> {
        self.stopped.as_deref()
    }
}

/// The text of a caught panic payload.
fn panic_message(payload: &(dyn std::any::Any + Send)) -> String {
    if let Some(s) = payload.downcast_ref::<&str>() {
        (*s).to_owned()
    } else if let Some(s) = payload.downcast_ref::<String>() {
        s.clone()
    } else {
        "panic".to_owned()
    }
}

impl Cmd {
    /// The open/render a failure report belongs to.
    fn failure_target(&self) -> Option<(PathBuf, u64, bool)> {
        match self {
            Cmd::Open {
                path, generation, ..
            } => Some((path.clone(), *generation, true)),
            Cmd::Render {
                path, generation, ..
            } => Some((path.clone(), *generation, false)),
            _ => None,
        }
    }
}

impl Drop for Engine {
    fn drop(&mut self) {
        let _ = self.tx.send(Cmd::Shutdown);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

struct Worker {
    backend: Arc<dyn RawBackend>,
    renderer: Arc<dyn Renderer>,
    /// Most recently used LAST.
    sessions: Vec<Session>,
    evt_tx: Sender<Evt>,
    repaint: Box<dyn Fn() + Send>,
    // --- U2: tools & dialogs ---
    export_jobs: Vec<(Arc<AtomicBool>, JoinHandle<()>)>,
    texture: Option<TextureRing>,
    /// Paths whose last `Open` failed, with the reason: a render/darkroom request for them
    /// reports that instead of decoding the broken file a second time (a new `Open` retries).
    failed_opens: Vec<(PathBuf, String)>,
}

impl Worker {
    fn emit(&self, evt: Evt) {
        if self.evt_tx.send(evt).is_ok() {
            (self.repaint)();
        }
    }

    fn run(&mut self, rx: Receiver<Cmd>) {
        loop {
            let Ok(first) = rx.recv() else { return };
            let mut queue = vec![first];
            while let Ok(c) = rx.try_recv() {
                queue.push(c);
            }
            // Latest-wins: a render superseded by a later render or open is skipped.
            let mut superseding_after = false;
            let mut keep = vec![true; queue.len()];
            for (i, c) in queue.iter().enumerate().rev() {
                if matches!(c, Cmd::Render { .. })
                    && superseding_after
                    && let Some(k) = keep.get_mut(i)
                {
                    *k = false;
                }
                if matches!(c, Cmd::Render { .. } | Cmd::Open { .. }) {
                    superseding_after = true;
                }
            }
            // --- U3: darkroom --- same rule for darkroom requests, per coalescing key.
            let mut seen_keys: Vec<u8> = Vec::new();
            for (i, c) in queue.iter().enumerate().rev() {
                if let Cmd::Darkroom { cmd, .. } = c
                    && let Some(key) = cmd.coalesce_key()
                {
                    if seen_keys.contains(&key) {
                        if let Some(k) = keep.get_mut(i) {
                            *k = false;
                        }
                    } else {
                        seen_keys.push(key);
                    }
                }
            }
            // --- end U3 ---
            for (cmd, keep) in queue.into_iter().zip(keep) {
                if keep && !self.handle_guarded(cmd) {
                    return;
                }
            }
        }
    }

    /// [`Worker::handle`] with a panic turned into a failure event: an open reports
    /// `Opened(Err)`, a render `RenderFailed`, anything else `EngineError`. The sessions are
    /// dropped (a half-updated cache tower must not serve again) and the worker keeps serving.
    fn handle_guarded(&mut self, cmd: Cmd) -> bool {
        let target = cmd.failure_target();
        let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| self.handle(cmd)));
        match outcome {
            Ok(go_on) => go_on,
            Err(payload) => {
                let message = panic_message(payload.as_ref());
                log::error!("engine: a command panicked: {message}");
                self.sessions.clear();
                let error = format!("engine error: {message}");
                match target {
                    Some((path, generation, true)) => self.emit(Evt::Opened {
                        path,
                        generation,
                        result: Err(error),
                    }),
                    Some((path, generation, false)) => self.emit(Evt::RenderFailed {
                        path,
                        generation,
                        error,
                    }),
                    None => self.emit(Evt::EngineError { message: error }),
                }
                true
            }
        }
    }

    /// Returns false on shutdown.
    fn handle(&mut self, cmd: Cmd) -> bool {
        match cmd {
            Cmd::Shutdown => return false,
            Cmd::ClearSessions => {
                self.sessions.clear();
                self.failed_opens.clear();
            }
            // --- U2: tools & dialogs ---
            Cmd::Export {
                items,
                default_settings,
                options,
                generation,
                cancel,
            } => self.spawn_export(items, default_settings, options, generation, cancel),
            Cmd::UseTextureTarget(sink) => self.enable_texture_target(sink),
            // --- end U2 ---
            Cmd::ReleaseEnhanced { path } => {
                if let Some(s) = self.session_mut(&path) {
                    s.release_enhanced_tiers();
                }
            }
            Cmd::Open {
                path,
                settings,
                generation,
            } => {
                self.failed_opens.retain(|(p, _)| p != &path);
                let result = self.open(&path, settings).map(|_| ());
                if let Err(e) = &result {
                    self.failed_opens.push((path.clone(), e.clone()));
                }
                self.emit(Evt::Opened {
                    path,
                    generation,
                    result,
                });
            }
            // --- U3: darkroom ---
            Cmd::Darkroom {
                path,
                settings,
                cmd,
                generation,
            } => {
                let event = self.darkroom(&path, settings, cmd);
                self.emit(Evt::Darkroom {
                    path,
                    generation,
                    event,
                });
            }
            // --- end U3 ---
            Cmd::Render {
                path,
                settings,
                variant,
                tool,
                request,
                generation,
                requested_at,
            } => {
                let outcome = self.render(&path, settings, variant, tool, request, generation);
                match outcome {
                    Ok((output, info, render_time, gpu)) => self.emit(Evt::Rendered {
                        path,
                        generation,
                        output: Box::new(output),
                        info,
                        render_time,
                        latency: requested_at.elapsed(),
                        gpu,
                    }),
                    Err(error) => self.emit(Evt::RenderFailed {
                        path,
                        generation,
                        error,
                    }),
                }
            }
        }
        true
    }

    fn position(&self, path: &Path) -> Option<usize> {
        self.sessions.iter().position(|s| s.path() == path)
    }

    fn session_mut(&mut self, path: &Path) -> Option<&mut Session> {
        let i = self.position(path)?;
        self.sessions.get_mut(i)
    }

    /// Reuse by path (moved to the back) else open; evict beyond the LRU; strip the enhanced
    /// tiers from every retained session except the one opened.
    fn open(&mut self, path: &Path, settings: ExposureSettings) -> Result<&mut Session, String> {
        if let Some((_, e)) = self.failed_opens.iter().find(|(p, _)| p == path) {
            return Err(e.clone());
        }
        if let Some(i) = self.position(path) {
            let s = self.sessions.remove(i);
            self.sessions.push(s);
        } else {
            let session =
                Session::open(path, Arc::clone(&self.backend), Arc::clone(&self.renderer))
                    .map_err(|e| e.to_string())?;
            self.sessions.push(session);
            while self.sessions.len() > SESSION_LRU {
                self.sessions.remove(0);
            }
        }
        let last = self.sessions.len().saturating_sub(1);
        for (i, s) in self.sessions.iter_mut().enumerate() {
            if i != last {
                s.release_enhanced_tiers();
            }
        }
        let Some(s) = self.sessions.last_mut() else {
            return Err("no session".into());
        };
        s.set_settings(settings);
        Ok(s)
    }

    fn render(
        &mut self,
        path: &Path,
        settings: ExposureSettings,
        variant: Option<ExposureSettings>,
        tool: ToolMode,
        request: RenderRequest,
        generation: u64,
    ) -> Result<RenderDone, String> {
        if self.position(path).is_none() {
            self.open(path, settings.clone())?;
        }
        let needs_prep = {
            let Some(s) = self.session_mut(path) else {
                return Err("session vanished".into());
            };
            s.set_settings(settings);
            s.needs_preparation(request.tier)
        };
        if needs_prep {
            self.emit(Evt::Analyzing {
                path: path.to_path_buf(),
                generation,
            });
        }
        // --- U2: tools & dialogs ---
        if let Some(out) = self.render_to_target(path, &variant, tool, request, generation) {
            return out;
        }
        // --- end U2 ---
        let Some(s) = self.session_mut(path) else {
            return Err("session vanished".into());
        };
        let t0 = Instant::now();
        let output = match &variant {
            Some(v) => {
                let mut req = request;
                req.uncropped = tool != ToolMode::None;
                s.render_with(v, req)
            }
            None => s.render_tool_preview(tool, request),
        }
        .map_err(|e| e.to_string())?;
        let render_time = t0.elapsed();
        let info = s.session_info().ok();
        Ok((output, info, render_time, None))
    }
}

/// A finished display render: the output, the session read-outs, the render time and (zero-copy
/// path) the texture holding the pixels.
type RenderDone = (
    RenderOutput,
    Option<SessionInfo>,
    Duration,
    Option<GpuFrame>,
);

// --- U2: tools & dialogs ---
impl Worker {
    fn spawn_export(
        &mut self,
        items: Vec<ExportItem>,
        default_settings: ExposureSettings,
        options: ExportOptions,
        generation: u64,
        cancel: Arc<AtomicBool>,
    ) {
        self.export_jobs.retain(|(_, h)| !h.is_finished());
        let backend = Arc::clone(&self.backend);
        let renderer = Arc::clone(&self.renderer);
        let tx = self.evt_tx.clone();
        let flag = Arc::clone(&cancel);
        let spawned = std::thread::Builder::new()
            .name("negui-export".into())
            .spawn(move || {
                run_export(
                    &backend,
                    &renderer,
                    &items,
                    &default_settings,
                    &options,
                    generation,
                    &flag,
                    &tx,
                )
            });
        match spawned {
            Ok(handle) => self.export_jobs.push((cancel, handle)),
            Err(e) => {
                log::error!("export: could not spawn the export thread: {e}");
                self.emit(Evt::ExportFinished {
                    generation,
                    total: 0,
                    written: Vec::new(),
                    failed: vec![(PathBuf::new(), e.to_string())],
                    cancelled: false,
                });
            }
        }
    }

    fn enable_texture_target(&mut self, sink: TextureSink) {
        match TextureRing::new(&self.renderer, sink) {
            Some(ring) => {
                log::info!("engine: presenting through shared GPU textures");
                self.texture = Some(ring);
            }
            None => log::warn!("engine: texture target refused (not a GPU renderer)"),
        }
    }

    /// The zero-readback display path for the ENHANCED tiers: the frame lands in a texture of
    /// the ring (a LEASED slot — never one on screen or in flight to the UI) and the returned
    /// output carries an EMPTY RGBA8 frame; the texture rides the same `Evt::Rendered`, and an [`Evt::ProbeFrame`] (a proxy
    /// readback of the same snapshot, ~6 MB) feeds the densitometer / zone overlay, which read
    /// displayed bytes. The proxy tier keeps the readback path: its bytes ARE the probe, so a
    /// texture would only add a second render. On a size change (the ring's textures are the
    /// wrong size) the frame comes back through `render_into` once — bytes the presenter
    /// uploads — and the ring is resized for the next render. `None` = use the readback path.
    fn render_to_target(
        &mut self,
        path: &Path,
        variant: &Option<ExposureSettings>,
        tool: ToolMode,
        request: RenderRequest,
        generation: u64,
    ) -> Option<Result<RenderDone, String>> {
        if request.tier == negsession::RenderTier::Proxy {
            return None;
        }
        let mut ring = self.texture.take()?;
        let result = self.render_with_ring(&mut ring, path, variant, tool, request, generation);
        if ring.is_usable() {
            self.texture = Some(ring);
        }
        Some(result)
    }

    fn render_with_ring(
        &mut self,
        ring: &mut TextureRing,
        path: &Path,
        variant: &Option<ExposureSettings>,
        tool: ToolMode,
        mut request: RenderRequest,
        generation: u64,
    ) -> Result<RenderDone, String> {
        let evt_tx = self.evt_tx.clone();
        let Some(s) = self.session_mut(path) else {
            return Err("session vanished".into());
        };
        // The same snapshot `render_tool_preview` / the baseline branch would render.
        request.uncropped = tool != ToolMode::None;
        let settings = match variant {
            Some(v) => v.clone(),
            None => {
                let mut eff = s.settings().clone();
                if tool == ToolMode::Crop {
                    eff.fine_rotation = 0.0;
                }
                eff
            }
        };
        let t0 = Instant::now();
        if let Some((lease, texture)) = ring.pick() {
            let tried = (texture.width(), texture.height());
            match s.render_to_texture_with(&settings, request, &texture) {
                Ok(meta) => {
                    let render_time = t0.elapsed();
                    let gpu = ring.frame(lease, texture);
                    // The densitometer's bytes: the same snapshot on the proxy, no histogram.
                    let probe_request = RenderRequest {
                        tier: negsession::RenderTier::Proxy,
                        compute_histogram: false,
                        ..request
                    };
                    let mut bytes = Vec::new();
                    match s.render_into_with(&settings, probe_request, &mut bytes) {
                        Ok(probe) => {
                            // Delivered with the Rendered event that follows (same poll).
                            let _ = evt_tx.send(Evt::ProbeFrame {
                                path: path.to_path_buf(),
                                generation,
                                output: Box::new(RenderOutput {
                                    frame: Frame::Rgba8(bytes),
                                    meta: probe,
                                }),
                            });
                        }
                        Err(e) => log::debug!("engine: probe render failed: {e}"),
                    }
                    let info = s.session_info().ok();
                    let output = RenderOutput {
                        frame: Frame::Rgba8(Vec::new()),
                        meta,
                    };
                    return Ok((output, info, render_time, Some(gpu)));
                }
                Err(e) => ring.note_failure(tried, &e.to_string()),
            }
        }
        let mut bytes = Vec::new();
        let meta = s
            .render_into_with(&settings, request, &mut bytes)
            .map_err(|e| e.to_string())?;
        let render_time = t0.elapsed();
        ring.resize(meta.width, meta.height);
        let info = s.session_info().ok();
        Ok((
            RenderOutput {
                frame: Frame::Rgba8(bytes),
                meta,
            },
            info,
            render_time,
            None,
        ))
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        for (cancel, _) in &self.export_jobs {
            cancel.store(true, Ordering::Relaxed);
        }
        for (_, handle) in self.export_jobs.drain(..) {
            let _ = handle.join();
        }
    }
}

/// The export loop (`performExport`), one frame at a time.
#[allow(clippy::too_many_arguments)]
fn run_export(
    backend: &Arc<dyn RawBackend>,
    renderer: &Arc<dyn Renderer>,
    items: &[ExportItem],
    default_settings: &ExposureSettings,
    options: &ExportOptions,
    generation: u64,
    cancel: &AtomicBool,
    tx: &Sender<Evt>,
) {
    let total = items.len();
    let mut written = Vec::new();
    let mut failed = Vec::new();
    let mut cancelled = false;
    for (done, item) in items.iter().enumerate() {
        if cancel.load(Ordering::Relaxed) {
            cancelled = true;
            break;
        }
        let _ = tx.send(Evt::ExportProgress {
            generation,
            done,
            total,
            current: item.path.clone(),
        });
        let settings = item
            .settings
            .clone()
            .or_else(|| SidecarStore::load(&item.path))
            .unwrap_or_else(|| default_settings.clone());
        let outcome = Session::open(&item.path, Arc::clone(backend), Arc::clone(renderer))
            .and_then(|mut session| {
                session.set_settings(settings);
                Exporter::write(&mut session, options)
            });
        match outcome {
            Ok(p) => {
                log::info!("export: wrote {}", p.display());
                written.push(p);
            }
            Err(e) => {
                log::warn!("export: {} failed: {e}", item.path.display());
                failed.push((item.path.clone(), e.to_string()));
            }
        }
    }
    let _ = tx.send(Evt::ExportFinished {
        generation,
        total,
        written,
        failed,
        cancelled,
    });
}
// --- end U2 ---
// --- U3: darkroom ---
/// The darkroom layers render on the proxy, cropped, sRGB (the canvas' encoding).
fn darkroom_request() -> RenderRequest {
    RenderRequest {
        srgb: true,
        compute_histogram: false,
        ..RenderRequest::default()
    }
}

impl Worker {
    fn darkroom(
        &mut self,
        path: &Path,
        settings: ExposureSettings,
        cmd: DarkroomCmd,
    ) -> DarkroomEvt {
        let session: Result<&mut Session, String> = if self.position(path).is_none() {
            self.open(path, settings.clone())
        } else {
            self.session_mut(path)
                .ok_or_else(|| "session vanished".to_owned())
        };
        let session = match session {
            Ok(s) => {
                s.set_settings(settings.clone());
                Ok(s)
            }
            Err(e) => Err(e),
        };
        match cmd {
            DarkroomCmd::TestStrip { orientation } => {
                let t0 = Instant::now();
                let result = session.and_then(|s| {
                    s.render_test_strip_with(
                        &settings,
                        TestStripRequest {
                            orientation,
                            srgb: true,
                        },
                    )
                    .map(Box::new)
                    .map_err(|e| e.to_string())
                });
                DarkroomEvt::TestStrip {
                    result,
                    build_time: t0.elapsed(),
                }
            }
            DarkroomCmd::Preview => DarkroomEvt::Preview {
                result: session.and_then(|s| {
                    s.render_with(&settings, darkroom_request())
                        .map(Box::new)
                        .map_err(|e| e.to_string())
                }),
            },
            DarkroomCmd::ZoneSample { u, v, armed, op } => DarkroomEvt::ZoneSampled {
                op,
                result: session.and_then(|s| {
                    let sample = s.sample_zone_pin(u, v, 2).map_err(|e| e.to_string())?;
                    let target = match armed {
                        Some(z) => z,
                        None => {
                            let z = s
                                .predicted_zone(&settings, sample.val_luma, sample.mask_val)
                                .map_err(|e| e.to_string())?;
                            (z * 3.0).round() / 3.0
                        }
                    };
                    let mut pin = negkit::zone_placement::Pin::new(
                        u,
                        v,
                        sample.val_rgb,
                        sample.val_luma,
                        target,
                    );
                    pin.retargeted = armed.is_some();
                    pin.mask_val = sample.mask_val;
                    Ok(pin)
                }),
            },
            DarkroomCmd::ZoneSolve { pins } => match session {
                Err(_) => DarkroomEvt::ZoneSolved {
                    labels: Vec::new(),
                    solution: None,
                    preview: None,
                },
                Ok(s) => {
                    let labels = pins
                        .iter()
                        .map(|p| {
                            s.predicted_zone(&settings, p.val_luma, 0.0)
                                .map(negsession::densitometer::zone_roman)
                                .unwrap_or_else(|_| "–".to_owned())
                        })
                        .collect();
                    let solution = s.solve_zone_placement(&settings, &pins).ok().flatten();
                    let preview = solution.as_ref().and_then(|sol| {
                        s.render_with(&sol.settings, darkroom_request())
                            .ok()
                            .map(Box::new)
                    });
                    DarkroomEvt::ZoneSolved {
                        labels,
                        solution: solution.map(Box::new),
                        preview,
                    }
                }
            },
        }
    }
}
// --- end U3 ---
