use std::collections::{HashMap, HashSet};

use anyhow::{Context, Result};
use reqwest::blocking::Client;
use serde::Deserialize;

use crate::loader::Loader;

/// A selectable loader version, with an optional note ("latest"/"recommended"/"stable").
pub struct LoaderVersion {
    pub version: String,
    pub note: Option<String>,
}

/// Fetches the live version catalogs `lode init` offers: Minecraft releases (from Modrinth) and
/// per-loader versions (from each loader's own metadata service).
pub struct Versions {
    client: Client,
}

impl Versions {
    pub fn new() -> Result<Versions> {
        Ok(Versions {
            client: crate::http::client()?,
        })
    }

    /// Minecraft release versions, newest first. Modrinth's tag endpoint is a single source that
    /// works regardless of the chosen loader.
    pub fn minecraft(&self) -> Result<Vec<String>> {
        let resp = self
            .client
            .get("https://api.modrinth.com/v2/tag/game_version")
            .send()
            .context("fetching Minecraft versions")?
            .error_for_status()
            .context("Modrinth game_version tag error")?;
        let tags: Vec<Tag> = crate::http::json_capped(resp, "Minecraft versions")?;
        Ok(select_minecraft_versions(tags))
    }

    /// Minecraft versions the given loader actually ships builds for, newest first. This is the set
    /// `lode init` offers so a loader+Minecraft pair can never dead-end: Fabric and Quilt are
    /// constrained by each service's supported game-version list; Forge and NeoForge by the versions
    /// their own release metadata publishes (NeoForge's 1.20.1 line, which lives under the legacy
    /// `forge` artifact, is folded in).
    pub fn minecraft_for(&self, loader: Loader) -> Result<Vec<String>> {
        let releases = self.minecraft()?;
        let supported = self.supported_minecraft(loader)?;
        Ok(releases
            .into_iter()
            .filter(|mc| supported.contains(mc))
            .collect())
    }

    /// The (unordered) set of Minecraft versions a loader publishes builds for.
    fn supported_minecraft(&self, loader: Loader) -> Result<HashSet<String>> {
        match loader {
            Loader::Fabric => self.game_versions("https://meta.fabricmc.net/v2/versions/game"),
            Loader::Quilt => self.game_versions("https://meta.quiltmc.org/v3/versions/game"),
            Loader::Forge => Ok(forge_minecraft_versions(&self.forge_promos()?)),
            Loader::Neoforge => self.neoforge_minecraft_versions(),
        }
    }

    /// Supported Minecraft versions for Fabric/Quilt, from each service's `/versions/game` feed.
    fn game_versions(&self, url: &str) -> Result<HashSet<String>> {
        let resp = self
            .client
            .get(url)
            .send()
            .context("fetching loader game versions")?
            .error_for_status()
            .context("loader game metadata error")?;
        let games: Vec<GameVersion> = crate::http::json_capped(resp, "loader game versions")?;
        Ok(games.into_iter().map(|g| g.version).collect())
    }

    /// Minecraft versions NeoForge supports: its modern line (1.20.2+, one `X.Y.*` series per MC
    /// under the `neoforge` artifact) plus 1.20.1, whose builds predate that scheme and live under
    /// the legacy `forge` artifact.
    fn neoforge_minecraft_versions(&self) -> Result<HashSet<String>> {
        let mut set: HashSet<String> = self
            .neoforge_maven()?
            .iter()
            .filter_map(|v| neoforge_version_to_mc(v))
            .collect();
        if self
            .neoforge_forge_maven()?
            .iter()
            .any(|v| v.starts_with("1.20.1-"))
        {
            set.insert("1.20.1".to_string());
        }
        Ok(set)
    }

    /// Loader versions applicable to `mc`, best (newest/recommended) first.
    pub fn loader(&self, loader: Loader, mc: &str) -> Result<Vec<LoaderVersion>> {
        match loader {
            Loader::Fabric => self.fabric_like("https://meta.fabricmc.net/v2/versions/loader"),
            Loader::Quilt => self.fabric_like("https://meta.quiltmc.org/v3/versions/loader"),
            Loader::Forge => self.forge(mc),
            Loader::Neoforge => self.neoforge(mc),
        }
    }

    /// Fabric and Quilt share a metadata shape: a newest-first list of loader builds, each with a
    /// `stable` flag. Loader builds are Minecraft-independent for both.
    fn fabric_like(&self, url: &str) -> Result<Vec<LoaderVersion>> {
        let resp = self
            .client
            .get(url)
            .send()
            .context("fetching loader versions")?
            .error_for_status()
            .context("loader metadata error")?;
        let builds: Vec<Build> = crate::http::json_capped(resp, "loader versions")?;
        Ok(builds_to_versions(builds))
    }

    /// Forge publishes only the latest + recommended build per Minecraft version in its promotions
    /// feed; that pair is what the picker offers.
    fn forge(&self, mc: &str) -> Result<Vec<LoaderVersion>> {
        Ok(select_forge_versions(&self.forge_promos()?, mc))
    }

    /// Forge's promotions feed: `<mc>-latest` / `<mc>-recommended` mapped to a build.
    fn forge_promos(&self) -> Result<HashMap<String, String>> {
        #[derive(Deserialize)]
        struct Promotions {
            promos: HashMap<String, String>,
        }
        let resp = self
            .client
            .get("https://files.minecraftforge.net/net/minecraftforge/forge/promotions_slim.json")
            .send()
            .context("fetching Forge versions")?
            .error_for_status()
            .context("Forge promotions error")?;
        let promos: Promotions = crate::http::json_capped(resp, "Forge promotions")?;
        Ok(promos.promos)
    }

    /// NeoForge versions for `mc`. The modern line (1.20.2+) encodes the Minecraft version in its
    /// own `X.Y.*` numbering under the `neoforge` artifact; 1.20.1 predates that scheme and lives
    /// under the legacy `forge` artifact as `1.20.1-47.*`.
    fn neoforge(&self, mc: &str) -> Result<Vec<LoaderVersion>> {
        if mc == "1.20.1" {
            Ok(select_neoforge_legacy_versions(
                self.neoforge_forge_maven()?,
            ))
        } else {
            Ok(select_neoforge_versions(self.neoforge_maven()?, mc))
        }
    }

    /// The modern NeoForge release list (`net/neoforged/neoforge`), maven ascending.
    fn neoforge_maven(&self) -> Result<Vec<String>> {
        self.maven_versions(
            "https://maven.neoforged.net/api/maven/versions/releases/net/neoforged/neoforge",
            "NeoForge versions",
        )
    }

    /// The legacy NeoForge release list (`net/neoforged/forge`) that carries the 1.20.1 line.
    fn neoforge_forge_maven(&self) -> Result<Vec<String>> {
        self.maven_versions(
            "https://maven.neoforged.net/api/maven/versions/releases/net/neoforged/forge",
            "NeoForge 1.20.1 versions",
        )
    }

    /// A NeoForge maven `versions` listing (both artifacts share this shape).
    fn maven_versions(&self, url: &str, what: &str) -> Result<Vec<String>> {
        #[derive(Deserialize)]
        struct Releases {
            versions: Vec<String>,
        }
        let resp = self
            .client
            .get(url)
            .send()
            .with_context(|| format!("fetching {what}"))?
            .error_for_status()
            .context("NeoForge maven error")?;
        let releases: Releases = crate::http::json_capped(resp, what)?;
        Ok(releases.versions)
    }
}

/// A Modrinth game-version tag. Only `release` tags are offered to the picker.
#[derive(Deserialize)]
struct Tag {
    version: String,
    version_type: String,
    date: String,
}

/// A Fabric/Quilt loader build. The two services share this shape.
#[derive(Deserialize)]
struct Build {
    version: String,
    #[serde(default)]
    stable: bool,
}

/// A Fabric/Quilt supported Minecraft game version (from each service's `/versions/game` feed).
#[derive(Deserialize)]
struct GameVersion {
    version: String,
}

/// Keep only release tags, newest publish-date first.
fn select_minecraft_versions(mut tags: Vec<Tag>) -> Vec<String> {
    tags.retain(|t| t.version_type == "release");
    tags.sort_by(|a, b| b.date.cmp(&a.date));
    tags.into_iter().map(|t| t.version).collect()
}

/// Map loader builds to selectable versions, tagging stable ones. Upstream order (newest-first)
/// is preserved.
fn builds_to_versions(builds: Vec<Build>) -> Vec<LoaderVersion> {
    builds
        .into_iter()
        .map(|b| LoaderVersion {
            note: b.stable.then(|| "stable".to_string()),
            version: b.version,
        })
        .collect()
}

/// From Forge's promotions map, offer this Minecraft version's latest then recommended build,
/// deduping when both point at the same build.
fn select_forge_versions(promos: &HashMap<String, String>, mc: &str) -> Vec<LoaderVersion> {
    let mut out = Vec::new();
    if let Some(v) = promos.get(&format!("{mc}-latest")) {
        out.push(LoaderVersion {
            version: v.clone(),
            note: Some("latest".to_string()),
        });
    }
    if let Some(v) = promos.get(&format!("{mc}-recommended")) {
        if !out.iter().any(|x| &x.version == v) {
            out.push(LoaderVersion {
                version: v.clone(),
                note: Some("recommended".to_string()),
            });
        }
    }
    out
}

/// Keep NeoForge builds whose version matches the Minecraft-derived prefix, newest first.
fn select_neoforge_versions(versions: Vec<String>, mc: &str) -> Vec<LoaderVersion> {
    let prefix = neoforge_prefix(mc);
    let mut matching: Vec<String> = versions
        .into_iter()
        .filter(|v| v.starts_with(&prefix))
        .collect();
    // Maven returns ascending; the picker wants newest first.
    matching.reverse();
    matching
        .into_iter()
        .map(|version| LoaderVersion {
            version,
            note: None,
        })
        .collect()
}

/// Map a Minecraft version `1.X.Y` to the NeoForge version prefix `X.Y.` (Y defaults to 0).
fn neoforge_prefix(mc: &str) -> String {
    let parts: Vec<&str> = mc.split('.').collect();
    let major = parts.get(1).copied().unwrap_or("0");
    let minor = parts.get(2).copied().unwrap_or("0");
    format!("{major}.{minor}.")
}

/// The Minecraft versions present in Forge's promotions feed (the `<mc>` part of each key).
fn forge_minecraft_versions(promos: &HashMap<String, String>) -> HashSet<String> {
    promos
        .keys()
        .filter_map(|k| {
            k.strip_suffix("-latest")
                .or_else(|| k.strip_suffix("-recommended"))
        })
        .map(|mc| mc.to_string())
        .collect()
}

/// Map a modern NeoForge version `X.Y.Z` back to its Minecraft version: `1.X.Y`, or `1.X` when the
/// minor is 0 (e.g. `21.0.169` -> `1.21`, `20.4.190` -> `1.20.4`). Inputs that aren't `X.Y.Z` (a
/// missing patch, non-numeric parts) yield `None`, so partial or malformed tags are ignored.
fn neoforge_version_to_mc(version: &str) -> Option<String> {
    // Exactly three dot-separated components: `X.Y.Z` (the patch may carry a `-beta`/`-rc` suffix,
    // which keeps its dot count at three). This rejects both too-short strings (`47.1`) and the
    // legacy hyphenated 1.20.1 form (`1.20.1-47.1.106`, five components) that belongs to the other
    // artifact.
    let parts: Vec<&str> = version.split('.').collect();
    if parts.len() != 3 {
        return None;
    }
    let major: u32 = parts[0].parse().ok()?;
    let minor: u32 = parts[1].parse().ok()?;
    Some(if minor == 0 {
        format!("1.{major}")
    } else {
        format!("1.{major}.{minor}")
    })
}

/// Keep the 1.20.1 builds from the legacy `forge` artifact, newest first. Maven returns ascending,
/// so the kept subset is reversed for the picker.
fn select_neoforge_legacy_versions(versions: Vec<String>) -> Vec<LoaderVersion> {
    let mut matching: Vec<String> = versions
        .into_iter()
        .filter(|v| v.starts_with("1.20.1-"))
        .collect();
    matching.reverse();
    matching
        .into_iter()
        .map(|version| LoaderVersion {
            version,
            note: None,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- builders --------------------------------------------------------------------------

    fn tag(version: &str, version_type: &str, date: &str) -> Tag {
        Tag {
            version: version.to_string(),
            version_type: version_type.to_string(),
            date: date.to_string(),
        }
    }

    fn build(version: &str, stable: bool) -> Build {
        Build {
            version: version.to_string(),
            stable,
        }
    }

    fn promos(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    // --- neoforge_prefix -------------------------------------------------------------------

    #[test]
    fn neoforge_prefix_maps_mc_major_minor_to_the_maven_prefix() {
        assert_eq!(
            neoforge_prefix("1.20.1"),
            "20.1.",
            "MC 1.X.Y must map to NeoForge prefix X.Y."
        );
    }

    #[test]
    fn neoforge_prefix_defaults_missing_patch_to_zero() {
        // MC releases like `1.21` carry no third component; NeoForge still tags them `21.0.*`.
        assert_eq!(
            neoforge_prefix("1.21"),
            "21.0.",
            "an absent patch component must default to 0, not be dropped"
        );
    }

    // --- select_minecraft_versions ---------------------------------------------------------

    #[test]
    fn select_minecraft_keeps_only_releases() {
        let tags = vec![
            tag("1.20.1", "release", "2023-06-12T00:00:00Z"),
            tag("23w31a", "snapshot", "2023-08-02T00:00:00Z"),
            tag("1.20", "release", "2023-06-07T00:00:00Z"),
            tag("1.20-pre1", "beta", "2023-05-30T00:00:00Z"),
        ];

        let out = select_minecraft_versions(tags);

        assert_eq!(
            out,
            vec!["1.20.1", "1.20"],
            "snapshots and betas must be filtered out, leaving only releases"
        );
    }

    #[test]
    fn select_minecraft_orders_newest_release_first_regardless_of_input_order() {
        // Modrinth does not guarantee ordering, so selection must sort by publish date descending.
        let tags = vec![
            tag("1.19.2", "release", "2022-08-05T00:00:00Z"),
            tag("1.20.1", "release", "2023-06-12T00:00:00Z"),
            tag("1.20", "release", "2023-06-07T00:00:00Z"),
        ];

        let out = select_minecraft_versions(tags);

        assert_eq!(
            out,
            vec!["1.20.1", "1.20", "1.19.2"],
            "versions must come out newest publish-date first"
        );
    }

    // --- builds_to_versions ----------------------------------------------------------------

    #[test]
    fn builds_to_versions_notes_only_stable_builds() {
        let builds = vec![build("0.16.10", true), build("0.16.11-beta", false)];

        let out = builds_to_versions(builds);

        assert_eq!(out[0].version, "0.16.10");
        assert_eq!(
            out[0].note.as_deref(),
            Some("stable"),
            "a stable build must be annotated \"stable\""
        );
        assert_eq!(out[1].version, "0.16.11-beta");
        assert_eq!(out[1].note, None, "a non-stable build must carry no note");
    }

    #[test]
    fn builds_to_versions_preserves_upstream_order() {
        // Fabric/Quilt already return newest-first; selection must not reorder them.
        let builds = vec![build("c", false), build("b", false), build("a", true)];

        let out = builds_to_versions(builds);

        let versions: Vec<&str> = out.iter().map(|v| v.version.as_str()).collect();
        assert_eq!(
            versions,
            vec!["c", "b", "a"],
            "loader build order from upstream must be kept intact"
        );
    }

    // --- select_forge_versions -------------------------------------------------------------

    #[test]
    fn select_forge_offers_latest_then_recommended_when_distinct() {
        let p = promos(&[
            ("1.20.1-latest", "47.3.0"),
            ("1.20.1-recommended", "47.2.0"),
        ]);

        let out = select_forge_versions(&p, "1.20.1");

        assert_eq!(
            out.len(),
            2,
            "distinct latest + recommended must yield two entries"
        );
        assert_eq!(out[0].version, "47.3.0");
        assert_eq!(
            out[0].note.as_deref(),
            Some("latest"),
            "latest must come first"
        );
        assert_eq!(out[1].version, "47.2.0");
        assert_eq!(out[1].note.as_deref(), Some("recommended"));
    }

    #[test]
    fn select_forge_dedupes_when_recommended_equals_latest() {
        // Forge routinely promotes the same build to both slots; it must appear once, as "latest".
        let p = promos(&[
            ("1.20.1-latest", "47.3.0"),
            ("1.20.1-recommended", "47.3.0"),
        ]);

        let out = select_forge_versions(&p, "1.20.1");

        assert_eq!(
            out.len(),
            1,
            "an identical recommended build must be deduped away"
        );
        assert_eq!(out[0].version, "47.3.0");
        assert_eq!(out[0].note.as_deref(), Some("latest"));
    }

    #[test]
    fn select_forge_returns_recommended_alone_when_no_latest() {
        let p = promos(&[("1.20.1-recommended", "47.2.0")]);

        let out = select_forge_versions(&p, "1.20.1");

        assert_eq!(out.len(), 1);
        assert_eq!(out[0].version, "47.2.0");
        assert_eq!(
            out[0].note.as_deref(),
            Some("recommended"),
            "a lone recommended build must still be offered"
        );
    }

    #[test]
    fn select_forge_ignores_promos_for_other_minecraft_versions() {
        let p = promos(&[("1.19.2-latest", "43.4.0"), ("1.20.1-latest", "47.3.0")]);

        let out = select_forge_versions(&p, "1.20.1");

        assert_eq!(
            out.len(),
            1,
            "only the requested MC version's promos are relevant"
        );
        assert_eq!(out[0].version, "47.3.0");
    }

    #[test]
    fn select_forge_is_empty_when_nothing_matches_the_version() {
        let p = promos(&[("1.19.2-latest", "43.4.0")]);

        let out = select_forge_versions(&p, "1.20.1");

        assert!(
            out.is_empty(),
            "no promo for the MC version means no offered builds"
        );
    }

    // --- select_neoforge_versions ----------------------------------------------------------

    #[test]
    fn select_neoforge_filters_to_the_version_prefix_and_reverses_to_newest_first() {
        // Maven returns ascending; only builds for the requested MC prefix are relevant, newest first.
        let versions = vec![
            "20.1.1".to_string(),
            "20.1.20".to_string(),
            "20.2.5".to_string(),
            "21.0.0".to_string(),
        ];

        let out = select_neoforge_versions(versions, "1.20.1");

        let picked: Vec<&str> = out.iter().map(|v| v.version.as_str()).collect();
        assert_eq!(
            picked,
            vec!["20.1.20", "20.1.1"],
            "only 20.1.* builds survive, and maven ascending order is reversed to newest-first"
        );
    }

    #[test]
    fn select_neoforge_carries_no_note() {
        let versions = vec!["20.1.1".to_string()];

        let out = select_neoforge_versions(versions, "1.20.1");

        assert_eq!(out.len(), 1);
        assert_eq!(
            out[0].note, None,
            "NeoForge builds are unannotated (no latest/recommended distinction)"
        );
    }

    #[test]
    fn select_neoforge_is_empty_when_no_build_matches_the_version() {
        let versions = vec!["20.2.5".to_string(), "21.0.0".to_string()];

        let out = select_neoforge_versions(versions, "1.20.1");

        assert!(
            out.is_empty(),
            "no build under the 20.1. prefix means nothing is offered"
        );
    }

    // --- forge_minecraft_versions ----------------------------------------------------------

    #[test]
    fn forge_minecraft_versions_extracts_unique_mc_from_promo_keys() {
        let p = promos(&[
            ("1.20.1-latest", "47.3.0"),
            ("1.20.1-recommended", "47.2.0"),
            ("1.19.2-latest", "43.4.0"),
        ]);

        let out = forge_minecraft_versions(&p);

        assert_eq!(
            out,
            HashSet::from(["1.20.1".to_string(), "1.19.2".to_string()]),
            "latest+recommended of one MC must collapse to a single MC entry"
        );
    }

    // --- neoforge_version_to_mc ------------------------------------------------------------

    #[test]
    fn neoforge_version_to_mc_maps_the_modern_line() {
        assert_eq!(
            neoforge_version_to_mc("20.4.190").as_deref(),
            Some("1.20.4")
        );
        assert_eq!(
            neoforge_version_to_mc("21.0.169").as_deref(),
            Some("1.21"),
            "a zero minor must drop the patch: 21.0.x is Minecraft 1.21, not 1.21.0"
        );
        assert_eq!(
            neoforge_version_to_mc("21.1.100").as_deref(),
            Some("1.21.1")
        );
    }

    #[test]
    fn neoforge_version_to_mc_rejects_non_xyz_inputs() {
        // Missing patch (the 1.20.1 legacy string) or non-numeric parts must not map here.
        assert_eq!(neoforge_version_to_mc("47.1"), None);
        assert_eq!(neoforge_version_to_mc("1.20.1-47.1.106"), None);
        assert_eq!(neoforge_version_to_mc("beta.1.0"), None);
    }

    // --- select_neoforge_legacy_versions ---------------------------------------------------

    #[test]
    fn select_neoforge_legacy_keeps_1_20_1_builds_newest_first() {
        let versions = vec![
            "1.20.1-47.1.5".to_string(),
            "1.20.1-47.1.106".to_string(),
            "20.2.0".to_string(),
        ];

        let out = select_neoforge_legacy_versions(versions);

        let picked: Vec<&str> = out.iter().map(|v| v.version.as_str()).collect();
        assert_eq!(
            picked,
            vec!["1.20.1-47.1.106", "1.20.1-47.1.5"],
            "only 1.20.1- builds survive, reversed from maven-ascending to newest-first"
        );
    }

    // --- deserialization: serde config that carries logic ----------------------------------

    #[test]
    fn build_defaults_stable_to_false_when_absent() {
        // Fabric/Quilt omit `stable` on unstable builds; the `#[serde(default)]` must hold, and a
        // missing flag must read as not-stable rather than failing the parse.
        let b: Build = serde_json::from_str(r#"{ "version": "0.16.0-beta" }"#).unwrap();

        assert!(!b.stable, "an absent `stable` field must default to false");
        assert_eq!(builds_to_versions(vec![b])[0].note, None);
    }
}
