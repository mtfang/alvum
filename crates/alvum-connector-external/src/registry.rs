use alvum_core::config::AlvumConfig;
use alvum_core::extension::{
    ExtensionManifest, ExtensionPackageRecord, ExtensionRegistry, MANIFEST_FILE,
};
use anyhow::{Context, Result, bail};
use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Debug, Clone)]
pub enum ExtensionInstallSource {
    Local(PathBuf),
    Git(String),
    Npm(String),
}

impl ExtensionInstallSource {
    pub fn parse(value: &str) -> Self {
        if let Some(rest) = value.strip_prefix("git:") {
            Self::Git(rest.to_string())
        } else if let Some(rest) = value.strip_prefix("npm:") {
            Self::Npm(rest.to_string())
        } else {
            Self::Local(PathBuf::from(value))
        }
    }

    fn label(&self) -> String {
        match self {
            Self::Local(path) => path.display().to_string(),
            Self::Git(value) => format!("git:{value}"),
            Self::Npm(value) => format!("npm:{value}"),
        }
    }
}

#[derive(Debug, Clone)]
pub struct ExtensionRegistryStore {
    pub root: PathBuf,
}

impl ExtensionRegistryStore {
    pub fn default() -> Self {
        let root = dirs_home()
            .join(".alvum")
            .join("runtime")
            .join("extensions");
        Self { root }
    }

    pub fn new(root: PathBuf) -> Self {
        Self { root }
    }

    pub fn registry_path(&self) -> PathBuf {
        self.root.join("registry.json")
    }

    pub fn load(&self) -> Result<ExtensionRegistry> {
        let path = self.registry_path();
        if !path.exists() {
            return Ok(ExtensionRegistry::default());
        }
        let raw = std::fs::read_to_string(&path)
            .with_context(|| format!("failed to read {}", path.display()))?;
        match serde_json::from_str(&raw) {
            Ok(registry) => Ok(registry),
            Err(e) => {
                let quarantine = self.root.join(format!(
                    "registry.corrupt-{}.json",
                    chrono::Utc::now().timestamp_nanos_opt().unwrap_or_default()
                ));
                std::fs::rename(&path, &quarantine).with_context(|| {
                    format!(
                        "failed to quarantine corrupt registry {} after parse error: {e}",
                        path.display()
                    )
                })?;
                Ok(ExtensionRegistry::default())
            }
        }
    }

    pub fn save(&self, registry: &ExtensionRegistry) -> Result<()> {
        std::fs::create_dir_all(&self.root)?;
        let bytes = serde_json::to_vec_pretty(registry)?;
        std::fs::write(self.registry_path(), bytes)?;
        Ok(())
    }

    pub fn install(&self, source: ExtensionInstallSource) -> Result<ExtensionPackageRecord> {
        std::fs::create_dir_all(&self.root)?;
        let staging = tempfile_dir(&self.root)?;
        let source_label = source.label();
        let preserve_node_modules = matches!(&source, ExtensionInstallSource::Npm(_));
        match &source {
            ExtensionInstallSource::Local(path) => copy_dir(path, &staging)?,
            ExtensionInstallSource::Git(url) => {
                run(Command::new("git")
                    .arg("clone")
                    .arg("--depth")
                    .arg("1")
                    .arg(url)
                    .arg(&staging))?;
            }
            ExtensionInstallSource::Npm(package) => {
                std::fs::create_dir_all(&staging)?;
                run(Command::new("npm")
                    .arg("install")
                    .arg("--ignore-scripts")
                    .arg("--prefix")
                    .arg(&staging)
                    .arg(package))?;
            }
        }

        let manifest_path = find_manifest(&staging)?;
        let manifest = ExtensionManifest::from_json_str(&std::fs::read_to_string(&manifest_path)?)?;
        let package_dir = self.root.join(&manifest.id);
        if package_dir.exists() {
            std::fs::remove_dir_all(&package_dir)?;
        }
        let manifest_parent = manifest_path
            .parent()
            .context("manifest path has no parent")?;
        copy_dir(manifest_parent, &package_dir)?;
        if preserve_node_modules {
            copy_npm_node_modules(&staging, &package_dir)?;
        }
        let final_manifest_path = package_dir.join(MANIFEST_FILE);

        let mut registry = self.load()?;
        let record = ExtensionPackageRecord {
            id: manifest.id.clone(),
            manifest_path: final_manifest_path,
            package_dir,
            enabled: false,
            install_source: Some(source_label),
        };
        registry.packages.insert(record.id.clone(), record.clone());
        self.save(&registry)?;
        let _ = std::fs::remove_dir_all(&staging);
        Ok(record)
    }

    pub fn remove(&self, id: &str) -> Result<()> {
        let mut registry = self.load()?;
        let Some(record) = registry.packages.remove(id) else {
            bail!("extension package not installed: {id}");
        };
        if record.package_dir.exists() {
            std::fs::remove_dir_all(record.package_dir)?;
        }
        self.save(&registry)
    }

    pub fn set_enabled(&self, id: &str, enabled: bool) -> Result<ExtensionPackageRecord> {
        let mut registry = self.load()?;
        let record = registry
            .packages
            .get_mut(id)
            .with_context(|| format!("extension package not installed: {id}"))?;
        record.enabled = enabled;
        let out = record.clone();
        self.save(&registry)?;
        Ok(out)
    }

    pub fn load_manifest(record: &ExtensionPackageRecord) -> Result<ExtensionManifest> {
        ExtensionManifest::from_json_str(&std::fs::read_to_string(&record.manifest_path)?)
    }

    pub fn load_package(&self, id: &str) -> Result<(ExtensionPackageRecord, ExtensionManifest)> {
        let registry = self.load()?;
        let record = registry
            .packages
            .get(id)
            .with_context(|| format!("extension package not installed: {id}"))?
            .clone();
        let manifest = Self::load_manifest(&record)?;
        Ok((record, manifest))
    }

    pub fn load_component_package(
        &self,
        component_id: &str,
    ) -> Result<(ExtensionPackageRecord, ExtensionManifest)> {
        let package_id = component_id
            .split_once('/')
            .map(|(package, _)| package)
            .with_context(|| {
                format!(
                    "component id must be fully qualified like package/component: {component_id}"
                )
            })?;
        let (record, manifest) = self.load_package(package_id)?;
        if !record.enabled {
            bail!("extension package is disabled: {package_id}");
        }
        Ok((record, manifest))
    }

    pub fn configured_external_records(
        &self,
        config: &AlvumConfig,
    ) -> Result<Vec<(String, ExtensionPackageRecord, ExtensionManifest, String)>> {
        let registry = self.load()?;
        let mut out = Vec::new();
        for (config_name, connector_cfg) in &config.connectors {
            if connector_cfg.settings.get("kind").and_then(|v| v.as_str()) != Some("external-http")
            {
                continue;
            }
            let package_id = connector_cfg
                .settings
                .get("package")
                .and_then(|v| v.as_str())
                .unwrap_or(config_name);
            let record = registry
                .packages
                .get(package_id)
                .with_context(|| format!("extension package not installed: {package_id}"))?;
            if !record.enabled || !connector_cfg.enabled {
                continue;
            }
            let manifest = Self::load_manifest(record)?;
            let connector_id = connector_cfg
                .settings
                .get("connector")
                .and_then(|v| v.as_str())
                .map(str::to_string)
                .or_else(|| manifest.connectors.first().map(|c| c.id.clone()))
                .with_context(|| format!("extension package {package_id} has no connector"))?;
            out.push((config_name.clone(), record.clone(), manifest, connector_id));
        }
        Ok(out)
    }
}

fn dirs_home() -> PathBuf {
    dirs::home_dir().unwrap_or_else(|| PathBuf::from("."))
}

fn tempfile_dir(root: &Path) -> Result<PathBuf> {
    let path = root.join(format!(
        ".install-{}-{}",
        std::process::id(),
        chrono::Utc::now().timestamp_nanos_opt().unwrap_or_default()
    ));
    if path.exists() {
        std::fs::remove_dir_all(&path)?;
    }
    std::fs::create_dir_all(&path)?;
    Ok(path)
}

fn find_manifest(dir: &Path) -> Result<PathBuf> {
    let direct = dir.join(MANIFEST_FILE);
    if direct.exists() {
        return Ok(direct);
    }
    let node_modules = dir.join("node_modules");
    if node_modules.exists() {
        for entry in std::fs::read_dir(node_modules)? {
            let entry = entry?;
            let path = entry.path();
            let candidate = path.join(MANIFEST_FILE);
            if candidate.exists() {
                return Ok(candidate);
            }
            if path
                .file_name()
                .and_then(|n| n.to_str())
                .map(|name| name.starts_with('@'))
                .unwrap_or(false)
            {
                for scoped in std::fs::read_dir(path)? {
                    let scoped = scoped?;
                    let candidate = scoped.path().join(MANIFEST_FILE);
                    if candidate.exists() {
                        return Ok(candidate);
                    }
                }
            }
        }
    }
    bail!("could not find {MANIFEST_FILE} under {}", dir.display())
}

fn copy_dir(from: &Path, to: &Path) -> Result<()> {
    if !from.exists() {
        bail!("source directory does not exist: {}", from.display());
    }
    if to.exists() {
        std::fs::remove_dir_all(to)?;
    }
    std::fs::create_dir_all(to)?;
    for entry in std::fs::read_dir(from)? {
        let entry = entry?;
        let src = entry.path();
        let dst = to.join(entry.file_name());
        if src.is_dir() {
            copy_dir(&src, &dst)?;
        } else {
            std::fs::copy(&src, &dst)?;
        }
    }
    Ok(())
}

fn copy_npm_node_modules(staging: &Path, package_dir: &Path) -> Result<()> {
    let node_modules = staging.join("node_modules");
    if node_modules.exists() {
        copy_dir(&node_modules, &package_dir.join("node_modules"))?;
    }
    Ok(())
}

fn run(command: &mut Command) -> Result<()> {
    let output = command
        .output()
        .context("failed to run extension package command")?;
    if !output.status.success() {
        bail!(
            "extension package command failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_manifest(dir: &Path, id: &str) {
        let manifest = serde_json::json!({
            "schema_version": 1,
            "id": id,
            "name": "Fixture",
            "version": "0.1.0",
            "server": {"start": ["node", "server.js"]},
            "connectors": [{"id": "main", "display_name": "Main"}]
        });
        std::fs::write(
            dir.join(MANIFEST_FILE),
            serde_json::to_vec_pretty(&manifest).unwrap(),
        )
        .unwrap();
    }

    #[test]
    fn local_install_validates_and_registers_package_disabled() {
        let tmp = tempfile::tempdir().unwrap();
        let source = tmp.path().join("source");
        std::fs::create_dir_all(&source).unwrap();
        write_manifest(&source, "fixture");
        std::fs::write(source.join("server.js"), "console.log('ok')").unwrap();
        let store = ExtensionRegistryStore::new(tmp.path().join("extensions"));

        let record = store
            .install(ExtensionInstallSource::Local(source))
            .unwrap();

        assert_eq!(record.id, "fixture");
        assert!(!record.enabled);
        assert!(record.manifest_path.exists());
        assert!(store.load().unwrap().packages.contains_key("fixture"));
    }

    #[test]
    fn set_enabled_updates_registry_record() {
        let tmp = tempfile::tempdir().unwrap();
        let source = tmp.path().join("source");
        std::fs::create_dir_all(&source).unwrap();
        write_manifest(&source, "fixture");
        let store = ExtensionRegistryStore::new(tmp.path().join("extensions"));
        store
            .install(ExtensionInstallSource::Local(source))
            .unwrap();

        let record = store.set_enabled("fixture", true).unwrap();

        assert!(record.enabled);
        assert!(store.load().unwrap().packages["fixture"].enabled);
    }

    #[test]
    fn copy_npm_node_modules_preserves_hoisted_dependencies() {
        let tmp = tempfile::tempdir().unwrap();
        let staging = tmp.path().join("staging");
        let dependency = staging.join("node_modules").join("dep");
        std::fs::create_dir_all(&dependency).unwrap();
        std::fs::write(dependency.join("package.json"), "{}").unwrap();
        let package_dir = tmp.path().join("package");
        std::fs::create_dir_all(&package_dir).unwrap();

        copy_npm_node_modules(&staging, &package_dir).unwrap();

        assert!(package_dir.join("node_modules/dep/package.json").exists());
    }

    #[test]
    fn load_quarantines_corrupt_registry_and_recovers_empty() {
        let tmp = tempfile::tempdir().unwrap();
        let store = ExtensionRegistryStore::new(tmp.path().join("extensions"));
        std::fs::create_dir_all(&store.root).unwrap();
        std::fs::write(store.registry_path(), "{not json").unwrap();

        let registry = store.load().unwrap();

        assert!(registry.packages.is_empty());
        assert!(!store.registry_path().exists());
        assert!(std::fs::read_dir(&store.root).unwrap().any(|entry| {
            entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with("registry.corrupt-")
        }));
    }
}
