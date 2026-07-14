use anyhow::{anyhow, bail, Context, Result};
use reqwest::blocking::Client;
use serde::Deserialize;

use super::{ImportedVersion, ResolvedDep, ResolvedVersion};
use crate::loader::Loader;
use crate::lock::DepKind;
use crate::side::Side;

const API_BASE: &str = "https://api.modrinth.com/v2";

/// Client for the Modrinth Labrinth API (open, keyless). The only provider the pilot resolves.
pub struct Modrinth {
    client: Client,
    base: String,
}

#[derive(Debug, Deserialize)]
struct ProjectResp {
    slug: String,
    title: String,
    client_side: EnvSupport,
    server_side: EnvSupport,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
enum EnvSupport {
    Required,
    Optional,
    Unsupported,
    Unknown,
}

impl EnvSupport {
    /// A side counts as "present" when the project marks it required or optional; only
    /// unsupported/unknown excludes it — matching packwiz's own derivation.
    fn present(self) -> bool {
        matches!(self, EnvSupport::Required | EnvSupport::Optional)
    }
}

#[derive(Debug, Deserialize)]
struct VersionResp {
    id: String,
    project_id: String,
    version_number: String,
    version_type: String,
    date_published: String,
    #[serde(default)]
    loaders: Vec<String>,
    files: Vec<VersionFile>,
    #[serde(default)]
    dependencies: Vec<VersionDep>,
}

#[derive(Debug, Deserialize)]
struct VersionFile {
    url: String,
    filename: String,
    size: Option<u64>,
    hashes: FileHashes,
    #[serde(default)]
    primary: bool,
}

#[derive(Debug, Deserialize)]
struct FileHashes {
    sha512: Option<String>,
    sha1: Option<String>,
}

#[derive(Debug, Deserialize)]
struct VersionDep {
    project_id: Option<String>,
    dependency_type: String,
}

/// Map Modrinth's `dependency_type` string to our `DepKind`, or `None` for a value we don't model.
fn dep_kind(dependency_type: &str) -> Option<DepKind> {
    match dependency_type {
        "required" => Some(DepKind::Required),
        "optional" => Some(DepKind::Optional),
        "incompatible" => Some(DepKind::Incompatible),
        "embedded" => Some(DepKind::Embedded),
        _ => None,
    }
}

/// Collect the dependency edges we model, dropping unknown kinds and edges without a project id.
fn collect_deps(deps: &[VersionDep]) -> Vec<ResolvedDep> {
    deps.iter()
        .filter_map(|d| {
            let kind = dep_kind(&d.dependency_type)?;
            d.project_id
                .clone()
                .map(|project_id| ResolvedDep { project_id, kind })
        })
        .collect()
}

/// Order versions newest-first (ISO-8601 dates sort lexically), breaking ties on the version id
/// so two versions published at the same instant always resolve deterministically, never by API
/// order.
fn sort_versions_newest_first(versions: &mut [VersionResp]) {
    versions.sort_by(|a, b| {
        b.date_published
            .cmp(&a.date_published)
            .then_with(|| b.id.cmp(&a.id))
    });
}

/// From versions already ordered newest-first, keep those satisfying `constraint`, then prefer the
/// newest stable release over any newer pre-release, falling back to the newest match of any type.
fn choose_version<'a>(versions: &'a [VersionResp], constraint: &str) -> Option<&'a VersionResp> {
    let matching: Vec<&VersionResp> = versions
        .iter()
        .filter(|v| crate::version_req::matches(&v.version_number, constraint))
        .collect();
    matching
        .iter()
        .find(|v| v.version_type == "release")
        .or_else(|| matching.first())
        .copied()
}

/// Whether a Modrinth version actually lists a loader this pack can use. Modrinth's `loaders` search
/// facet is unreliable cross-loader, so a chosen version's own `loaders` are the source of truth — a
/// Quilt pack accepts `fabric` builds, matching `Loader::compatible_facets`.
fn version_supports_loader(version: &VersionResp, loader: Loader) -> bool {
    version
        .loaders
        .iter()
        .any(|l| loader.compatible_facets().contains(&l.as_str()))
}

/// The file to download for a version: the primary-flagged file, else the first listed.
fn select_file(files: &[VersionFile]) -> Option<&VersionFile> {
    files.iter().find(|f| f.primary).or_else(|| files.first())
}

/// The provider-native (hash_format, hash) to record: prefer sha512, fall back to sha1.
fn pick_hash(hashes: &FileHashes) -> Option<(String, String)> {
    if let Some(sha512) = &hashes.sha512 {
        Some(("sha512".to_string(), sha512.clone()))
    } else {
        hashes
            .sha1
            .as_ref()
            .map(|sha1| ("sha1".to_string(), sha1.clone()))
    }
}

#[derive(Debug, Deserialize)]
struct SearchResp {
    hits: Vec<SearchHit>,
}

/// A single search result. Public so the `add` command can present the picker.
#[derive(Debug, Clone, Deserialize)]
pub struct SearchHit {
    pub slug: String,
    pub title: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub downloads: u64,
}

impl Modrinth {
    pub fn new() -> Result<Modrinth> {
        Ok(Modrinth {
            client: crate::http::client()?,
            base: API_BASE.to_string(),
        })
    }

    /// The canonical slug for `id_or_slug`, but only when the project actually has a build for this
    /// loader + Minecraft version. Returns `None` if the project doesn't exist *or* has no compatible
    /// build — so `add` can fall through to a facet-filtered search instead of committing to an exact
    /// match it can't install. (A bare project lookup can't see this, which is how `add <slug>` for a
    /// wrong-loader mod used to be accepted and then fail late.)
    pub fn compatible_slug(
        &self,
        id_or_slug: &str,
        loader: Loader,
        mc_version: &str,
    ) -> Result<Option<String>> {
        let resp = self
            .client
            .get(format!("{}/project/{}", self.base, id_or_slug))
            .send()
            .with_context(|| format!("fetching Modrinth project '{id_or_slug}'"))?;
        if resp.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(None);
        }
        let resp = resp
            .error_for_status()
            .with_context(|| format!("Modrinth error for '{id_or_slug}'"))?;
        let project: ProjectResp = crate::http::json_capped(resp, "Modrinth project")?;

        let loaders_facet =
            serde_json::to_string(&loader.compatible_facets().iter().collect::<Vec<_>>())?;
        let versions_facet = format!(r#"["{mc_version}"]"#);
        let resp = self
            .client
            .get(format!("{}/project/{}/version", self.base, id_or_slug))
            .query(&[
                ("loaders", loaders_facet.as_str()),
                ("game_versions", versions_facet.as_str()),
            ])
            .send()
            .with_context(|| format!("listing versions for '{id_or_slug}'"))?
            .error_for_status()
            .with_context(|| format!("listing versions for '{id_or_slug}'"))?;
        let versions: Vec<VersionResp> = crate::http::json_capped(resp, "Modrinth versions")?;

        let compatible = versions.iter().any(|v| version_supports_loader(v, loader));
        Ok(compatible.then_some(project.slug))
    }

    fn side_from(project: &ProjectResp) -> Side {
        Side::from_env(project.client_side.present(), project.server_side.present())
    }

    /// Full-text search for projects matching `query`, restricted to mods for this loader + MC
    /// version. Returns the ranked hits for an interactive picker.
    pub fn search(&self, query: &str, loader: Loader, mc_version: &str) -> Result<Vec<SearchHit>> {
        let facets = format!(
            r#"[["project_type:mod"],["categories:{}"],["versions:{}"]]"#,
            loader.modrinth_facet(),
            mc_version
        );
        let resp = self
            .client
            .get(format!("{}/search", self.base))
            .query(&[("query", query), ("facets", &facets), ("limit", "10")])
            .send()
            .context("querying Modrinth search")?
            .error_for_status()
            .context("Modrinth search returned an error")?;
        let search: SearchResp = crate::http::json_capped(resp, "Modrinth search response")?;
        Ok(search.hits)
    }

    /// Resolve the exact version to install for a project, honoring the pack's loader + MC
    /// version and the manifest constraint (`"*"` = latest, otherwise an exact version string).
    pub fn resolve(
        &self,
        id_or_slug: &str,
        loader: Loader,
        mc_version: &str,
        constraint: &str,
    ) -> Result<ResolvedVersion> {
        let resp = self
            .client
            .get(format!("{}/project/{}", self.base, id_or_slug))
            .send()
            .with_context(|| format!("fetching Modrinth project '{id_or_slug}'"))?
            .error_for_status()
            .with_context(|| format!("Modrinth project '{id_or_slug}' not found"))?;
        let project: ProjectResp = crate::http::json_capped(resp, "Modrinth project")?;

        let loaders_facet =
            serde_json::to_string(&loader.compatible_facets().iter().collect::<Vec<_>>())?;
        let versions_facet = format!(r#"["{mc_version}"]"#);

        let resp = self
            .client
            .get(format!("{}/project/{}/version", self.base, id_or_slug))
            .query(&[
                ("loaders", loaders_facet.as_str()),
                ("game_versions", versions_facet.as_str()),
            ])
            .send()
            .with_context(|| format!("listing versions for '{id_or_slug}'"))?
            .error_for_status()
            .with_context(|| format!("listing versions for '{id_or_slug}'"))?;
        let mut versions: Vec<VersionResp> = crate::http::json_capped(resp, "Modrinth versions")?;

        // The loaders facet is unreliable cross-loader; drop any version that doesn't actually list
        // a compatible loader so a wrong-loader build can never be chosen.
        versions.retain(|v| version_supports_loader(v, loader));

        if versions.is_empty() {
            bail!(
                "no Modrinth version of '{}' matches loader {} on Minecraft {}",
                project.slug,
                loader.modrinth_facet(),
                mc_version
            );
        }

        sort_versions_newest_first(&mut versions);

        let chosen = choose_version(&versions, constraint).ok_or_else(|| {
            anyhow!(
                "no version of '{}' matches '{constraint}' for loader {} on Minecraft {mc_version}",
                project.slug,
                loader.modrinth_facet()
            )
        })?;

        let file = select_file(&chosen.files).ok_or_else(|| {
            anyhow!(
                "Modrinth version of '{}' has no downloadable file",
                project.slug
            )
        })?;

        let (hash_format, hash) = pick_hash(&file.hashes).ok_or_else(|| {
            anyhow!(
                "Modrinth file '{}' carries no sha512/sha1 hash",
                file.filename
            )
        })?;

        let dependencies = collect_deps(&chosen.dependencies);

        Ok(ResolvedVersion {
            project_id: chosen.project_id.clone(),
            slug: project.slug.clone(),
            project_name: project.title.clone(),
            file_id: chosen.id.clone(),
            version: chosen.version_number.clone(),
            filename: file.filename.clone(),
            url: Some(file.url.clone()),
            hash_format,
            hash,
            size: file.size,
            side: Self::side_from(&project),
            dependencies,
        })
    }

    /// Look up a single version by its id, returning the metadata packwiz's `.pw.toml` omits —
    /// the human version number, file size, and dependency edges. Used by `lode import`.
    pub fn version_by_id(&self, version_id: &str) -> Result<ImportedVersion> {
        let resp = self
            .client
            .get(format!("{}/version/{}", self.base, version_id))
            .send()
            .with_context(|| format!("fetching Modrinth version '{version_id}'"))?
            .error_for_status()
            .with_context(|| format!("Modrinth version '{version_id}' not found"))?;
        let version: VersionResp = crate::http::json_capped(resp, "Modrinth version")?;

        let size = select_file(&version.files).and_then(|f| f.size);

        let dependencies = collect_deps(&version.dependencies);

        Ok(ImportedVersion {
            version_number: version.version_number,
            size,
            dependencies,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- fixtures / builders ---------------------------------------------------------------

    fn dep(project_id: Option<&str>, dependency_type: &str) -> VersionDep {
        VersionDep {
            project_id: project_id.map(String::from),
            dependency_type: dependency_type.to_string(),
        }
    }

    fn project(client: EnvSupport, server: EnvSupport) -> ProjectResp {
        ProjectResp {
            slug: "sodium".to_string(),
            title: "Sodium".to_string(),
            client_side: client,
            server_side: server,
        }
    }

    fn hashes(sha512: Option<&str>, sha1: Option<&str>) -> FileHashes {
        FileHashes {
            sha512: sha512.map(String::from),
            sha1: sha1.map(String::from),
        }
    }

    fn file(filename: &str, primary: bool) -> VersionFile {
        VersionFile {
            url: format!("https://cdn.modrinth.com/{filename}"),
            filename: filename.to_string(),
            size: Some(1),
            hashes: hashes(Some("deadbeef"), None),
            primary,
        }
    }

    fn ver(
        id: &str,
        date_published: &str,
        version_type: &str,
        version_number: &str,
    ) -> VersionResp {
        VersionResp {
            id: id.to_string(),
            project_id: "proj".to_string(),
            version_number: version_number.to_string(),
            version_type: version_type.to_string(),
            date_published: date_published.to_string(),
            loaders: Vec::new(),
            files: Vec::new(),
            dependencies: Vec::new(),
        }
    }

    // --- dep_kind --------------------------------------------------------------------------

    #[test]
    fn dep_kind_maps_the_four_modelled_types() {
        assert_eq!(dep_kind("required"), Some(DepKind::Required));
        assert_eq!(dep_kind("optional"), Some(DepKind::Optional));
        assert_eq!(dep_kind("incompatible"), Some(DepKind::Incompatible));
        assert_eq!(dep_kind("embedded"), Some(DepKind::Embedded));
    }

    #[test]
    fn dep_kind_rejects_unknown_and_miscased_types() {
        // Modrinth always sends lowercase tokens; casing is not normalized, so a miscased or
        // unmodelled value must fall through to None rather than being silently coerced.
        assert_eq!(dep_kind("Required"), None);
        assert_eq!(dep_kind("recommended"), None);
        assert_eq!(dep_kind(""), None);
    }

    // --- collect_deps ----------------------------------------------------------------------

    #[test]
    fn collect_deps_maps_known_kinds_and_preserves_order() {
        let deps = vec![
            dep(Some("aaa"), "required"),
            dep(Some("bbb"), "optional"),
            dep(Some("ccc"), "incompatible"),
            dep(Some("ddd"), "embedded"),
        ];

        let out = collect_deps(&deps);

        assert_eq!(out.len(), 4);
        assert_eq!(out[0].project_id, "aaa");
        assert_eq!(out[0].kind, DepKind::Required);
        assert_eq!(out[1].kind, DepKind::Optional);
        assert_eq!(out[2].kind, DepKind::Incompatible);
        assert_eq!(out[3].project_id, "ddd");
        assert_eq!(out[3].kind, DepKind::Embedded);
    }

    #[test]
    fn collect_deps_drops_unknown_kinds_and_projectless_edges() {
        let deps = vec![
            dep(Some("keep"), "required"),
            dep(Some("drop-kind"), "recommended"),
            dep(None, "required"),
        ];

        let out = collect_deps(&deps);

        assert_eq!(
            out.len(),
            1,
            "only the mapped edge with a project id survives"
        );
        assert_eq!(out[0].project_id, "keep");
        assert_eq!(out[0].kind, DepKind::Required);
    }

    #[test]
    fn collect_deps_on_empty_is_empty() {
        assert!(collect_deps(&[]).is_empty());
    }

    // --- EnvSupport / side derivation ------------------------------------------------------

    #[test]
    fn env_support_present_only_for_required_or_optional() {
        assert!(EnvSupport::Required.present());
        assert!(EnvSupport::Optional.present());
        assert!(!EnvSupport::Unsupported.present());
        assert!(!EnvSupport::Unknown.present());
    }

    #[test]
    fn side_from_maps_the_client_server_matrix() {
        use EnvSupport::*;
        assert_eq!(
            Modrinth::side_from(&project(Required, Required)),
            Side::Both
        );
        assert_eq!(
            Modrinth::side_from(&project(Required, Unsupported)),
            Side::Client
        );
        assert_eq!(
            Modrinth::side_from(&project(Unsupported, Required)),
            Side::Server
        );
        assert_eq!(
            Modrinth::side_from(&project(Unsupported, Unknown)),
            Side::None
        );
    }

    #[test]
    fn side_from_treats_optional_as_present_and_unknown_as_absent() {
        use EnvSupport::*;
        // "optional" on a side still counts that side in; "unknown" does not.
        assert_eq!(
            Modrinth::side_from(&project(Optional, Unknown)),
            Side::Client
        );
        assert_eq!(
            Modrinth::side_from(&project(Unknown, Optional)),
            Side::Server
        );
    }

    // --- deserialization of the Modrinth API shapes ----------------------------------------

    #[test]
    fn version_resp_deserializes_and_applies_serde_defaults() {
        let json = r#"{
            "id": "vAAA",
            "project_id": "pXYZ",
            "version_number": "1.2.0",
            "version_type": "release",
            "date_published": "2024-01-02T00:00:00Z",
            "files": [
                {
                    "url": "https://cdn.modrinth.com/a.jar",
                    "filename": "a.jar",
                    "size": 123,
                    "hashes": { "sha512": "abc", "sha1": "def" }
                }
            ]
        }"#;

        let v: VersionResp = serde_json::from_str(json).unwrap();

        assert_eq!(v.id, "vAAA");
        assert_eq!(v.project_id, "pXYZ");
        assert_eq!(v.version_number, "1.2.0");
        assert!(
            v.dependencies.is_empty(),
            "a missing `dependencies` array must default to empty, not fail parsing"
        );
        assert!(
            !v.files[0].primary,
            "a missing `primary` must default to false"
        );
        assert_eq!(v.files[0].size, Some(123));
        assert_eq!(v.files[0].hashes.sha512.as_deref(), Some("abc"));
    }

    #[test]
    fn version_file_defaults_size_and_primary_when_absent() {
        let json = r#"{ "url": "u", "filename": "f.jar", "hashes": { "sha1": "aa" } }"#;

        let f: VersionFile = serde_json::from_str(json).unwrap();

        assert_eq!(f.size, None);
        assert!(!f.primary);
        assert_eq!(f.hashes.sha512, None);
        assert_eq!(f.hashes.sha1.as_deref(), Some("aa"));
    }

    #[test]
    fn project_resp_deserializes_env_support_lowercase() {
        let json = r#"{
            "slug": "sodium",
            "title": "Sodium",
            "client_side": "required",
            "server_side": "unsupported"
        }"#;

        let p: ProjectResp = serde_json::from_str(json).unwrap();

        assert_eq!(p.slug, "sodium");
        assert_eq!(Modrinth::side_from(&p), Side::Client);
    }

    #[test]
    fn version_dep_treats_absent_project_id_as_none() {
        let json = r#"{ "dependency_type": "required" }"#;

        let d: VersionDep = serde_json::from_str(json).unwrap();

        assert!(d.project_id.is_none());
        assert!(
            collect_deps(std::slice::from_ref(&d)).is_empty(),
            "a version-pinned dep with no project id is dropped"
        );
    }

    // --- version / file selection + tiebreak + hash format ---------------------------------
    // These exercise the pure selection helpers extracted out of `resolve` (see refactorNeeded).

    #[test]
    fn sort_orders_by_date_then_breaks_ties_on_version_id_descending() {
        let mut versions = vec![
            ver("aaa", "2024-01-01T00:00:00Z", "release", "1.0.0"),
            ver("zzz", "2024-01-01T00:00:00Z", "release", "1.0.0"),
            ver("mmm", "2024-02-01T00:00:00Z", "release", "1.1.0"),
        ];

        sort_versions_newest_first(&mut versions);

        // Newest publish date wins; the same-instant pair is ordered by descending id so the
        // resolution is deterministic regardless of the order the API returned them.
        assert_eq!(versions[0].id, "mmm");
        assert_eq!(versions[1].id, "zzz");
        assert_eq!(versions[2].id, "aaa");
    }

    #[test]
    fn choose_prefers_newest_stable_release_over_a_newer_prerelease() {
        let mut versions = vec![
            ver("beta", "2024-03-01T00:00:00Z", "beta", "2.0.0-beta"),
            ver("rel", "2024-02-01T00:00:00Z", "release", "1.9.0"),
        ];
        sort_versions_newest_first(&mut versions);

        let chosen = choose_version(&versions, "*").unwrap();

        assert_eq!(
            chosen.id, "rel",
            "a stable release is preferred even when a newer beta exists"
        );
    }

    #[test]
    fn choose_falls_back_to_newest_match_when_no_release_exists() {
        let mut versions = vec![
            ver("older-beta", "2024-01-01T00:00:00Z", "beta", "1.0.0-beta"),
            ver("newer-beta", "2024-05-01T00:00:00Z", "beta", "1.1.0-beta"),
        ];
        sort_versions_newest_first(&mut versions);

        let chosen = choose_version(&versions, "*").unwrap();

        assert_eq!(chosen.id, "newer-beta");
    }

    #[test]
    fn choose_filters_by_constraint_and_returns_none_when_nothing_matches() {
        let versions = vec![
            ver("a", "2024-01-01T00:00:00Z", "release", "1.0.0"),
            ver("b", "2024-02-01T00:00:00Z", "release", "2.0.0"),
        ];

        assert_eq!(choose_version(&versions, "1.0.0").unwrap().id, "a");
        assert!(choose_version(&versions, "9.9.9").is_none());
    }

    #[test]
    fn select_file_prefers_the_primary_then_the_first() {
        let with_primary = vec![file("first.jar", false), file("primary.jar", true)];
        assert_eq!(select_file(&with_primary).unwrap().filename, "primary.jar");

        let no_primary = vec![file("a.jar", false), file("b.jar", false)];
        assert_eq!(select_file(&no_primary).unwrap().filename, "a.jar");

        assert!(select_file(&[]).is_none());
    }

    #[test]
    fn pick_hash_prefers_sha512_then_sha1_then_none() {
        assert_eq!(
            pick_hash(&hashes(Some("SHA512VAL"), Some("sha1val"))),
            Some(("sha512".to_string(), "SHA512VAL".to_string()))
        );
        assert_eq!(
            pick_hash(&hashes(None, Some("only-sha1"))),
            Some(("sha1".to_string(), "only-sha1".to_string()))
        );
        assert_eq!(pick_hash(&hashes(None, None)), None);
    }

    // --- version_supports_loader -----------------------------------------------------------

    #[test]
    fn version_supports_loader_matches_compatible_facets() {
        let mut v = ver("a", "2024-01-01T00:00:00Z", "release", "1.0.0");
        v.loaders = vec!["fabric".to_string()];

        assert!(version_supports_loader(&v, Loader::Fabric));
        assert!(
            version_supports_loader(&v, Loader::Quilt),
            "a Quilt pack accepts a fabric-loader build (compatible_facets)"
        );
        assert!(
            !version_supports_loader(&v, Loader::Forge),
            "a fabric-only build must not satisfy a Forge pack"
        );
    }

    #[test]
    fn version_supports_loader_is_false_when_loaders_absent() {
        // A version listing no loaders can't be assumed compatible with any pack loader.
        let v = ver("a", "2024-01-01T00:00:00Z", "release", "1.0.0");
        assert!(!version_supports_loader(&v, Loader::Fabric));
    }
}
