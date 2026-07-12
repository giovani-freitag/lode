use anyhow::Result;

use crate::cli::ListArgs;
use crate::lock::Lock;
use crate::manifest::Manifest;
use crate::paths::PackPaths;
use crate::resolve::ROOT_REQUESTER;

pub fn run(args: ListArgs) -> Result<()> {
    let paths = PackPaths::discover_from_cwd()?;
    let lock_path = paths.lock();

    if !lock_path.exists() {
        let manifest = Manifest::load(&paths.manifest())?;
        if args.json {
            println!("[]");
            return Ok(());
        }
        println!("No lockfile yet. Declared mods (run `lode refresh` to resolve):");
        for (slug, spec) in &manifest.mods {
            println!("  {slug} {}", spec.constraint());
        }
        return Ok(());
    }

    let lock = Lock::load(&lock_path)?;

    if args.json {
        println!("{}", lock.to_json()?.trim_end());
        return Ok(());
    }

    if lock.mods.is_empty() {
        println!("No mods in the pack yet. Try `lode add <mod>`.");
        return Ok(());
    }

    let name_width = lock
        .mods
        .iter()
        .map(|m| m.slug.len())
        .max()
        .unwrap_or(4)
        .max(4);
    for m in &lock.mods {
        let kind = if m.requested_by.iter().any(|r| r == ROOT_REQUESTER) {
            " "
        } else {
            "└"
        };
        println!(
            "{kind} {slug:<name_width$}  {version:<14}  {side:<7}  {provider:?}",
            slug = m.slug,
            version = m.version,
            side = m.side.packwiz_token(),
            provider = m.provider,
        );
    }
    let (direct, deps) = super::count_kinds(&lock);
    println!(
        "\n{} mods — {direct} direct, {deps} dependencies.",
        lock.mods.len()
    );
    Ok(())
}
