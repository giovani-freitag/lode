//! Crash-safe file writes. `std::fs::write` truncates the file when it opens it and only then writes
//! the new bytes, so an interrupt (Ctrl-C, power loss, OOM, a full disk) between the two leaves a
//! zero-length or half-written file. For durable state — the hand-authored `lode.json` and the
//! generated `lode.lock` — that means a mistimed interrupt could destroy the source of truth.
//! Staging into a sibling temp file and renaming over the target makes the swap atomic on one
//! volume: a reader only ever sees the whole old file or the whole new one, never a torn one.

use std::io::Write;
use std::path::Path;

use anyhow::{Context, Result};

/// Write `contents` to `path` atomically: stage into a temp file in the *same directory* (so the
/// final rename stays on one volume and is atomic), fsync it, then rename over `path`.
pub fn write(path: &Path, contents: impl AsRef<[u8]>) -> Result<()> {
    let dir = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let mut tmp = tempfile::NamedTempFile::new_in(dir)
        .with_context(|| format!("staging a temp file next to {}", path.display()))?;
    tmp.write_all(contents.as_ref())
        .with_context(|| format!("writing {}", path.display()))?;
    tmp.as_file()
        .sync_all()
        .with_context(|| format!("flushing {}", path.display()))?;
    tmp.persist(path)
        .map_err(|e| e.error)
        .with_context(|| format!("finalizing {}", path.display()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn write_creates_then_atomically_replaces() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state.json");

        write(&path, b"first").unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "first");

        // A second write replaces the whole contents via rename-over — never appends or half-writes.
        write(&path, b"second value").unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "second value");
    }

    #[test]
    fn write_leaves_no_stray_temp_files_behind() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state.json");

        write(&path, b"x").unwrap();

        let entries: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(entries, vec![std::ffi::OsString::from("state.json")]);
    }
}
