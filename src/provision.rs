use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{anyhow, bail, Context, Result};
use reqwest::blocking::Client;
use serde::Deserialize;

use crate::http::{download_capped, download_client, DEFAULT_MAX_DOWNLOAD};
use crate::loader::Loader;

/// Installs a headless dedicated server for a loader into a directory, so `lode install --server`
/// can go from an empty folder to a runnable server. Forge/NeoForge use their `--installServer`
/// installer; other loaders are not provisioned automatically yet.
pub struct LoaderProvisioner {
    client: Client,
    java: String,
}

impl LoaderProvisioner {
    pub fn new(java: Option<String>) -> Result<LoaderProvisioner> {
        Ok(LoaderProvisioner {
            client: download_client()?,
            java: java.unwrap_or_else(resolve_java),
        })
    }

    /// Ensure a server for `loader` is installed in `dir`. Returns `true` if it provisioned now,
    /// `false` if a server was already present. Idempotent.
    pub fn ensure_server(
        &self,
        loader: Loader,
        mc: &str,
        version: &str,
        dir: &Path,
    ) -> Result<bool> {
        if self.already_provisioned(loader, dir) {
            return Ok(false);
        }
        self.check_java()?;
        match loader {
            Loader::Forge => self.run_installer(dir, &forge_installer_url(mc, version))?,
            Loader::Neoforge => self.run_installer(dir, &neoforge_installer_url(version))?,
            Loader::Fabric => self.install_fabric_server(mc, version, dir)?,
            Loader::Quilt => self.install_quilt_server(mc, version, dir)?,
        }
        Ok(true)
    }

    /// Whether a server for `loader` is already installed in `dir`, by the marker each loader
    /// leaves behind (Forge/NeoForge args dirs, or the Fabric/Quilt launch jar).
    fn already_provisioned(&self, loader: Loader, dir: &Path) -> bool {
        match loader {
            Loader::Forge => dir.join("libraries/net/minecraftforge/forge").is_dir(),
            Loader::Neoforge => {
                // 1.20.1 installs under the legacy `forge` artifact path, 1.20.2+ under `neoforge`.
                dir.join("libraries/net/neoforged/neoforge").is_dir()
                    || dir.join("libraries/net/neoforged/forge").is_dir()
            }
            Loader::Fabric => dir.join("fabric-server-launch.jar").is_file(),
            Loader::Quilt => dir.join("quilt-server-launch.jar").is_file(),
        }
    }

    /// Fabric ships a self-bootstrapping server launcher jar via its meta service; downloading it
    /// is the whole install (it fetches the Minecraft server + libraries on first run).
    fn install_fabric_server(&self, mc: &str, loader: &str, dir: &Path) -> Result<()> {
        let installer = self.latest_installer("https://meta.fabricmc.net/v2/versions/installer")?;
        let url = fabric_server_url(mc, loader, &installer);
        let bytes = download_capped(self.client.get(&url), DEFAULT_MAX_DOWNLOAD, "Fabric server")?;
        std::fs::write(dir.join("fabric-server-launch.jar"), &bytes)
            .context("writing fabric-server-launch.jar")?;
        Ok(())
    }

    /// Quilt installs its server with the quilt-installer jar (`install server`), mirroring Forge.
    fn install_quilt_server(&self, mc: &str, loader: &str, dir: &Path) -> Result<()> {
        let installer = self.latest_installer("https://meta.quiltmc.org/v3/versions/installer")?;
        let url = quilt_installer_url(&installer);
        let jar = dir.join("quilt-installer.jar");
        let bytes = download_capped(
            self.client.get(&url),
            DEFAULT_MAX_DOWNLOAD,
            "Quilt installer",
        )?;
        std::fs::write(&jar, &bytes).context("writing quilt installer")?;

        let status = Command::new(&self.java)
            .arg("-jar")
            .arg(&jar)
            .args(["install", "server", mc, loader, "--download-server"])
            .arg(format!("--install-dir={}", dir.display()))
            .current_dir(dir)
            .status()
            .context("running the Quilt installer")?;
        let _ = std::fs::remove_file(&jar);
        if !status.success() {
            bail!("Quilt installer exited with {status}");
        }
        Ok(())
    }

    /// Newest installer version from a Fabric/Quilt meta endpoint (prefers a `stable` build).
    fn latest_installer(&self, url: &str) -> Result<String> {
        #[derive(Deserialize)]
        struct InstallerVersion {
            version: String,
            #[serde(default)]
            stable: bool,
        }
        let resp = self
            .client
            .get(url)
            .send()
            .with_context(|| format!("fetching installer versions from {url}"))?
            .error_for_status()
            .context("installer metadata error")?;
        let versions: Vec<InstallerVersion> = crate::http::json_capped(resp, "installer versions")?;
        versions
            .iter()
            .find(|v| v.stable)
            .or_else(|| versions.first())
            .map(|v| v.version.clone())
            .ok_or_else(|| anyhow!("no installer version available"))
    }

    fn check_java(&self) -> Result<()> {
        let ok = Command::new(&self.java)
            .arg("-version")
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false);
        if !ok {
            bail!(
                "Java not found (tried '{}') — install a JDK 17+ or pass --java <path>",
                self.java
            );
        }
        Ok(())
    }

    fn run_installer(&self, dir: &Path, url: &str) -> Result<()> {
        let jar = dir.join("loader-installer.jar");
        let bytes = download_capped(
            self.client.get(url),
            DEFAULT_MAX_DOWNLOAD,
            "loader installer",
        )?;
        std::fs::write(&jar, &bytes).context("writing loader installer jar")?;

        let status = Command::new(&self.java)
            .arg("-jar")
            .arg(&jar)
            .arg("--installServer")
            .current_dir(dir)
            .status()
            .context("running the loader installer")?;
        let _ = std::fs::remove_file(&jar);

        if !status.success() {
            bail!("loader installer exited with {status}");
        }
        Ok(())
    }
}

/// The java executable's file name on this platform.
const JAVA_EXE: &str = if cfg!(windows) { "java.exe" } else { "java" };

/// Resolve a java to run installers with when `--java` isn't given, preferring an *absolute* path.
/// A bare `"java"` is resolved by the OS against its search path — which on some platforms includes
/// the current directory — so when an installer is spawned with `current_dir` set to an unpacked
/// pack, a bare name could execute an attacker-planted `java`. Preferring `JAVA_HOME/bin/java`, then
/// an absolute match on `PATH` (relative `PATH` entries are skipped so the CWD is never a source),
/// keeps the spawn anchored to a trusted location. Falls back to `"java"` only when nothing
/// resolves — in which case `check_java` fails with a clear message before any installer runs.
fn resolve_java() -> String {
    let java_home = std::env::var_os("JAVA_HOME");
    let path_dirs: Vec<PathBuf> = std::env::var_os("PATH")
        .map(|path| std::env::split_paths(&path).collect())
        .unwrap_or_default();
    resolve_java_core(java_home.as_deref(), path_dirs, |p| p.is_file())
}

/// The pure preference logic behind [`resolve_java`], with the environment and filesystem injected
/// so it can be exercised without touching real `JAVA_HOME`/`PATH` or the disk. `exists` answers
/// whether a candidate java binary is present. Relative `PATH` entries are skipped so a
/// CWD-relative java is never resolved (the security property this guards).
fn resolve_java_core(
    java_home: Option<&OsStr>,
    path_dirs: Vec<PathBuf>,
    exists: impl Fn(&Path) -> bool,
) -> String {
    if let Some(home) = java_home {
        let candidate = Path::new(home).join("bin").join(JAVA_EXE);
        if exists(&candidate) {
            return candidate.to_string_lossy().into_owned();
        }
    }
    for dir in path_dirs {
        if !dir.is_absolute() {
            continue;
        }
        let candidate = dir.join(JAVA_EXE);
        if exists(&candidate) {
            return candidate.to_string_lossy().into_owned();
        }
    }
    "java".to_string()
}

fn forge_installer_url(mc: &str, version: &str) -> String {
    format!(
        "https://maven.minecraftforge.net/net/minecraftforge/forge/{mc}-{version}/forge-{mc}-{version}-installer.jar"
    )
}

fn neoforge_installer_url(version: &str) -> String {
    // 1.20.1 predates NeoForge's own versioning; its installer lives under the legacy `forge`
    // artifact as `forge-1.20.1-47.x-installer.jar`, whereas 1.20.2+ use the `neoforge` artifact.
    if version.starts_with("1.20.1-") {
        format!("https://maven.neoforged.net/releases/net/neoforged/forge/{version}/forge-{version}-installer.jar")
    } else {
        format!("https://maven.neoforged.net/releases/net/neoforged/neoforge/{version}/neoforge-{version}-installer.jar")
    }
}

fn fabric_server_url(mc: &str, loader: &str, installer: &str) -> String {
    format!("https://meta.fabricmc.net/v2/versions/loader/{mc}/{loader}/{installer}/server/jar")
}

fn quilt_installer_url(installer: &str) -> String {
    format!(
        "https://maven.quiltmc.org/repository/release/org/quiltmc/quilt-installer/{installer}/quilt-installer-{installer}.jar"
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use tempfile::TempDir;

    // ---- installer URL construction (security-review flagged) ----

    #[test]
    fn forge_installer_url_embeds_mc_and_version_in_both_places() {
        let url = forge_installer_url("1.20.1", "47.3.0");
        assert_eq!(
            url,
            "https://maven.minecraftforge.net/net/minecraftforge/forge/1.20.1-47.3.0/forge-1.20.1-47.3.0-installer.jar",
            "forge URL must interpolate mc-version into both the directory and the jar file name"
        );
    }

    #[test]
    fn neoforge_installer_url_embeds_version_in_both_places() {
        let url = neoforge_installer_url("20.4.190");
        assert_eq!(
            url,
            "https://maven.neoforged.net/releases/net/neoforged/neoforge/20.4.190/neoforge-20.4.190-installer.jar",
            "neoforge URL must interpolate the version into both the directory and the jar file name"
        );
    }

    #[test]
    fn neoforge_installer_url_routes_1_20_1_to_the_legacy_forge_artifact() {
        let url = neoforge_installer_url("1.20.1-47.1.106");
        assert_eq!(
            url,
            "https://maven.neoforged.net/releases/net/neoforged/forge/1.20.1-47.1.106/forge-1.20.1-47.1.106-installer.jar",
            "a hyphenated 1.20.1 version must resolve to the legacy forge artifact and forge- jar prefix"
        );
    }

    #[test]
    fn fabric_server_url_orders_mc_loader_and_installer() {
        let url = fabric_server_url("1.20.1", "0.16.9", "1.0.1");
        assert_eq!(
            url, "https://meta.fabricmc.net/v2/versions/loader/1.20.1/0.16.9/1.0.1/server/jar",
            "fabric server URL must place mc, loader, and installer versions in that order"
        );
    }

    #[test]
    fn quilt_installer_url_repeats_the_installer_version() {
        let url = quilt_installer_url("0.9.2");
        assert_eq!(
            url,
            "https://maven.quiltmc.org/repository/release/org/quiltmc/quilt-installer/0.9.2/quilt-installer-0.9.2.jar",
            "quilt installer URL must use the installer version in both the path segment and the jar name"
        );
    }

    // ---- resolve_java preference order (security-critical: never resolve a CWD-relative java) ----

    #[test]
    fn resolve_java_core_prefers_java_home_over_a_path_match() {
        let home = TempDir::new().unwrap();
        let path_dir = TempDir::new().unwrap();
        let expected = home.path().join("bin").join(JAVA_EXE);
        let path_java = path_dir.path().join(JAVA_EXE);
        let exp = expected.clone();
        let pj = path_java.clone();

        let got = resolve_java_core(
            Some(home.path().as_os_str()),
            vec![path_dir.path().to_path_buf()],
            move |p| p == exp.as_path() || p == pj.as_path(),
        );

        assert_eq!(
            got,
            expected.to_string_lossy().into_owned(),
            "JAVA_HOME/bin/java must win even when PATH also has a java"
        );
    }

    #[test]
    fn resolve_java_core_falls_through_when_java_home_has_no_binary() {
        let home = TempDir::new().unwrap();
        let path_dir = TempDir::new().unwrap();
        let expected = path_dir.path().join(JAVA_EXE);
        let exp = expected.clone();

        // Only the PATH candidate "exists"; the JAVA_HOME candidate does not.
        let got = resolve_java_core(
            Some(home.path().as_os_str()),
            vec![path_dir.path().to_path_buf()],
            move |p| p == exp.as_path(),
        );

        assert_eq!(
            got,
            expected.to_string_lossy().into_owned(),
            "a JAVA_HOME without bin/java must fall through to an absolute PATH match"
        );
    }

    #[test]
    fn resolve_java_core_uses_an_absolute_path_entry() {
        let path_dir = TempDir::new().unwrap();
        let expected = path_dir.path().join(JAVA_EXE);
        let exp = expected.clone();

        let got = resolve_java_core(None, vec![path_dir.path().to_path_buf()], move |p| {
            p == exp.as_path()
        });

        assert_eq!(
            got,
            expected.to_string_lossy().into_owned(),
            "an absolute PATH entry containing java must resolve"
        );
    }

    #[test]
    fn resolve_java_core_skips_relative_entries_in_favor_of_absolute() {
        let relative = PathBuf::from("planted-dir");
        let abs_dir = TempDir::new().unwrap();
        let abs_java = abs_dir.path().join(JAVA_EXE);
        let rel_java = relative.join(JAVA_EXE);
        let aj = abs_java.clone();
        let rj = rel_java.clone();

        // Both candidates "exist"; the relative one comes first but must be skipped.
        let got = resolve_java_core(
            None,
            vec![relative.clone(), abs_dir.path().to_path_buf()],
            move |p| p == aj.as_path() || p == rj.as_path(),
        );

        assert_eq!(
            got,
            abs_java.to_string_lossy().into_owned(),
            "a relative PATH entry must be skipped so the absolute one is chosen"
        );
    }

    #[test]
    fn resolve_java_core_never_resolves_a_cwd_relative_java() {
        // "." is the CWD; even if a java sits there it must never be returned — the whole point of
        // the absolute-only rule. With no other entry, resolution falls back to the bare name.
        let got = resolve_java_core(None, vec![PathBuf::from(".")], |_| true);
        assert_eq!(
            got, "java",
            "a '.' PATH entry must never resolve to a CWD java; it must fall back to bare java"
        );
    }

    #[test]
    fn resolve_java_core_falls_back_to_bare_java_when_nothing_resolves() {
        let got = resolve_java_core(None, Vec::<PathBuf>::new(), |_| false);
        assert_eq!(
            got, "java",
            "with no JAVA_HOME and no usable PATH entry, resolution falls back to bare java"
        );
    }

    // ---- provisioned-marker detection (post-install verification path logic) ----

    fn provisioner() -> LoaderProvisioner {
        LoaderProvisioner::new(Some("java".to_string())).unwrap()
    }

    #[test]
    fn already_provisioned_is_false_for_an_empty_dir() {
        let dir = TempDir::new().unwrap();
        let p = provisioner();
        for loader in [
            Loader::Forge,
            Loader::Neoforge,
            Loader::Fabric,
            Loader::Quilt,
        ] {
            assert!(
                !p.already_provisioned(loader, dir.path()),
                "an empty dir must not look provisioned for {loader:?}"
            );
        }
    }

    #[test]
    fn already_provisioned_detects_each_loaders_marker() {
        let dir = TempDir::new().unwrap();
        let root = dir.path();
        std::fs::create_dir_all(root.join("libraries/net/minecraftforge/forge")).unwrap();
        std::fs::create_dir_all(root.join("libraries/net/neoforged/neoforge")).unwrap();
        std::fs::write(root.join("fabric-server-launch.jar"), b"x").unwrap();
        std::fs::write(root.join("quilt-server-launch.jar"), b"x").unwrap();

        let p = provisioner();
        assert!(
            p.already_provisioned(Loader::Forge, root),
            "the forge library dir must count as provisioned"
        );
        assert!(
            p.already_provisioned(Loader::Neoforge, root),
            "the neoforge library dir must count as provisioned"
        );
        assert!(
            p.already_provisioned(Loader::Fabric, root),
            "the fabric launch jar must count as provisioned"
        );
        assert!(
            p.already_provisioned(Loader::Quilt, root),
            "the quilt launch jar must count as provisioned"
        );
    }

    #[test]
    fn already_provisioned_requires_the_right_marker_kind() {
        let dir = TempDir::new().unwrap();
        let root = dir.path();
        // Forge's marker is a directory: a plain file at that path must not satisfy it.
        std::fs::create_dir_all(root.join("libraries/net/minecraftforge")).unwrap();
        std::fs::write(root.join("libraries/net/minecraftforge/forge"), b"x").unwrap();
        // Fabric's marker is a file: a directory at that path must not satisfy it.
        std::fs::create_dir_all(root.join("fabric-server-launch.jar")).unwrap();

        let p = provisioner();
        assert!(
            !p.already_provisioned(Loader::Forge, root),
            "a file at the forge library path must not satisfy the is_dir marker"
        );
        assert!(
            !p.already_provisioned(Loader::Fabric, root),
            "a directory at the fabric jar path must not satisfy the is_file marker"
        );
    }

    #[test]
    fn already_provisioned_does_not_cross_detect_between_loaders() {
        let dir = TempDir::new().unwrap();
        let root = dir.path();
        std::fs::create_dir_all(root.join("libraries/net/minecraftforge/forge")).unwrap();

        let p = provisioner();
        assert!(
            !p.already_provisioned(Loader::Fabric, root),
            "a forge marker must not make a fabric server look provisioned"
        );
        assert!(
            !p.already_provisioned(Loader::Neoforge, root),
            "the forge marker path must not be mistaken for the neoforge one"
        );
    }
}
