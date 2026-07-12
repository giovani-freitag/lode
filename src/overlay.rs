use std::path::{Path, PathBuf};

use crate::manifest::Manifest;
use crate::side::Side;

/// A concrete non-mod file (config, script, resource) the pack ships alongside its mods.
pub struct OverlayFile {
    /// Path relative to the pack root, forward-slashed (matches packwiz index paths).
    pub rel: String,
    pub abs: PathBuf,
    pub side: Side,
}

/// Resolve the manifest's overlay globs to concrete files under the pack root. A glob like
/// `config/**` is treated as "every file under `config/`"; each file inherits the overlay's side
/// (falling back to the pack's default side).
pub fn collect(root: &Path, manifest: &Manifest) -> Vec<OverlayFile> {
    let mut out = Vec::new();
    for overlay in &manifest.overlays {
        let side = overlay.side.unwrap_or(manifest.defaults.side);
        let dir_part = overlay
            .path
            .trim_end_matches("/**")
            .trim_end_matches("/*")
            .trim_end_matches('/');
        walk(&root.join(dir_part), root, side, &mut out);
    }
    out.sort_by(|a, b| a.rel.cmp(&b.rel));
    out
}

fn walk(dir: &Path, root: &Path, side: Side, out: &mut Vec<OverlayFile>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            walk(&path, root, side, out);
        } else if path.is_file() {
            if let Ok(rel) = path.strip_prefix(root) {
                out.push(OverlayFile {
                    rel: rel.to_string_lossy().replace('\\', "/"),
                    abs: path.clone(),
                    side,
                });
            }
        }
    }
}
