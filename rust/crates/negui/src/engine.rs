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

// --- U2: tools & dialogs ---
use std::sync::atomic::{AtomicBool, Ordering};

use negsession::{ExportOptions, Exporter, Frame, SidecarStore};

use crate::presenter_gpu::{TextureRing, TextureSink};

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
    /// The batch ended (finished or cancelled).
    ExportFinished {
        generation: u64,
        total: usize,
        written: Vec<PathBuf>,
        failed: Vec<(PathBuf, String)>,
        cancelled: bool,
    },
    // --- end U2 ---
}

/// The UI's handle: a sender, a receiver and the thread.
pub struct Engine {
    tx: Sender<Cmd>,
    rx: Receiver<Evt>,
    thread: Option<JoinHandle<()>>,
    pub renderer_name: String,
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
                };
                worker.run(cmd_rx);
            })
            .ok();
        if thread.is_none() {
            log::error!("engine: could not spawn the worker thread");
        }
        Engine {
            tx,
            rx,
            thread,
            renderer_name,
        }
    }

    /// Queue a command (dropped silently if the worker is gone).
    pub fn send(&self, cmd: Cmd) {
        if self.tx.send(cmd).is_err() {
            log::warn!("engine: worker gone; command dropped");
        }
    }

    /// Drain pending events.
    pub fn poll(&self) -> Vec<Evt> {
        let mut out = Vec::new();
        while let Ok(e) = self.rx.try_recv() {
            out.push(e);
        }
        out
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
            for (cmd, keep) in queue.into_iter().zip(keep) {
                if keep && !self.handle(cmd) {
                    return;
                }
            }
        }
    }

    /// Returns false on shutdown.
    fn handle(&mut self, cmd: Cmd) -> bool {
        match cmd {
            Cmd::Shutdown => return false,
            Cmd::ClearSessions => self.sessions.clear(),
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
                let result = self.open(&path, settings).map(|_| ());
                self.emit(Evt::Opened {
                    path,
                    generation,
                    result,
                });
            }
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
                    Ok((output, info, render_time)) => self.emit(Evt::Rendered {
                        path,
                        generation,
                        output: Box::new(output),
                        info,
                        render_time,
                        latency: requested_at.elapsed(),
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
    ) -> Result<(RenderOutput, Option<SessionInfo>, Duration), String> {
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
        if let Some(out) = self.render_to_target(path, &variant, tool, request) {
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
        Ok((output, info, render_time))
    }
}

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

    /// The zero-readback display path: the frame lands in a texture of the ring (the one not on
    /// screen) and the returned output carries an EMPTY RGBA8 frame; the presenter picks the
    /// texture up from the sink. On a size change (the ring's textures are the wrong size) the
    /// frame comes back through `render_into` once — bytes the presenter uploads — and the ring
    /// is resized for the next render. `None` when the target is off.
    fn render_to_target(
        &mut self,
        path: &Path,
        variant: &Option<ExposureSettings>,
        tool: ToolMode,
        request: RenderRequest,
    ) -> Option<Result<(RenderOutput, Option<SessionInfo>, Duration), String>> {
        let mut ring = self.texture.take()?;
        let result = self.render_with_ring(&mut ring, path, variant, tool, request);
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
    ) -> Result<(RenderOutput, Option<SessionInfo>, Duration), String> {
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
        if let Some((slot, texture)) = ring.pick() {
            let tried = (texture.width(), texture.height());
            match s.render_to_texture_with(&settings, request, &texture) {
                Ok(meta) => {
                    let render_time = t0.elapsed();
                    ring.deliver(slot, &texture);
                    let info = s.session_info().ok();
                    let output = RenderOutput {
                        frame: Frame::Rgba8(Vec::new()),
                        meta,
                    };
                    return Ok((output, info, render_time));
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
