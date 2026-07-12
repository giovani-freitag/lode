pub mod curseforge;
pub mod modrinth;

use crate::lock::DepKind;

/// A version resolved from a provider, normalized across providers so the resolver and lock
/// don't care which backend produced it.
#[derive(Debug, Clone)]
pub struct ResolvedVersion {
    /// Provider project id (Modrinth base62, CurseForge numeric-as-string).
    pub project_id: String,
    /// Stable human slug for this project (the manifest/lock key and `.pw.toml` filename).
    pub slug: String,
    /// Display name of the project.
    pub project_name: String,
    /// Provider file/version id identifying this exact file.
    pub file_id: String,
    /// Human version string (e.g. "0.5.8").
    pub version: String,
    pub filename: String,
    /// Download URL, or None when the provider forbids persisting it (CurseForge).
    pub url: Option<String>,
    pub hash_format: String,
    pub hash: String,
    pub size: Option<u64>,
    /// Environment support as declared by the provider, pre-manifest-override.
    pub side: crate::side::Side,
    pub dependencies: Vec<ResolvedDep>,
}

/// A dependency edge as declared by a provider version.
#[derive(Debug, Clone)]
pub struct ResolvedDep {
    /// Provider project id of the dependency.
    pub project_id: String,
    pub kind: DepKind,
}

/// A version looked up by its id — the metadata `lode import` recovers from a provider that a
/// packwiz `.pw.toml` doesn't record (the human version number, file size, dependency edges).
#[derive(Debug, Clone)]
pub struct ImportedVersion {
    pub version_number: String,
    pub size: Option<u64>,
    pub dependencies: Vec<ResolvedDep>,
}
