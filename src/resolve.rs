use std::collections::{HashMap, VecDeque};
use std::path::Path;

use anyhow::{anyhow, bail, Context, Result};

use crate::hash::{sha256_hex, sha512_hex};
use crate::lock::{DepEdge, DepKind, Download, Lock, LockedMod, ResolverMeta, LOCKFILE_VERSION};
use crate::manifest::{Manifest, ModSpec};
use crate::provider::{DownloadMode, Provider};
use crate::providers::curseforge::Curseforge;
use crate::providers::modrinth::Modrinth;
use crate::providers::ResolvedVersion;
use crate::side::Side;

/// The root marker in `requestedBy`: a node the manifest declares directly (vs. one pulled in
/// only as a transitive dependency).
pub const ROOT_REQUESTER: &str = "manifest";

struct Task {
    /// Slug or project id to resolve.
    id: String,
    constraint: String,
    requester: String,
    side_override: Option<Side>,
    optional: bool,
    provider: Provider,
}

struct Node {
    resolved: ResolvedVersion,
    side: Side,
    requested_by: Vec<String>,
    optional: bool,
    provider: Provider,
}

/// How `resolve` treats versions already in the lockfile.
pub enum ResolveMode {
    /// Keep every version already locked; only newly-declared mods resolve fresh. Used by add,
    /// del, install, refresh and build — so the lock is respected, not re-bumped every command.
    Locked,
    /// Re-resolve to the latest allowed versions. `only: Some(slug)` limits the bump to that mod
    /// (and its dependencies); everything else stays locked. Pinned mods never bump.
    Update { only: Option<String> },
}

/// Versions from the previous lock, keyed both by declared slug and by provider project id.
#[derive(Default)]
struct Frozen {
    by_slug: HashMap<String, String>,
    by_project: HashMap<String, String>,
}

impl Frozen {
    fn from(previous: Option<&Lock>) -> Frozen {
        let mut frozen = Frozen::default();
        if let Some(lock) = previous {
            for m in &lock.mods {
                frozen.by_slug.insert(m.slug.clone(), m.version.clone());
                frozen
                    .by_project
                    .insert(m.project_id.clone(), m.version.clone());
            }
        }
        frozen
    }
}

/// Constraint to resolve a declared mod with: its locked version when frozen, else the manifest's.
fn declared_constraint(
    slug: &str,
    manifest_constraint: &str,
    pinned: bool,
    frozen: &Frozen,
    mode: &ResolveMode,
) -> String {
    let locked = frozen.by_slug.get(slug).cloned();
    let bump = match mode {
        ResolveMode::Locked => false,
        ResolveMode::Update { only } => {
            !pinned && (only.is_none() || only.as_deref() == Some(slug))
        }
    };
    if bump {
        manifest_constraint.to_string()
    } else {
        locked.unwrap_or_else(|| manifest_constraint.to_string())
    }
}

/// Constraint to resolve a dependency with: its locked version when frozen, else the latest.
fn dependency_constraint(project_id: &str, frozen: &Frozen, mode: &ResolveMode) -> String {
    let locked = frozen.by_project.get(project_id).cloned();
    let bump = matches!(mode, ResolveMode::Update { only: None });
    if bump {
        "*".to_string()
    } else {
        locked.unwrap_or_else(|| "*".to_string())
    }
}

/// Resolve the full graph declared by the manifest into a lockfile: every declared mod plus the
/// transitive closure of its required dependencies, deduplicated by project id with merged
/// reverse edges.
pub fn resolve(
    manifest: &Manifest,
    previous: Option<&Lock>,
    mode: ResolveMode,
    root: &Path,
) -> Result<Lock> {
    let modrinth = Modrinth::new()?;
    // Built lazily so packs that never touch CurseForge never need a key.
    let mut curseforge: Option<Curseforge> = None;
    let loader = manifest.loader.name;
    let mc = manifest.loader.minecraft.clone();
    let frozen = Frozen::from(previous);

    let mut queue: VecDeque<Task> = VecDeque::new();
    for (slug, spec) in &manifest.mods {
        // Local jars aren't resolved from a provider — they're read from disk further down.
        if spec.provider() == Some(Provider::Local) {
            continue;
        }
        queue.push_back(Task {
            id: slug.clone(),
            constraint: declared_constraint(slug, spec.constraint(), spec.pin(), &frozen, &mode),
            requester: ROOT_REQUESTER.to_string(),
            side_override: spec.side(),
            optional: matches!(spec, ModSpec::Detailed(d) if d.optional),
            provider: spec.provider().unwrap_or(Provider::Modrinth),
        });
    }

    // Resolution order preserved for a deterministic lock; dedup keyed by project id.
    let mut order: Vec<String> = Vec::new();
    let mut nodes: HashMap<String, Node> = HashMap::new();

    while let Some(task) = queue.pop_front() {
        let resolved = match task.provider {
            Provider::Modrinth => crate::http::with_retry(&task.id, || {
                modrinth.resolve(&task.id, loader, &mc, &task.constraint)
            })?,
            Provider::Curseforge => {
                if curseforge.is_none() {
                    curseforge = Some(Curseforge::from_config()?);
                }
                let cf = curseforge.as_ref().unwrap();
                crate::http::with_retry(&task.id, || {
                    cf.resolve(&task.id, loader, &mc, &task.constraint)
                })?
            }
            other => bail!("provider {other:?} is not supported yet"),
        };

        if let Some(existing) = nodes.get_mut(&resolved.project_id) {
            if !existing.requested_by.contains(&task.requester) {
                existing.requested_by.push(task.requester.clone());
            }
            continue;
        }

        for dep in &resolved.dependencies {
            if dep.kind == DepKind::Required {
                queue.push_back(Task {
                    id: dep.project_id.clone(),
                    constraint: dependency_constraint(&dep.project_id, &frozen, &mode),
                    requester: resolved.slug.clone(),
                    side_override: None,
                    optional: false,
                    // A dependency comes from the same provider as the mod that pulled it in.
                    provider: task.provider,
                });
            }
        }

        order.push(resolved.project_id.clone());
        nodes.insert(
            resolved.project_id.clone(),
            Node {
                side: task.side_override.unwrap_or(resolved.side),
                requested_by: vec![task.requester],
                optional: task.optional,
                provider: task.provider,
                resolved,
            },
        );
    }

    let slug_of: HashMap<String, String> = nodes
        .iter()
        .map(|(id, n)| (id.clone(), n.resolved.slug.clone()))
        .collect();

    let mut mods: Vec<LockedMod> = order
        .iter()
        .map(|project_id| {
            let node = &nodes[project_id];
            let dependencies = node
                .resolved
                .dependencies
                .iter()
                // Record edges only for deps present in the graph, so every edge has a real slug.
                .filter_map(|d| {
                    slug_of.get(&d.project_id).map(|slug| DepEdge {
                        slug: slug.clone(),
                        kind: d.kind,
                    })
                })
                .collect();

            LockedMod {
                slug: node.resolved.slug.clone(),
                name: node.resolved.project_name.clone(),
                provider: node.provider,
                project_id: project_id.clone(),
                file_id: Some(node.resolved.file_id.clone()),
                version: node.resolved.version.clone(),
                filename: node.resolved.filename.clone(),
                download: Download {
                    url: node.resolved.url.clone(),
                    mode: DownloadMode::Url,
                    hash_format: node.resolved.hash_format.clone(),
                    hash: node.resolved.hash.clone(),
                    size: node.resolved.size,
                },
                side: node.side,
                optional: node.optional,
                dependencies,
                requested_by: node.requested_by.clone(),
            }
        })
        .collect();

    // Append locally-supplied jars: read from `<root>/local/<filename>`, hashed, no network.
    for (slug, spec) in &manifest.mods {
        if spec.provider() != Some(Provider::Local) {
            continue;
        }
        let filename = spec
            .project_id()
            .ok_or_else(|| anyhow!("local mod '{slug}' is missing its file path"))?;
        // Validate before the read, not just before the write: `filename` is attacker-controlled
        // (it comes straight from the manifest), and `root.join("local").join(filename)` honours a
        // `..`/absolute component, so an unchecked one would read an arbitrary file off disk.
        crate::lock::safe_component(filename)
            .with_context(|| format!("local mod '{slug}' has an unsafe file path"))?;
        let path = root.join("local").join(filename);
        let bytes = std::fs::read(&path)
            .with_context(|| format!("reading local jar {}", path.display()))?;
        mods.push(LockedMod {
            slug: slug.clone(),
            name: slug.clone(),
            provider: Provider::Local,
            project_id: filename.to_string(),
            file_id: None,
            version: "local".to_string(),
            filename: filename.to_string(),
            download: Download {
                url: None,
                mode: DownloadMode::Url,
                hash_format: "sha512".to_string(),
                hash: sha512_hex(&bytes),
                size: Some(bytes.len() as u64),
            },
            side: spec.side().unwrap_or(manifest.defaults.side),
            optional: matches!(spec, ModSpec::Detailed(d) if d.optional),
            dependencies: Vec::new(),
            requested_by: vec![ROOT_REQUESTER.to_string()],
        });
    }

    let manifest_hash = format!("sha256:{}", sha256_hex(manifest.to_json()?.as_bytes()));

    let lock = Lock {
        lockfile_version: LOCKFILE_VERSION,
        manifest_hash,
        loader: manifest.loader.clone(),
        resolver: ResolverMeta {
            lode_version: env!("CARGO_PKG_VERSION").to_string(),
        },
        mods,
    };
    // Guard the freshly-resolved lock too, not just loaded ones: a hostile provider response could
    // carry a filename that escapes the instance dir once written.
    lock.validate()?;
    Ok(lock)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frozen() -> Frozen {
        let mut f = Frozen::default();
        f.by_slug.insert("sodium".into(), "0.5.8".into());
        f.by_project.insert("AABB".into(), "1.0.0".into());
        f
    }

    fn update_all() -> ResolveMode {
        ResolveMode::Update { only: None }
    }

    #[test]
    fn locked_keeps_the_locked_version_and_resolves_new_mods_fresh() {
        let f = frozen();
        assert_eq!(
            declared_constraint("sodium", "^0.6", false, &f, &ResolveMode::Locked),
            "0.5.8"
        );
        assert_eq!(
            declared_constraint("new", "^0.6", false, &f, &ResolveMode::Locked),
            "^0.6"
        );
    }

    #[test]
    fn update_all_bumps_unpinned_but_freezes_pinned() {
        let f = frozen();
        assert_eq!(
            declared_constraint("sodium", "^0.6", false, &f, &update_all()),
            "^0.6"
        );
        assert_eq!(
            declared_constraint("sodium", "^0.6", true, &f, &update_all()),
            "0.5.8"
        );
    }

    #[test]
    fn targeted_update_bumps_only_the_named_mod() {
        let f = frozen();
        let only_sodium = ResolveMode::Update {
            only: Some("sodium".into()),
        };
        assert_eq!(
            declared_constraint("sodium", "^0.6", false, &f, &only_sodium),
            "^0.6"
        );
        let only_other = ResolveMode::Update {
            only: Some("other".into()),
        };
        assert_eq!(
            declared_constraint("sodium", "^0.6", false, &f, &only_other),
            "0.5.8"
        );
    }

    #[test]
    fn local_jar_with_a_traversing_path_is_rejected_before_any_read() {
        // A local mod's file path is attacker-controlled (straight from the manifest). A `..`
        // component must be refused by the choke point, never fs::read off the target tree — and
        // with only a local mod declared, resolve touches no network to get there.
        let manifest = Manifest::parse(
            r#"{
                "pack": {"name":"t","author":"a","version":"0"},
                "loader": {"name":"fabric","minecraft":"1.20.1","version":"0.16.0"},
                "mods": {"evil": {"version":"local","provider":"local","projectId":"../escape.jar"}}
            }"#,
        )
        .unwrap();
        let dir = tempfile::tempdir().unwrap();
        let err = resolve(&manifest, None, ResolveMode::Locked, dir.path()).unwrap_err();
        assert!(format!("{err:#}").contains("unsafe"), "{err:#}");
    }

    #[test]
    fn dependencies_freeze_unless_a_full_update() {
        let f = frozen();
        assert_eq!(
            dependency_constraint("AABB", &f, &ResolveMode::Locked),
            "1.0.0"
        );
        assert_eq!(dependency_constraint("AABB", &f, &update_all()), "*");
        assert_eq!(dependency_constraint("NEW", &f, &ResolveMode::Locked), "*");
    }
}
