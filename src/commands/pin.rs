use std::mem;

use anyhow::{anyhow, Result};

use crate::cli::PinArgs;
use crate::manifest::{Manifest, ModSpec, ModSpecDetailed};
use crate::paths::PackPaths;

pub fn pin(args: PinArgs) -> Result<()> {
    set_pin(&args.name, true)
}

pub fn unpin(args: PinArgs) -> Result<()> {
    set_pin(&args.name, false)
}

fn set_pin(name: &str, pin: bool) -> Result<()> {
    let paths = PackPaths::discover_from_cwd()?;
    let mut manifest = Manifest::load(&paths.manifest())?;

    let spec = manifest
        .mods
        .get_mut(name)
        .ok_or_else(|| anyhow!("'{name}' is not a declared mod in this pack"))?;

    // Take ownership so we can freely convert between the bare-string and detailed forms.
    let detailed = match mem::replace(spec, ModSpec::Constraint(String::new())) {
        ModSpec::Constraint(version) => ModSpecDetailed {
            version,
            side: None,
            provider: None,
            project_id: None,
            optional: false,
            pin,
        },
        ModSpec::Detailed(mut d) => {
            d.pin = pin;
            d
        }
    };

    // Collapse back to the compact string form when nothing but the version remains.
    let plain = !detailed.pin
        && detailed.side.is_none()
        && detailed.provider.is_none()
        && detailed.project_id.is_none()
        && !detailed.optional;
    *spec = if plain {
        ModSpec::Constraint(detailed.version)
    } else {
        ModSpec::Detailed(detailed)
    };

    manifest.save(&paths.manifest())?;
    println!("{} {name}", if pin { "Pinned" } else { "Unpinned" });
    Ok(())
}
