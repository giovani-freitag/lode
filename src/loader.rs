use serde::{Deserialize, Serialize};

/// The mod loader a pack targets. A pack targets exactly one: the loaders are distinct runtimes
/// whose mods are not interchangeable.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, clap::ValueEnum)]
#[serde(rename_all = "lowercase")]
pub enum Loader {
    Forge,
    Neoforge,
    Fabric,
    Quilt,
}

impl Loader {
    /// The value Modrinth expects in a `loaders` search/version facet.
    pub fn modrinth_facet(self) -> &'static str {
        match self {
            Loader::Forge => "forge",
            Loader::Neoforge => "neoforge",
            Loader::Fabric => "fabric",
            Loader::Quilt => "quilt",
        }
    }

    /// The key packwiz uses for this loader's version inside `pack.toml [versions]`.
    pub fn packwiz_version_key(self) -> &'static str {
        self.modrinth_facet()
    }

    /// Loaders whose mods this pack can also consume, most-preferred first. Quilt runs most
    /// Fabric mods; NeoForge shares lineage with Forge. Used to widen provider queries without
    /// pretending the loaders are identical.
    pub fn compatible_facets(self) -> &'static [&'static str] {
        match self {
            Loader::Forge => &["forge"],
            Loader::Neoforge => &["neoforge"],
            Loader::Fabric => &["fabric"],
            Loader::Quilt => &["quilt", "fabric"],
        }
    }
}
