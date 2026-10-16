//! The library sidebar — `LibraryView.swift` + `AppModel.buildTree/flatten` (controls-spec §12):
//! a VSCode-style folder tree whose folder rows read "name — N" and toggle on a single click
//! (collapse state persisted per root, the Qt rule), each folder's RAW files under it as an
//! adaptive thumbnail grid (120…180 pt cells, 3:2, accent halo, index badge). Thumbnails are the
//! embedded camera JPEGs, decoded on a worker thread; textures are created on the UI thread only.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc::{Receiver, Sender, TryRecvError, channel};

use egui::{Align2, Color32, Rect, Sense, Stroke, StrokeKind, TextureHandle, pos2, vec2};
use negsession::{FolderNode, RawBackend, build_tree, flatten};

use crate::theme::Tokens;

/// Thumbnail long edge (ThumbnailStore.swift: max pixel 384).
pub const THUMB_MAX_PX: u32 = 384;
/// Grid cell width range and spacing.
pub const CELL_MIN: f32 = 120.0;
pub const CELL_MAX: f32 = 180.0;
pub const CELL_GAP: f32 = 8.0;
pub const INDENT: f32 = 14.0;

enum ThumbSlot {
    Pending,
    Missing,
    Ready(TextureHandle, [usize; 2]),
}

/// What the library asked the app to do.
#[derive(Clone, Debug, PartialEq)]
pub enum LibraryAction {
    Select {
        path: PathBuf,
        additive: bool,
        range: bool,
    },
    ChooseFolder,
    ToggleLibrary,
    /// Collapse state changed (persist).
    CollapsedChanged,
}

/// Library state: the scanned tree, the flat order, collapse state and the thumbnail cache.
pub struct LibraryState {
    pub root: Option<PathBuf>,
    pub tree: Option<FolderNode>,
    pub flat: Vec<PathBuf>,
    flat_index: HashMap<PathBuf, usize>,
    /// Collapsed folder identities ("a/b/c" relative to the root).
    pub collapsed: HashSet<String>,
    pub scanning: bool,
    scan_rx: Option<Receiver<(PathBuf, Option<FolderNode>)>>,
    thumbs: HashMap<PathBuf, ThumbSlot>,
    thumb_tx: Sender<PathBuf>,
    thumb_rx: Receiver<(PathBuf, Option<egui::ColorImage>)>,
    scroll_to: Option<PathBuf>,
}

impl LibraryState {
    /// Create the state and its thumbnail worker.
    pub fn new(backend: Arc<dyn RawBackend>, repaint: impl Fn() + Send + 'static) -> Self {
        let (thumb_tx, req_rx) = channel::<PathBuf>();
        let (done_tx, thumb_rx) = channel::<(PathBuf, Option<egui::ColorImage>)>();
        let spawned = std::thread::Builder::new()
            .name("negui-thumbs".into())
            .spawn(move || {
                while let Ok(path) = req_rx.recv() {
                    let img = decode_thumbnail(backend.as_ref(), &path);
                    if done_tx.send((path, img)).is_err() {
                        return;
                    }
                    repaint();
                }
            });
        if spawned.is_err() {
            log::error!("library: could not spawn the thumbnail thread");
        }
        LibraryState {
            root: None,
            tree: None,
            flat: Vec::new(),
            flat_index: HashMap::new(),
            collapsed: HashSet::new(),
            scanning: false,
            scan_rx: None,
            thumbs: HashMap::new(),
            thumb_tx,
            thumb_rx,
            scroll_to: None,
        }
    }

    /// Start scanning `root` on a thread (`build_tree`); `poll` adopts the result.
    pub fn open_folder(
        &mut self,
        root: PathBuf,
        collapsed: HashSet<String>,
        repaint: impl Fn() + Send + 'static,
    ) {
        self.root = Some(root.clone());
        self.collapsed = collapsed;
        self.scanning = true;
        self.thumbs.clear();
        let (tx, rx) = channel();
        self.scan_rx = Some(rx);
        let spawned = std::thread::Builder::new()
            .name("negui-scan".into())
            .spawn(move || {
                let tree = build_tree(&root);
                let _ = tx.send((root, tree));
                repaint();
            });
        if spawned.is_err() {
            self.scanning = false;
            self.scan_rx = None;
        }
    }

    /// Adopt a finished scan and uploaded thumbnails. Returns true when the tree changed.
    pub fn poll(&mut self, ctx: &egui::Context) -> bool {
        let mut changed = false;
        if let Some(rx) = &self.scan_rx {
            match rx.try_recv() {
                Ok((root, tree)) => {
                    if self.root.as_deref() == Some(root.as_path()) {
                        self.flat = tree.as_ref().map(flatten).unwrap_or_default();
                        self.flat_index = self
                            .flat
                            .iter()
                            .enumerate()
                            .map(|(i, p)| (p.clone(), i))
                            .collect();
                        self.tree = tree;
                        self.scanning = false;
                        self.scan_rx = None;
                        changed = true;
                    }
                }
                Err(TryRecvError::Disconnected) => {
                    self.scanning = false;
                    self.scan_rx = None;
                }
                Err(TryRecvError::Empty) => {}
            }
        }
        loop {
            match self.thumb_rx.try_recv() {
                Ok((path, Some(img))) => {
                    let size = img.size;
                    let name = path
                        .file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_default();
                    let tex = ctx.load_texture(
                        format!("thumb:{name}"),
                        img,
                        egui::TextureOptions::LINEAR,
                    );
                    self.thumbs.insert(path, ThumbSlot::Ready(tex, size));
                }
                Ok((path, None)) => {
                    self.thumbs.insert(path, ThumbSlot::Missing);
                }
                Err(_) => break,
            }
        }
        changed
    }

    /// 1-based index in the flat order (the cell badge).
    pub fn index_of(&self, path: &Path) -> Option<usize> {
        self.flat_index.get(path).map(|i| i + 1)
    }

    /// The file `delta` steps from `current` in flat order (clamped at the ends; the first file
    /// when `current` is unknown).
    pub fn adjacent(&self, current: Option<&Path>, delta: isize) -> Option<PathBuf> {
        if self.flat.is_empty() {
            return None;
        }
        let i = current.and_then(|c| self.flat_index.get(c).copied());
        let next = match i {
            Some(i) => (i as isize + delta).clamp(0, self.flat.len() as isize - 1) as usize,
            None => 0,
        };
        self.flat.get(next).cloned()
    }

    /// Files between `from` and `to` inclusive, in flat order (⇧-click).
    pub fn range(&self, from: &Path, to: &Path) -> Vec<PathBuf> {
        match (self.flat_index.get(from), self.flat_index.get(to)) {
            (Some(&a), Some(&b)) => {
                let (lo, hi) = (a.min(b), a.max(b));
                self.flat
                    .get(lo..=hi)
                    .map(|s| s.to_vec())
                    .unwrap_or_default()
            }
            _ => vec![to.to_path_buf()],
        }
    }

    /// Whether `path` is in the current tree.
    pub fn contains(&self, path: &Path) -> bool {
        self.flat_index.contains_key(path)
    }

    /// Scroll the selected cell into view on the next frame.
    pub fn reveal(&mut self, path: &Path) {
        self.scroll_to = Some(path.to_path_buf());
    }

    /// Whether the thumbnail of `path` has been resolved (ready or known missing).
    pub fn thumbnail_settled(&self, path: &Path) -> bool {
        matches!(
            self.thumbs.get(path),
            Some(ThumbSlot::Ready(..) | ThumbSlot::Missing)
        )
    }

    fn request_thumb(&mut self, path: &Path) {
        if self.thumbs.contains_key(path) {
            return;
        }
        if self.thumb_tx.send(path.to_path_buf()).is_ok() {
            self.thumbs.insert(path.to_path_buf(), ThumbSlot::Pending);
        }
    }

    fn identity(&self, folder: &Path) -> String {
        let rel = self
            .root
            .as_deref()
            .and_then(|r| folder.strip_prefix(r).ok())
            .unwrap_or(folder);
        rel.components()
            .map(|c| c.as_os_str().to_string_lossy().into_owned())
            .collect::<Vec<_>>()
            .join("/")
    }
}

/// Decode the embedded JPEG (EXIF orientation applied), bounded to [`THUMB_MAX_PX`].
fn decode_thumbnail(backend: &dyn RawBackend, path: &Path) -> Option<egui::ColorImage> {
    let bytes = backend.thumbnail(path).ok().flatten()?;
    let cursor = std::io::Cursor::new(bytes);
    let mut decoder = image::codecs::jpeg::JpegDecoder::new(cursor).ok()?;
    let orientation = image::ImageDecoder::orientation(&mut decoder)
        .unwrap_or(image::metadata::Orientation::NoTransforms);
    let mut img = image::DynamicImage::from_decoder(decoder).ok()?;
    img.apply_orientation(orientation);
    let (w, h) = (img.width(), img.height());
    if w == 0 || h == 0 {
        return None;
    }
    let scale = f64::from(THUMB_MAX_PX) / f64::from(w.max(h));
    let img = if scale < 1.0 {
        let nw = ((f64::from(w) * scale).round() as u32).max(1);
        let nh = ((f64::from(h) * scale).round() as u32).max(1);
        img.thumbnail_exact(nw, nh)
    } else {
        img
    };
    let rgba = img.to_rgba8();
    let size = [rgba.width() as usize, rgba.height() as usize];
    Some(egui::ColorImage::from_rgba_unmultiplied(
        size,
        rgba.as_raw(),
    ))
}

/// The library panel contents (header + tree).
pub fn library_panel(
    ui: &mut egui::Ui,
    lib: &mut LibraryState,
    selection: Option<&Path>,
    multi: &HashSet<PathBuf>,
) -> Vec<LibraryAction> {
    let t = Tokens::get(ui.ctx());
    let mut actions = Vec::new();
    ui.horizontal(|ui| {
        let name = lib
            .root
            .as_deref()
            .and_then(|r| r.file_name())
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "Library".to_owned());
        ui.add(egui::Label::new(egui::RichText::new(name).size(12.5).strong()).truncate())
            .on_hover_text(
                lib.root
                    .as_deref()
                    .map(|p| p.display().to_string())
                    .unwrap_or_default(),
            );
        if lib.scanning {
            ui.add(egui::Spinner::new().size(12.0));
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui
                .add(egui::Button::new(egui::RichText::new("◧").size(15.0)).frame(false))
                .on_hover_text(crate::app::with_shortcut(
                    ui.ctx(),
                    "Hide library",
                    crate::app::Command::ToggleLibrary,
                ))
                .clicked()
            {
                actions.push(LibraryAction::ToggleLibrary);
            }
            if ui
                .add(egui::Button::new(egui::RichText::new("▤").size(15.0)).frame(false))
                .on_hover_text(crate::app::with_shortcut(
                    ui.ctx(),
                    "Open folder…",
                    crate::app::Command::OpenFolder,
                ))
                .clicked()
            {
                actions.push(LibraryAction::ChooseFolder);
            }
        });
    });
    ui.separator();
    egui::ScrollArea::vertical()
        .id_salt("library-scroll")
        .auto_shrink([false, false])
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            match lib.tree.take() {
                Some(tree) => {
                    folder_body(ui, lib, &tree, 0, selection, multi, &mut actions);
                    for sub in &tree.subfolders {
                        folder_node(ui, lib, sub, 0, selection, multi, &mut actions);
                    }
                    lib.tree = Some(tree);
                }
                None => {
                    if !lib.scanning {
                        ui.add_space(20.0);
                        ui.vertical_centered(|ui| {
                            ui.label(
                                egui::RichText::new(if lib.root.is_some() {
                                    "No RAW files here"
                                } else {
                                    "Open a folder to begin"
                                })
                                .color(t.text_tertiary),
                            );
                        });
                    }
                }
            }
            lib.scroll_to = None;
        });
    actions
}

fn folder_node(
    ui: &mut egui::Ui,
    lib: &mut LibraryState,
    node: &FolderNode,
    depth: usize,
    selection: Option<&Path>,
    multi: &HashSet<PathBuf>,
    actions: &mut Vec<LibraryAction>,
) {
    let t = Tokens::get(ui.ctx());
    let identity = lib.identity(&node.path);
    let collapsed = lib.collapsed.contains(&identity);
    let indent = INDENT * depth as f32;
    let (row, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 22.0), Sense::click());
    if resp.hovered() {
        ui.painter().rect_filled(row, 3.0, t.hover);
    }
    let p = ui.painter();
    let cx = row.left() + indent + 8.0;
    let cy = row.center().y;
    let pts = if collapsed {
        vec![
            pos2(cx - 2.0, cy - 4.0),
            pos2(cx + 2.0, cy),
            pos2(cx - 2.0, cy + 4.0),
        ]
    } else {
        vec![
            pos2(cx - 4.0, cy - 2.0),
            pos2(cx, cy + 2.0),
            pos2(cx + 4.0, cy - 2.0),
        ]
    };
    p.add(egui::Shape::line(pts, Stroke::new(1.5, t.text_secondary)));
    // folder glyph
    let fx = cx + 12.0;
    p.rect_filled(
        Rect::from_min_size(pos2(fx, cy - 4.0), vec2(12.0, 9.0)),
        1.5,
        t.text_secondary.gamma_multiply(0.8),
    );
    p.rect_filled(
        Rect::from_min_size(pos2(fx, cy - 6.0), vec2(5.0, 3.0)),
        1.0,
        t.text_secondary.gamma_multiply(0.8),
    );
    p.text(
        pos2(fx + 18.0, cy),
        Align2::LEFT_CENTER,
        format!("{} — {}", node.name, node.total_count),
        t.font(12.0),
        t.text,
    );
    if resp.clicked() {
        if collapsed {
            lib.collapsed.remove(&identity);
        } else {
            lib.collapsed.insert(identity);
        }
        actions.push(LibraryAction::CollapsedChanged);
        return;
    }
    if collapsed {
        return;
    }
    folder_body(ui, lib, node, depth + 1, selection, multi, actions);
    for sub in &node.subfolders {
        folder_node(ui, lib, sub, depth + 1, selection, multi, actions);
    }
}

/// The thumbnail grid of a folder's OWN files.
fn folder_body(
    ui: &mut egui::Ui,
    lib: &mut LibraryState,
    node: &FolderNode,
    depth: usize,
    selection: Option<&Path>,
    multi: &HashSet<PathBuf>,
    actions: &mut Vec<LibraryAction>,
) {
    if node.files.is_empty() {
        return;
    }
    let t = Tokens::get(ui.ctx());
    let indent = INDENT * depth as f32;
    let avail = (ui.available_width() - indent - 4.0).max(CELL_MIN);
    let cols = ((avail + CELL_GAP) / (CELL_MIN + CELL_GAP))
        .floor()
        .max(1.0) as usize;
    let cell_w = ((avail - CELL_GAP * (cols as f32 - 1.0)) / cols as f32).min(CELL_MAX);
    let cell_h = cell_w * 2.0 / 3.0;
    let rows = node.files.len().div_ceil(cols);
    let total_h = rows as f32 * (cell_h + CELL_GAP);
    let (area, _) = ui.allocate_exact_size(vec2(ui.available_width(), total_h), Sense::hover());
    let modifiers = ui.input(|i| i.modifiers);
    for (k, path) in node.files.iter().enumerate() {
        let (r, c) = (k / cols, k % cols);
        let min = pos2(
            area.left() + indent + c as f32 * (cell_w + CELL_GAP),
            area.top() + r as f32 * (cell_h + CELL_GAP),
        );
        let cell = Rect::from_min_size(min, vec2(cell_w, cell_h));
        if lib.scroll_to.as_deref() == Some(path.as_path()) {
            ui.scroll_to_rect(cell, Some(egui::Align::Center));
        }
        let visible = ui.clip_rect().intersects(cell);
        if visible {
            lib.request_thumb(path);
        }
        let resp = ui.interact(cell, ui.id().with(path), Sense::click());
        let p = ui.painter();
        p.rect_filled(cell, 6.0, t.field);
        match lib.thumbs.get(path) {
            Some(ThumbSlot::Ready(tex, size)) => {
                // aspect-fill: crop the texture to the cell's 3:2
                let (iw, ih) = (size[0] as f32, size[1] as f32);
                let cell_aspect = cell.width() / cell.height();
                let img_aspect = iw / ih.max(1.0);
                let uv = if img_aspect > cell_aspect {
                    let frac = cell_aspect / img_aspect;
                    Rect::from_min_max(pos2((1.0 - frac) / 2.0, 0.0), pos2((1.0 + frac) / 2.0, 1.0))
                } else {
                    let frac = img_aspect / cell_aspect;
                    Rect::from_min_max(pos2(0.0, (1.0 - frac) / 2.0), pos2(1.0, (1.0 + frac) / 2.0))
                };
                let mut mesh = egui::Mesh::with_texture(tex.id());
                mesh.add_rect_with_uv(cell.shrink(1.0), uv, Color32::WHITE);
                p.add(egui::Shape::mesh(mesh));
            }
            Some(ThumbSlot::Missing) => {
                p.text(
                    cell.center(),
                    Align2::CENTER_CENTER,
                    "RAW",
                    t.font(11.0),
                    t.text_tertiary,
                );
            }
            _ => {
                p.text(
                    cell.center(),
                    Align2::CENTER_CENTER,
                    "…",
                    t.font(12.0),
                    t.text_tertiary,
                );
            }
        }
        let is_current = selection == Some(path.as_path());
        let is_selected = is_current || multi.contains(path);
        if is_selected {
            p.rect_stroke(
                cell,
                6.0,
                Stroke::new(if is_current { 3.0 } else { 2.0 }, t.accent),
                StrokeKind::Inside,
            );
        } else if resp.hovered() {
            p.rect_stroke(
                cell,
                6.0,
                Stroke::new(1.0, t.button_border),
                StrokeKind::Inside,
            );
        }
        if let Some(idx) = lib.index_of(path) {
            let text = idx.to_string();
            let galley = p.layout_no_wrap(text, t.font(9.0), Color32::WHITE);
            let size = galley.size() + vec2(8.0, 2.0);
            let badge =
                Rect::from_min_size(pos2(cell.left() + 5.0, cell.bottom() - 5.0 - size.y), size);
            p.rect_filled(badge, size.y / 2.0, Color32::from_black_alpha(140));
            p.galley(badge.min + vec2(4.0, 1.0), galley, Color32::WHITE);
        }
        let resp = resp.on_hover_text(
            path.file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default(),
        );
        if resp.clicked() {
            actions.push(LibraryAction::Select {
                path: path.clone(),
                additive: modifiers.command,
                range: modifiers.shift,
            });
        }
    }
}
