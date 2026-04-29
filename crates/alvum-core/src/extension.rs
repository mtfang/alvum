//! External extension manifest and registry types.
//!
//! Extension packages can provide capture, processor, analysis, and connector
//! components. The runtime loads these manifests from installed package
//! directories and adapts them into the in-process Connector pipeline.

use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::PathBuf;

pub const MANIFEST_FILE: &str = "alvum.extension.json";
pub const MANIFEST_SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ExtensionManifest {
    pub schema_version: u32,
    pub id: String,
    pub name: String,
    pub version: String,
    #[serde(default)]
    pub description: String,
    pub server: ExtensionServer,
    #[serde(default)]
    pub captures: Vec<CaptureComponent>,
    #[serde(default)]
    pub processors: Vec<ProcessorComponent>,
    #[serde(default)]
    pub analyses: Vec<AnalysisComponent>,
    #[serde(default)]
    pub connectors: Vec<ConnectorComponent>,
    #[serde(default)]
    pub permissions: Vec<PermissionDescriptor>,
}

impl ExtensionManifest {
    pub fn from_json_str(value: &str) -> Result<Self> {
        let manifest: Self = serde_json::from_str(value)?;
        manifest.validate()?;
        Ok(manifest)
    }

    pub fn validate(&self) -> Result<()> {
        if self.schema_version != MANIFEST_SCHEMA_VERSION {
            bail!(
                "unsupported extension manifest schema_version {}; expected {}",
                self.schema_version,
                MANIFEST_SCHEMA_VERSION
            );
        }
        validate_local_id("package id", &self.id)?;
        if self.server.start.is_empty() {
            bail!("server.start must contain at least one command token");
        }
        for capture in &self.captures {
            validate_local_id("capture id", &capture.id)?;
        }
        for processor in &self.processors {
            validate_local_id("processor id", &processor.id)?;
        }
        for analysis in &self.analyses {
            validate_local_id("analysis id", &analysis.id)?;
        }
        for connector in &self.connectors {
            validate_local_id("connector id", &connector.id)?;
            for route in &connector.routes {
                validate_component_id("route from component", &route.from.component)?;
                if route.to.is_empty() {
                    bail!(
                        "connector {} route from {} has no processors",
                        connector.id,
                        route.from.component
                    );
                }
                for processor in &route.to {
                    validate_component_id("route processor component", processor)?;
                }
            }
            for analysis in &connector.analyses {
                validate_component_id("connector analysis component", analysis)?;
            }
        }
        Ok(())
    }

    pub fn component_id(&self, local_id: &str) -> String {
        format!("{}/{}", self.id, local_id)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ExtensionServer {
    pub start: Vec<String>,
    #[serde(default = "default_health_path")]
    pub health_path: String,
    #[serde(default = "default_startup_timeout_ms")]
    pub startup_timeout_ms: u64,
}

fn default_health_path() -> String {
    "/v1/health".into()
}

fn default_startup_timeout_ms() -> u64 {
    5000
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CaptureComponent {
    pub id: String,
    pub display_name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub sources: Vec<SourceDescriptor>,
    #[serde(default)]
    pub schemas: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProcessorComponent {
    pub id: String,
    pub display_name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub accepts: Vec<RouteSelector>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AnalysisComponent {
    pub id: String,
    pub display_name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub scopes: Vec<DataScope>,
    #[serde(default)]
    pub output: AnalysisOutput,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum AnalysisOutput {
    #[default]
    Artifact,
    GraphOverlay,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ConnectorComponent {
    pub id: String,
    pub display_name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub routes: Vec<RouteDescriptor>,
    #[serde(default)]
    pub analyses: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RouteDescriptor {
    pub from: RouteSelector,
    pub to: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RouteSelector {
    pub component: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mime: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub schema: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SourceDescriptor {
    pub id: String,
    pub display_name: String,
    #[serde(default)]
    pub expected: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PermissionDescriptor {
    pub kind: String,
    pub description: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DataScope {
    Capture,
    Observations,
    Threads,
    Decisions,
    Edges,
    Briefing,
    Knowledge,
    RawFiles,
    All,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
pub struct ExtensionRegistry {
    #[serde(default)]
    pub packages: BTreeMap<String, ExtensionPackageRecord>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ExtensionPackageRecord {
    pub id: String,
    pub manifest_path: PathBuf,
    pub package_dir: PathBuf,
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub install_source: Option<String>,
}

pub fn validate_component_id(label: &str, value: &str) -> Result<()> {
    let Some((package, component)) = value.split_once('/') else {
        bail!("{label} must be a fully qualified component id like package/component");
    };
    validate_local_id(label, package)?;
    validate_local_id(label, component)?;
    Ok(())
}

fn validate_local_id(label: &str, value: &str) -> Result<()> {
    if value.is_empty() {
        bail!("{label} cannot be empty");
    }
    if !value
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
    {
        bail!("{label} contains invalid characters: {value}");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_manifest_json() -> String {
        serde_json::json!({
            "schema_version": 1,
            "id": "github",
            "name": "GitHub",
            "version": "0.1.0",
            "server": {
                "start": ["node", "dist/server.js"]
            },
            "captures": [{
                "id": "events",
                "display_name": "GitHub events",
                "sources": [{"id": "github", "display_name": "GitHub", "expected": false}],
                "schemas": ["github.event.v1"]
            }],
            "processors": [{
                "id": "summarize",
                "display_name": "GitHub summarizer",
                "accepts": [{"component": "github/events", "schema": "github.event.v1"}]
            }],
            "analyses": [{
                "id": "weekly-review",
                "display_name": "Weekly review",
                "scopes": ["all"],
                "output": "artifact"
            }],
            "connectors": [{
                "id": "activity",
                "display_name": "GitHub activity",
                "routes": [{
                    "from": {"component": "github/events", "schema": "github.event.v1"},
                    "to": ["github/summarize"]
                }],
                "analyses": ["github/weekly-review"]
            }],
            "permissions": [{"kind": "network", "description": "Connects to api.github.com"}]
        })
        .to_string()
    }

    #[test]
    fn parses_valid_manifest_with_capture_processor_analysis_and_connector() {
        let manifest = ExtensionManifest::from_json_str(&sample_manifest_json()).unwrap();

        assert_eq!(manifest.id, "github");
        assert_eq!(manifest.server.health_path, "/v1/health");
        assert_eq!(manifest.server.startup_timeout_ms, 5000);
        assert_eq!(
            manifest.connectors[0].routes[0].to,
            vec!["github/summarize"]
        );
        assert_eq!(manifest.analyses[0].scopes, vec![DataScope::All]);
    }

    #[test]
    fn rejects_routes_without_processors() {
        let mut manifest = ExtensionManifest::from_json_str(&sample_manifest_json()).unwrap();
        manifest.connectors[0].routes[0].to.clear();

        let err = manifest.validate().unwrap_err().to_string();

        assert!(err.contains("has no processors"));
    }

    #[test]
    fn rejects_unqualified_component_ids() {
        let mut manifest = ExtensionManifest::from_json_str(&sample_manifest_json()).unwrap();
        manifest.connectors[0].routes[0].from.component = "events".into();

        let err = manifest.validate().unwrap_err().to_string();

        assert!(err.contains("fully qualified"));
    }

    #[test]
    fn builtin_catalog_exposes_core_capture_and_route_components() {
        let manifests = crate::builtin_components::manifests();
        let ids: Vec<&str> = manifests
            .iter()
            .map(|manifest| manifest.id.as_str())
            .collect();

        assert_eq!(ids, vec!["alvum.audio", "alvum.screen", "alvum.session"]);
        for manifest in &manifests {
            manifest.validate().unwrap();
            assert_eq!(manifest.server.start, vec!["builtin"]);
        }
        assert!(crate::builtin_components::capture_component("alvum.audio/audio-mic").is_some());
        assert!(crate::builtin_components::capture_component("alvum.screen/snapshot").is_some());
        assert!(crate::builtin_components::capture_component("alvum.session/codex").is_some());
        assert!(crate::builtin_components::capture_component("fixture/events").is_none());
    }
}
