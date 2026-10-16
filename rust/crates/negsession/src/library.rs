//! Library folder scan — `AppModel.buildTree` / `flatten` in `Sources/SwiftInvert/AppModel.swift`
//! (the VSCode-style sidebar tree the Qt shell also follows).
//!
//! Rules: RAW files per folder (`negraw::is_raw_file`), name-sorted with a natural
//! (digit-aware, case-insensitive) order — `localizedStandardCompare`'s useful core — hidden
//! entries skipped, a folder's own files before its subfolders, RAW-less branches pruned, depth
//! capped at 8 below the root. `flatten` is the depth-first order behind index numbers, ←/→
//! navigation and shift-click ranges. Thumbnails are the embedded camera JPEG bytes
//! ([`thumbnail`]); the UI decodes them.

use std::cmp::Ordering;
use std::path::{Path, PathBuf};

use negraw::RawBackend;

/// Deepest folder level scanned below the root (`depth <= 8`).
pub const MAX_SCAN_DEPTH: usize = 8;

/// One folder of the tree.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FolderNode {
    pub path: PathBuf,
    pub name: String,
    /// This folder's own RAW files, natural-sorted.
    pub files: Vec<PathBuf>,
    /// Subfolders that contain RAW files somewhere below, natural-sorted.
    pub subfolders: Vec<FolderNode>,
    /// Files in this folder and every subfolder (the "name — N" badge).
    pub total_count: usize,
}

/// Natural string order: digit runs compare numerically (leading zeros break ties by length),
/// everything else case-insensitively; a full tie falls back to the plain byte order so the
/// sort is total.
pub fn natural_cmp(a: &str, b: &str) -> Ordering {
    let (ab, bb) = (a.as_bytes(), b.as_bytes());
    let (mut i, mut j) = (0usize, 0usize);
    while i < ab.len() && j < bb.len() {
        let (ca, cb) = (ab[i], bb[j]);
        if ca.is_ascii_digit() && cb.is_ascii_digit() {
            let (si, sj) = (i, j);
            while i < ab.len() && ab[i].is_ascii_digit() {
                i += 1;
            }
            while j < bb.len() && bb[j].is_ascii_digit() {
                j += 1;
            }
            let da = trim_zeros(&ab[si..i]);
            let db = trim_zeros(&bb[sj..j]);
            let ord = da.len().cmp(&db.len()).then_with(|| da.cmp(db));
            if ord != Ordering::Equal {
                return ord;
            }
            // Same value: more leading zeros sorts first (`01` before `1`, as the Finder's
            // character fallback orders them) — a deterministic tie-break.
            let ord = (j - sj).cmp(&(i - si));
            if ord != Ordering::Equal {
                return ord;
            }
        } else {
            let ord = ca.to_ascii_lowercase().cmp(&cb.to_ascii_lowercase());
            if ord != Ordering::Equal {
                return ord;
            }
            i += 1;
            j += 1;
        }
    }
    (ab.len() - i).cmp(&(bb.len() - j)).then_with(|| a.cmp(b))
}

fn trim_zeros(digits: &[u8]) -> &[u8] {
    let first = digits
        .iter()
        .position(|&d| d != b'0')
        .unwrap_or(digits.len());
    &digits[first..]
}

fn file_name(p: &Path) -> String {
    p.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}

fn is_hidden(p: &Path) -> bool {
    file_name(p).starts_with('.')
}

/// Recursive scan from `root` (depth 0). `None` when no RAW file lives anywhere below.
pub fn build_tree(root: &Path) -> Option<FolderNode> {
    build_at(root, 0)
}

fn build_at(dir: &Path, depth: usize) -> Option<FolderNode> {
    if depth > MAX_SCAN_DEPTH {
        return None;
    }
    let entries: Vec<PathBuf> = std::fs::read_dir(dir)
        .map(|rd| rd.flatten().map(|e| e.path()).collect())
        .unwrap_or_default();
    let mut files: Vec<PathBuf> = Vec::new();
    let mut dirs: Vec<PathBuf> = Vec::new();
    for p in entries {
        if is_hidden(&p) {
            continue;
        }
        // `metadata` follows symlinks, like URLResourceKey's isDirectory.
        let Ok(meta) = std::fs::metadata(&p) else {
            continue;
        };
        if meta.is_dir() {
            dirs.push(p);
        } else if meta.is_file() && negraw::is_raw_file(&p) {
            files.push(p);
        }
    }
    files.sort_by(|a, b| natural_cmp(&file_name(a), &file_name(b)));
    dirs.sort_by(|a, b| natural_cmp(&file_name(a), &file_name(b)));
    let subfolders: Vec<FolderNode> = dirs.iter().filter_map(|d| build_at(d, depth + 1)).collect();
    if files.is_empty() && subfolders.is_empty() {
        return None;
    }
    let total_count = files.len() + subfolders.iter().map(|s| s.total_count).sum::<usize>();
    Some(FolderNode {
        path: dir.to_path_buf(),
        name: file_name(dir),
        files,
        subfolders,
        total_count,
    })
}

/// Depth-first: a folder's own files first, then its subfolders.
pub fn flatten(node: &FolderNode) -> Vec<PathBuf> {
    let mut out = Vec::with_capacity(node.total_count);
    flatten_into(node, &mut out);
    out
}

fn flatten_into(node: &FolderNode, out: &mut Vec<PathBuf>) {
    out.extend(node.files.iter().cloned());
    for sub in &node.subfolders {
        flatten_into(sub, out);
    }
}

/// The embedded camera JPEG for the library grid (`None` when the file has none).
pub fn thumbnail(backend: &dyn RawBackend, path: &Path) -> crate::Result<Option<Vec<u8>>> {
    Ok(backend.thumbnail(path)?)
}
