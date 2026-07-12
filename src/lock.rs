use std::fs;
use std::path::Path;

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};

use crate::manifest::LoaderSpec;
use crate::provider::{DownloadMode, Provider};
use crate::side::Side;

/// Bump when the lockfile's own shape changes, so `lode` can migrate old locks.
pub const LOCKFILE_VERSION: u32 = 1;

/// The machine-generated lockfile (`lode.lock`) — the exact resolved graph. Never hand-edited.
/// It carries enough to rebuild the packwiz `pack/` and let an installer fetch+verify every
/// file with no re-resolution.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Lock {
    pub lockfile_version: u32,
    /// Integrity hash of the `lode.jsonc` that produced this lock. If the manifest changed,
    /// the lock is stale and must be re-resolved (the `Cargo.lock` vs `Cargo.toml` check).
    pub manifest_hash: String,
    pub loader: LoaderSpec,
    pub resolver: ResolverMeta,
    /// One entry per node — every declared mod plus every transitive dependency.
    pub mods: Vec<LockedMod>,
}

/// Provenance kept intentionally timestamp-free: a resolution that changes nothing must produce
/// a byte-identical lock (determinism), so no `resolvedAt` — only the tool version, which moves
/// solely on upgrade.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResolverMeta {
    pub lode_version: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LockedMod {
    pub slug: String,
    pub name: String,
    pub provider: Provider,
    pub project_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub file_id: Option<String>,
    pub version: String,
    pub filename: String,
    pub download: Download,
    pub side: Side,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub optional: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub dependencies: Vec<DepEdge>,
    /// Reverse edges: which declared mods (or `"manifest"` for a root) caused this node to be
    /// present. Drives `lode why` and safe pruning of orphaned transitive deps — the provenance
    /// packwiz's flat index throws away.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub requested_by: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Download {
    /// Empty for CurseForge (its terms forbid persisting the URL).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    pub mode: DownloadMode,
    /// Provider-native hash format — sha512 (Modrinth), sha1 (CurseForge), sha256 (overlays).
    /// Never normalized to one format: packwiz stores exactly the native one, and byte-for-byte
    /// `pack/` emission depends on it.
    pub hash_format: String,
    pub hash: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub size: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DepEdge {
    pub slug: String,
    pub kind: DepKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DepKind {
    Required,
    Optional,
    Incompatible,
    Embedded,
}

pub const LOCK_FILENAME: &str = "lode.lock";

impl Lock {
    pub fn load(path: &Path) -> Result<Lock> {
        let text =
            fs::read_to_string(path).with_context(|| format!("reading lock {}", path.display()))?;
        let lock: Lock =
            serde_json::from_str(&text).context("lode.lock does not match the lockfile schema")?;
        lock.validate()?;
        Ok(lock)
    }

    /// Reject any lock whose slugs or filenames are not safe single path components — the single
    /// choke point every lock consumer passes through. In a lock authored by whoever published the
    /// pack, both fields become on-disk write destinations (`filename` → `mods/<filename>` and the
    /// `local/` copy source; `slug` → `mods/<slug>.pw.toml`), so a hostile one must not be able to
    /// escape the target tree.
    pub fn validate(&self) -> Result<()> {
        for m in &self.mods {
            safe_component(&m.slug)
                .with_context(|| format!("lockfile entry '{}' has an unsafe slug", m.name))?;
            safe_component(&m.filename)
                .with_context(|| format!("lockfile entry '{}' has an unsafe filename", m.slug))?;
        }
        Ok(())
    }

    pub fn to_json(&self) -> Result<String> {
        let mut json = serde_json::to_string_pretty(self).context("serializing lock")?;
        json.push('\n');
        Ok(json)
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        fs::write(path, self.to_json()?)
            .with_context(|| format!("writing lock {}", path.display()))?;
        Ok(())
    }

    pub fn find(&self, slug: &str) -> Option<&LockedMod> {
        self.mods.iter().find(|m| m.slug == slug)
    }
}

/// Accept only a single, safe path component — rejecting empty, `.`/`..`, path separators, and
/// absolute or Windows drive/reserved-device forms. Each is used as a `<dir>/<component>` join,
/// and `Path::join` honours `..`/absolute components, so an unchecked one escapes its directory.
pub fn safe_component(name: &str) -> Result<()> {
    if name.is_empty() {
        bail!("empty filename");
    }
    if name == "." || name == ".." {
        bail!("filename '{name}' is a directory reference, not a file");
    }
    if name.contains('/') || name.contains('\\') {
        bail!("filename '{name}' contains a path separator");
    }
    // Reject absolute paths (`/x` on Unix) even though a separator check already covers most forms.
    if Path::new(name).is_absolute() {
        bail!("filename '{name}' is an absolute path");
    }
    // A Windows drive-relative form like `C:evil` carries no separator but still redirects writes.
    let bytes = name.as_bytes();
    if bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':' {
        bail!("filename '{name}' contains a drive letter");
    }
    if is_windows_reserved(name) {
        bail!("filename '{name}' is a reserved device name");
    }
    Ok(())
}

/// Whether `name`'s stem is a Windows reserved device name (CON, PRN, AUX, NUL, COM1-9, LPT1-9),
/// which resolves to a device rather than a file even with an extension appended.
fn is_windows_reserved(name: &str) -> bool {
    let stem = name.split('.').next().unwrap_or(name).to_ascii_uppercase();
    if matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL") {
        return true;
    }
    let b = stem.as_bytes();
    b.len() == 4
        && (stem.starts_with("COM") || stem.starts_with("LPT"))
        && b[3].is_ascii_digit()
        && b[3] != b'0'
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn safe_component_rejects_traversal_absolute_and_special_forms() {
        for bad in [
            "", "..", ".", "../x", "/abs", "..\\x", "a/b", "C:\\x", "C:evil", "NUL", "con.txt",
            "COM1", "lpt9.jar",
        ] {
            assert!(safe_component(bad).is_err(), "should reject {bad:?}");
        }
    }

    #[test]
    fn safe_component_accepts_plain_filenames() {
        for good in [
            "mod.jar",
            "sodium-fabric-0.5.8.jar",
            "a.b.c.jar",
            "COM0.jar",
        ] {
            assert!(safe_component(good).is_ok(), "should accept {good:?}");
        }
    }
}
