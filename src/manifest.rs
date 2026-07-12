use std::fs;
use std::path::Path;

use anyhow::{bail, Context, Result};
use indexmap::IndexMap;
use serde::{Deserialize, Serialize};

use crate::loader::Loader;
use crate::provider::Provider;
use crate::side::Side;

/// Upper bound on a manifest's on-disk size. A hand-authored `lode.jsonc` is kilobytes; the cap
/// keeps an attacker-authored file from being read wholesale into memory before parsing.
const MAX_MANIFEST_BYTES: u64 = 4 * 1024 * 1024;

/// Maximum bracket-nesting depth accepted before parsing, matching serde_json's own recursion
/// limit. A deeply nested `lode.jsonc` would otherwise recurse the parser into a stack overflow.
const MAX_NESTING_DEPTH: usize = 128;

/// The hand-authored manifest (`lode.jsonc`) — the single source of truth a maintainer edits.
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

pub const MANIFEST_FILENAME: &str = "lode.jsonc";

impl Manifest {
    /// Read and parse a manifest, tolerating JSONC (comments + trailing commas).
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
                "lode.jsonc is larger than the {MAX_MANIFEST_BYTES}-byte limit — refusing to parse"
            );
        }
        // Pre-scan nesting depth before handing the text to a recursive-descent parser, so a
        // deeply-nested input is rejected as a normal parse error rather than overflowing the stack.
        if exceeds_max_depth(text, MAX_NESTING_DEPTH) {
            bail!("lode.jsonc nests deeper than {MAX_NESTING_DEPTH} levels — refusing to parse");
        }
        let value = jsonc_parser::parse_to_serde_value(text, &Default::default())
            .context("parsing lode.jsonc")?
            .context("lode.jsonc is empty")?;
        let manifest: Manifest = serde_json::from_value(value)
            .context("lode.jsonc does not match the manifest schema")?;
        // The loader tokens flow into installer argv and interpolated URLs, so pin them to a strict
        // charset here — the one place every consumer loads through — to foreclose option injection.
        validate_loader_token("minecraft", &manifest.loader.minecraft)?;
        validate_loader_token("version", &manifest.loader.version)?;
        Ok(manifest)
    }

    /// Serialize to canonical JSON. A programmatic rewrite re-emits the manifest canonically
    /// rather than editing in place, so any hand-written comments in the source are dropped.
    pub fn to_json(&self) -> Result<String> {
        let mut json = serde_json::to_string_pretty(self).context("serializing manifest")?;
        json.push('\n');
        Ok(json)
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        fs::write(path, self.to_json()?)
            .with_context(|| format!("writing manifest {}", path.display()))?;
        Ok(())
    }
}

/// Whether `text` nests `{`/`[` deeper than `limit` at any point. Brackets inside string literals
/// and JSONC comments are ignored so a legitimate manifest is never miscounted. Operates on bytes:
/// every character it reasons about (`"`, `/`, `*`, brackets, `\`) is ASCII, so multi-byte UTF-8
/// content passes through untouched.
fn exceeds_max_depth(text: &str, limit: usize) -> bool {
    let bytes = text.as_bytes();
    let mut i = 0;
    let mut depth = 0usize;
    while i < bytes.len() {
        match bytes[i] {
            b'"' => {
                // Skip a string literal, honouring backslash escapes.
                i += 1;
                while i < bytes.len() {
                    match bytes[i] {
                        b'\\' => i += 1,
                        b'"' => break,
                        _ => {}
                    }
                    i += 1;
                }
            }
            b'/' if bytes.get(i + 1) == Some(&b'/') => {
                i += 2;
                while i < bytes.len() && bytes[i] != b'\n' {
                    i += 1;
                }
            }
            b'/' if bytes.get(i + 1) == Some(&b'*') => {
                i += 2;
                while i + 1 < bytes.len() && !(bytes[i] == b'*' && bytes[i + 1] == b'/') {
                    i += 1;
                }
                i += 1;
            }
            b'{' | b'[' => {
                depth += 1;
                if depth > limit {
                    return true;
                }
            }
            b'}' | b']' => depth = depth.saturating_sub(1),
            _ => {}
        }
        i += 1;
    }
    false
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

/// Insert a `"slug": value` entry into the `mods` object of raw manifest text, preserving comments
/// and formatting everywhere else — so `lode add` doesn't wipe the hand-written notes a full
/// re-serialize would. `value_json` is the compact JSON for the spec. Returns `None` if the `mods`
/// object can't be located (the caller then falls back to a full re-serialize).
///
/// The entry is *appended* after the last existing one, mirroring the in-memory `IndexMap`, which
/// also appends. This alignment is load-bearing: `lode add` hashes the in-memory manifest into the
/// lock but writes this spliced text to disk, so if the two disagreed on order the reparsed disk
/// manifest would serialize differently and `install` would see a stale lock and needlessly
/// re-resolve. Same order in both places keeps the lock fresh.
pub fn insert_mod_text(text: &str, slug: &str, value_json: &str) -> Option<String> {
    let mods_key = text.find("\"mods\"")?;
    let brace = text[mods_key..].find('{')? + mods_key;
    let insert_pos = brace + 1;
    let close = matching_brace(text.as_bytes(), brace)?;

    match last_meaningful(text.as_bytes(), insert_pos, close) {
        // Empty `mods` object (possibly holding only comments): drop the sole entry inside it.
        None => {
            let insertion = format!("\n    \"{slug}\": {value_json}\n  ");
            let mut out = String::with_capacity(text.len() + insertion.len());
            out.push_str(&text[..insert_pos]);
            out.push_str(&insertion);
            out.push_str(&text[insert_pos..]);
            Some(out)
        }
        // Append right after the last entry's value, adding a separating comma unless a trailing
        // one is already there. Anything after that point (an inline comment, the closing brace)
        // is preserved verbatim.
        Some(p) => {
            let needs_comma = text.as_bytes()[p] != b',';
            let mut out = String::with_capacity(text.len() + value_json.len() + slug.len() + 16);
            out.push_str(&text[..=p]);
            if needs_comma {
                out.push(',');
            }
            out.push_str(&format!("\n    \"{slug}\": {value_json}"));
            out.push_str(&text[p + 1..]);
            Some(out)
        }
    }
}

/// Byte index of the `}` matching the `{` at `open`, honouring string literals and JSONC comments
/// so braces nested inside a detailed mod spec — or inside a comment or string — don't miscount.
/// Returns `None` if the braces are unbalanced.
fn matching_brace(bytes: &[u8], open: usize) -> Option<usize> {
    let mut i = open;
    let mut depth = 0usize;
    while i < bytes.len() {
        match bytes[i] {
            b'"' => {
                i += 1;
                while i < bytes.len() {
                    match bytes[i] {
                        b'\\' => i += 1,
                        b'"' => break,
                        _ => {}
                    }
                    i += 1;
                }
            }
            b'/' if bytes.get(i + 1) == Some(&b'/') => {
                i += 2;
                while i < bytes.len() && bytes[i] != b'\n' {
                    i += 1;
                }
            }
            b'/' if bytes.get(i + 1) == Some(&b'*') => {
                i += 2;
                while i + 1 < bytes.len() && !(bytes[i] == b'*' && bytes[i + 1] == b'/') {
                    i += 1;
                }
                i += 1;
            }
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(i);
                }
            }
            _ => {}
        }
        i += 1;
    }
    None
}

/// Byte index of the last "meaningful" character in `bytes[start..end]` — the last one that is
/// neither whitespace nor part of a comment (a string's closing quote counts). Returns `None` when
/// the range holds only whitespace and comments, i.e. an effectively empty object body.
fn last_meaningful(bytes: &[u8], start: usize, end: usize) -> Option<usize> {
    let mut i = start;
    let mut last = None;
    while i < end {
        match bytes[i] {
            b'"' => {
                i += 1;
                while i < end {
                    match bytes[i] {
                        b'\\' => i += 1,
                        b'"' => break,
                        _ => {}
                    }
                    i += 1;
                }
                last = Some(i.min(end - 1));
            }
            b'/' if bytes.get(i + 1) == Some(&b'/') => {
                i += 2;
                while i < end && bytes[i] != b'\n' {
                    i += 1;
                }
            }
            b'/' if bytes.get(i + 1) == Some(&b'*') => {
                i += 2;
                while i + 1 < end && !(bytes[i] == b'*' && bytes[i + 1] == b'/') {
                    i += 1;
                }
                i += 1;
            }
            b' ' | b'\t' | b'\r' | b'\n' => {}
            _ => last = Some(i),
        }
        i += 1;
    }
    last
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn insert_preserves_comments_and_existing_entries() {
        let text = "{\n  // curated by hand\n  \"mods\": {\n    \"jei\": \"*\" // ui\n  }\n}\n";
        let out = insert_mod_text(text, "sodium", "\"^0.5.8\"").unwrap();
        assert!(out.contains("// curated by hand"), "{out}");
        assert!(out.contains("// ui"), "{out}");
        assert!(out.contains("\"sodium\": \"^0.5.8\""));
        // Still valid JSONC with both mods.
        let manifest = Manifest::parse(&format!(
            "{{\"pack\":{{\"name\":\"t\",\"author\":\"a\",\"version\":\"0\"}},\
             \"loader\":{{\"name\":\"fabric\",\"minecraft\":\"1.20.1\",\"version\":\"0\"}},{}",
            &out[out.find("\"mods\"").unwrap()..]
        ))
        .unwrap();
        assert_eq!(manifest.mods.len(), 2);
    }

    #[test]
    fn insert_into_empty_mods() {
        let out = insert_mod_text("{ \"mods\": {} }", "sodium", "\"*\"").unwrap();
        assert!(out.contains("\"sodium\": \"*\""));
    }

    #[test]
    fn spliced_add_reparses_identically_to_the_in_memory_manifest() {
        // `lode add` hashes the in-memory manifest (mod appended to the `IndexMap`) into the lock,
        // but writes the *spliced* text to disk. If the splice put the new mod anywhere but where
        // the append does, the reparsed disk manifest would serialize differently, the lock's
        // manifest_hash would not match, and the very next `install` would needlessly re-resolve.
        // This pins the invariant that both paths produce a canonically identical manifest.
        let base = "{\n  \"pack\": {\"name\": \"t\", \"author\": \"a\", \"version\": \"0\"},\n  \
                    \"loader\": {\"name\": \"forge\", \"minecraft\": \"1.20.1\", \"version\": \"47.2.0\"},\n  \
                    // hand-written note, must survive\n  \
                    \"mods\": {\n    \"jei\": \"*\" // ui\n  }\n}\n";
        let slug = "sodium";
        let spec = ModSpec::Constraint("^0.5.8".to_string());
        let spec_json = serde_json::to_string(&spec).unwrap();

        let mut in_memory = Manifest::parse(base).unwrap();
        in_memory.mods.insert(slug.to_string(), spec);

        let spliced = insert_mod_text(base, slug, &spec_json).unwrap();
        assert!(
            spliced.contains("hand-written note"),
            "comment dropped: {spliced}"
        );
        assert!(
            spliced.contains("// ui"),
            "inline comment dropped: {spliced}"
        );
        let from_disk = Manifest::parse(&spliced).unwrap();

        assert_eq!(from_disk.to_json().unwrap(), in_memory.to_json().unwrap());
    }

    #[test]
    fn rejects_deeply_nested_input_as_a_normal_error() {
        // A stack-overflow attempt via deep nesting is refused before the recursive parser runs.
        let deep = format!("{}{}", "[".repeat(500), "]".repeat(500));
        let err = Manifest::parse(&deep).unwrap_err();
        assert!(format!("{err:#}").contains("nests deeper"), "{err:#}");
    }

    #[test]
    fn depth_scan_ignores_brackets_in_strings_and_comments() {
        // Brackets inside a string or a comment must not count toward nesting depth.
        assert!(!exceeds_max_depth("\"[[[[[[[[[[\"", 2));
        assert!(!exceeds_max_depth("// [[[[[[[[[[\n{}", 2));
        assert!(!exceeds_max_depth("/* [[[[[[[[[[ */ {}", 2));
        assert!(exceeds_max_depth("[[[", 2));
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
