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

use anyhow::{bail, Result};

use crate::loader::Loader;
use crate::lock::Lock;
use crate::manifest::Manifest;
use crate::paths::PackPaths;
use crate::resolve;
use crate::side::Side;

pub(crate) fn parse_side(s: &str) -> Result<Side> {
    Ok(match s.to_ascii_lowercase().as_str() {
        "client" => Side::Client,
        "server" => Side::Server,
        "both" => Side::Both,
        "none" => Side::None,
        other => bail!("unknown side '{other}' (use client|server|both|none)"),
    })
}

pub(crate) fn parse_loader(s: &str) -> Result<Loader> {
    Ok(match s.to_ascii_lowercase().as_str() {
        "forge" => Loader::Forge,
        "neoforge" => Loader::Neoforge,
        "fabric" => Loader::Fabric,
        "quilt" => Loader::Quilt,
        other => bail!("unknown loader '{other}' (use forge|neoforge|fabric|quilt)"),
    })
}

/// Resolve the manifest into a lockfile and persist it, **respecting** the existing lock — mods
/// already locked keep their versions; only new ones resolve fresh. The shared core of add, del,
/// and refresh.
pub(crate) fn resolve_and_save(paths: &PackPaths, manifest: &Manifest) -> Result<Lock> {
    let previous = Lock::load(&paths.lock()).ok();
    let lock = resolve::resolve(
        manifest,
        previous.as_ref(),
        resolve::ResolveMode::Locked,
        &paths.root,
    )?;
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lock::{Download, LockedMod, ResolverMeta, LOCKFILE_VERSION};
    use crate::manifest::LoaderSpec;
    use crate::provider::{DownloadMode, Provider};

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
