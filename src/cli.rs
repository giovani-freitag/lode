use clap::{Args, Parser, Subcommand, ValueEnum};

use crate::loader::Loader;
use crate::side::Side;

/// A package manager for Minecraft modpacks.
#[derive(Debug, Parser)]
#[command(name = "lode", version, about)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Scaffold a new pack (`lode.json`) in the current directory.
    Init(InitArgs),
    /// Convert an existing pack from another tool into this lode project.
    Import(ImportArgs),
    /// Add a mod: declare it, resolve dependencies, and download the jars.
    Add(AddArgs),
    /// Remove a mod, prune orphaned dependencies, and delete its jars.
    #[command(alias = "remove", alias = "rm")]
    Del(DelArgs),
    /// Pin a mod to its locked version so `update` won't bump it.
    Pin(PinArgs),
    /// Remove a mod's pin so `update` can bump it again.
    Unpin(PinArgs),
    /// Re-resolve the pack, picking the latest allowed versions.
    Update(UpdateArgs),
    /// List the mods in the pack.
    List(ListArgs),
    /// Re-resolve the lockfile from the manifest (creates it if missing).
    Refresh,
    /// Download everything in the lockfile into an instance, like `npm install`.
    #[command(alias = "i")]
    Install(InstallArgs),
    /// Fetch a published pack from a ref (host/owner/repo[@tag]) and set it up here.
    Get(GetArgs),
    /// Bundle the pack and publish it as a GitHub release (needs GITHUB_TOKEN).
    Publish(PublishArgs),
    /// Export the pack to another tool's format (currently packwiz), for launcher interop.
    Export(ExportArgs),
    /// Bundle the pack definition into a distributable `.tar.gz` + `.sha256` checksum.
    Bundle(BundleArgs),
    /// Verify installed jars against the lockfile (integrity check).
    Verify(VerifyArgs),
    /// Explain why a mod is in the pack (declared, or required by which mods).
    Why(WhyArgs),
    /// Get or set stored configuration (e.g. the CurseForge API key).
    Config(ConfigArgs),
}

#[derive(Debug, Args)]
pub struct ConfigArgs {
    #[command(subcommand)]
    pub action: ConfigAction,
}

#[derive(Debug, Subcommand)]
pub enum ConfigAction {
    /// Set a value, e.g. `lode config set curseforge.key <KEY>`.
    Set { key: String, value: String },
    /// Print a value, e.g. `lode config get curseforge.key`.
    Get { key: String },
}

#[derive(Debug, Args)]
pub struct InitArgs {
    #[arg(long)]
    pub name: Option<String>,
    #[arg(long)]
    pub author: Option<String>,
    #[arg(long, default_value = "0.1.0")]
    pub version: String,
    #[arg(long)]
    pub minecraft: Option<String>,
    /// The mod loader.
    #[arg(long)]
    pub loader: Option<Loader>,
    #[arg(long = "loader-version")]
    pub loader_version: Option<String>,
    /// Accept defaults without prompting.
    #[arg(short = 'y', long)]
    pub yes: bool,
}

#[derive(Debug, Args)]
pub struct ImportArgs {
    /// The pack format to convert from (currently `packwiz`).
    pub source: ImportSource,
    /// The source pack directory (or its pack.toml); defaults to the current directory.
    pub path: Option<std::path::PathBuf>,
    /// Write the lode project into this directory instead of the current one.
    #[arg(long)]
    pub out: Option<std::path::PathBuf>,
    /// Overwrite an existing lode.json.
    #[arg(long)]
    pub force: bool,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum ImportSource {
    /// A packwiz pack (pack.toml + index.toml + mods/*.pw.toml).
    Packwiz,
}

#[derive(Debug, Args)]
pub struct ExportArgs {
    /// The format to export to (currently `packwiz`).
    pub target: ExportTarget,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum ExportTarget {
    /// A packwiz distribution tree (pack.toml + index.toml + mods/*.pw.toml) in `pack/`.
    Packwiz,
}

#[derive(Debug, Args)]
pub struct AddArgs {
    /// A Modrinth slug, project id, URL, or a search term.
    pub query: String,
    /// Pin an exact version number instead of the latest.
    #[arg(long)]
    pub version: Option<String>,
    /// Override the side the mod installs on.
    #[arg(long)]
    pub side: Option<Side>,
    /// Search Modrinth and pick from the matches, instead of requiring an exact slug/id/URL.
    #[arg(long)]
    pub search: bool,
    /// Resolve from CurseForge instead of Modrinth (needs CF_API_KEY).
    #[arg(long, alias = "cf")]
    pub curseforge: bool,
    /// Only update the manifest + lockfile; don't download the jars.
    #[arg(long)]
    pub lock_only: bool,
}

#[derive(Debug, Args)]
pub struct DelArgs {
    /// Slug of the mod to remove.
    pub name: String,
}

#[derive(Debug, Args)]
pub struct PinArgs {
    /// Slug of the mod to pin or unpin.
    pub name: String,
}

#[derive(Debug, Args)]
pub struct UpdateArgs {
    /// Slug to update; omit (or use --all) to update everything.
    pub name: Option<String>,
    #[arg(long)]
    pub all: bool,
}

#[derive(Debug, Args)]
pub struct ListArgs {
    /// Emit the resolved lockfile as JSON.
    #[arg(long)]
    pub json: bool,
}

#[derive(Debug, Args)]
pub struct WhyArgs {
    /// Slug of the mod to explain.
    pub name: String,
}

#[derive(Debug, Args)]
pub struct BundleArgs {
    /// Directory to write the artifact into (defaults to the current directory).
    #[arg(long)]
    pub out: Option<std::path::PathBuf>,
}

#[derive(Debug, Args)]
pub struct GetArgs {
    /// A published pack ref: host/owner/repo[@tag], e.g. github.com/owner/repo (a GitHub release).
    pub reference: Option<String>,
    /// Fetch a `.tar.gz` from a direct URL instead of a ref.
    #[arg(long)]
    pub from_url: Option<String>,
    /// Use a local `.tar.gz` instead of fetching.
    #[arg(long)]
    pub from_file: Option<std::path::PathBuf>,
    /// Directory to unpack into (defaults to a folder named after the repo/archive).
    #[arg(long)]
    pub into: Option<std::path::PathBuf>,
    /// Require a valid Sigstore attestation proving the archive was built by the ref's repo.
    /// A signature present on the source is always verified; this makes one mandatory.
    #[arg(long)]
    pub verify: bool,
    /// Accept a remote pack whose provenance can't be verified — a missing/invalid checksum or an
    /// absent/failed signature. Off by default (lode is fail-closed for remote sources). A genuine
    /// checksum *mismatch* is never bypassed, and `--verify` still forces a valid signature.
    #[arg(long)]
    pub insecure: bool,
    /// Unpack and verify only; don't download the jars.
    #[arg(long)]
    pub no_install: bool,
}

#[derive(Debug, Args)]
pub struct PublishArgs {
    /// The GitHub repo to publish to (owner/repo); inferred from the `origin` remote if omitted.
    #[arg(long)]
    pub repo: Option<String>,
    /// The release tag to create or attach to (e.g. v1.0.0).
    #[arg(long)]
    pub tag: String,
    /// Also sign the archive (keyless, via cosign) and upload the .sigstore bundle.
    #[arg(long)]
    pub sign: bool,
    /// Mark the release as a prerelease.
    #[arg(long)]
    pub prerelease: bool,
}

#[derive(Debug, Args)]
pub struct VerifyArgs {
    /// Instance directory whose `mods/` to check (defaults to the pack root).
    #[arg(long)]
    pub into: Option<std::path::PathBuf>,
}

#[derive(Debug, Args)]
pub struct InstallArgs {
    /// Target instance directory (defaults to the pack root).
    #[arg(long)]
    pub into: Option<std::path::PathBuf>,
    /// Install the server side instead of the client side.
    #[arg(long)]
    pub server: bool,
    /// Skip provisioning the loader server (assume Forge/NeoForge is already installed).
    #[arg(long)]
    pub skip_loader: bool,
    /// Java executable to run the loader installer with (defaults to `java`).
    #[arg(long)]
    pub java: Option<String>,
}
