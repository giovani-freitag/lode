use anyhow::Result;

use crate::cli::{ExportArgs, ExportTarget};
use crate::hash::sha256_hex;
use crate::lock::Lock;
use crate::manifest::Manifest;
use crate::packwiz;
use crate::paths::PackPaths;
use crate::resolve;

pub fn run(args: ExportArgs) -> Result<()> {
    match args.target {
        ExportTarget::Packwiz => export_packwiz(),
    }
}

/// Export the pack as a packwiz distribution tree in `pack/`, for launchers/tools that consume the
/// packwiz format (via packwiz-installer). This is an interop bridge — lode's own `install`/`get`
/// need none of it. Mirrors `import packwiz`.
fn export_packwiz() -> Result<()> {
    let paths = PackPaths::discover_from_cwd()?;
    let manifest = Manifest::load(&paths.manifest())?;

    let manifest_hash = format!("sha256:{}", sha256_hex(manifest.to_json()?.as_bytes()));

    // Use the existing lock only if it matches the current manifest; otherwise re-resolve so a
    // stale lock never produces a stale pack.
    let previous = Lock::load(&paths.lock()).ok();
    let fresh = previous
        .as_ref()
        .map(|l| l.manifest_hash == manifest_hash)
        .unwrap_or(false);
    let lock = if fresh {
        previous.unwrap()
    } else {
        println!("Lockfile missing or stale — re-resolving.");
        let lock = resolve::resolve(
            &manifest,
            previous.as_ref(),
            resolve::ResolveMode::Locked,
            &paths.root,
        )?;
        lock.save(&paths.lock())?;
        lock
    };

    packwiz::emit(&paths.pack_dir(), &paths.root, &manifest, &lock)?;
    println!(
        "Exported a packwiz pack into {}",
        paths.pack_dir().display()
    );
    println!("Serve that directory over HTTP for packwiz-installer (MultiMC/Prism).");
    Ok(())
}
