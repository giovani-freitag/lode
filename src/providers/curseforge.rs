use anyhow::{anyhow, bail, Context, Result};
use reqwest::blocking::{Client, RequestBuilder};
use serde::Deserialize;

use super::{ResolvedDep, ResolvedVersion};
use crate::loader::Loader;
use crate::lock::DepKind;
use crate::side::Side;

const API_BASE: &str = "https://api.curseforge.com";
const MINECRAFT_GAME_ID: u32 = 432;

/// Files-per-page when paging the CurseForge files endpoint. CurseForge caps `index + pageSize`
/// at 10000, so we stop before that ceiling.
const FILES_PAGE_SIZE: u32 = 50;

/// The `index` ceiling CurseForge enforces on the files endpoint; paging past it 400s.
const FILES_INDEX_CAP: u32 = 10_000;

/// Client for the CurseForge "Eternal" API. The API key is never embedded: it comes from the
/// user (env `CF_API_KEY`), so we never redistribute a key and never require a login.
pub struct Curseforge {
    client: Client,
    key: String,
}

/// A CurseForge file resolved at install time — the download URL is fetched just-in-time and
/// never persisted (its terms forbid caching the URL). `url` is `None` when the author disabled
/// third-party distribution, in which case the file must be downloaded by hand from `website_url`.
pub struct CfDownload {
    pub url: Option<String>,
    pub filename: String,
    pub hash_format: String,
    pub hash: String,
    pub website_url: Option<String>,
}

#[derive(Debug, Deserialize)]
struct Wrap<T> {
    data: T,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CfMod {
    id: u32,
    name: String,
    slug: String,
    #[serde(default)]
    links: CfLinks,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CfLinks {
    website_url: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CfFile {
    id: u32,
    display_name: String,
    file_name: String,
    download_url: Option<String>,
    #[serde(default)]
    hashes: Vec<CfHash>,
    file_length: Option<u64>,
    file_date: String,
    release_type: u32,
    #[serde(default)]
    dependencies: Vec<CfDep>,
}

#[derive(Debug, Deserialize)]
struct CfHash {
    value: String,
    /// CurseForge algo ids: 1 = sha1, 2 = md5.
    algo: u32,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CfDep {
    mod_id: u32,
    relation_type: u32,
}

impl Curseforge {
    /// Build a client from the stored key (env `CF_API_KEY` or `lode config`).
    pub fn from_config() -> Result<Curseforge> {
        let key = crate::config::curseforge_key().ok_or_else(|| {
            anyhow!(
                "CurseForge needs an API key — set it with `lode config set curseforge.key <KEY>` \
                 or the CF_API_KEY env var (get one at https://console.curseforge.com/). \
                 Modrinth mods need no key"
            )
        })?;
        Ok(Curseforge {
            client: crate::http::client()?,
            key,
        })
    }

    fn get(&self, url: String) -> RequestBuilder {
        self.client.get(url).header("x-api-key", &self.key)
    }

    fn loader_type(loader: Loader) -> u32 {
        match loader {
            Loader::Forge => 1,
            Loader::Fabric => 4,
            Loader::Quilt => 5,
            Loader::Neoforge => 6,
        }
    }

    fn best_hash(file: &CfFile) -> Option<(String, String)> {
        file.hashes
            .iter()
            .find(|h| h.algo == 1)
            .map(|h| ("sha1".to_string(), h.value.clone()))
            .or_else(|| {
                file.hashes
                    .iter()
                    .find(|h| h.algo == 2)
                    .map(|h| ("md5".to_string(), h.value.clone()))
            })
    }

    /// The next `index` to request after a page of `page_len` items fetched at `index`, or `None`
    /// when paging should stop: a short page is the last one, and paging must not cross the API's
    /// `index` ceiling (requests past it 400).
    fn next_page_index(page_len: u32, index: u32) -> Option<u32> {
        if page_len < FILES_PAGE_SIZE {
            return None;
        }
        let next = index + FILES_PAGE_SIZE;
        if next >= FILES_INDEX_CAP {
            None
        } else {
            Some(next)
        }
    }

    /// Deterministic newest-first ordering: file date descending, numeric file id descending as the
    /// tiebreak so identical timestamps still resolve to a stable choice.
    fn sort_files_newest_first(files: &mut [CfFile]) {
        files.sort_by(|a, b| b.file_date.cmp(&a.file_date).then(b.id.cmp(&a.id)));
    }

    /// From an already-sorted list, the file matching `constraint` (`"*"` = anything, a range =
    /// best-effort semver on the display name, otherwise a substring of either name), preferring a
    /// stable release (`release_type == 1`) and falling back to the first match.
    fn choose_matching<'a>(files: &'a [CfFile], constraint: &str) -> Option<&'a CfFile> {
        let is_range = crate::version_req::is_range(constraint);
        let matching: Vec<&CfFile> = files
            .iter()
            .filter(|f| {
                if constraint == "*" {
                    true
                } else if is_range {
                    crate::version_req::matches(&f.display_name, constraint)
                } else {
                    f.file_name.contains(constraint) || f.display_name.contains(constraint)
                }
            })
            .collect();
        matching
            .iter()
            .find(|f| f.release_type == 1)
            .or_else(|| matching.first())
            .copied()
    }

    /// Resolve the mod's project id + name, accepting a numeric id or a slug.
    fn find_mod(&self, id_or_slug: &str) -> Result<CfMod> {
        if let Ok(id) = id_or_slug.parse::<u32>() {
            let resp = self
                .get(format!("{API_BASE}/v1/mods/{id}"))
                .send()
                .context("fetching CurseForge mod")?
                .error_for_status()
                .with_context(|| format!("CurseForge mod {id} not found"))?;
            let wrapped: Wrap<CfMod> = crate::http::json_capped(resp, "CurseForge mod")?;
            return Ok(wrapped.data);
        }

        let resp = self
            .get(format!("{API_BASE}/v1/mods/search"))
            .query(&[
                ("gameId", MINECRAFT_GAME_ID.to_string()),
                ("slug", id_or_slug.to_string()),
            ])
            .send()
            .context("searching CurseForge")?
            .error_for_status()
            .context("CurseForge search error")?;
        let wrapped: Wrap<Vec<CfMod>> = crate::http::json_capped(resp, "CurseForge search")?;
        wrapped
            .data
            .into_iter()
            .next()
            .ok_or_else(|| anyhow!("no CurseForge mod matches slug '{id_or_slug}'"))
    }

    /// The canonical slug for a numeric id or slug, used by `add` to key the manifest entry.
    pub fn slug_of(&self, id_or_slug: &str) -> Result<String> {
        Ok(self.find_mod(id_or_slug)?.slug)
    }

    /// Every file of a mod for the given loader + MC version, paging past CurseForge's per-request
    /// cap so a targeted or older version isn't lost beyond the first page. Ordering isn't relied
    /// on here — the caller sorts — so paging by index is enough.
    fn list_files(&self, cf_mod: &CfMod, loader: Loader, mc_version: &str) -> Result<Vec<CfFile>> {
        let mut files: Vec<CfFile> = Vec::new();
        let mut index = 0u32;
        loop {
            let resp = self
                .get(format!("{API_BASE}/v1/mods/{}/files", cf_mod.id))
                .query(&[
                    ("gameVersion", mc_version.to_string()),
                    ("modLoaderType", Self::loader_type(loader).to_string()),
                    ("index", index.to_string()),
                    ("pageSize", FILES_PAGE_SIZE.to_string()),
                ])
                .send()
                .with_context(|| format!("listing files for '{}'", cf_mod.slug))?
                .error_for_status()
                .with_context(|| format!("listing files for '{}'", cf_mod.slug))?;
            let wrapped: Wrap<Vec<CfFile>> = crate::http::json_capped(resp, "CurseForge files")?;

            let page = wrapped.data.len() as u32;
            files.extend(wrapped.data);
            match Self::next_page_index(page, index) {
                Some(next) => index = next,
                None => break,
            }
        }
        Ok(files)
    }

    /// Resolve the exact file to install, honoring the pack's loader + MC version and the
    /// constraint (`"*"` = latest release, otherwise a substring match on the file name).
    pub fn resolve(
        &self,
        id_or_slug: &str,
        loader: Loader,
        mc_version: &str,
        constraint: &str,
    ) -> Result<ResolvedVersion> {
        let cf_mod = self.find_mod(id_or_slug)?;

        let mut files = self.list_files(&cf_mod, loader, mc_version)?;
        if files.is_empty() {
            bail!(
                "no CurseForge file of '{}' matches loader {} on Minecraft {mc_version}",
                cf_mod.slug,
                loader.modrinth_facet()
            );
        }
        Self::sort_files_newest_first(&mut files);

        let chosen = Self::choose_matching(&files, constraint).ok_or_else(|| {
            anyhow!(
                "no CurseForge file of '{}' matches '{constraint}'",
                cf_mod.slug
            )
        })?;

        let (hash_format, hash) = Self::best_hash(chosen).ok_or_else(|| {
            anyhow!(
                "CurseForge file '{}' has no sha1/md5 hash",
                chosen.file_name
            )
        })?;

        let dependencies = chosen
            .dependencies
            .iter()
            .filter(|d| d.relation_type == 3)
            .map(|d| ResolvedDep {
                project_id: d.mod_id.to_string(),
                kind: DepKind::Required,
            })
            .collect();

        Ok(ResolvedVersion {
            project_id: cf_mod.id.to_string(),
            slug: cf_mod.slug,
            project_name: cf_mod.name,
            file_id: chosen.id.to_string(),
            version: chosen.display_name.clone(),
            filename: chosen.file_name.clone(),
            // Never persist the download URL (terms); it is re-fetched at install time.
            url: None,
            hash_format,
            hash,
            size: chosen.file_length,
            // CurseForge exposes no environment data, so mods default to both sides (as packwiz does).
            side: Side::Both,
            dependencies,
        })
    }

    /// Re-fetch a file's download URL at install time (never stored). `url` is `None` for files
    /// whose author opted out of third-party distribution.
    pub fn file_download(&self, project_id: &str, file_id: &str) -> Result<CfDownload> {
        let resp = self
            .get(format!("{API_BASE}/v1/mods/{project_id}/files/{file_id}"))
            .send()
            .context("fetching CurseForge file")?
            .error_for_status()
            .context("CurseForge file lookup error")?;
        let file: Wrap<CfFile> = crate::http::json_capped(resp, "CurseForge file")?;

        let website_url = self
            .get(format!("{API_BASE}/v1/mods/{project_id}"))
            .send()
            .ok()
            .and_then(|r| crate::http::json_capped::<Wrap<CfMod>>(r, "CurseForge mod").ok())
            .and_then(|m| m.data.links.website_url);

        let (hash_format, hash) = Self::best_hash(&file.data).unwrap_or_default();
        Ok(CfDownload {
            url: file.data.download_url,
            filename: file.data.file_name,
            hash_format,
            hash,
            website_url,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Parse a CurseForge file from an inline JSON fixture, exercising the real serde shape
    /// (camelCase renames + `#[serde(default)]` fallbacks) the way the API responses do.
    fn cf_file(json: &str) -> CfFile {
        serde_json::from_str(json).expect("fixture should parse as CfFile")
    }

    /// A minimal file fixture parametrized over the fields the selection logic reads.
    fn file(id: u32, date: &str, release_type: u32, display: &str, name: &str) -> CfFile {
        cf_file(&format!(
            r#"{{"id":{id},"displayName":"{display}","fileName":"{name}","fileDate":"{date}","releaseType":{release_type}}}"#
        ))
    }

    // ---- best_hash: sha1 wins over md5, md5 is the fallback, neither -> None ----

    #[test]
    fn best_hash_prefers_sha1_even_when_md5_listed_first() {
        let f = cf_file(
            r#"{"id":1,"displayName":"v","fileName":"a.jar","fileDate":"2024-01-01","releaseType":1,
                "hashes":[{"value":"MD5VAL","algo":2},{"value":"SHA1VAL","algo":1}]}"#,
        );

        let picked = Curseforge::best_hash(&f);

        assert_eq!(picked, Some(("sha1".to_string(), "SHA1VAL".to_string())));
    }

    #[test]
    fn best_hash_falls_back_to_md5_when_no_sha1() {
        let f = cf_file(
            r#"{"id":1,"displayName":"v","fileName":"a.jar","fileDate":"2024-01-01","releaseType":1,
                "hashes":[{"value":"MD5VAL","algo":2}]}"#,
        );

        let picked = Curseforge::best_hash(&f);

        assert_eq!(picked, Some(("md5".to_string(), "MD5VAL".to_string())));
    }

    #[test]
    fn best_hash_is_none_without_sha1_or_md5() {
        let empty = cf_file(
            r#"{"id":1,"displayName":"v","fileName":"a.jar","fileDate":"2024-01-01","releaseType":1}"#,
        );
        let unknown_algo = cf_file(
            r#"{"id":1,"displayName":"v","fileName":"a.jar","fileDate":"2024-01-01","releaseType":1,
                "hashes":[{"value":"OTHER","algo":9}]}"#,
        );

        assert_eq!(Curseforge::best_hash(&empty), None);
        assert_eq!(Curseforge::best_hash(&unknown_algo), None);
    }

    // ---- loader_type: the CurseForge modLoaderType ids ----

    #[test]
    fn loader_type_maps_each_loader_to_its_curseforge_id() {
        assert_eq!(Curseforge::loader_type(Loader::Forge), 1);
        assert_eq!(Curseforge::loader_type(Loader::Fabric), 4);
        assert_eq!(Curseforge::loader_type(Loader::Quilt), 5);
        assert_eq!(Curseforge::loader_type(Loader::Neoforge), 6);
    }

    // ---- serde shapes: camelCase renames and defaulted collections ----

    #[test]
    fn cf_file_defaults_missing_collections_and_optionals() {
        let f = cf_file(
            r#"{"id":42,"displayName":"Sodium 0.5.8","fileName":"sodium-0.5.8.jar",
                "fileDate":"2024-05-01T00:00:00Z","releaseType":1}"#,
        );

        assert_eq!(f.id, 42);
        assert_eq!(f.display_name, "Sodium 0.5.8");
        assert_eq!(f.file_name, "sodium-0.5.8.jar");
        assert!(f.hashes.is_empty());
        assert!(f.dependencies.is_empty());
        assert_eq!(f.download_url, None);
        assert_eq!(f.file_length, None);
    }

    #[test]
    fn cf_file_reads_camelcase_length_url_and_dependencies() {
        let f = cf_file(
            r#"{"id":7,"displayName":"v","fileName":"a.jar","fileDate":"2024-01-01","releaseType":1,
                "downloadUrl":"https://edge/a.jar","fileLength":12345,
                "dependencies":[{"modId":99,"relationType":3},{"modId":100,"relationType":2}]}"#,
        );

        assert_eq!(f.download_url.as_deref(), Some("https://edge/a.jar"));
        assert_eq!(f.file_length, Some(12345));
        assert_eq!(f.dependencies.len(), 2);
        assert_eq!(f.dependencies[0].mod_id, 99);
        assert_eq!(f.dependencies[0].relation_type, 3);
    }

    #[test]
    fn cf_mod_links_default_when_absent_and_parse_when_present() {
        let no_links: Wrap<CfMod> =
            serde_json::from_str(r#"{"data":{"id":5,"name":"Sodium","slug":"sodium"}}"#).unwrap();
        let with_links: Wrap<CfMod> = serde_json::from_str(
            r#"{"data":{"id":5,"name":"Sodium","slug":"sodium","links":{"websiteUrl":"https://cf/sodium"}}}"#,
        )
        .unwrap();

        assert_eq!(no_links.data.slug, "sodium");
        assert_eq!(no_links.data.links.website_url, None);
        assert_eq!(
            with_links.data.links.website_url.as_deref(),
            Some("https://cf/sodium")
        );
    }

    #[test]
    fn wrap_parses_the_files_response_envelope() {
        let resp: Wrap<Vec<CfFile>> = serde_json::from_str(
            r#"{"data":[
                {"id":1,"displayName":"a","fileName":"a.jar","fileDate":"2024-01-01","releaseType":1},
                {"id":2,"displayName":"b","fileName":"b.jar","fileDate":"2024-01-02","releaseType":2}
            ]}"#,
        )
        .unwrap();

        assert_eq!(resp.data.len(), 2);
        assert_eq!(resp.data[1].id, 2);
    }

    // ---- pagination math: advance a full page until the API index ceiling ----

    #[test]
    fn next_page_index_advances_after_a_full_page() {
        assert_eq!(
            Curseforge::next_page_index(FILES_PAGE_SIZE, 0),
            Some(FILES_PAGE_SIZE)
        );
    }

    #[test]
    fn next_page_index_stops_on_a_short_or_empty_page() {
        assert_eq!(Curseforge::next_page_index(FILES_PAGE_SIZE - 1, 0), None);
        assert_eq!(Curseforge::next_page_index(0, 0), None);
    }

    #[test]
    fn next_page_index_stops_before_paging_past_the_index_cap() {
        let last_ok = FILES_INDEX_CAP - 2 * FILES_PAGE_SIZE;
        let at_cap_edge = FILES_INDEX_CAP - FILES_PAGE_SIZE;

        assert_eq!(
            Curseforge::next_page_index(FILES_PAGE_SIZE, last_ok),
            Some(FILES_INDEX_CAP - FILES_PAGE_SIZE)
        );
        assert_eq!(
            Curseforge::next_page_index(FILES_PAGE_SIZE, at_cap_edge),
            None
        );
    }

    // ---- sort: newest first, file id breaks timestamp ties ----

    #[test]
    fn sort_orders_newest_first() {
        let mut files = vec![
            file(10, "2024-01-01T00:00:00Z", 1, "old", "old.jar"),
            file(20, "2024-03-01T00:00:00Z", 1, "new", "new.jar"),
            file(30, "2024-02-01T00:00:00Z", 1, "mid", "mid.jar"),
        ];

        Curseforge::sort_files_newest_first(&mut files);

        assert_eq!(
            files.iter().map(|f| f.id).collect::<Vec<_>>(),
            vec![20, 30, 10]
        );
    }

    #[test]
    fn sort_breaks_equal_dates_by_higher_file_id() {
        let mut files = vec![
            file(100, "2024-01-01T00:00:00Z", 1, "a", "a.jar"),
            file(200, "2024-01-01T00:00:00Z", 1, "b", "b.jar"),
        ];

        Curseforge::sort_files_newest_first(&mut files);

        assert_eq!(files[0].id, 200);
        assert_eq!(files[1].id, 100);
    }

    // ---- choose_matching: constraint routing + stable-release preference ----

    #[test]
    fn choose_star_prefers_the_stable_release_over_a_newer_beta() {
        let files = vec![
            file(3, "2024-03-01", 2, "0.6.0-beta", "mod-0.6.0-beta.jar"),
            file(2, "2024-02-01", 1, "0.5.9", "mod-0.5.9.jar"),
            file(1, "2024-01-01", 1, "0.5.8", "mod-0.5.8.jar"),
        ];

        let chosen = Curseforge::choose_matching(&files, "*").expect("some file matches *");

        assert_eq!(
            chosen.id, 2,
            "should pick the newest stable, not the newer beta"
        );
    }

    #[test]
    fn choose_star_falls_back_to_first_when_no_stable_exists() {
        let files = vec![
            file(3, "2024-03-01", 2, "0.6.0-beta", "mod-0.6.0-beta.jar"),
            file(1, "2024-01-01", 3, "0.5.0-alpha", "mod-0.5.0-alpha.jar"),
        ];

        let chosen = Curseforge::choose_matching(&files, "*").expect("some file matches *");

        assert_eq!(
            chosen.id, 3,
            "no stable release -> first (newest) matching file"
        );
    }

    #[test]
    fn choose_plain_constraint_is_a_substring_match_on_names() {
        let files = vec![
            file(2, "2024-02-01", 1, "Sodium 0.5.9", "sodium-0.5.9.jar"),
            file(1, "2024-01-01", 1, "Sodium 0.5.8", "sodium-0.5.8.jar"),
        ];

        let by_filename = Curseforge::choose_matching(&files, "0.5.8").unwrap();
        let no_match = Curseforge::choose_matching(&files, "9.9.9");

        assert_eq!(by_filename.id, 1);
        assert!(no_match.is_none());
    }

    #[test]
    fn choose_matches_substring_against_display_name_too() {
        let files = vec![file(
            1,
            "2024-01-01",
            1,
            "Fancy Build QUUX",
            "fancy-1.0.jar",
        )];

        let chosen = Curseforge::choose_matching(&files, "QUUX").unwrap();

        assert_eq!(chosen.id, 1);
    }

    #[test]
    fn is_range_switches_between_substring_and_semver_matching() {
        let files = vec![file(1, "2024-01-01", 1, "0.9.0", "mod-0.9.0.jar")];

        let substring = Curseforge::choose_matching(&files, "0.9");
        let range_excludes = Curseforge::choose_matching(&files, "^1.0.0");

        assert!(
            substring.is_some(),
            "plain '0.9' is a substring of display name '0.9.0'"
        );
        assert!(
            range_excludes.is_none(),
            "range '^1.0.0' is a semver requirement 0.9.0 does not satisfy"
        );
    }

    #[test]
    fn choose_range_selects_the_semver_satisfying_file() {
        let files = vec![
            file(2, "2024-02-01", 1, "2.5.0", "mod-2.5.0.jar"),
            file(1, "2024-01-01", 1, "0.9.0", "mod-0.9.0.jar"),
        ];

        let chosen = Curseforge::choose_matching(&files, "^2.0.0").unwrap();

        assert_eq!(
            chosen.id, 2,
            "only 2.5.0 satisfies ^2.0.0 (0.9.0 is excluded)"
        );
    }

    #[test]
    fn sort_then_choose_reproduces_resolve_latest_release() {
        let mut files = vec![
            file(1, "2024-01-01", 1, "0.5.0", "mod-0.5.0.jar"),
            file(3, "2024-03-01", 2, "0.7.0-beta", "mod-0.7.0-beta.jar"),
            file(2, "2024-02-01", 1, "0.6.0", "mod-0.6.0.jar"),
        ];

        Curseforge::sort_files_newest_first(&mut files);
        let chosen = Curseforge::choose_matching(&files, "*").unwrap();

        assert_eq!(chosen.id, 2, "latest stable release across the sorted set");
    }
}
