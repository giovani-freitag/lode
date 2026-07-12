use anyhow::{bail, Result};

use crate::cli::{ConfigAction, ConfigArgs};
use crate::config;

pub fn run(args: ConfigArgs) -> Result<()> {
    match args.action {
        ConfigAction::Set { key, value } => set(&key, &value),
        ConfigAction::Get { key } => get(&key),
    }
}

fn set(key: &str, value: &str) -> Result<()> {
    let mut cfg = config::load()?;
    match key {
        "curseforge.key" => {
            cfg.curseforge.get_or_insert_with(Default::default).key = Some(value.to_string());
        }
        other => bail!("unknown config key '{other}' (known: curseforge.key)"),
    }
    config::save(&cfg)?;
    println!("Set {key} in {}", config::config_path()?.display());
    Ok(())
}

fn get(key: &str) -> Result<()> {
    let cfg = config::load()?;
    let value = match key {
        "curseforge.key" => cfg.curseforge.and_then(|c| c.key),
        other => bail!("unknown config key '{other}' (known: curseforge.key)"),
    };
    match value {
        Some(v) => println!("{v}"),
        None => println!("(unset)"),
    }
    Ok(())
}
