pub mod add;
pub mod bundle;
pub mod config;
pub mod del;
pub mod export;
pub mod get;
pub mod import;
pub mod init;
pub mod install;
pub mod list;
pub mod pin;
pub mod publish;
pub mod refresh;
pub mod update;
pub mod verify;
pub mod why;

use anyhow::Result;

use crate::lock::Lock;
use crate::manifest::Manifest;
use crate::paths::PackPaths;
use crate::resolve;

/// Resolve the manifest into a lockfile and persist it, **respecting** the existing lock — mods
/// already locked keep their versions; only new ones resolve fresh. The shared core of add, del,
/// and refresh.
pub(crate) fn resolve_and_save(paths: &PackPaths, manifest: &Manifest) -> Result<Lock> {
    let previous = Lock::load(&paths.lock()).ok();
    let lock = crate::ui::spin("Resolving dependencies", "Dependencies resolved", || {
        resolve::resolve(
            manifest,
            previous.as_ref(),
            resolve::ResolveMode::Locked,
            &paths.root,
        )
    })?;
    lock.save(&paths.lock())?;
    Ok(lock)
}

/// Count directly-declared vs transitive nodes in a lock, for user-facing summaries.
pub(crate) fn count_kinds(lock: &Lock) -> (usize, usize) {
    let direct = lock
        .mods
        .iter()
        .filter(|m| m.requested_by.iter().any(|r| r == resolve::ROOT_REQUESTER))
        .count();
    (direct, lock.mods.len() - direct)
}

/// Warn — on stderr, so a `--json` stdout stays clean — when the lock no longer matches the manifest
/// (it was edited since the lock was written). Read-only commands surface this instead of silently
/// reporting stale pack contents. The hash formula mirrors how the resolver stamps `manifest_hash`.
pub(crate) fn warn_if_stale(manifest: &Manifest, lock: &Lock) -> Result<()> {
    let manifest_hash = format!(
        "sha256:{}",
        crate::hash::sha256_hex(manifest.to_json()?.as_bytes())
    );
    if manifest_hash != lock.manifest_hash {
        eprintln!(
            "! lode.lock is stale (the manifest changed since it was written) — run `lode refresh`."
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::loader::Loader;
    use crate::lock::{Download, LockedMod, ResolverMeta, LOCKFILE_VERSION};
    use crate::manifest::LoaderSpec;
    use crate::provider::{DownloadMode, Provider};
    use crate::side::Side;

    fn node(slug: &str, requested_by: &[&str]) -> LockedMod {
        LockedMod {
            slug: slug.to_string(),
            name: slug.to_string(),
            provider: Provider::Modrinth,
            project_id: "x".into(),
            file_id: None,
            version: "1.0".into(),
            filename: format!("{slug}.jar"),
            download: Download {
                url: None,
                mode: DownloadMode::Url,
                hash_format: "sha512".into(),
                hash: "h".into(),
                size: None,
            },
            side: Side::Both,
            optional: false,
            dependencies: Vec::new(),
            requested_by: requested_by.iter().map(|s| s.to_string()).collect(),
        }
    }

    fn lock_with(mods: Vec<LockedMod>) -> Lock {
        Lock {
            lockfile_version: LOCKFILE_VERSION,
            manifest_hash: "sha256:x".into(),
            loader: LoaderSpec {
                name: Loader::Forge,
                minecraft: "1.20.1".into(),
                version: "47.0.0".into(),
            },
            resolver: ResolverMeta {
                lode_version: "0".into(),
            },
            mods,
        }
    }

    #[test]
    fn count_kinds_splits_direct_declarations_from_transitive_deps() {
        let root = resolve::ROOT_REQUESTER;
        let lock = lock_with(vec![
            node("sodium", &[root]),           // directly declared
            node("fabric-api", &["sodium"]),   // pulled in only as a dependency
            node("shared", &[root, "sodium"]), // declared AND pulled in -> still counts as direct
        ]);

        let (direct, deps) = count_kinds(&lock);

        assert_eq!(direct, 2, "sodium and shared are directly declared");
        assert_eq!(deps, 1, "fabric-api is only a transitive dependency");
    }
}
