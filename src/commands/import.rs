use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{anyhow, bail, Context, Result};
use indexmap::IndexMap;
use serde::Deserialize;

use crate::cli::{ImportArgs, ImportSource};
use crate::hash::sha256_hex;
use crate::loader::Loader;
use crate::lock::{
    DepEdge, Download, Lock, LockedMod, ResolverMeta, LOCKFILE_VERSION, LOCK_FILENAME,
};
use crate::manifest::{
    Defaults, LoaderSpec, Manifest, ModSpec, ModSpecDetailed, PackMeta, MANIFEST_FILENAME,
};
use crate::provider::{DownloadMode, Provider};
use crate::providers::modrinth::Modrinth;
use crate::resolve::ROOT_REQUESTER;
use crate::side::Side;

pub fn run(args: ImportArgs) -> Result<()> {
    match args.source {
        ImportSource::Packwiz => import_packwiz(&args),
    }
}

/// Convert a packwiz pack (`pack.toml` + `mods/*.pw.toml`) into a lode project — writing
/// `lode.jsonc` + `lode.lock`, leaving the source untouched. The metafiles already carry the
/// resolved file (url + hash + ids), so the jars aren't re-picked; the one thing packwiz never
/// records — the human version number — is recovered from Modrinth (keyless), which also yields
/// the dependency edges packwiz's flat index throws away.
fn import_packwiz(args: &ImportArgs) -> Result<()> {
    let src = args.path.clone().unwrap_or_else(|| PathBuf::from("."));
    let out = args.out.clone().unwrap_or_else(|| PathBuf::from("."));

    // Accept either the pack directory or the pack.toml itself.
    let pack_toml = if src.is_file() {
        src.clone()
    } else {
        src.join("pack.toml")
    };
    if !pack_toml.is_file() {
        bail!("no packwiz pack.toml found at {}", pack_toml.display());
    }
    let pack_dir = pack_toml
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."));

    // Never clobber an existing lode project without consent.
    let manifest_path = out.join(MANIFEST_FILENAME);
    if manifest_path.exists() && !args.force {
        bail!(
            "{} already exists — pass --force to overwrite",
            manifest_path.display()
        );
    }

    let pack: PackToml = parse_toml(&pack_toml)?;
    let minecraft = pack
        .versions
        .get("minecraft")
        .cloned()
        .ok_or_else(|| anyhow!("pack.toml [versions] has no minecraft version"))?;
    let (loader, loader_version) = loader_from(&pack.versions)?;

    let mut records = read_metafiles(&pack_dir)?;
    // Sort for a deterministic manifest + lock, independent of directory-read order.
    records.sort_by(|a, b| a.slug.cmp(&b.slug));

    // Map every project id present in the pack to its slug, so recovered dependency edges only
    // reference mods that are actually here (a dep the author pruned is simply not recorded).
    let project_to_slug: HashMap<&str, &str> = records
        .iter()
        .map(|r| (r.project_id.as_str(), r.slug.as_str()))
        .collect();

    let modrinth = Modrinth::new()?;
    let mut locked: Vec<LockedMod> = Vec::with_capacity(records.len());
    let mut manifest_mods: IndexMap<String, ModSpec> = IndexMap::new();
    let mut warnings: Vec<String> = Vec::new();

    for r in &records {
        // packwiz doesn't store the human version. Modrinth exposes it (and the deps) via the
        // pinned version id; CurseForge keeps the filename, which its substring matcher re-selects.
        let (version, size, dependencies) = match r.provider {
            Provider::Modrinth => match modrinth.version_by_id(&r.file_id) {
                Ok(v) => {
                    let edges = v
                        .dependencies
                        .iter()
                        .filter_map(|d| {
                            project_to_slug
                                .get(d.project_id.as_str())
                                .map(|slug| DepEdge {
                                    slug: (*slug).to_string(),
                                    kind: d.kind,
                                })
                        })
                        .collect();
                    (v.version_number, v.size, edges)
                }
                Err(e) => {
                    warnings.push(format!(
                        "{}: couldn't fetch the version number ({e}); using the filename",
                        r.slug
                    ));
                    (r.filename.clone(), None, Vec::new())
                }
            },
            _ => (r.filename.clone(), None, Vec::new()),
        };

        locked.push(LockedMod {
            slug: r.slug.clone(),
            name: r.name.clone(),
            provider: r.provider,
            project_id: r.project_id.clone(),
            file_id: Some(r.file_id.clone()),
            version,
            filename: r.filename.clone(),
            download: Download {
                url: r.url.clone(),
                mode: r.mode,
                hash_format: r.hash_format.clone(),
                hash: r.hash.clone(),
                size,
            },
            side: r.side,
            optional: false,
            dependencies,
            // packwiz keeps no provenance, so every imported mod is treated as directly declared.
            requested_by: vec![ROOT_REQUESTER.to_string()],
        });

        manifest_mods.insert(r.slug.clone(), manifest_spec(r));
    }

    let manifest = Manifest {
        pack: PackMeta {
            name: pack.name,
            author: pack.author.unwrap_or_default(),
            version: pack.version.unwrap_or_else(|| "0.1.0".to_string()),
            description: pack.description,
        },
        loader: LoaderSpec {
            name: loader,
            minecraft,
            version: loader_version,
        },
        defaults: Defaults { side: Side::Both },
        overlays: Vec::new(),
        mods: manifest_mods,
    };

    // Persist the manifest first, then hash exactly what landed on disk — so the lock is born
    // fresh (matching manifest_hash) and `install` uses it directly, without a re-resolve.
    fs::create_dir_all(&out).with_context(|| format!("creating {}", out.display()))?;
    manifest.save(&manifest_path)?;
    let manifest_hash = format!("sha256:{}", sha256_hex(manifest.to_json()?.as_bytes()));

    let lock = Lock {
        lockfile_version: LOCKFILE_VERSION,
        manifest_hash,
        loader: manifest.loader.clone(),
        resolver: ResolverMeta {
            lode_version: env!("CARGO_PKG_VERSION").to_string(),
        },
        mods: locked,
    };
    let lock_path = out.join(LOCK_FILENAME);
    lock.save(&lock_path)?;

    let cf = records
        .iter()
        .filter(|r| r.provider == Provider::Curseforge)
        .count();
    let mr = records.len() - cf;
    println!(
        "Imported {} mods from packwiz ({mr} Modrinth, {cf} CurseForge).",
        records.len()
    );
    println!(
        "  wrote {} and {}",
        manifest_path.display(),
        lock_path.display()
    );
    for w in &warnings {
        println!("  ! {w}");
    }
    if cf > 0 {
        println!(
            "CurseForge mods need a key to install — set CF_API_KEY or `lode config set curseforge.key <KEY>`."
        );
    }
    println!("Next: `lode install` to download the jars.");
    Ok(())
}

/// Read every `mods/*.pw.toml` under a packwiz pack into normalized records.
fn read_metafiles(pack_dir: &Path) -> Result<Vec<PwRecord>> {
    let mods_dir = pack_dir.join("mods");
    if !mods_dir.is_dir() {
        bail!("no mods/ directory under {}", pack_dir.display());
    }

    let mut records = Vec::new();
    for entry in
        fs::read_dir(&mods_dir).with_context(|| format!("reading {}", mods_dir.display()))?
    {
        let path = entry?.path();
        let Some(slug) = path
            .file_name()
            .and_then(|n| n.to_str())
            .and_then(|n| n.strip_suffix(".pw.toml"))
        else {
            continue;
        };
        let slug = slug.to_string();
        let pw: PwMod = parse_toml(&path)?;
        records.push(PwRecord::from_pw(slug, pw)?);
    }
    if records.is_empty() {
        bail!("no .pw.toml mod files found under {}", mods_dir.display());
    }
    Ok(records)
}

/// The manifest entry for an imported mod: a bare `"*"` constraint for a plain Modrinth mod,
/// the detailed object when it needs a provider or a non-default side (mirroring `lode add`).
fn manifest_spec(r: &PwRecord) -> ModSpec {
    if r.provider == Provider::Modrinth && r.side == Side::Both {
        return ModSpec::Constraint("*".to_string());
    }
    ModSpec::Detailed(ModSpecDetailed {
        version: "*".to_string(),
        side: (r.side != Side::Both).then_some(r.side),
        provider: (r.provider != Provider::Modrinth).then_some(r.provider),
        project_id: None,
        optional: false,
        pin: false,
    })
}

/// Find the single loader key in a packwiz `[versions]` table (everything but `minecraft`).
fn loader_from(versions: &HashMap<String, String>) -> Result<(Loader, String)> {
    for (key, value) in versions {
        let loader = match key.as_str() {
            "forge" => Loader::Forge,
            "neoforge" => Loader::Neoforge,
            "fabric" => Loader::Fabric,
            "quilt" => Loader::Quilt,
            _ => continue,
        };
        return Ok((loader, value.clone()));
    }
    bail!("pack.toml [versions] declares no known loader (forge|neoforge|fabric|quilt)")
}

fn parse_toml<T: serde::de::DeserializeOwned>(path: &Path) -> Result<T> {
    let text = fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    toml::from_str(&text).with_context(|| format!("parsing {}", path.display()))
}

/// A packwiz `pack.toml`, only the fields lode needs.
#[derive(Deserialize)]
struct PackToml {
    name: String,
    author: Option<String>,
    version: Option<String>,
    description: Option<String>,
    #[serde(default)]
    versions: HashMap<String, String>,
}

/// A packwiz `.pw.toml` metafile.
#[derive(Deserialize)]
struct PwMod {
    name: String,
    filename: String,
    side: Option<String>,
    download: PwDownload,
    #[serde(default)]
    update: PwUpdate,
}

#[derive(Deserialize)]
struct PwDownload {
    url: Option<String>,
    #[serde(rename = "hash-format")]
    hash_format: String,
    hash: String,
    #[allow(dead_code)]
    mode: Option<String>,
}

#[derive(Deserialize, Default)]
struct PwUpdate {
    modrinth: Option<PwModrinth>,
    curseforge: Option<PwCurseforge>,
}

#[derive(Deserialize)]
struct PwModrinth {
    #[serde(rename = "mod-id")]
    mod_id: String,
    version: String,
}

#[derive(Deserialize)]
struct PwCurseforge {
    // packwiz writes these as integers; accept either an integer or a string.
    #[serde(rename = "project-id")]
    project_id: toml::Value,
    #[serde(rename = "file-id")]
    file_id: toml::Value,
}

/// A `.pw.toml` normalized into the fields lode's lock/manifest need.
struct PwRecord {
    slug: String,
    name: String,
    provider: Provider,
    project_id: String,
    file_id: String,
    filename: String,
    url: Option<String>,
    hash_format: String,
    hash: String,
    mode: DownloadMode,
    side: Side,
}

impl PwRecord {
    fn from_pw(slug: String, pw: PwMod) -> Result<PwRecord> {
        let side = match pw.side.as_deref() {
            // packwiz omits `side` entirely for the both case.
            None => Side::Both,
            Some("") => Side::None,
            Some("client") => Side::Client,
            Some("server") => Side::Server,
            Some("both") => Side::Both,
            Some(other) => bail!("unknown side '{other}' in {slug}.pw.toml"),
        };

        let (provider, project_id, file_id, url, mode) = if let Some(m) = pw.update.modrinth {
            (
                Provider::Modrinth,
                m.mod_id,
                m.version,
                pw.download.url.clone(),
                DownloadMode::Url,
            )
        } else if let Some(c) = pw.update.curseforge {
            // Match lode's model: never persist the CurseForge URL (its terms forbid it).
            (
                Provider::Curseforge,
                value_to_id(&c.project_id)?,
                value_to_id(&c.file_id)?,
                None,
                DownloadMode::MetadataCurseforge,
            )
        } else {
            bail!("{slug}.pw.toml has no [update.modrinth] or [update.curseforge] block");
        };

        Ok(PwRecord {
            slug,
            name: pw.name,
            provider,
            project_id,
            file_id,
            filename: pw.filename,
            url,
            hash_format: pw.download.hash_format,
            hash: pw.download.hash,
            mode,
            side,
        })
    }
}

fn value_to_id(value: &toml::Value) -> Result<String> {
    match value {
        toml::Value::Integer(i) => Ok(i.to_string()),
        toml::Value::String(s) => Ok(s.clone()),
        other => bail!("expected an id (integer or string), got {other:?}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MODRINTH_PW: &str = r#"
name = "Just Enough Items (JEI)"
filename = "jei-1.20.1-forge-15.20.0.132.jar"
side = "both"

[download]
url = "https://cdn.modrinth.com/data/u6dRKJwZ/versions/p5mYHvjx/jei.jar"
hash-format = "sha512"
hash = "abc123"

[update]
[update.modrinth]
mod-id = "u6dRKJwZ"
version = "p5mYHvjx"
"#;

    const CURSEFORGE_PW: &str = r#"
name = "FTB Quests"
filename = "ftb-quests-forge-2001.jar"

[download]
hash-format = "sha1"
hash = "def456"
mode = "metadata:curseforge"

[update]
[update.curseforge]
project-id = 289412
file-id = 5678901
"#;

    #[test]
    fn maps_a_modrinth_metafile() {
        let pw: PwMod = toml::from_str(MODRINTH_PW).unwrap();
        let r = PwRecord::from_pw("jei".into(), pw).unwrap();
        assert_eq!(r.provider, Provider::Modrinth);
        assert_eq!(r.project_id, "u6dRKJwZ");
        assert_eq!(r.file_id, "p5mYHvjx");
        assert_eq!(r.side, Side::Both);
        assert_eq!(r.mode, DownloadMode::Url);
        assert!(r.url.is_some());
    }

    #[test]
    fn maps_a_curseforge_metafile_with_integer_ids_and_no_url() {
        let pw: PwMod = toml::from_str(CURSEFORGE_PW).unwrap();
        let r = PwRecord::from_pw("ftb-quests-forge".into(), pw).unwrap();
        assert_eq!(r.provider, Provider::Curseforge);
        assert_eq!(r.project_id, "289412");
        assert_eq!(r.file_id, "5678901");
        // packwiz omits `side` -> both; the URL is never persisted for CurseForge.
        assert_eq!(r.side, Side::Both);
        assert_eq!(r.mode, DownloadMode::MetadataCurseforge);
        assert!(r.url.is_none());
    }

    #[test]
    fn side_omitted_is_both_and_empty_is_none() {
        let base = "name=\"x\"\nfilename=\"x.jar\"\n[download]\nhash-format=\"sha1\"\nhash=\"h\"\n[update.modrinth]\nmod-id=\"a\"\nversion=\"b\"\n";
        let both: PwMod = toml::from_str(base).unwrap();
        assert_eq!(
            PwRecord::from_pw("x".into(), both).unwrap().side,
            Side::Both
        );

        let with_none: PwMod =
            toml::from_str(&base.replace("filename=\"x.jar\"", "filename=\"x.jar\"\nside=\"\""))
                .unwrap();
        assert_eq!(
            PwRecord::from_pw("x".into(), with_none).unwrap().side,
            Side::None
        );
    }

    #[test]
    fn manifest_spec_is_bare_for_plain_modrinth_and_detailed_otherwise() {
        let cf = PwRecord {
            slug: "q".into(),
            name: "Q".into(),
            provider: Provider::Curseforge,
            project_id: "1".into(),
            file_id: "2".into(),
            filename: "q.jar".into(),
            url: None,
            hash_format: "sha1".into(),
            hash: "h".into(),
            mode: DownloadMode::MetadataCurseforge,
            side: Side::Both,
        };
        assert!(matches!(manifest_spec(&cf), ModSpec::Detailed(_)));

        let mr = PwRecord {
            provider: Provider::Modrinth,
            mode: DownloadMode::Url,
            ..cf
        };
        assert!(matches!(manifest_spec(&mr), ModSpec::Constraint(c) if c == "*"));
    }

    #[test]
    fn loader_extracted_from_versions_table() {
        let mut versions = HashMap::new();
        versions.insert("minecraft".to_string(), "1.20.1".to_string());
        versions.insert("forge".to_string(), "47.3.0".to_string());
        let (loader, version) = loader_from(&versions).unwrap();
        assert_eq!(loader, Loader::Forge);
        assert_eq!(version, "47.3.0");
    }
}
