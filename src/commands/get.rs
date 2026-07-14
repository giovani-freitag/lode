use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{anyhow, bail, Context, Result};
use flate2::read::GzDecoder;
use reqwest::blocking::Client;
use serde::Deserialize;

use crate::cli::GetArgs;
use crate::commands::install::{install_pack, InstallOpts};
use crate::hash::sha256_hex;
use crate::http::{
    download_capped, download_client, json_capped, DEFAULT_MAX_DOWNLOAD, MAX_CHECKSUM,
};
use crate::lock::Lock;
use crate::manifest::Manifest;
use crate::paths::PackPaths;

const API_BASE: &str = "https://api.github.com";

/// Where the bytes of a published pack came from — the trust story differs per source.
enum Source {
    /// A GitHub release: `owner/repo[@tag]`. The release checksum is the trust anchor.
    GitHub {
        owner: String,
        repo: String,
        tag: Option<String>,
    },
    /// A direct URL to a `.tar.gz`. A sibling `<url>.sha256` is used if present.
    Url(String),
    /// A local tarball. No external anchor exists — provenance is unknown.
    File(PathBuf),
}

/// What a source yielded: the archive, an optional trusted checksum, an optional signature bundle,
/// and a default target-directory name.
struct Fetched {
    targz: Vec<u8>,
    checksum: Option<String>,
    bundle: Option<Vec<u8>>,
    name: String,
}

pub fn run(args: GetArgs) -> Result<()> {
    let source = pick_source(&args)?;

    let client = download_client()?;
    // A token (env GITHUB_TOKEN) lets `get` reach PRIVATE releases — auth on both the release
    // metadata request and the asset download.
    let token = std::env::var("GITHUB_TOKEN").ok().filter(|t| !t.is_empty());

    let fetched = match &source {
        Source::GitHub { owner, repo, tag } => {
            fetch_github(&client, token.as_deref(), owner, repo, tag.as_deref())?
        }
        Source::Url(url) => fetch_url(&client, url)?,
        Source::File(path) => Fetched {
            targz: fs::read(path).with_context(|| format!("reading {}", path.display()))?,
            checksum: None,
            bundle: None,
            name: stem_of(path),
        },
    };
    let targz = &fetched.targz;

    // Integrity: a checksum obtained over a trusted channel (the GitHub release over TLS) proves the
    // archive matches what was published. A *mismatch* is always fatal — the bytes are not the
    // published ones, which is corruption/tampering of content, not merely absent provenance, so
    // `--insecure` deliberately does not cover it.
    let checksum_ok = match &fetched.checksum {
        Some(expected) => {
            let got = sha256_hex(targz);
            if &got != expected {
                bail!(
                    "checksum mismatch: the archive does not match its sha256\n  expected {expected}\n  got      {got}"
                );
            }
            match &source {
                // A GitHub release checksum is fetched from the API over TLS — an anchor an attacker
                // can't forge, so it establishes provenance ("published").
                Source::GitHub { .. } => {
                    println!("Checksum verified against the published sha256.")
                }
                // A same-origin `<url>.sha256` is served by whoever served the archive, so it proves
                // the bytes are intact — never who authored them. Report integrity only, never
                // "published".
                _ => println!(
                    "Checksum matches the sibling .sha256 (integrity only — a same-origin checksum does not establish authorship)."
                ),
            }
            true
        }
        None => false,
    };

    // Authenticity: a Sigstore signature proves the archive was signed by a workflow in the ref's
    // repo, bound to a GitHub identity an attacker can't forge. Fail-closed — a bundle on the source
    // is ALWAYS verified (never silently discarded). `--insecure` downgrades a failed/absent
    // signature to a warning (e.g. a legitimately rotated signing identity); `--verify` overrides
    // that and *requires* a valid signature regardless.
    let signature_ok = match &source {
        Source::GitHub { owner, repo, .. } => match fetched.bundle.as_deref() {
            Some(bundle) => match verify_bundle(targz, bundle, owner, repo) {
                Ok(()) => true,
                Err(e) if args.insecure && !args.verify => {
                    println!("! Signature verification failed — continuing anyway (--insecure): {e:#}");
                    false
                }
                Err(e) => return Err(e),
            },
            None if args.verify => bail!(
                "the release has no .sigstore signature to verify — sign it (see docs/signing.md)"
            ),
            None => false,
        },
        _ if args.verify => bail!("--verify needs a host/owner/repo ref — signatures are bound to a repo identity, not a bare file/URL"),
        _ => false,
    };

    // Fail-closed for remote sources: a pack with neither a verified checksum nor a valid signature
    // has no confirmed provenance, so refuse it unless the user explicitly opts out with `--insecure`.
    // A local file has no external anchor by nature — note it, but don't block.
    match &source {
        Source::File(_) => {
            if !checksum_ok {
                println!("! No checksum to verify against — integrity unconfirmed (provenance of a local file is unknown).");
            }
        }
        Source::Url(_) => {
            // A bare URL shares a local file's footing: a same-origin checksum is written by whoever
            // served the archive (integrity, not authorship), and there is no repo identity to
            // anchor a signature against. So provenance is unverifiable regardless of the checksum —
            // fail-closed like any remote source, rescuable only with --insecure.
            if !args.insecure {
                bail!("refusing to install a URL pack — a same-origin checksum proves integrity, not authorship, and a bare URL has no signable repo identity, so its provenance is unverifiable. Re-run with --insecure to install anyway");
            }
        }
        Source::GitHub { .. } => {
            if !checksum_ok && !signature_ok && !args.insecure {
                bail!("refusing to install a remote pack with no verified checksum and no valid signature — its provenance is unverifiable. Re-run with --insecure to install anyway");
            }
        }
    }

    let target = args
        .into
        .clone()
        .unwrap_or_else(|| PathBuf::from(&fetched.name));
    unpack_into(targz, &target)?;
    println!("Unpacked the pack into {}", target.display());

    // The unpacked pack is self-describing; from here it's an ordinary local install.
    let paths = PackPaths { root: target };
    if !paths.manifest().is_file() {
        bail!("the archive did not contain a lode.json — is it a lode pack?");
    }
    if args.no_install {
        println!(
            "Skipping install (--no-install). Run `lode install` in the pack to fetch the jars."
        );
        return Ok(());
    }
    let manifest = Manifest::load(&paths.manifest())?;
    let lock = Lock::load(&paths.lock())?;
    // Mods are fetched from their providers and each verified against its lock hash (chain of trust).
    install_pack(&paths, &manifest, &lock, &InstallOpts::default())
}

/// Exactly one source must be given: a `host/owner/repo[@tag]` reference, `--from-url`, or
/// `--from-file`.
fn pick_source(args: &GetArgs) -> Result<Source> {
    let count = args.reference.is_some() as u8
        + args.from_url.is_some() as u8
        + args.from_file.is_some() as u8;
    if count != 1 {
        bail!(
            "give exactly one source: `host/owner/repo[@tag]`, --from-url <url>, or --from-file <path>"
        );
    }
    if let Some(reference) = &args.reference {
        let parsed = parse_reference(reference)?;
        // The host decides the provider — explicitly, never guessed. Only GitHub for now.
        if parsed.host != "github.com" {
            bail!(
                "host '{}' isn't supported yet — only github.com for now (GitLab / self-hosted are planned)",
                parsed.host
            );
        }
        return Ok(Source::GitHub {
            owner: parsed.owner,
            repo: parsed.repo,
            tag: parsed.tag,
        });
    }
    if let Some(url) = &args.from_url {
        // https-only, consistent with the download guard: an http URL is both unencrypted and
        // outside the scheme/SSRF policy the fetch layer enforces.
        if !url.starts_with("https://") {
            bail!("--from-url must be an https:// URL (got '{url}')");
        }
        return Ok(Source::Url(url.clone()));
    }
    Ok(Source::File(args.from_file.clone().unwrap()))
}

/// A parsed repo reference: the host is **mandatory** so the source is never guessed — no default
/// platform, no GitHub-then-GitLab fallback (which would be ambiguous and a trust hazard).
struct Ref {
    host: String,
    owner: String,
    repo: String,
    tag: Option<String>,
}

/// Parse `host/owner/repo[@tag]` (or the same as an `https://` URL) into its parts. Requires the
/// host (e.g. `github.com/owner/repo`, à la `go get`); a bare `owner/repo` is rejected.
fn parse_reference(reference: &str) -> Result<Ref> {
    let stripped = reference
        .strip_prefix("https://")
        .or_else(|| reference.strip_prefix("http://"))
        .unwrap_or(reference)
        .trim_end_matches('/');
    let (path, tag) = match stripped.split_once('@') {
        Some((p, t)) => (p, Some(t.to_string())),
        None => (stripped, None),
    };
    let parts: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
    match parts.as_slice() {
        // The host is the first segment and always contains a dot (github.com, gitlab.com, …).
        [host, owner, repo] if host.contains('.') => Ok(Ref {
            host: host.to_string(),
            owner: owner.to_string(),
            repo: repo.trim_end_matches(".git").to_string(),
            tag,
        }),
        [_, _] => bail!(
            "'{reference}' has no host — give the full ref, e.g. github.com/owner/repo (no default, no guessing)"
        ),
        _ => bail!(
            "'{reference}' is not a valid pack ref (expected host/owner/repo[@tag], e.g. github.com/owner/repo)"
        ),
    }
}

#[derive(Deserialize)]
struct Release {
    assets: Vec<Asset>,
}

#[derive(Deserialize)]
struct Asset {
    name: String,
    /// The API asset URL (not `browser_download_url`) — with `Accept: octet-stream` + a token it
    /// downloads private-repo assets too.
    url: String,
}

/// Resolve a GitHub release (latest or a tag), download its `.tar.gz` asset, and — the trust
/// anchor — its `.sha256` sibling. A token (env `GITHUB_TOKEN`) is used if set, for private repos.
fn fetch_github(
    client: &Client,
    token: Option<&str>,
    owner: &str,
    repo: &str,
    tag: Option<&str>,
) -> Result<Fetched> {
    let url = match tag {
        Some(tag) => format!("{API_BASE}/repos/{owner}/{repo}/releases/tags/{tag}"),
        None => format!("{API_BASE}/repos/{owner}/{repo}/releases/latest"),
    };
    let mut req = client
        .get(&url)
        .header("Accept", "application/vnd.github+json");
    if let Some(token) = token {
        req = req.header("Authorization", format!("Bearer {token}"));
    }
    let resp = req
        .send()
        .with_context(|| format!("fetching release {owner}/{repo}"))?
        .error_for_status()
        .with_context(|| format!("release not found for {owner}/{repo}"))?;
    let release: Release = json_capped(resp, "the GitHub release")?;

    let tar_asset = release
        .assets
        .iter()
        .find(|a| a.name.ends_with(".tar.gz"))
        .ok_or_else(|| anyhow!("release for {owner}/{repo} has no .tar.gz asset"))?;
    let targz = download_asset(client, token, &tar_asset.url)?;

    // The checksum asset is the trust anchor — fetched over the same TLS channel from the release.
    let sibling = |suffix: &str| {
        release
            .assets
            .iter()
            .find(|a| a.name == format!("{}{suffix}", tar_asset.name))
            .map(|a| download_asset(client, token, &a.url))
            .transpose()
    };
    // A missing/empty/garbage `.sha256` collapses to "no checksum" rather than erroring here, so the
    // fail-closed authenticity gate (rescuable with `--insecure`) is the one place that decides.
    let checksum =
        sibling(".sha256")?.and_then(|bytes| parse_checksum(&String::from_utf8_lossy(&bytes)));
    let bundle = sibling(".sigstore")?;

    Ok(Fetched {
        targz,
        checksum,
        bundle,
        name: repo.to_string(),
    })
}

/// Download a release asset by its API URL. `Accept: octet-stream` + a token reaches private-repo
/// assets; reqwest strips the auth header on the cross-host redirect to the storage backend.
fn download_asset(client: &Client, token: Option<&str>, api_url: &str) -> Result<Vec<u8>> {
    let mut req = client
        .get(api_url)
        .header("Accept", "application/octet-stream");
    if let Some(token) = token {
        req = req.header("Authorization", format!("Bearer {token}"));
    }
    download_capped(req, DEFAULT_MAX_DOWNLOAD, api_url)
}

/// Download a `.tar.gz` from a direct URL, using a sibling `<url>.sha256` as the anchor if it exists.
fn fetch_url(client: &Client, url: &str) -> Result<Fetched> {
    let targz = download(client, url)?;
    // The sibling `.sha256` is read under a small byte cap (like fetch_github's siblings), so a
    // hostile host can't stream an unbounded body in place of a checksum and OOM the process. A
    // missing/empty/garbage sibling collapses to "no checksum" rather than erroring here, so the
    // fail-closed authenticity gate (rescuable with `--insecure`) is the one place that decides.
    let checksum = download_capped(
        client.get(format!("{url}.sha256")),
        MAX_CHECKSUM,
        "checksum",
    )
    .ok()
    .and_then(|bytes| parse_checksum(&String::from_utf8_lossy(&bytes)));
    Ok(Fetched {
        targz,
        checksum,
        bundle: None,
        name: stem_of(Path::new(url)),
    })
}

fn download(client: &Client, url: &str) -> Result<Vec<u8>> {
    download_capped(client.get(url), DEFAULT_MAX_DOWNLOAD, url)
}

/// The hash from a `sha256sum`-format line (`<hash>  <filename>`): the first whitespace-delimited
/// token, but only when it's a non-empty run of hex digits. Empty or garbage input yields `None`,
/// so a broken sibling `.sha256` (e.g. a CDN 200 error page) counts as "no usable checksum" and is
/// handled by the single authenticity gate rather than being mistaken for a real checksum.
fn parse_checksum(text: &str) -> Option<String> {
    let token = text.split_whitespace().next()?;
    if !token.is_empty() && token.chars().all(|c| c.is_ascii_hexdigit()) {
        Some(token.to_string())
    } else {
        None
    }
}

/// The `.tar.gz`/`.tgz`-stripped basename of a path, for a default target directory name.
fn stem_of(path: &Path) -> String {
    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("pack");
    name.strip_suffix(".tar.gz")
        .or_else(|| name.strip_suffix(".tgz"))
        .unwrap_or(name)
        .to_string()
}

/// The most bytes a pack archive may expand to. A lode pack is a manifest, a lock, and config —
/// kilobytes to a few megabytes. The ceiling exists only so a gzip bomb can't expand a tiny
/// download into an enormous on-disk write.
const MAX_UNPACK_BYTES: u64 = 2 * 1024 * 1024 * 1024;

/// The most entries a pack archive may contain, so a "many tiny files" bomb can't exhaust inodes or
/// spin the loop indefinitely regardless of the byte total.
const MAX_UNPACK_ENTRIES: u64 = 20_000;

/// Extract a `.tar.gz` into `dest`. Each entry is unpacked *within* `dest` — the tar crate refuses
/// paths that would escape it (no `../` traversal, no absolute paths) — and the cumulative expanded
/// size and entry count are capped so a decompression bomb can't turn a small download into an
/// unbounded write. This runs on every source path (a verified GitHub pack still reaches here).
fn unpack(targz: &[u8], dest: &Path) -> Result<()> {
    fs::create_dir_all(dest).with_context(|| format!("creating {}", dest.display()))?;
    let mut archive = tar::Archive::new(GzDecoder::new(targz));
    let mut total_bytes: u64 = 0;
    let mut entries_seen: u64 = 0;
    for entry in archive.entries().context("reading the archive")? {
        let mut entry = entry.context("reading an archive entry")?;

        entries_seen += 1;
        if entries_seen > MAX_UNPACK_ENTRIES {
            bail!("archive has more than {MAX_UNPACK_ENTRIES} entries — refusing (possible decompression bomb)");
        }
        // The header size bounds how many bytes `unpack_in` writes for a regular file; a lie that
        // under-declares can't over-write, because tar reads exactly that many bytes per entry.
        total_bytes = total_bytes.saturating_add(entry.size());
        if total_bytes > MAX_UNPACK_BYTES {
            bail!("archive expands to more than {MAX_UNPACK_BYTES} bytes — refusing (possible decompression bomb)");
        }

        if !entry
            .unpack_in(dest)
            .context("unpacking an archive entry")?
        {
            let path = entry
                .path()
                .map(|p| p.display().to_string())
                .unwrap_or_default();
            bail!(
                "refused to unpack '{path}' — it points outside {}",
                dest.display()
            );
        }
    }
    Ok(())
}

/// Extract into `dest`, atomically for a fresh target: stage into a temp sibling directory so an
/// interrupted or malformed extraction leaves a throwaway dir — never a half-populated pack where
/// `dest` should be — then rename it into place. When `dest` already exists we can't rename onto it,
/// so we merge in place (the prior behavior); the only exposure left is a re-`get` over an existing
/// pack.
fn unpack_into(targz: &[u8], dest: &Path) -> Result<()> {
    if dest.exists() {
        return unpack(targz, dest);
    }
    let parent = dest
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;

    let staging = tempfile::TempDir::new_in(parent).context("creating a staging directory")?;
    unpack(targz, staging.path())?;
    if !staging
        .path()
        .join(crate::manifest::MANIFEST_FILENAME)
        .is_file()
    {
        // `staging` drops here, deleting the partial extraction — nothing lands at `dest`.
        bail!(
            "the archive did not contain a {} — is it a lode pack? (nothing was written)",
            crate::manifest::MANIFEST_FILENAME
        );
    }
    let staged = staging.keep();
    if let Err(e) = fs::rename(&staged, dest) {
        let _ = fs::remove_dir_all(&staged);
        return Err(e).with_context(|| format!("finalizing {}", dest.display()));
    }
    Ok(())
}

const GITHUB_ACTIONS_ISSUER: &str = "https://token.actions.githubusercontent.com";

/// Verify a Sigstore signature bundle for the archive (native, via sigstore-rs), proving it was
/// signed by a GitHub Actions workflow in `owner/repo`. The identity policy requires both the
/// GitHub Actions OIDC issuer and the workflow-repository extension — robust to the workflow's
/// filename/branch, and an attacker can't mint that identity. Offline, this proves the Fulcio
/// certificate chain, the certificate-transparency SCT, and that owner/repo identity binding — it
/// does NOT verify Rekor transparency-log inclusion or the SET (sigstore-rs 0.14 does not check
/// those offline). What it establishes is *who signed*, not that the signature was logged.
// TODO(sigstore-rs#285): drop this caveat once offline Rekor inclusion/SET verification lands.
fn verify_bundle(targz: &[u8], bundle_bytes: &[u8], owner: &str, repo: &str) -> Result<()> {
    use sigstore::bundle::verify::policy::SingleX509ExtPolicy;
    use sigstore::bundle::verify::{blocking::Verifier, policy};
    use sigstore::trust::sigstore::SigstoreTrustRoot;

    // The Sigstore trust material (Fulcio CA + Rekor/CT keys), vendored. We load it directly instead
    // of via `Verifier::production()`'s TUF bootstrap: the crate's embedded TUF *root* expires yearly
    // (and breaks verification when it lapses), whereas this trusted-root material is long-lived.
    // Refresh src/assets/sigstore-trusted-root.json if Sigstore rotates its roots.
    const TRUSTED_ROOT: &[u8] = include_bytes!("../assets/sigstore-trusted-root.json");

    let bundle: sigstore::bundle::Bundle =
        serde_json::from_slice(bundle_bytes).context("parsing the .sigstore bundle")?;
    let trust_root = SigstoreTrustRoot::from_trusted_root_json_unchecked(TRUSTED_ROOT)
        .map_err(|e| anyhow!("loading the Sigstore trusted root: {e}"))?;
    let verifier = Verifier::new(Default::default(), trust_root)
        .map_err(|e| anyhow!("initializing the Sigstore verifier: {e}"))?;

    let issuer = policy::OIDCIssuer::new(GITHUB_ACTIONS_ISSUER);
    let repository = policy::GitHubWorkflowRepository::new(format!("{owner}/{repo}"));
    let id_policy = policy::AllOf::new([
        &issuer as &dyn policy::VerificationPolicy,
        &repository as &dyn policy::VerificationPolicy,
    ])
    .expect("policy list is non-empty");

    verifier
        .verify(targz, bundle, &id_policy, true)
        .map_err(|e| {
            anyhow!("signature verification failed — the archive is not signed by a workflow in {owner}/{repo}: {e}")
        })?;
    println!("Signature verified: built by a GitHub Actions workflow in {owner}/{repo} (Sigstore — Fulcio chain + CT SCT + repo identity; not Rekor inclusion/SET).");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_host_owner_repo_with_and_without_tag() {
        let r = parse_reference("github.com/giovani-freitag/vestige").unwrap();
        assert_eq!(
            (r.host.as_str(), r.owner.as_str(), r.repo.as_str(), r.tag),
            ("github.com", "giovani-freitag", "vestige", None)
        );
        let r = parse_reference("github.com/giovani-freitag/vestige@v1.0.0").unwrap();
        assert_eq!(
            (
                r.host.as_str(),
                r.owner.as_str(),
                r.repo.as_str(),
                r.tag.as_deref()
            ),
            ("github.com", "giovani-freitag", "vestige", Some("v1.0.0"))
        );
    }

    #[test]
    fn accepts_a_full_url_and_strips_scheme_and_git_suffix() {
        let r = parse_reference("https://github.com/giovani-freitag/vestige.git").unwrap();
        assert_eq!(
            (r.host.as_str(), r.owner.as_str(), r.repo.as_str()),
            ("github.com", "giovani-freitag", "vestige")
        );
    }

    #[test]
    fn rejects_hostless_and_malformed_refs() {
        // No host (bare owner/repo), a single word, and too many segments are all rejected —
        // the host must be explicit.
        for bad in ["sodium", "giovani-freitag/vestige", "a/b/c", "/x", "x/"] {
            assert!(parse_reference(bad).is_err(), "should reject {bad}");
        }
    }

    #[test]
    fn parse_checksum_takes_the_first_hex_token() {
        assert_eq!(
            parse_checksum("abc123  vestige-1.0.0.tar.gz\n").as_deref(),
            Some("abc123")
        );
        assert_eq!(parse_checksum("deadbeef").as_deref(), Some("deadbeef"));
    }

    #[test]
    fn parse_checksum_rejects_empty_or_non_hex() {
        // A broken/empty sibling checksum must be distinguishable from a genuine mismatch.
        assert_eq!(parse_checksum(""), None);
        assert_eq!(parse_checksum("   \n"), None);
        assert_eq!(parse_checksum("not-a-hash"), None);
        assert_eq!(parse_checksum("<html>404</html>"), None);
    }

    #[test]
    fn stem_strips_tar_gz() {
        assert_eq!(
            stem_of(Path::new("dist/vestige-0.1.0.tar.gz")),
            "vestige-0.1.0"
        );
        assert_eq!(stem_of(Path::new("pack.tgz")), "pack");
    }

    #[test]
    fn unpack_round_trips_a_bundle() {
        // A tar.gz built by `bundle::archive` unpacks back to the same files.
        let entries = vec![
            ("lode.json".to_string(), b"{}".to_vec()),
            ("lode.lock".to_string(), b"lock".to_vec()),
            ("config/x.toml".to_string(), b"cfg".to_vec()),
        ];
        let targz = crate::commands::bundle::archive(&entries).unwrap();
        let dir = tempfile::tempdir().unwrap();
        unpack(&targz, dir.path()).unwrap();
        assert_eq!(fs::read(dir.path().join("lode.json")).unwrap(), b"{}");
        assert_eq!(fs::read(dir.path().join("config/x.toml")).unwrap(), b"cfg");
    }

    #[test]
    fn unpack_into_stages_then_renames_a_fresh_target() {
        let entries = vec![
            ("lode.json".to_string(), b"{}".to_vec()),
            ("config/x.toml".to_string(), b"cfg".to_vec()),
        ];
        let targz = crate::commands::bundle::archive(&entries).unwrap();
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("mypack");

        unpack_into(&targz, &target).unwrap();

        assert!(target.join("lode.json").is_file());
        assert_eq!(fs::read(target.join("config/x.toml")).unwrap(), b"cfg");
        // Only the finalized target remains in the parent — no leftover staging dir.
        let siblings: Vec<_> = fs::read_dir(dir.path())
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(siblings, vec![std::ffi::OsString::from("mypack")]);
    }

    #[test]
    fn unpack_into_rejects_a_manifestless_archive_and_writes_nothing() {
        let entries = vec![("config/x.toml".to_string(), b"cfg".to_vec())];
        let targz = crate::commands::bundle::archive(&entries).unwrap();
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("mypack");

        assert!(unpack_into(&targz, &target).is_err());

        assert!(
            !target.exists(),
            "a manifest-less archive must leave no target behind"
        );
        assert_eq!(
            fs::read_dir(dir.path()).unwrap().count(),
            0,
            "the staging dir must be cleaned up on rejection"
        );
    }
}
