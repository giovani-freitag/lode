use std::collections::HashSet;
use std::fs;
use std::io::IsTerminal;
use std::path::{Path, PathBuf};

use anyhow::{anyhow, bail, Context, Result};
use reqwest::blocking::Client;

use crate::cli::InstallArgs;
use crate::hash::{hash_by_format, sha256_hex};
use crate::http::{download_capped, download_client, DEFAULT_MAX_DOWNLOAD, SIZE_MARGIN};
use crate::lock::{Lock, LockedMod};
use crate::manifest::Manifest;
use crate::paths::PackPaths;
use crate::provider::Provider;
use crate::providers::curseforge::Curseforge;
use crate::resolve;
use crate::side::Side;

/// Options controlling an install — shared by `lode install` and `lode add`.
#[derive(Default)]
pub struct InstallOpts {
    pub into: Option<PathBuf>,
    pub server: bool,
    pub skip_loader: bool,
    pub java: Option<String>,
}

pub fn run(args: InstallArgs) -> Result<()> {
    let paths = PackPaths::discover_from_cwd()?;
    let manifest = Manifest::load(&paths.manifest())?;

    let manifest_hash = format!("sha256:{}", sha256_hex(manifest.to_json()?.as_bytes()));
    let previous = Lock::load(&paths.lock()).ok();
    let fresh = previous
        .as_ref()
        .map(|l| l.manifest_hash == manifest_hash)
        .unwrap_or(false);
    let lock = if fresh {
        previous.unwrap()
    } else {
        println!("Lockfile missing or stale — resolving.");
        let lock = crate::ui::spin("Resolving dependencies", "Dependencies resolved", || {
            resolve::resolve(
                &manifest,
                previous.as_ref(),
                resolve::ResolveMode::Locked,
                &paths.root,
            )
        })?;
        lock.save(&paths.lock())?;
        lock
    };

    install_pack(
        &paths,
        &manifest,
        &lock,
        &InstallOpts {
            into: args.into,
            server: args.server,
            skip_loader: args.skip_loader,
            java: args.java,
        },
    )
}

/// Download the resolved mods into an instance's `mods/` folder, verifying each file against its
/// locked hash — and, on a server install, provision the loader first.
pub fn install_pack(
    paths: &PackPaths,
    manifest: &Manifest,
    lock: &Lock,
    opts: &InstallOpts,
) -> Result<()> {
    let target = opts.into.clone().unwrap_or_else(|| paths.root.clone());

    // On a server install, bring up the loader itself first (unless told to skip), so the target
    // goes from empty folder to runnable server in one command.
    if opts.server && !opts.skip_loader {
        let provisioner = crate::provision::LoaderProvisioner::new(opts.java.clone())?;
        let did = provisioner.ensure_server(
            manifest.loader.name,
            &manifest.loader.minecraft,
            &manifest.loader.version,
            &target,
        )?;
        if did {
            println!(
                "Provisioned {} server in {}",
                manifest.loader.version,
                target.display()
            );
        }
    }

    let mods_dir = target.join("mods");
    fs::create_dir_all(&mods_dir).with_context(|| format!("creating {}", mods_dir.display()))?;

    let client = download_client()?;

    let mut installed = 0u32;
    let mut up_to_date = 0u32;
    let mut manual: Vec<ManualDownload> = Vec::new();
    // CurseForge never persists a URL; it is re-fetched here, only if a CF mod is present.
    let mut curseforge: Option<Curseforge> = None;
    // A missing CurseForge key shouldn't sink the whole install — the Modrinth mods still work.
    // CF mods are collected here and reported at the end instead.
    let mut needs_key: Vec<String> = Vec::new();
    let mut cf_unavailable = false;
    // One mod's download/hash failure must not discard the rest of the batch — collect them and
    // report together, mirroring how manual/needs_key already degrade gracefully.
    let mut failures: Vec<ModFailure> = Vec::new();

    // A per-mod spinner gives live download feedback in a terminal; when stderr is piped/redirected
    // it stays `None` and the original plain lines are printed instead, so captured output is
    // unchanged. Every branch below stops or errors the spinner so one is never left dangling.
    let spinners = std::io::stderr().is_terminal();

    for m in &lock.mods {
        if !side_wanted(m.side, opts.server) {
            continue;
        }

        let spinner = spinners.then(|| {
            let sp = cliclack::spinner();
            sp.start(format!("Downloading {}", m.slug));
            sp
        });

        let outcome = install_one(
            m,
            &paths.root,
            &mods_dir,
            &client,
            &mut curseforge,
            &mut cf_unavailable,
        );
        match outcome {
            Ok(Outcome::Installed) => {
                installed += 1;
                match &spinner {
                    Some(sp) => sp.stop(format!("+ {} ({})", m.filename, m.slug)),
                    None => println!("  + {} ({})", m.filename, m.slug),
                }
            }
            Ok(Outcome::InstalledLocal) => {
                installed += 1;
                match &spinner {
                    Some(sp) => sp.stop(format!("+ {} ({}) [local]", m.filename, m.slug)),
                    None => println!("  + {} ({}) [local]", m.filename, m.slug),
                }
            }
            Ok(Outcome::UpToDate) => {
                up_to_date += 1;
                if let Some(sp) = &spinner {
                    sp.stop(format!("{} ({}) up to date", m.filename, m.slug));
                }
            }
            Ok(Outcome::Manual(md)) => {
                if let Some(sp) = &spinner {
                    sp.cancel(format!(
                        "{} ({}) needs a manual download",
                        md.name, md.filename
                    ));
                }
                manual.push(md);
            }
            Ok(Outcome::NeedsKey(entry)) => {
                if let Some(sp) = &spinner {
                    sp.cancel(format!("{entry} needs a CurseForge key"));
                }
                needs_key.push(entry);
            }
            Err(err) => {
                // Keep a mismatch (and any hard error) loud, but attribute it to this mod and press
                // on so the caller still learns everything that did — and didn't — install.
                match &spinner {
                    Some(sp) => sp.error(format!("{} ({}) — {err:#}", m.name, m.filename)),
                    None => eprintln!("  x {} ({}) — {err:#}", m.name, m.filename),
                }
                failures.push(ModFailure {
                    name: m.name.clone(),
                    filename: m.filename.clone(),
                    error: format!("{err:#}"),
                });
            }
        }
    }

    // A version bump or dropped dependency leaves the previous jar behind; with only writes, that
    // stale jar becomes a "duplicate mod" crash at launch. Prune anything not in this side's
    // expected set — but only in the managed instance dir, never a user's `--into` target, and only
    // on a run where every wanted mod resolved to a jar that is now present. A mod that failed,
    // needs a key, or must be fetched by hand did NOT get its new jar written, and its old jar
    // (under the previous filename) is no longer in the expected set — so pruning now would strand
    // it with no jar at all (worse than stale-but-working). A re-run once it resolves clears cleanly.
    let mut pruned = 0u32;
    if target == paths.root && failures.is_empty() && manual.is_empty() && needs_key.is_empty() {
        let expected: HashSet<&str> = lock
            .mods
            .iter()
            .filter(|m| side_wanted(m.side, opts.server))
            .map(|m| m.filename.as_str())
            .collect();
        pruned = prune_mods(&mods_dir, &expected)?;
    }

    // Overlays (config, scripts) only need copying when installing into a separate instance —
    // when the target IS the pack root they already live in place.
    let mut overlays = 0u32;
    if target != paths.root {
        for overlay in crate::overlay::collect(&paths.root, manifest) {
            if !side_wanted(overlay.side, opts.server) {
                continue;
            }
            let dest = target.join(&overlay.rel);
            if let Some(parent) = dest.parent() {
                fs::create_dir_all(parent)
                    .with_context(|| format!("creating {}", parent.display()))?;
            }
            fs::copy(&overlay.abs, &dest)
                .with_context(|| format!("copying overlay {}", overlay.rel))?;
            overlays += 1;
        }
    }

    let side_label = if opts.server { "server" } else { "client" };
    println!(
        "\nInstalled {installed}, up-to-date {up_to_date}, overlays {overlays}, pruned {pruned} ({side_label}) into {}",
        target.display()
    );

    if !manual.is_empty() {
        println!(
            "\n{} mod(s) must be downloaded manually (the author disabled third-party distribution):",
            manual.len()
        );
        for m in &manual {
            match &m.page {
                Some(page) => println!("  - {} ({}) — {}", m.name, m.filename, page),
                None => println!("  - {} ({})", m.name, m.filename),
            }
        }
        println!(
            "Drop those jars into {}, then run `lode install` again.",
            mods_dir.display()
        );
    }

    if !needs_key.is_empty() {
        println!(
            "\n{} CurseForge mod(s) need an API key to download:",
            needs_key.len()
        );
        for m in &needs_key {
            println!("  - {m}");
        }
        println!(
            "Set one with `lode config set curseforge.key <KEY>` (or the CF_API_KEY env var), \
             then run `lode install` again."
        );
    }

    // Hard failures are the only non-zero exit: manual/needs_key are expected, recoverable states.
    if !failures.is_empty() {
        println!("\n{} mod(s) failed to install:", failures.len());
        for f in &failures {
            println!("  - {} ({}) — {}", f.name, f.filename, f.error);
        }
        bail!(
            "{} mod(s) failed to install — see the list above",
            failures.len()
        );
    }
    Ok(())
}

/// What installing one locked mod produced. Everything except a hard `Err` lets the batch proceed.
enum Outcome {
    /// Downloaded, verified, and written.
    Installed,
    /// Copied in from the pack's `local/` folder.
    InstalledLocal,
    /// Already on disk with a matching hash — nothing to do.
    UpToDate,
    /// The provider offers no download URL; the user must fetch it by hand.
    Manual(ManualDownload),
    /// A CurseForge mod that can't be fetched without an API key.
    NeedsKey(String),
}

/// Install a single locked mod, returning how it was handled. Network/hash errors surface as
/// `Err` so the caller can record them per-mod instead of aborting the whole install.
fn install_one(
    m: &LockedMod,
    root: &Path,
    mods_dir: &Path,
    client: &Client,
    curseforge: &mut Option<Curseforge>,
    cf_unavailable: &mut bool,
) -> Result<Outcome> {
    // Determine the URL to fetch from. CurseForge mods carry no URL in the lock, so re-resolve it
    // now; a `None` there means the author opted out and the file needs a manual download.
    let url = match &m.download.url {
        Some(u) => u.clone(),
        None if m.provider == Provider::Local => {
            let src = root.join("local").join(&m.filename);
            let dest = mods_dir.join(&m.filename);
            fs::copy(&src, &dest).with_context(|| format!("copying local jar {}", m.filename))?;
            return Ok(Outcome::InstalledLocal);
        }
        None if m.provider == Provider::Curseforge => {
            // Once we know there's no key, skip the rest of the CF mods without retrying.
            if *cf_unavailable {
                return Ok(Outcome::NeedsKey(format!("{} ({})", m.name, m.filename)));
            }
            if curseforge.is_none() {
                match Curseforge::from_config() {
                    Ok(cf) => *curseforge = Some(cf),
                    Err(_) => {
                        *cf_unavailable = true;
                        return Ok(Outcome::NeedsKey(format!("{} ({})", m.name, m.filename)));
                    }
                }
            }
            let file_id = m.file_id.as_deref().unwrap_or_default();
            let dl = curseforge
                .as_ref()
                .unwrap()
                .file_download(&m.project_id, file_id)?;
            match dl.url {
                Some(u) => u,
                None => {
                    return Ok(Outcome::Manual(ManualDownload {
                        name: m.name.clone(),
                        filename: m.filename.clone(),
                        page: dl
                            .website_url
                            .map(|w| format!("{}/files/{}", w.trim_end_matches('/'), file_id)),
                    }));
                }
            }
        }
        None => {
            return Ok(Outcome::Manual(ManualDownload {
                name: m.name.clone(),
                filename: m.filename.clone(),
                page: None,
            }));
        }
    };

    let dest = mods_dir.join(&m.filename);
    if dest.exists() {
        if let Ok(existing) = fs::read(&dest) {
            if hash_by_format(&existing, &m.download.hash_format).as_deref()
                == Some(m.download.hash.as_str())
            {
                return Ok(Outcome::UpToDate);
            }
        }
    }

    // Cap the download to the locked size (plus a small margin) when it's known, so a hostile URL
    // can't stream an endless body; fall back to a generous ceiling when it isn't.
    let cap = m
        .download
        .size
        .map(|s| s.saturating_add(SIZE_MARGIN))
        .unwrap_or(DEFAULT_MAX_DOWNLOAD);
    let bytes = download_capped(client.get(&url), cap, &m.slug)?;

    let got = hash_by_format(&bytes, &m.download.hash_format)
        .ok_or_else(|| anyhow!("unsupported hash format '{}'", m.download.hash_format))?;
    if got != m.download.hash {
        bail!(
            "hash mismatch: expected {}, got {} — refusing to write",
            m.download.hash,
            got
        );
    }

    fs::write(&dest, &bytes).with_context(|| format!("writing {}", dest.display()))?;
    Ok(Outcome::Installed)
}

/// Remove jars in `mods_dir` that aren't in `expected`, returning how many were pruned — so a
/// version bump leaves the loader one jar per mod, not the old and new side by side.
fn prune_mods(mods_dir: &Path, expected: &HashSet<&str>) -> Result<u32> {
    let mut pruned = 0u32;
    let entries = match fs::read_dir(mods_dir) {
        Ok(entries) => entries,
        Err(_) => return Ok(0),
    };
    for entry in entries {
        let entry = entry.with_context(|| format!("reading {}", mods_dir.display()))?;
        let path = entry.path();
        let is_jar = path
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("jar"));
        if !is_jar {
            continue;
        }
        let name = match path.file_name().and_then(|n| n.to_str()) {
            Some(name) => name,
            None => continue,
        };
        if !expected.contains(name) {
            fs::remove_file(&path).with_context(|| format!("removing {}", path.display()))?;
            println!("  - {name} (pruned — no longer in the pack)");
            pruned += 1;
        }
    }
    Ok(pruned)
}

/// A file that couldn't be fetched automatically, with the page a user can grab it from.
struct ManualDownload {
    name: String,
    filename: String,
    page: Option<String>,
}

/// A mod that hit a hard error (network/hash) during install, recorded so the batch can finish
/// and report every failure at once.
struct ModFailure {
    name: String,
    filename: String,
    error: String,
}

/// Whether a mod belongs on the side being installed. `Both` always installs; `None` (neither
/// side) never does.
fn side_wanted(side: Side, want_server: bool) -> bool {
    match side {
        Side::Both => true,
        Side::Client => !want_server,
        Side::Server => want_server,
        Side::None => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prune_removes_only_unexpected_jars() {
        let dir = tempfile::tempdir().unwrap();
        let mods = dir.path();
        fs::write(mods.join("keep-1.0.jar"), b"new").unwrap();
        fs::write(mods.join("stale-0.9.jar"), b"old").unwrap();
        fs::write(mods.join("notes.txt"), b"leave me").unwrap();

        let expected: HashSet<&str> = ["keep-1.0.jar"].into_iter().collect();
        let pruned = prune_mods(mods, &expected).unwrap();

        assert_eq!(pruned, 1);
        assert!(mods.join("keep-1.0.jar").exists());
        assert!(!mods.join("stale-0.9.jar").exists());
        // Non-jar files are never touched — configs and the like live in mods/ for some packs.
        assert!(mods.join("notes.txt").exists());
    }

    #[test]
    fn prune_on_a_missing_dir_is_a_no_op() {
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("does-not-exist");
        let expected: HashSet<&str> = HashSet::new();
        assert_eq!(prune_mods(&missing, &expected).unwrap(), 0);
    }
}
