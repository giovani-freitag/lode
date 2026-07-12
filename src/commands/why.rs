use anyhow::{anyhow, bail, Result};

use crate::cli::WhyArgs;
use crate::lock::Lock;
use crate::paths::PackPaths;
use crate::resolve::ROOT_REQUESTER;

pub fn run(args: WhyArgs) -> Result<()> {
    let paths = PackPaths::discover_from_cwd()?;
    if !paths.lock().exists() {
        bail!("no lockfile yet — run `lode refresh` first");
    }
    let lock = Lock::load(&paths.lock())?;
    let node = lock
        .find(&args.name)
        .ok_or_else(|| anyhow!("'{}' is not in the pack", args.name))?;

    println!("{} ({})", node.slug, node.version);

    if node.requested_by.iter().any(|r| r == ROOT_REQUESTER) {
        println!("  directly declared in the manifest");
    }
    let parents: Vec<&str> = node
        .requested_by
        .iter()
        .filter(|r| r.as_str() != ROOT_REQUESTER)
        .map(String::as_str)
        .collect();
    if !parents.is_empty() {
        println!("  required by: {}", parents.join(", "));
    }
    if !node.dependencies.is_empty() {
        let deps: Vec<&str> = node.dependencies.iter().map(|d| d.slug.as_str()).collect();
        println!("  depends on: {}", deps.join(", "));
    }
    Ok(())
}
