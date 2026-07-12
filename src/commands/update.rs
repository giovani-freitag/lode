use std::collections::HashMap;

use anyhow::{bail, Result};

use crate::cli::UpdateArgs;
use crate::lock::Lock;
use crate::manifest::Manifest;
use crate::paths::PackPaths;
use crate::resolve::{self, ResolveMode};

pub fn run(args: UpdateArgs) -> Result<()> {
    let paths = PackPaths::discover_from_cwd()?;
    let manifest = Manifest::load(&paths.manifest())?;

    // Refuse to update a mod the manifest pins — the pin is the whole point.
    if let Some(slug) = &args.name {
        if manifest.mods.get(slug).map(|s| s.pin()).unwrap_or(false) {
            bail!("'{slug}' is pinned — remove the pin in lode.jsonc to update it");
        }
    }

    let previous = Lock::load(&paths.lock()).ok();
    let before: HashMap<String, String> = previous
        .as_ref()
        .map(|lock| {
            lock.mods
                .iter()
                .map(|m| (m.slug.clone(), m.version.clone()))
                .collect()
        })
        .unwrap_or_default();

    // `lode update <slug>` bumps just that mod; `lode update` / `--all` bumps everything unpinned.
    let mode = ResolveMode::Update {
        only: args.name.clone(),
    };
    let lock = crate::ui::spin("Resolving dependencies", "Dependencies resolved", || {
        resolve::resolve(&manifest, previous.as_ref(), mode, &paths.root)
    })?;
    lock.save(&paths.lock())?;

    let mut changed = 0;
    for m in &lock.mods {
        match before.get(&m.slug) {
            Some(old) if old != &m.version => {
                println!("  {} {old} -> {}", m.slug, m.version);
                changed += 1;
            }
            None => {
                println!("  {} (new) {}", m.slug, m.version);
                changed += 1;
            }
            _ => {}
        }
    }

    if changed == 0 {
        println!("Everything is up to date.");
    } else {
        println!("\nUpdated {changed} mod(s). Run `lode install` to download the changes.");
    }
    Ok(())
}
