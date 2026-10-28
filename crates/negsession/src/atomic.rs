//! Atomic file replacement shared by the sidecar store, the profile store and the exporter:
//! write a temp file IN THE TARGET'S DIRECTORY (so the rename is a same-filesystem move),
//! `sync_all` it, then rename over the target. A crash, a full disk or a failed encode leaves
//! the previous file intact; a reader never sees a half-written one.
//!
//! The temp name is unique per CALL (pid + a process-wide counter + the thread id), so two
//! saves of one file racing from different threads — the debounced sidecar save and an export
//! flush, say — can never rename each other's half-written temp into place (review 7 finding 1).

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use crate::{Result, SessionError};

static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

/// A fresh temp path beside `target`: `<file name>.tmp-<pid>-<n>-<thread>`. Unique per call
/// within the process (the counter) and across processes (the pid).
pub fn temp_path(target: &Path) -> PathBuf {
    let n = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
    let mut h = DefaultHasher::new();
    std::thread::current().id().hash(&mut h);
    let thread = h.finish() & 0xFFFF;
    let mut name = target
        .file_name()
        .map(|f| f.to_os_string())
        .unwrap_or_default();
    name.push(format!(".tmp-{}-{n}-{thread:x}", std::process::id()));
    target.with_file_name(name)
}

/// Write `bytes` to `target` atomically (temp + fsync + rename). On any failure the temp is
/// removed and `target` is untouched.
pub fn write_atomic(target: &Path, bytes: &[u8]) -> Result<()> {
    write_atomic_with(target, |file| file.write_all(bytes))
}

/// [`write_atomic`] for producers that stream into the file (encoders): `fill` writes the
/// content into the temp file; on `Err` or a failed sync/rename the temp is removed and
/// `target` stays as it was.
pub fn write_atomic_with(
    target: &Path,
    fill: impl FnOnce(&mut std::fs::File) -> std::io::Result<()>,
) -> Result<()> {
    let tmp = temp_path(target);
    let outcome = (|| -> std::io::Result<()> {
        let mut file = std::fs::File::create(&tmp)?;
        fill(&mut file)?;
        file.flush()?;
        file.sync_all()?;
        drop(file);
        std::fs::rename(&tmp, target)
    })();
    match outcome {
        Ok(()) => Ok(()),
        Err(e) => {
            let _ = std::fs::remove_file(&tmp);
            Err(SessionError::io(target, e))
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
    use super::*;

    #[test]
    fn temp_paths_are_unique_and_beside_the_target() {
        let target = Path::new("/roll/IMG_0001.swiftinvert.json");
        let a = temp_path(target);
        let b = temp_path(target);
        assert_ne!(a, b);
        assert_eq!(a.parent(), target.parent());
        assert!(
            a.file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("IMG_0001.swiftinvert.json.tmp-")
        );
        let other = std::thread::spawn(move || temp_path(target))
            .join()
            .unwrap();
        assert_ne!(other, a);
        assert_ne!(other, b);
    }
}
