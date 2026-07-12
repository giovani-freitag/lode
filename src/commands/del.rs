use std::collections::HashSet;
use std::fs;

use anyhow::{bail, Context, Result};

use super::{count_kinds, resolve_and_save};
use crate::cli::DelArgs;
use crate::lock::Lock;
use crate::manifest::Manifest;
use crate::paths::PackPaths;

pub fn run(args: DelArgs) -> Result<()> {
    let paths = PackPaths::discover_from_cwd()?;
    let mut manifest = Manifest::load(&paths.manifest())?;

    if manifest.mods.shift_remove(&args.name).is_none() {
        bail!("'{}' is not a declared mod in this pack", args.name);
    }

    // Filenames present before removal, so the jars that drop out can be deleted afterwards.
    let previous_files: Vec<String> = Lock::load(&paths.lock())
        .map(|lock| lock.mods.into_iter().map(|m| m.filename).collect())
        .unwrap_or_default();

    // Re-resolving prunes any transitive dependency that only this mod pulled in.
    let lock = resolve_and_save(&paths, &manifest)?;
    manifest.save(&paths.manifest())?;

    // Delete the jars no longer part of the pack (the removed mod plus any orphaned deps),
    // mirroring `npm uninstall` clearing them from node_modules.
    let kept: HashSet<&str> = lock.mods.iter().map(|m| m.filename.as_str()).collect();
    let mods_dir = paths.root.join("mods");
    let mut deleted = 0u32;
    for filename in &previous_files {
        if !kept.contains(filename.as_str()) {
            let path = mods_dir.join(filename);
            if path.exists() {
                fs::remove_file(&path).with_context(|| format!("removing {}", path.display()))?;
                deleted += 1;
            }
        }
    }

    let (direct, deps) = count_kinds(&lock);
    println!("Removed {} ({deleted} jar(s) deleted).", args.name);
    println!(
        "Pack now has {} mods ({direct} direct, {deps} dependencies).",
        lock.mods.len()
    );
    Ok(())
}
