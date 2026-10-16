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
