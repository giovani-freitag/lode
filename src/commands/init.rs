use std::env;

use anyhow::{bail, Result};

use super::parse_loader;
use crate::cli::InitArgs;
use crate::loader::Loader;
use crate::manifest::{Defaults, LoaderSpec, Manifest, PackMeta, MANIFEST_FILENAME};
use crate::side::Side;
use crate::versions::Versions;

pub fn run(args: InitArgs) -> Result<()> {
    let dir = env::current_dir()?;
    let manifest_path = dir.join(MANIFEST_FILENAME);
    if manifest_path.exists() {
        bail!("{MANIFEST_FILENAME} already exists here");
    }

    let interactive = crate::ui::is_interactive(args.yes);
    if interactive {
        cliclack::intro("lode · new pack")?;
    }
    let versions = Versions::new()?;

    let default_name = dir.file_name().map(|s| s.to_string_lossy().into_owned());
    let name = match args.name {
        Some(n) => n,
        None if interactive => prompt_text("Pack name", default_name)?,
        None => default_name.unwrap_or_else(|| "pack".to_string()),
    };
    let author = match args.author {
        Some(a) => a,
        None if interactive => prompt_text("Author", None)?,
        None => String::new(),
    };

    let loader = match &args.loader {
        Some(s) => parse_loader(s)?,
        None if interactive => pick_loader()?,
        None => bail!("--loader is required with --yes (forge|neoforge|fabric|quilt)"),
    };

    // Minecraft and loader versions are fetched live so the picker only offers real combinations,
    // and `--yes` (or an omitted flag) resolves to the newest.
    let minecraft = match args.minecraft {
        Some(m) => m,
        None => {
            let mcs = versions.minecraft()?;
            let first = mcs
                .first()
                .cloned()
                .ok_or_else(|| anyhow_empty("Minecraft"))?;
            if interactive {
                // Long list (all releases + snapshots): cap the viewport and let the user
                // type to filter (e.g. "1.20.1") instead of scrolling hundreds of rows.
                let mut picker = cliclack::select("Minecraft version")
                    .filter_mode()
                    .max_rows(10);
                for v in &mcs {
                    picker = picker.item(v.clone(), v, "");
                }
                picker.interact()?
            } else {
                first
            }
        }
    };

    let loader_version = match args.loader_version {
        Some(v) => v,
        None => {
            let lvs = versions.loader(loader, &minecraft)?;
            if lvs.is_empty() {
                bail!(
                    "no {} versions found for Minecraft {minecraft}",
                    label_of(loader)
                );
            }
            if interactive {
                let mut picker = cliclack::select(format!("{} version", label_of(loader)))
                    .filter_mode()
                    .max_rows(10);
                for v in &lvs {
                    let label = match &v.note {
                        Some(note) => format!("{} ({note})", v.version),
                        None => v.version.clone(),
                    };
                    picker = picker.item(v.version.clone(), label, "");
                }
                picker.interact()?
            } else {
                lvs[0].version.clone()
            }
        }
    };

    let manifest = Manifest {
        pack: PackMeta {
            name,
            author,
            version: args.version,
            description: None,
        },
        loader: LoaderSpec {
            name: loader,
            minecraft,
            version: loader_version,
        },
        defaults: Defaults { side: Side::Both },
        overlays: Vec::new(),
        mods: Default::default(),
    };
    manifest.save(&manifest_path)?;

    let summary = format!(
        "{} {} · Minecraft {}",
        label_of(loader),
        manifest.loader.version,
        manifest.loader.minecraft
    );
    let next_steps = "lode add <mod>    add a mod — resolves deps, downloads the jars\n\
         lode install      set up an instance from the lockfile\n\
         lode list         show the resolved pack";
    if interactive {
        cliclack::log::success(format!("Created {}", manifest_path.display()))?;
        cliclack::log::info(summary)?;
        cliclack::note("Next steps", next_steps)?;
        cliclack::outro("Docs: https://github.com/giovani-freitag/lode")?;
    } else {
        println!("Created {}", manifest_path.display());
        println!("  {summary}");
        println!();
        println!("Next steps:");
        println!("  lode add <mod>    add a mod — resolves deps, downloads the jars");
        println!("  lode install      set up an instance from the lockfile");
        println!("  lode list         show the resolved pack");
        println!();
        println!("Docs: https://github.com/giovani-freitag/lode");
    }
    Ok(())
}

fn pick_loader() -> Result<Loader> {
    let loader = cliclack::select("Loader")
        .item(Loader::Fabric, label_of(Loader::Fabric), "")
        .item(Loader::Neoforge, label_of(Loader::Neoforge), "")
        .item(Loader::Forge, label_of(Loader::Forge), "")
        .item(Loader::Quilt, label_of(Loader::Quilt), "")
        .interact()?;
    Ok(loader)
}

fn prompt_text(prompt: &str, default: Option<String>) -> Result<String> {
    let mut input = cliclack::input(prompt);
    if let Some(d) = &default {
        input = input.default_input(d);
    }
    let value: String = input.interact()?;
    Ok(value)
}

fn label_of(loader: Loader) -> &'static str {
    match loader {
        Loader::Fabric => "Fabric",
        Loader::Neoforge => "NeoForge",
        Loader::Forge => "Forge",
        Loader::Quilt => "Quilt",
    }
}

fn anyhow_empty(what: &str) -> anyhow::Error {
    anyhow::anyhow!("could not fetch {what} versions")
}
