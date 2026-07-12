use anyhow::Result;

use super::{count_kinds, resolve_and_save};
use crate::manifest::Manifest;
use crate::paths::PackPaths;

pub fn run() -> Result<()> {
    let paths = PackPaths::discover_from_cwd()?;
    let manifest = Manifest::load(&paths.manifest())?;

    // Self-healing: resolves and writes the lock whether or not one already exists.
    let lock = resolve_and_save(&paths, &manifest)?;

    let (direct, deps) = count_kinds(&lock);
    println!(
        "Resolved {} mods ({direct} direct, {deps} dependencies).",
        lock.mods.len()
    );
    println!("Wrote {}", paths.lock().display());
    Ok(())
}
