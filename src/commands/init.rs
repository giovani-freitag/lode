use std::env;

use anyhow::{bail, Result};

use crate::cli::InitArgs;
use crate::loader::Loader;
use crate::manifest::{Defaults, LoaderSpec, Manifest, PackMeta, MANIFEST_FILENAME};
use crate::side::Side;
use crate::versions::{LoaderVersion, Versions};

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
    let name = match &args.name {
        Some(n) => n.clone(),
        None if interactive => prompt_text("Pack name", default_name)?,
        None => default_name.unwrap_or_else(|| "pack".to_string()),
    };
    let author = match &args.author {
        Some(a) => a.clone(),
        None if interactive => prompt_text("Author", None)?,
        None => String::new(),
    };

    let (loader, minecraft, loader_version) = select_versions(&versions, &args, interactive)?;

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

/// Resolve (loader, Minecraft, loader version). A flag pins its step and skips the prompt; unset
/// steps are prompted interactively in a linear wizard — no back-step, matching `npm init` /
/// `create-vite` (Esc/Ctrl-C cancels cleanly and it's quick to re-run). The Minecraft list is
/// filtered to versions the chosen loader actually ships (`Versions::minecraft_for`), so a pick
/// can't dead-end.
fn select_versions(
    versions: &Versions,
    args: &InitArgs,
    interactive: bool,
) -> Result<(Loader, String, String)> {
    let arg_loader = args.loader;

    if !interactive {
        return resolve_noninteractive(versions, args, arg_loader);
    }

    let loader = match arg_loader {
        Some(l) => l,
        None => pick_loader()?,
    };

    let minecraft = match &args.minecraft {
        Some(m) => m.clone(),
        None => {
            // Spinner so the live fetch reads as progress, not a frozen terminal (matches add/install).
            let mcs = crate::ui::spin(
                "Fetching Minecraft versions",
                "Minecraft versions loaded",
                || versions.minecraft_for(loader),
            )?;
            if mcs.is_empty() {
                bail!(
                    "no Minecraft versions found for {} — the loader metadata may be unreachable",
                    label_of(loader)
                );
            }
            pick_minecraft(&mcs)?
        }
    };

    let loader_version = match &args.loader_version {
        Some(v) => v.clone(),
        None => {
            let lvs = crate::ui::spin(
                &format!("Fetching {} versions", label_of(loader)),
                &format!("{} versions loaded", label_of(loader)),
                || versions.loader(loader, &minecraft),
            )?;
            if lvs.is_empty() {
                // The loader-filtered Minecraft list should preclude this; if it slips through, the
                // dead-end error is still actionable rather than bare.
                return Err(dead_end(versions, loader, &minecraft));
            }
            pick_loader_version(loader, &lvs)?
        }
    };

    Ok((loader, minecraft, loader_version))
}

/// Non-interactive resolution (`--yes`, or stdin isn't a TTY): everything unset falls back to the
/// newest build, and a missing loader is a hard error whose message names the *real* cause.
fn resolve_noninteractive(
    versions: &Versions,
    args: &InitArgs,
    arg_loader: Option<Loader>,
) -> Result<(Loader, String, String)> {
    let loader = match arg_loader {
        Some(l) => l,
        // Only blame `--yes` when it was actually passed; otherwise the trigger was a non-TTY stdin
        // that silently skipped the wizard, which the message must say (and `--loader` alone works).
        None if args.yes => bail!("--loader is required with --yes (forge|neoforge|fabric|quilt)"),
        None => bail!(
            "lode init needs a terminal to prompt (stdin isn't a TTY).\n\
             Re-run in an interactive terminal, or pass --loader <forge|neoforge|fabric|quilt> \
             (optionally --name/--author/--minecraft) to scaffold non-interactively."
        ),
    };
    let minecraft = match &args.minecraft {
        Some(m) => m.clone(),
        None => versions
            .minecraft_for(loader)?
            .into_iter()
            .next()
            .ok_or_else(|| anyhow_empty("Minecraft"))?,
    };
    let loader_version = match &args.loader_version {
        Some(v) => v.clone(),
        None => first_loader_version(versions, loader, &minecraft)?,
    };
    Ok((loader, minecraft, loader_version))
}

/// The build `--yes` picks for a loader+MC: the newest stable one if any are marked stable, else the
/// newest. Bails with an actionable message (nearby supported versions + the `--loader-version`
/// escape hatch) when the pair genuinely has no build, instead of a bare "not found".
fn first_loader_version(versions: &Versions, loader: Loader, minecraft: &str) -> Result<String> {
    let lvs = versions.loader(loader, minecraft)?;
    let best = lvs
        .iter()
        .find(|v| v.note.as_deref() == Some("stable"))
        .or_else(|| lvs.first());
    match best {
        Some(v) => Ok(v.version.clone()),
        None => Err(dead_end(versions, loader, minecraft)),
    }
}

/// The actionable "this loader has no build for this Minecraft version" error: it names nearby
/// versions the loader *does* support and points at the `--loader-version` escape hatch, rather than
/// bailing with a bare "not found".
fn dead_end(versions: &Versions, loader: Loader, minecraft: &str) -> anyhow::Error {
    let hint = match nearby_supported(versions, loader, minecraft) {
        Some(list) => format!(" Supported nearby: {list}."),
        None => String::new(),
    };
    anyhow::anyhow!(
        "{} has no build for Minecraft {minecraft}.{hint} Pass --loader-version <ver> if you know one.",
        label_of(loader)
    )
}

/// A short list of versions the loader *does* support, preferring the same `1.X` line as the failed
/// request (genuinely "nearby"), falling back to the newest handful when nothing shares it — for the
/// dead-end error's hint.
fn nearby_supported(versions: &Versions, loader: Loader, minecraft: &str) -> Option<String> {
    let mcs = versions.minecraft_for(loader).ok()?;
    if mcs.is_empty() {
        return None;
    }
    let near: Vec<String> = match same_series(minecraft) {
        Some(prefix) => mcs
            .iter()
            .filter(|v| v.starts_with(&prefix))
            .take(5)
            .cloned()
            .collect(),
        None => Vec::new(),
    };
    let pick = if near.is_empty() {
        mcs.into_iter().take(5).collect::<Vec<_>>()
    } else {
        near
    };
    Some(pick.join(", "))
}

/// The `major.minor.` prefix of a `1.X.Y` version (`1.20.5` -> `1.20.`), for finding versions on the
/// same line. `None` when there's no patch component to anchor "nearby" on.
fn same_series(mc: &str) -> Option<String> {
    let parts: Vec<&str> = mc.split('.').collect();
    (parts.len() >= 3).then(|| format!("{}.{}.", parts[0], parts[1]))
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

/// Minecraft-version picker (already filtered to the loader's supported set). `filter_mode` lets the
/// user type to narrow the list; `max_rows` caps the viewport so it scrolls in place instead of
/// flooding the terminal. Newest-first, so the pre-selected first row is the sensible default.
fn pick_minecraft(items: &[String]) -> Result<String> {
    let mut picker = cliclack::select("Minecraft version")
        .filter_mode()
        .max_rows(12);
    for v in items {
        picker = picker.item(v.clone(), v.clone(), "");
    }
    Ok(picker.interact()?)
}

/// Loader-version picker. Fabric/Quilt publish hundreds of builds; when any are marked stable we
/// offer just those (newest first) to keep the list navigable — power users can still pin any build
/// with `--loader-version`.
fn pick_loader_version(loader: Loader, lvs: &[LoaderVersion]) -> Result<String> {
    let stable: Vec<&LoaderVersion> = lvs
        .iter()
        .filter(|v| v.note.as_deref() == Some("stable"))
        .collect();
    let shown: Vec<&LoaderVersion> = if stable.is_empty() {
        lvs.iter().collect()
    } else {
        stable
    };

    let mut picker = cliclack::select(format!("{} version", label_of(loader)))
        .filter_mode()
        .max_rows(12);
    for v in shown {
        let label = match &v.note {
            Some(note) => format!("{} ({note})", v.version),
            None => v.version.clone(),
        };
        picker = picker.item(v.version.clone(), label, "");
    }
    Ok(picker.interact()?)
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

#[cfg(test)]
mod tests {
    use super::same_series;

    #[test]
    fn same_series_prefixes_a_patch_version_and_skips_short_ones() {
        assert_eq!(same_series("1.20.5").as_deref(), Some("1.20."));
        assert_eq!(same_series("1.21.4").as_deref(), Some("1.21."));
        // No patch component to anchor "nearby" on — caller falls back to the newest handful.
        assert_eq!(same_series("1.21"), None);
        assert_eq!(same_series("26.2"), None);
    }
}
