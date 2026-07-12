use std::fs;
use std::path::Path;
use std::process::Command;

use anyhow::{anyhow, bail, Context, Result};
use reqwest::blocking::Client;
use serde::Deserialize;
use serde_json::json;

use crate::cli::PublishArgs;
use crate::commands::bundle::build_archive;
use crate::manifest::Manifest;
use crate::paths::PackPaths;

const API_BASE: &str = "https://api.github.com";
const UPLOADS_BASE: &str = "https://uploads.github.com";

pub fn run(args: PublishArgs) -> Result<()> {
    let paths = PackPaths::discover_from_cwd()?;
    let manifest = Manifest::load(&paths.manifest())?;
    let built = build_archive(&paths, &manifest)?;

    // Optionally sign the archive before publishing — keyless, via cosign (Fulcio + Rekor).
    let signature = if args.sign {
        Some(sign_blob(&built.filename, &built.targz)?)
    } else {
        None
    };

    let token = std::env::var("GITHUB_TOKEN")
        .ok()
        .filter(|t| !t.is_empty())
        .ok_or_else(|| {
            anyhow!(
                "publishing needs a GitHub token — set GITHUB_TOKEN to a token with `contents: write` on the repo"
            )
        })?;
    let (owner, repo) = resolve_repo(args.repo.as_deref(), &paths.root)?;
    let gh = Gh::new(owner, repo, token)?;

    // Reuse the release if the tag already exists; otherwise create it.
    let release_id = match gh.get_release(&args.tag)? {
        Some(id) => {
            println!("Attaching to existing release {}@{}", gh.repo, args.tag);
            id
        }
        None => gh.create_release(&args.tag, args.prerelease)?,
    };

    // The artifact, then its checksum — the checksum is the consumer's trust anchor.
    gh.upload_asset(
        release_id,
        &built.filename,
        "application/gzip",
        &built.targz,
    )?;
    gh.upload_asset(
        release_id,
        &built.checksum_filename(),
        "text/plain",
        built.checksum_file().as_bytes(),
    )?;
    if let Some(sig) = &signature {
        gh.upload_asset(
            release_id,
            &format!("{}.sigstore", built.filename),
            "application/json",
            sig,
        )?;
    }

    println!(
        "Published {} to {}/{}@{}",
        built.filename, gh.owner, gh.repo, args.tag
    );
    let verify = if signature.is_some() { " --verify" } else { "" };
    println!(
        "  install it with:  lode get github.com/{}/{}@{}{verify}",
        gh.owner, gh.repo, args.tag
    );
    Ok(())
}

/// Keyless-sign the archive with cosign (Fulcio + Rekor) and return the `.sigstore` bundle bytes.
/// Uses the ambient OIDC identity in CI, or an interactive browser locally. Keyless signing in pure
/// Rust is still immature, so this delegates to cosign — the reference signer.
fn sign_blob(filename: &str, targz: &[u8]) -> Result<Vec<u8>> {
    // A fresh 0700 per-invocation directory (O_EXCL create) instead of a predictable path in the
    // shared temp dir — on a multi-user host a pre-planted symlink at `temp_dir()/<filename>` would
    // otherwise redirect the write and the bundle read. It's removed when `dir` drops.
    let dir = tempfile::tempdir().context("creating a temporary directory to sign the archive")?;
    let tar_path = dir.path().join(filename);
    let bundle_path = dir.path().join(format!("{filename}.sigstore"));
    fs::write(&tar_path, targz)
        .with_context(|| format!("writing temp artifact {}", tar_path.display()))?;

    let status = Command::new("cosign")
        .args(["sign-blob", "--yes", "--new-bundle-format", "--bundle"])
        .arg(&bundle_path)
        .arg(&tar_path)
        .status();

    match status {
        Ok(s) if s.success() => {
            fs::read(&bundle_path).with_context(|| format!("reading {}", bundle_path.display()))
        }
        Ok(_) => bail!("cosign sign-blob failed — check the OIDC flow (CI needs id-token: write)"),
        Err(e) => bail!("could not run cosign ({e}) — install cosign to use --sign"),
    }
}

#[derive(Deserialize)]
struct ReleaseResp {
    id: u64,
}

/// An authenticated GitHub REST client scoped to one repo — the release + asset endpoints.
struct Gh {
    client: Client,
    token: String,
    owner: String,
    repo: String,
}

impl Gh {
    fn new(owner: String, repo: String, token: String) -> Result<Gh> {
        Ok(Gh {
            client: crate::http::client()?,
            token,
            owner,
            repo,
        })
    }

    fn auth(&self, req: reqwest::blocking::RequestBuilder) -> reqwest::blocking::RequestBuilder {
        req.header("Accept", "application/vnd.github+json")
            .header("Authorization", format!("Bearer {}", self.token))
    }

    /// The release id for an existing tag, or `None` if the tag has no release yet.
    fn get_release(&self, tag: &str) -> Result<Option<u64>> {
        let resp = self
            .auth(self.client.get(format!(
                "{API_BASE}/repos/{}/{}/releases/tags/{tag}",
                self.owner, self.repo
            )))
            .send()
            .context("querying existing release")?;
        if resp.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(None);
        }
        let resp = resp
            .error_for_status()
            .context("querying existing release")?;
        let release: ReleaseResp = crate::http::json_capped(resp, "release")?;
        Ok(Some(release.id))
    }

    fn create_release(&self, tag: &str, prerelease: bool) -> Result<u64> {
        let resp = self
            .auth(self.client.post(format!(
                "{API_BASE}/repos/{}/{}/releases",
                self.owner, self.repo
            )))
            .json(&json!({ "tag_name": tag, "name": tag, "prerelease": prerelease }))
            .send()
            .context("creating the release")?
            .error_for_status()
            .context("creating the release (check the token has contents:write)")?;
        let release: ReleaseResp = crate::http::json_capped(resp, "the created release")?;
        println!("Created release {}@{tag}", self.repo);
        Ok(release.id)
    }

    fn upload_asset(
        &self,
        release_id: u64,
        name: &str,
        content_type: &str,
        body: &[u8],
    ) -> Result<()> {
        let resp = self
            .auth(self.client.post(format!(
                "{UPLOADS_BASE}/repos/{}/{}/releases/{release_id}/assets?name={name}",
                self.owner, self.repo
            )))
            .header("Content-Type", content_type)
            .body(body.to_vec())
            .send()
            .with_context(|| format!("uploading {name}"))?;
        if resp.status() == reqwest::StatusCode::UNPROCESSABLE_ENTITY {
            bail!("an asset named '{name}' already exists on this release — delete it or publish a new tag");
        }
        resp.error_for_status()
            .with_context(|| format!("uploading {name}"))?;
        println!("  uploaded {name}");
        Ok(())
    }
}

/// The `owner/repo` to publish to: the explicit `--repo`, else parsed from the `origin` remote.
fn resolve_repo(explicit: Option<&str>, root: &Path) -> Result<(String, String)> {
    if let Some(repo) = explicit {
        return parse_owner_repo(repo);
    }
    let out = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["remote", "get-url", "origin"])
        .output()
        .context("running git to read the origin remote")?;
    if !out.status.success() {
        bail!("no --repo given and couldn't read the git `origin` remote — pass --repo owner/repo");
    }
    let url = String::from_utf8_lossy(&out.stdout).trim().to_string();
    parse_remote_url(&url)
}

fn parse_owner_repo(s: &str) -> Result<(String, String)> {
    let mut parts = s.split('/');
    match (
        parts.next().filter(|p| !p.is_empty()),
        parts.next().filter(|p| !p.is_empty()),
        parts.next(),
    ) {
        (Some(owner), Some(repo), None) => {
            Ok((owner.to_string(), repo.trim_end_matches(".git").to_string()))
        }
        _ => bail!("'{s}' is not a valid owner/repo"),
    }
}

/// Extract owner/repo from a GitHub remote URL (ssh or https form).
fn parse_remote_url(url: &str) -> Result<(String, String)> {
    // git@github.com:owner/repo.git  |  https://github.com/owner/repo(.git)
    let tail = url
        .rsplit_once("github.com")
        .map(|(_, tail)| tail.trim_start_matches([':', '/']))
        .ok_or_else(|| {
            anyhow!("origin remote '{url}' is not a github.com URL — pass --repo owner/repo")
        })?;
    parse_owner_repo(tail)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_owner_repo() {
        assert_eq!(
            parse_owner_repo("giovani-freitag/vestige").unwrap(),
            ("giovani-freitag".into(), "vestige".into())
        );
        assert!(parse_owner_repo("nope").is_err());
        assert!(parse_owner_repo("a/b/c").is_err());
    }

    #[test]
    fn parses_ssh_and_https_remotes() {
        let expected = ("giovani-freitag".to_string(), "vestige".to_string());
        assert_eq!(
            parse_remote_url("git@github.com:giovani-freitag/vestige.git").unwrap(),
            expected
        );
        assert_eq!(
            parse_remote_url("https://github.com/giovani-freitag/vestige.git").unwrap(),
            expected
        );
        assert_eq!(
            parse_remote_url("https://github.com/giovani-freitag/vestige").unwrap(),
            expected
        );
        assert!(parse_remote_url("https://gitlab.com/x/y").is_err());
    }
}
