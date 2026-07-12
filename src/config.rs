use std::fs;
use std::path::PathBuf;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

/// lode's user-level config (`<config-dir>/lode/config.toml`) — where secrets like the CurseForge
/// API key live so they aren't retyped every command, the way npm and composer keep credentials.
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct Config {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub curseforge: Option<CurseforgeConfig>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct CurseforgeConfig {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key: Option<String>,
}

pub fn config_path() -> Result<PathBuf> {
    let dir = dirs::config_dir().context("could not determine the user config directory")?;
    Ok(dir.join("lode").join("config.toml"))
}

pub fn load() -> Result<Config> {
    let path = config_path()?;
    if !path.exists() {
        return Ok(Config::default());
    }
    let text = fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
    toml::from_str(&text).with_context(|| format!("parsing {}", path.display()))
}

pub fn save(config: &Config) -> Result<()> {
    let path = config_path()?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;
    }
    let text = toml::to_string_pretty(config).context("serializing config")?;
    fs::write(&path, text).with_context(|| format!("writing {}", path.display()))?;
    Ok(())
}

/// The CurseForge API key, from the `CF_API_KEY` environment variable first (CI-friendly), then
/// the stored config. `None` if neither is set.
pub fn curseforge_key() -> Option<String> {
    if let Ok(key) = std::env::var("CF_API_KEY") {
        if !key.is_empty() {
            return Some(key);
        }
    }
    load().ok().and_then(|c| c.curseforge).and_then(|cf| cf.key)
}
