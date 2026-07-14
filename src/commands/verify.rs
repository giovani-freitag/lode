use std::path::Path;

use anyhow::{bail, Result};

use crate::cli::VerifyArgs;
use crate::hash::hash_by_format;
use crate::lock::{Lock, LockedMod};
use crate::manifest::Manifest;
use crate::paths::PackPaths;

/// The outcome of checking one locked mod against what's on disk.
#[derive(Debug, PartialEq, Eq)]
enum Status {
    /// File present and its hash matches the lock.
    Ok,
    /// File not on disk (may simply not be installed for this side — not a failure).
    Missing,
    /// File present but its bytes don't match the locked hash — tampering or corruption.
    Mismatch,
    /// File present but the lock's hash format isn't one we can recompute (a format lode doesn't
    /// implement — sha1/256/512 and md5 all verify).
    Unsupported,
}

pub fn run(args: VerifyArgs) -> Result<()> {
    let paths = PackPaths::discover_from_cwd()?;
    let manifest = Manifest::load(&paths.manifest())?;
    if !paths.lock().is_file() {
        bail!("no lode.lock — nothing to verify (run `lode install` first)");
    }
    let lock = Lock::load(&paths.lock())?;

    // Verifying against a lock that no longer matches the manifest is checking a moving target.
    super::warn_if_stale(&manifest, &lock)?;

    let target = args.into.unwrap_or_else(|| paths.root.clone());
    let mods_dir = target.join("mods");

    let (mut ok, mut missing, mut mismatch, mut unsupported) = (0u32, 0u32, 0u32, 0u32);
    for (slug, filename, status) in check_mods(&lock.mods, &mods_dir) {
        match status {
            Status::Ok => ok += 1,
            Status::Missing => missing += 1,
            Status::Unsupported => {
                unsupported += 1;
                println!("  ? {slug} ({filename}) — hash format not verifiable here");
            }
            Status::Mismatch => {
                mismatch += 1;
                println!("  x {slug} ({filename}) — hash mismatch");
            }
        }
    }

    println!(
        "\n{ok} ok, {mismatch} mismatched, {missing} not present, {unsupported} unverifiable (of {} locked mods) in {}",
        lock.mods.len(),
        mods_dir.display()
    );
    if mismatch > 0 {
        bail!(
            "{mismatch} mod(s) do not match the lockfile — the pack has been altered or corrupted"
        );
    }
    Ok(())
}

/// Check each locked mod's jar in `mods_dir` against its recorded hash. Returns `(slug, filename,
/// status)` per mod. Kept free of I/O beyond reading the jars so it is unit-testable.
fn check_mods(mods: &[LockedMod], mods_dir: &Path) -> Vec<(String, String, Status)> {
    mods.iter()
        .map(|m| {
            let status = match std::fs::read(mods_dir.join(&m.filename)) {
                Err(_) => Status::Missing,
                Ok(bytes) => match hash_by_format(&bytes, &m.download.hash_format) {
                    Some(h) if h == m.download.hash => Status::Ok,
                    Some(_) => Status::Mismatch,
                    None => Status::Unsupported,
                },
            };
            (m.slug.clone(), m.filename.clone(), status)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hash::{md5_hex, sha256_hex};
    use crate::lock::Download;
    use crate::provider::{DownloadMode, Provider};
    use crate::side::Side;

    fn locked(filename: &str, hash_format: &str, hash: &str) -> LockedMod {
        LockedMod {
            slug: filename.trim_end_matches(".jar").to_string(),
            name: filename.to_string(),
            provider: Provider::Modrinth,
            project_id: "x".into(),
            file_id: None,
            version: "1".into(),
            filename: filename.to_string(),
            download: Download {
                url: None,
                mode: DownloadMode::Url,
                hash_format: hash_format.to_string(),
                hash: hash.to_string(),
                size: None,
            },
            side: Side::Both,
            optional: false,
            dependencies: Vec::new(),
            requested_by: Vec::new(),
        }
    }

    #[test]
    fn classifies_ok_missing_mismatch_and_unsupported() {
        let dir = tempfile::tempdir().unwrap();
        let mods_dir = dir.path().join("mods");
        std::fs::create_dir_all(&mods_dir).unwrap();
        std::fs::write(mods_dir.join("good.jar"), b"good bytes").unwrap();
        std::fs::write(mods_dir.join("bad.jar"), b"tampered bytes").unwrap();
        std::fs::write(mods_dir.join("md5.jar"), b"cf bytes").unwrap();
        std::fs::write(mods_dir.join("weird.jar"), b"whatever").unwrap();

        let mods = vec![
            locked("good.jar", "sha256", &sha256_hex(b"good bytes")),
            locked("bad.jar", "sha256", &sha256_hex(b"the original bytes")),
            locked("gone.jar", "sha256", &sha256_hex(b"absent")),
            locked("md5.jar", "md5", &md5_hex(b"cf bytes")),
            locked("weird.jar", "crc32", "deadbeef"),
        ];

        let results = check_mods(&mods, &mods_dir);
        let status = |slug: &str| {
            &results
                .iter()
                .find(|(s, _, _)| s == slug)
                .expect("present")
                .2
        };
        assert_eq!(status("good"), &Status::Ok);
        assert_eq!(status("bad"), &Status::Mismatch);
        assert_eq!(status("gone"), &Status::Missing);
        assert_eq!(status("md5"), &Status::Ok);
        assert_eq!(status("weird"), &Status::Unsupported);
    }
}
