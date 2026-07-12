use std::collections::HashMap;

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
        Ok(select_forge_versions(&promos.promos, mc))
    }

    /// NeoForge versions encode the Minecraft version: MC `1.X.Y` maps to NeoForge `X.Y.*`.
    fn neoforge(&self, mc: &str) -> Result<Vec<LoaderVersion>> {
        #[derive(Deserialize)]
        struct Releases {
            versions: Vec<String>,
        }
        let resp = self
            .client
            .get("https://maven.neoforged.net/api/maven/versions/releases/net/neoforged/neoforge")
            .send()
            .context("fetching NeoForge versions")?
            .error_for_status()
            .context("NeoForge maven error")?;
        let releases: Releases = crate::http::json_capped(resp, "NeoForge versions")?;
        Ok(select_neoforge_versions(releases.versions, mc))
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
