use anyhow::Result;

use crate::config_doc;

pub(crate) fn init() -> Result<()> {
    let path = alvum_core::config::config_path();
    if path.exists() {
        println!("Config already exists: {}", path.display());
        println!("Edit it directly or delete it to re-initialize.");
        return Ok(());
    }
    let config = alvum_core::config::AlvumConfig::default();
    config.save()?;
    println!("Created default config: {}", path.display());
    Ok(())
}

pub(crate) fn show() -> Result<()> {
    let config = alvum_core::config::AlvumConfig::load()?;
    let toml_str = toml::to_string_pretty(&config)?;
    println!("{toml_str}");
    Ok(())
}

pub(crate) fn set(key: &str, value: &str) -> Result<()> {
    let (config_path, mut doc) = config_doc::load()?;
    let toml_value = config_doc::parse_value(value);
    config_doc::set_value(&mut doc, key, toml_value.clone())?;
    config_doc::save(&config_path, &doc)?;

    println!("{key} = {toml_value}");
    println!("Saved to {}", config_path.display());
    Ok(())
}
