use std::fs;
use std::path::Path;

use anyhow::{Context, Result};

use crate::hash::sha256_hex;
use crate::lock::{Lock, LockedMod};
use crate::manifest::Manifest;
use crate::provider::DownloadMode;

const PACK_FORMAT: &str = "packwiz:1.1.0";

/// One row of the generated `index.toml`.
struct IndexEntry {
    file: String,
    hash: String,
    metafile: bool,
}

/// Emit the packwiz distribution tree into `pack_dir`: `pack.toml`, `index.toml`, and one
/// `mods/<slug>.pw.toml` per locked mod. Generated, never hand-edited.
pub fn emit(pack_dir: &Path, root: &Path, manifest: &Manifest, lock: &Lock) -> Result<()> {
    let mods_dir = pack_dir.join("mods");
    // Wipe stale metafiles so a removed mod never lingers in the generated tree.
    if mods_dir.exists() {
        fs::remove_dir_all(&mods_dir)
            .with_context(|| format!("clearing {}", mods_dir.display()))?;
    }
    fs::create_dir_all(&mods_dir).with_context(|| format!("creating {}", mods_dir.display()))?;

    let mut entries: Vec<IndexEntry> = Vec::new();

    for m in &lock.mods {
        // Local jars are bundled into the pack as raw files; provider mods get a `.pw.toml`.
        if m.provider == crate::provider::Provider::Local {
            let rel = format!("mods/{}", m.filename);
            let dest = pack_dir.join(&rel);
            fs::copy(root.join("local").join(&m.filename), &dest)
                .with_context(|| format!("bundling {rel}"))?;
            let bytes = fs::read(&dest).with_context(|| format!("reading {rel}"))?;
            entries.push(IndexEntry {
                hash: sha256_hex(&bytes),
                file: rel,
                metafile: false,
            });
            continue;
        }
        let contents = mod_pw_toml(m);
        let rel = format!("mods/{}.pw.toml", m.slug);
        fs::write(pack_dir.join(&rel), &contents).with_context(|| format!("writing {rel}"))?;
        entries.push(IndexEntry {
            hash: sha256_hex(contents.as_bytes()),
            file: rel,
            metafile: true,
        });
    }

    // Overlay files (config, scripts, resource packs) ship as plain indexed files, not metafiles.
    for overlay in crate::overlay::collect(root, manifest) {
        let dest = pack_dir.join(&overlay.rel);
        if let Some(parent) = dest.parent() {
            fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;
        }
        fs::copy(&overlay.abs, &dest)
            .with_context(|| format!("copying overlay {}", overlay.rel))?;
        let bytes = fs::read(&dest).with_context(|| format!("reading {}", overlay.rel))?;
        entries.push(IndexEntry {
            hash: sha256_hex(&bytes),
            file: overlay.rel,
            metafile: false,
        });
    }

    entries.sort_by(|a, b| a.file.cmp(&b.file));

    let index = index_toml(&entries);
    fs::write(pack_dir.join("index.toml"), &index).context("writing index.toml")?;
    let index_hash = sha256_hex(index.as_bytes());

    let pack = pack_toml(manifest, &index_hash);
    fs::write(pack_dir.join("pack.toml"), pack).context("writing pack.toml")?;

    Ok(())
}

fn toml_string(value: &str) -> String {
    let escaped = value.replace('\\', "\\\\").replace('"', "\\\"");
    format!("\"{escaped}\"")
}

fn mod_pw_toml(m: &LockedMod) -> String {
    let mut out = String::new();
    out.push_str(&format!("name = {}\n", toml_string(&m.name)));
    out.push_str(&format!("filename = {}\n", toml_string(&m.filename)));
    // packwiz omits `side` only for the empty/none case; client/server/both are written.
    if m.side != crate::side::Side::None {
        out.push_str(&format!("side = {}\n", toml_string(m.side.packwiz_token())));
    }

    out.push_str("\n[download]\n");
    if let Some(url) = &m.download.url {
        out.push_str(&format!("url = {}\n", toml_string(url)));
    }
    out.push_str(&format!(
        "hash-format = {}\n",
        toml_string(&m.download.hash_format)
    ));
    out.push_str(&format!("hash = {}\n", toml_string(&m.download.hash)));
    if let DownloadMode::MetadataCurseforge = m.download.mode {
        out.push_str(&format!("mode = {}\n", toml_string("metadata:curseforge")));
    }

    // Update block: legacy packwiz key names (`mod-id`, `version`) are the interop contract.
    match m.provider {
        crate::provider::Provider::Modrinth => {
            out.push_str("\n[update.modrinth]\n");
            out.push_str(&format!("mod-id = {}\n", toml_string(&m.project_id)));
            if let Some(file_id) = &m.file_id {
                out.push_str(&format!("version = {}\n", toml_string(file_id)));
            }
        }
        crate::provider::Provider::Curseforge => {
            out.push_str("\n[update.curseforge]\n");
            out.push_str(&format!("project-id = {}\n", m.project_id));
            if let Some(file_id) = &m.file_id {
                out.push_str(&format!("file-id = {file_id}\n"));
            }
        }
        _ => {}
    }

    out
}

fn index_toml(entries: &[IndexEntry]) -> String {
    let mut out = String::from("hash-format = \"sha256\"\n");
    for entry in entries {
        out.push_str("\n[[files]]\n");
        out.push_str(&format!("file = {}\n", toml_string(&entry.file)));
        out.push_str(&format!("hash = {}\n", toml_string(&entry.hash)));
        if entry.metafile {
            out.push_str("metafile = true\n");
        }
    }
    out
}

fn pack_toml(manifest: &Manifest, index_hash: &str) -> String {
    let mut out = String::new();
    out.push_str(&format!("name = {}\n", toml_string(&manifest.pack.name)));
    out.push_str(&format!(
        "author = {}\n",
        toml_string(&manifest.pack.author)
    ));
    out.push_str(&format!(
        "version = {}\n",
        toml_string(&manifest.pack.version)
    ));
    if let Some(desc) = &manifest.pack.description {
        out.push_str(&format!("description = {}\n", toml_string(desc)));
    }
    out.push_str(&format!("pack-format = {}\n", toml_string(PACK_FORMAT)));

    out.push_str("\n[index]\n");
    out.push_str(&format!("file = {}\n", toml_string("index.toml")));
    out.push_str(&format!("hash-format = {}\n", toml_string("sha256")));
    out.push_str(&format!("hash = {}\n", toml_string(index_hash)));

    out.push_str("\n[versions]\n");
    out.push_str(&format!(
        "minecraft = {}\n",
        toml_string(&manifest.loader.minecraft)
    ));
    out.push_str(&format!(
        "{} = {}\n",
        manifest.loader.name.packwiz_version_key(),
        toml_string(&manifest.loader.version)
    ));

    out
}
