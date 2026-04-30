use anyhow::{Context, Result, bail};
use std::path::{Path, PathBuf};

pub(crate) fn default_table() -> Result<toml::Table> {
    Ok(toml::to_string(&alvum_core::config::AlvumConfig::default())?.parse()?)
}

pub(crate) fn load_table() -> Result<toml::Table> {
    let config_path = alvum_core::config::config_path();
    if config_path.exists() {
        Ok(std::fs::read_to_string(&config_path)?.parse()?)
    } else {
        default_table()
    }
}

pub(crate) fn write_table(doc: &toml::Table) -> Result<()> {
    let config_path = alvum_core::config::config_path();
    save(&config_path, doc)
}

pub(crate) fn load() -> Result<(PathBuf, toml::Table)> {
    let config_path = alvum_core::config::config_path();
    let doc: toml::Table = if config_path.exists() {
        let content = std::fs::read_to_string(&config_path)?;
        content.parse().context("failed to parse config")?
    } else {
        let config = alvum_core::config::AlvumConfig::default();
        let toml_str = toml::to_string(&config)?;
        toml_str.parse().context("failed to serialize defaults")?
    };
    Ok((config_path, doc))
}

pub(crate) fn set_value(doc: &mut toml::Table, key: &str, value: toml::Value) -> Result<()> {
    let parts: Vec<&str> = key.split('.').collect();
    if parts.len() < 2 {
        bail!("key must be dotted path (e.g., capture.screen.enabled)");
    }

    let mut current = doc;
    for part in &parts[..parts.len() - 1] {
        current = current
            .entry(part.to_string())
            .or_insert_with(|| toml::Value::Table(toml::Table::new()))
            .as_table_mut()
            .with_context(|| format!("{part} is not a table"))?;
    }

    let leaf = parts.last().unwrap();
    current.insert(leaf.to_string(), value);
    Ok(())
}

pub(crate) fn save(config_path: &Path, doc: &toml::Table) -> Result<()> {
    if let Some(parent) = config_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(config_path, toml::to_string_pretty(doc)?)?;
    Ok(())
}

pub(crate) fn parse_value(value: &str) -> toml::Value {
    if value == "true" {
        toml::Value::Boolean(true)
    } else if value == "false" {
        toml::Value::Boolean(false)
    } else if let Ok(n) = value.parse::<i64>() {
        toml::Value::Integer(n)
    } else if let Ok(f) = value.parse::<f64>() {
        toml::Value::Float(f)
    } else {
        toml::Value::String(value.to_string())
    }
}
