use std::fs;
use std::path::PathBuf;

use anyhow::{bail, Context, Result};
use flate2::write::GzEncoder;
use flate2::Compression;

use crate::cli::BundleArgs;
use crate::hash::sha256_hex;
use crate::lock::LOCK_FILENAME;
use crate::manifest::{Manifest, MANIFEST_FILENAME};
use crate::paths::PackPaths;

pub fn run(args: BundleArgs) -> Result<()> {
    let paths = PackPaths::discover_from_cwd()?;
    let manifest = Manifest::load(&paths.manifest())?;
    let built = build_archive(&paths, &manifest)?;

    let out_dir = args.out.unwrap_or_else(|| PathBuf::from("."));
    fs::create_dir_all(&out_dir).with_context(|| format!("creating {}", out_dir.display()))?;
    let tar_path = out_dir.join(&built.filename);
    fs::write(&tar_path, &built.targz)
        .with_context(|| format!("writing {}", tar_path.display()))?;
    let sum_path = out_dir.join(built.checksum_filename());
    fs::write(&sum_path, built.checksum_file())
        .with_context(|| format!("writing {}", sum_path.display()))?;

    println!(
        "Bundled {} ({} overlay file(s), no jars) — {} bytes",
        built.filename,
        built.overlay_count,
        built.targz.len()
    );
    println!("  {}", tar_path.display());
    println!("  {}  (sha256 {})", sum_path.display(), &built.digest[..16]);
    Ok(())
}

/// A built distributable held in memory — shared by `bundle` (writes it to disk) and `publish`
/// (uploads it to a release).
pub struct BuiltArchive {
    pub filename: String,
    pub targz: Vec<u8>,
    pub digest: String,
    pub overlay_count: usize,
}

impl BuiltArchive {
    pub fn checksum_filename(&self) -> String {
        format!("{}.sha256", self.filename)
    }

    /// The `sha256sum`-format line (`<hash>  <filename>`), verifiable with `sha256sum -c`.
    pub fn checksum_file(&self) -> String {
        format!("{}  {}\n", self.digest, self.filename)
    }
}

/// Build the pack's distributable archive in memory. THIN by design: the pack DEFINITION only —
/// the manifest, the lockfile, and the overlay sources (config/, kubejs/). Deliberately NO mod jars
/// (redistribution is forbidden; they're fetched from the provider and verified against the lock on
/// install) and no runtime files.
pub fn build_archive(paths: &PackPaths, manifest: &Manifest) -> Result<BuiltArchive> {
    if !paths.lock().is_file() {
        bail!("no lode.lock — run `lode install` or `lode refresh` first");
    }
    let mut entries: Vec<(String, Vec<u8>)> = Vec::new();
    entries.push((
        MANIFEST_FILENAME.to_string(),
        fs::read(paths.manifest()).context("reading manifest")?,
    ));
    entries.push((
        LOCK_FILENAME.to_string(),
        fs::read(paths.lock()).context("reading lockfile")?,
    ));
    for overlay in crate::overlay::collect(&paths.root, manifest) {
        let bytes =
            fs::read(&overlay.abs).with_context(|| format!("reading overlay {}", overlay.rel))?;
        entries.push((overlay.rel, bytes));
    }
    let overlay_count = entries.len() - 2;

    let targz = archive(&entries)?;
    let digest = sha256_hex(&targz);
    let filename = format!(
        "{}.tar.gz",
        stem(&manifest.pack.name, &manifest.pack.version)
    );
    Ok(BuiltArchive {
        filename,
        targz,
        digest,
        overlay_count,
    })
}

/// Pack the `(arcname, bytes)` entries into a **deterministic** `.tar.gz`: entries sorted by name,
/// headers with zeroed mtime/uid/gid and a fixed mode, gzip with no filename/timestamp. Re-bundling
/// identical inputs yields byte-identical output, so the published checksum is meaningful.
pub fn archive(entries: &[(String, Vec<u8>)]) -> Result<Vec<u8>> {
    let mut sorted: Vec<&(String, Vec<u8>)> = entries.iter().collect();
    sorted.sort_by(|a, b| a.0.cmp(&b.0));

    // GzEncoder (unlike GzBuilder) writes mtime 0 and no filename — no wall-clock leaks in.
    let gz = GzEncoder::new(Vec::new(), Compression::default());
    let mut builder = tar::Builder::new(gz);
    for (name, bytes) in sorted {
        let mut header = tar::Header::new_gnu();
        header.set_size(bytes.len() as u64);
        header.set_mode(0o644);
        header.set_mtime(0);
        builder
            .append_data(&mut header, name, bytes.as_slice())
            .with_context(|| format!("adding {name} to the archive"))?;
    }
    let gz = builder.into_inner().context("finalizing tar")?;
    gz.finish().context("finalizing gzip")
}

/// A filesystem-safe `<name>-<version>` stem for the artifact filename.
fn stem(name: &str, version: &str) -> String {
    let safe: String = name
        .to_ascii_lowercase()
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.') {
                c
            } else {
                '-'
            }
        })
        .collect();
    let safe = safe.trim_matches('-');
    let safe = if safe.is_empty() { "pack" } else { safe };
    format!("{safe}-{version}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn archive_is_deterministic_and_order_independent() {
        let entries = vec![
            ("lode.lock".to_string(), b"lockbytes".to_vec()),
            ("lode.jsonc".to_string(), b"manifestbytes".to_vec()),
            ("config/a.toml".to_string(), b"aaa".to_vec()),
        ];
        let first = archive(&entries).unwrap();
        // Same input, and input in a different order, must both be byte-identical.
        assert_eq!(archive(&entries).unwrap(), first);
        let mut shuffled = entries.clone();
        shuffled.reverse();
        assert_eq!(archive(&shuffled).unwrap(), first);
    }

    #[test]
    fn archive_round_trips_every_entry() {
        let entries = vec![
            ("lode.jsonc".to_string(), b"m".to_vec()),
            ("lode.lock".to_string(), b"l".to_vec()),
            ("config/a.toml".to_string(), b"aaa".to_vec()),
        ];
        let targz = archive(&entries).unwrap();
        let dec = flate2::read::GzDecoder::new(&targz[..]);
        let mut tar = tar::Archive::new(dec);
        let mut got: Vec<String> = Vec::new();
        for entry in tar.entries().unwrap() {
            let entry = entry.unwrap();
            got.push(entry.path().unwrap().to_string_lossy().into_owned());
        }
        got.sort();
        assert_eq!(got, vec!["config/a.toml", "lode.jsonc", "lode.lock"]);
    }

    #[test]
    fn stem_is_filesystem_safe() {
        assert_eq!(stem("My Pack!", "1.0.0"), "my-pack-1.0.0");
        assert_eq!(stem("Vestige", "0.1.0"), "vestige-0.1.0");
        assert_eq!(stem("***", "2"), "pack-2");
    }
}
