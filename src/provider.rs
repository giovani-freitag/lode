use serde::{Deserialize, Serialize};

/// Where a mod's file comes from. The pilot resolves Modrinth; CurseForge/URL/GitHub are
/// modeled here so the lock and manifest schemas are stable before those resolvers land.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Provider {
    Modrinth,
    Curseforge,
    Url,
    Github,
    /// A jar the maintainer supplies locally (private/dev mods), bundled under `local/` in the pack.
    Local,
}

/// How packwiz downloads a file. Modrinth/direct files carry a URL; CurseForge files carry no
/// URL (its API terms forbid persisting it) and are re-resolved from ids at install time.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum DownloadMode {
    #[default]
    #[serde(rename = "url")]
    Url,
    #[serde(rename = "metadata:curseforge")]
    MetadataCurseforge,
}
