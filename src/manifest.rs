use std::fs;
use std::path::Path;

use anyhow::{bail, Context, Result};
use indexmap::IndexMap;
use serde::{Deserialize, Serialize};

use crate::loader::Loader;
use crate::provider::Provider;
use crate::side::Side;

/// Upper bound on a manifest's on-disk size. A hand-authored `lode.json` is kilobytes; the cap
/// keeps an attacker-authored file from being read wholesale into memory before parsing.
const MAX_MANIFEST_BYTES: u64 = 4 * 1024 * 1024;

/// The hand-authored manifest (`lode.json`) — the single source of truth a maintainer edits.
/// It records desires (which mods, which version constraints, policy), never resolved facts;
/// those live in the lockfile.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Manifest {
    pub pack: PackMeta,
    pub loader: LoaderSpec,
    #[serde(default)]
    pub defaults: Defaults,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub overlays: Vec<Overlay>,
    /// Declared mods, keyed by a stable slug. Order is preserved so `lode add` appends a
    /// self-contained line rather than reshuffling the map (clean diffs, clean merges).
    #[serde(default)]
    pub mods: IndexMap<String, ModSpec>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PackMeta {
    pub name: String,
    pub author: String,
    pub version: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoaderSpec {
    pub name: Loader,
    pub minecraft: String,
    pub version: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Defaults {
    #[serde(default)]
    pub side: Side,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Overlay {
    pub path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub side: Option<Side>,
}

/// A declared mod: either a bare version constraint (`"^0.5.8"`, `"*"`) or an object when the
/// maintainer needs to override side, pin a provider, or disambiguate an ambiguous slug.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ModSpec {
    Constraint(String),
    Detailed(ModSpecDetailed),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModSpecDetailed {
    /// Version constraint. `"*"` means "latest compatible with the pack's loader + MC version".
    pub version: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub side: Option<Side>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<Provider>,
    #[serde(rename = "projectId", default, skip_serializing_if = "Option::is_none")]
    pub project_id: Option<String>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub optional: bool,
    /// When true, `lode update` won't bump this mod — its locked version is kept.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub pin: bool,
}

impl ModSpec {
    pub fn constraint(&self) -> &str {
        match self {
            ModSpec::Constraint(c) => c,
            ModSpec::Detailed(d) => &d.version,
        }
    }

    pub fn side(&self) -> Option<Side> {
        match self {
            ModSpec::Constraint(_) => None,
            ModSpec::Detailed(d) => d.side,
        }
    }

    pub fn provider(&self) -> Option<Provider> {
        match self {
            ModSpec::Constraint(_) => None,
            ModSpec::Detailed(d) => d.provider,
        }
    }

    pub fn pin(&self) -> bool {
        matches!(self, ModSpec::Detailed(d) if d.pin)
    }

    /// For a local mod, the jar's filename under `local/` (stored in `projectId`).
    pub fn project_id(&self) -> Option<&str> {
        match self {
            ModSpec::Constraint(_) => None,
            ModSpec::Detailed(d) => d.project_id.as_deref(),
        }
    }
}

pub const MANIFEST_FILENAME: &str = "lode.json";

impl Manifest {
    /// Read and parse a manifest. The format is strict JSON, like `package.json` / `composer.json`:
    /// comments and trailing commas are rejected, and `add`/`del` re-serialize it canonically.
    pub fn load(path: &Path) -> Result<Manifest> {
        // Reject an oversized file by its metadata first, so a giant attacker-authored manifest is
        // never read into memory in the first place.
        if let Ok(meta) = fs::metadata(path) {
            if meta.len() > MAX_MANIFEST_BYTES {
                bail!(
                    "manifest {} is larger than the {MAX_MANIFEST_BYTES}-byte limit — refusing to parse",
                    path.display()
                );
            }
        }
        let text = fs::read_to_string(path)
            .with_context(|| format!("reading manifest {}", path.display()))?;
        Self::parse(&text)
    }

    /// Parse manifest text. Kept separate from `load` so it is unit-testable without the disk.
    pub fn parse(text: &str) -> Result<Manifest> {
        if text.len() as u64 > MAX_MANIFEST_BYTES {
            bail!(
                "lode.json is larger than the {MAX_MANIFEST_BYTES}-byte limit — refusing to parse"
            );
        }
        // Strict JSON, like package.json / composer.json — comments and trailing commas are errors.
        // serde_json enforces its own recursion limit, so deeply-nested input fails as a parse error
        // rather than overflowing the stack.
        let manifest: Manifest = serde_json::from_str(text)
            .context("lode.json is not valid JSON (or doesn't match the manifest schema)")?;
        // The loader tokens flow into installer argv and interpolated URLs, so pin them to a strict
        // charset here — the one place every consumer loads through — to foreclose option injection.
        validate_loader_token("minecraft", &manifest.loader.minecraft)?;
        validate_loader_token("version", &manifest.loader.version)?;
        Ok(manifest)
    }

    /// Serialize to canonical JSON. `add`/`del` write through this, so a programmatic edit re-emits
    /// the manifest canonically (any hand-written comments/formatting in the source are dropped) —
    /// mirroring how `npm`/`composer` rewrite their manifests.
    pub fn to_json(&self) -> Result<String> {
        let mut json = serde_json::to_string_pretty(self).context("serializing manifest")?;
        json.push('\n');
        Ok(json)
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        crate::atomic::write(path, self.to_json()?)
    }
}

/// Reject a loader token that isn't a strict, argv-safe identifier. `loader.minecraft` and
/// `loader.version` reach a spawned installer's argv and interpolated download URLs, so a value with
/// a leading `-` (parsed as an option) or exotic characters is refused here — at the single load
/// choke point — before any consumer sees it. Charset: `^[A-Za-z0-9][A-Za-z0-9._+-]*$`.
fn validate_loader_token(field: &str, value: &str) -> Result<()> {
    let mut chars = value.chars();
    let first_ok = matches!(chars.next(), Some(c) if c.is_ascii_alphanumeric());
    let rest_ok = value
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '+' | '-'));
    if !first_ok || !rest_ok {
        bail!(
            "loader.{field} '{value}' is not a valid token — it must start with a letter or digit \
             and contain only letters, digits, '.', '_', '+', or '-'"
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deeply_nested_input_is_a_parse_error_not_a_crash() {
        // serde_json's own recursion limit rejects deep nesting as an error (no stack overflow).
        let deep = format!("{}{}", "[".repeat(500), "]".repeat(500));
        assert!(Manifest::parse(&deep).is_err());
    }

    #[test]
    fn rejects_comments_and_trailing_commas() {
        // Strict JSON, like package.json: JSONC-isms that used to be tolerated are now hard errors.
        let base = "{\"pack\":{\"name\":\"t\",\"author\":\"a\",\"version\":\"0\"},\
                    \"loader\":{\"name\":\"fabric\",\"minecraft\":\"1.20.1\",\"version\":\"0\"}}";
        assert!(Manifest::parse(base).is_ok(), "plain JSON must still parse");
        assert!(
            Manifest::parse(&format!("// note\n{base}")).is_err(),
            "a // comment must be rejected"
        );
        assert!(
            Manifest::parse(
                "{\"pack\":{\"name\":\"t\",\"author\":\"a\",\"version\":\"0\",},\
                 \"loader\":{\"name\":\"fabric\",\"minecraft\":\"1.20.1\",\"version\":\"0\"}}"
            )
            .is_err(),
            "a trailing comma must be rejected"
        );
    }

    #[test]
    fn rejects_loader_tokens_that_could_inject_installer_options() {
        // A leading '-' would be read as an option by a spawned installer; exotic characters and
        // whitespace are refused too.
        for bad in ["-rf", "--installServer", "1.20; rm", "a b", "", "$(x)"] {
            let text = format!(
                "{{\"pack\":{{\"name\":\"t\",\"author\":\"a\",\"version\":\"0\"}},\
                 \"loader\":{{\"name\":\"forge\",\"minecraft\":\"{bad}\",\"version\":\"1\"}}}}"
            );
            assert!(
                Manifest::parse(&text).is_err(),
                "should reject minecraft {bad:?}"
            );
        }
        // Real loader tokens (dots, dashes-not-leading, plus, underscore) still parse.
        for good in ["1.20.1", "47.2.0", "20.4.80-beta", "0.16.0"] {
            let text = format!(
                "{{\"pack\":{{\"name\":\"t\",\"author\":\"a\",\"version\":\"0\"}},\
                 \"loader\":{{\"name\":\"forge\",\"minecraft\":\"1.20.1\",\"version\":\"{good}\"}}}}"
            );
            assert!(
                Manifest::parse(&text).is_ok(),
                "should accept version {good:?}"
            );
        }
    }

    #[test]
    fn parses_and_reports_pin() {
        let text = "{\"pack\":{\"name\":\"t\",\"author\":\"a\",\"version\":\"0\"},\
                    \"loader\":{\"name\":\"fabric\",\"minecraft\":\"1.20.1\",\"version\":\"0\"},\
                    \"mods\":{\"sodium\":{\"version\":\"*\",\"pin\":true},\"jei\":\"*\"}}";
        let manifest = Manifest::parse(text).unwrap();
        assert!(manifest.mods["sodium"].pin());
        assert!(!manifest.mods["jei"].pin());
    }
}
