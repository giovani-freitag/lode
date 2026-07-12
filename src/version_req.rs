use semver::{Version, VersionReq};

/// Does a provider's version string satisfy a manifest constraint?
///
/// - `"*"` matches anything (latest).
/// - a plain version with no range operator (`"0.5.8"`) is an exact string match.
/// - a range (`"^0.5.8"`, `"~1.2"`, `">=1, <2"`) is a semver requirement, applied to a
///   best-effort parse of the version string.
///
/// Mod versions are not always clean semver (e.g. `mc1.20.1-0.5.13-fabric`). Exact and `*`
/// always work; ranges work for semver-ish versions and are best-effort otherwise.
pub fn matches(version: &str, constraint: &str) -> bool {
    let constraint = constraint.trim();
    if constraint == "*" {
        return true;
    }
    if !is_range(constraint) {
        return version == constraint;
    }
    match (VersionReq::parse(constraint), parse_lenient(version)) {
        (Ok(req), Some(parsed)) => req.matches(&parsed),
        _ => false,
    }
}

/// Whether a constraint uses range syntax (vs. being a bare exact version).
pub(crate) fn is_range(constraint: &str) -> bool {
    constraint.starts_with(['^', '~', '>', '<', '=']) || constraint.contains(',')
}

/// Parse a version leniently: try strict semver first (handles `1.0.36+mc1.20.1`), then fall back
/// to the first `MAJOR.MINOR[.PATCH]` run found in the string.
fn parse_lenient(version: &str) -> Option<Version> {
    if let Ok(parsed) = Version::parse(version) {
        return Some(parsed);
    }
    let bytes = version.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if !bytes[i].is_ascii_digit() {
            i += 1;
            continue;
        }
        let start = i;
        while i < bytes.len() && (bytes[i].is_ascii_digit() || bytes[i] == b'.') {
            i += 1;
        }
        let parts: Vec<&str> = version[start..i]
            .split('.')
            .filter(|s| !s.is_empty())
            .collect();
        if parts.len() >= 2 {
            let core = format!(
                "{}.{}.{}",
                parts[0],
                parts[1],
                parts.get(2).copied().unwrap_or("0")
            );
            if let Ok(parsed) = Version::parse(&core) {
                return Some(parsed);
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn star_matches_anything() {
        assert!(matches("mc1.20.1-0.5.13-fabric", "*"));
    }

    #[test]
    fn plain_version_is_exact() {
        assert!(matches("0.5.8", "0.5.8"));
        assert!(!matches("0.5.9", "0.5.8"));
    }

    #[test]
    fn caret_and_tilde_ranges() {
        assert!(matches("0.5.13", "^0.5.8"));
        assert!(!matches("0.6.0", "~0.5.8"));
        assert!(matches("0.5.20", "~0.5.0"));
    }

    #[test]
    fn ranges_apply_to_semver_with_build_metadata() {
        assert!(matches("1.0.36+mc1.20.1", "^1.0.0"));
    }
}
