use std::path::Path;

use anyhow::{anyhow, bail, Context, Result};
use dialoguer::Select;

use super::{count_kinds, parse_side, resolve_and_save};
use crate::cli::AddArgs;
use crate::manifest::{Manifest, ModSpec, ModSpecDetailed};
use crate::paths::PackPaths;
use crate::provider::Provider;
use crate::providers::curseforge::Curseforge;
use crate::providers::modrinth::Modrinth;

pub fn run(args: AddArgs) -> Result<()> {
    let paths = PackPaths::discover_from_cwd()?;
    let mut manifest = Manifest::load(&paths.manifest())?;

    // A local `.jar` path is bundled straight from disk; otherwise resolve from a provider.
    let (slug, spec) = if is_local_jar(&args.query) {
        prepare_local(&paths, &args)?
    } else {
        prepare_remote(&args, &manifest)?
    };

    // Capture before moving `spec` — persistence differs for a brand-new vs. existing entry.
    let already_declared = manifest.mods.contains_key(&slug);
    let spec_json = serde_json::to_string(&spec)?;
    manifest.mods.insert(slug.clone(), spec);

    // Resolve against the full manifest so the new mod's dependencies are pulled in too.
    let lock = resolve_and_save(&paths, &manifest)?;

    // Persist the manifest. For a new mod, splice it in textually so hand-written comments survive;
    // for a version change on an existing mod, a full re-serialize is fine.
    if already_declared {
        manifest.save(&paths.manifest())?;
    } else {
        let text = std::fs::read_to_string(paths.manifest())?;
        match crate::manifest::insert_mod_text(&text, &slug, &spec_json) {
            Some(updated) => std::fs::write(paths.manifest(), updated)?,
            None => manifest.save(&paths.manifest())?,
        }
    }

    let added = lock
        .find(&slug)
        .map(|m| m.version.clone())
        .unwrap_or_else(|| "?".to_string());
    let (direct, deps) = count_kinds(&lock);
    println!("Added {slug} ({added}).");
    println!(
        "Pack now has {} mods ({direct} direct, {deps} dependencies).",
        lock.mods.len()
    );

    // Like `npm install <pkg>`: adding also downloads the jar (and any new deps) into the
    // instance, unless the user only wants the manifest/lockfile updated.
    if !args.lock_only {
        crate::commands::install::install_pack(&paths, &manifest, &lock, &Default::default())?;
    }
    Ok(())
}

/// Whether the query points at an existing local `.jar` file.
fn is_local_jar(query: &str) -> bool {
    let path = Path::new(query);
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case("jar"))
        && path.is_file()
}

/// Resolve a mod from a provider (Modrinth by default, or CurseForge), returning its slug + spec.
fn prepare_remote(args: &AddArgs, manifest: &Manifest) -> Result<(String, ModSpec)> {
    let use_curseforge = args.curseforge || args.query.contains("curseforge.com");
    let query = normalize_query(&args.query);
    let constraint = args.version.clone().unwrap_or_else(|| "*".to_string());
    let side = args.side.as_deref().map(parse_side).transpose()?;

    let (slug, provider) = if use_curseforge {
        let cf = Curseforge::from_config()?;
        (cf.slug_of(&query)?, Some(Provider::Curseforge))
    } else {
        let modrinth = Modrinth::new()?;
        (resolve_slug(&modrinth, &query, manifest, args.yes)?, None)
    };

    // A bare version constraint suffices for a plain Modrinth mod; anything with a side or a
    // non-default provider needs the detailed object form.
    let spec = if provider.is_some() || side.is_some() {
        ModSpec::Detailed(ModSpecDetailed {
            version: constraint,
            side,
            provider,
            project_id: None,
            optional: false,
            pin: false,
        })
    } else {
        ModSpec::Constraint(constraint)
    };
    Ok((slug, spec))
}

/// Bundle a local jar: copy it under `local/` and return a `local`-provider spec keyed by slug.
fn prepare_local(paths: &PackPaths, args: &AddArgs) -> Result<(String, ModSpec)> {
    let src = Path::new(&args.query);
    let filename = src
        .file_name()
        .and_then(|f| f.to_str())
        .ok_or_else(|| anyhow!("invalid jar path '{}'", args.query))?
        .to_string();
    let slug = local_slug(&filename);

    let local_dir = paths.root.join("local");
    std::fs::create_dir_all(&local_dir)
        .with_context(|| format!("creating {}", local_dir.display()))?;
    let dest = local_dir.join(&filename);
    // Skip the copy when the source already is the bundled file (re-adding).
    if src.canonicalize().ok() != dest.canonicalize().ok() {
        std::fs::copy(src, &dest).with_context(|| format!("bundling {filename} into local/"))?;
    }

    let side = args.side.as_deref().map(parse_side).transpose()?;
    let spec = ModSpec::Detailed(ModSpecDetailed {
        version: "local".to_string(),
        side,
        provider: Some(Provider::Local),
        project_id: Some(filename),
        optional: false,
        pin: false,
    });
    println!("Bundled {slug} into local/");
    Ok((slug, spec))
}

/// Derive a mod slug from a bundled jar's filename: drop the `.jar`, lowercase, spaces to dashes.
fn local_slug(filename: &str) -> String {
    filename
        .strip_suffix(".jar")
        .unwrap_or(filename)
        .to_ascii_lowercase()
        .replace(' ', "-")
}

/// Strip a Modrinth or CurseForge URL down to its slug; otherwise return the query unchanged.
fn normalize_query(query: &str) -> String {
    if let Some(rest) = query.split("modrinth.com/").nth(1) {
        // e.g. "mod/sodium" or "mod/sodium/version/xyz" -> "sodium"
        if let Some(slug) = rest.split('/').nth(1) {
            return slug.to_string();
        }
    }
    if query.contains("curseforge.com") {
        // e.g. ".../minecraft/mc-mods/jei" -> "jei"
        let path = query.split(['?', '#']).next().unwrap_or(query);
        if let Some(segment) = path.trim_end_matches('/').rsplit('/').next() {
            return segment.to_string();
        }
    }
    query.to_string()
}

/// Turn a slug/id/search-term into a concrete Modrinth slug, prompting to disambiguate a search.
fn resolve_slug(
    modrinth: &Modrinth,
    query: &str,
    manifest: &Manifest,
    yes: bool,
) -> Result<String> {
    if let Some(slug) = modrinth.project_slug(query)? {
        return Ok(slug);
    }

    let hits = modrinth.search(query, manifest.loader.name, &manifest.loader.minecraft)?;
    if hits.is_empty() {
        bail!("no Modrinth project matches '{query}' for this loader/MC version");
    }
    if yes || hits.len() == 1 {
        return Ok(hits[0].slug.clone());
    }

    let labels: Vec<String> = hits
        .iter()
        .map(|h| format!("{} — {}", h.title, truncate(&h.description, 60)))
        .collect();
    let choice = Select::new()
        .with_prompt("Select a mod")
        .items(&labels)
        .default(0)
        .interact()?;
    Ok(hits[choice].slug.clone())
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        format!("{}…", s.chars().take(max).collect::<String>())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_query_extracts_slug_from_modrinth_urls() {
        // Both the bare project page and a versioned URL collapse to the slug.
        assert_eq!(normalize_query("https://modrinth.com/mod/sodium"), "sodium");
        assert_eq!(
            normalize_query("https://modrinth.com/mod/sodium/version/AbCdEf12"),
            "sodium"
        );
    }

    #[test]
    fn normalize_query_extracts_slug_from_curseforge_urls() {
        // The last path segment is the slug; a query string, fragment, or trailing slash is stripped.
        assert_eq!(
            normalize_query("https://www.curseforge.com/minecraft/mc-mods/jei"),
            "jei"
        );
        assert_eq!(
            normalize_query("https://www.curseforge.com/minecraft/mc-mods/jei/"),
            "jei"
        );
        assert_eq!(
            normalize_query("https://www.curseforge.com/minecraft/mc-mods/jei?page=1#files"),
            "jei"
        );
    }

    #[test]
    fn normalize_query_passes_through_a_bare_slug() {
        // A plain slug (not a URL) is returned untouched — the common `lode add sodium` case.
        assert_eq!(normalize_query("sodium"), "sodium");
    }

    #[test]
    fn truncate_leaves_short_and_exact_length_strings_untouched() {
        // At or under the limit, nothing is cut and no ellipsis is appended.
        assert_eq!(truncate("hello", 5), "hello");
        assert_eq!(truncate("hi", 5), "hi");
    }

    #[test]
    fn truncate_cuts_over_length_input_and_appends_an_ellipsis() {
        // 6 chars, max 5 -> first 5 chars plus the ellipsis.
        assert_eq!(truncate("hello!", 5), "hello…");
    }

    #[test]
    fn truncate_counts_characters_not_bytes() {
        // Multibyte input must be measured and sliced by char, never by byte — a byte-based
        // truncate would miscount the length (and could panic splitting a multibyte char).
        // "café" is 4 chars but 5 bytes.
        assert_eq!(truncate("café", 4), "café");
        assert_eq!(truncate("café latte", 4), "café…");
    }

    #[test]
    fn local_slug_strips_jar_lowercases_and_dashes_spaces() {
        assert_eq!(local_slug("Cool Mod.jar"), "cool-mod");
        assert_eq!(local_slug("sodium.jar"), "sodium");
        assert_eq!(local_slug("Multi Word Name.jar"), "multi-word-name");
    }

    #[test]
    fn local_slug_without_a_jar_suffix_keeps_the_whole_name() {
        // No `.jar` to strip -> the full name is lowercased/dashed as-is.
        assert_eq!(local_slug("NoExt"), "noext");
    }

    #[test]
    fn is_local_jar_true_for_an_existing_jar_file() {
        let dir = tempfile::tempdir().unwrap();
        let jar = dir.path().join("mod.jar");
        std::fs::write(&jar, b"x").unwrap();
        assert!(is_local_jar(jar.to_str().unwrap()));
    }

    #[test]
    fn is_local_jar_extension_check_is_case_insensitive() {
        let dir = tempfile::tempdir().unwrap();
        let jar = dir.path().join("MOD.JAR");
        std::fs::write(&jar, b"x").unwrap();
        assert!(is_local_jar(jar.to_str().unwrap()));
    }

    #[test]
    fn is_local_jar_false_for_non_jar_and_for_a_missing_path() {
        let dir = tempfile::tempdir().unwrap();
        let txt = dir.path().join("notes.txt");
        std::fs::write(&txt, b"x").unwrap();
        // Wrong extension, even though the file exists.
        assert!(!is_local_jar(txt.to_str().unwrap()));
        // Right extension but no such file -> must route to a provider, not local bundling.
        let ghost = dir.path().join("ghost.jar");
        assert!(!is_local_jar(ghost.to_str().unwrap()));
        // A bare slug is neither a jar nor a file.
        assert!(!is_local_jar("sodium"));
    }
}
