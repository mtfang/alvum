use crate::client::{
    CaptureStartRequest, CaptureStopRequest, GatherRequest, GatherResponse, ManagedExtension,
    ProcessRequest,
};
use crate::registry::ExtensionRegistryStore;
use alvum_core::builtin_components;
use alvum_core::capture::CaptureSource;
use alvum_core::config::AlvumConfig;
use alvum_core::connector::Connector;
use alvum_core::data_ref::DataRef;
use alvum_core::extension::{
    ConnectorComponent, ExtensionManifest, ExtensionPackageRecord, RouteSelector,
};
use alvum_core::observation::Observation;
use alvum_core::pipeline_events::{self as events, Event};
use alvum_core::processor::Processor;
use anyhow::{Context, Result};
use async_trait::async_trait;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use tokio::sync::watch;

pub fn connectors_from_config(config: &AlvumConfig) -> Result<Vec<Box<dyn Connector>>> {
    let store = ExtensionRegistryStore::default();
    let mut connectors: Vec<Box<dyn Connector>> = Vec::new();
    for (config_name, record, manifest, connector_id) in
        store.configured_external_records(config)?
    {
        connectors.push(Box::new(ExternalConnector::new(
            config_name,
            record,
            manifest,
            connector_id,
            store.clone(),
        )?));
    }
    Ok(connectors)
}

pub fn capture_sources_from_config(config: &AlvumConfig) -> Result<Vec<Box<dyn CaptureSource>>> {
    let store = ExtensionRegistryStore::default();
    let mut sources: Vec<Box<dyn CaptureSource>> = Vec::new();
    for (_config_name, _record, manifest, connector_id) in
        store.configured_external_records(config)?
    {
        let connector = find_connector(&manifest, &connector_id)?;
        let capture_ids = connector
            .routes
            .iter()
            .map(|route| route.from.component.clone())
            .collect::<std::collections::BTreeSet<_>>();
        for capture in capture_ids {
            if builtin_components::capture_component(&capture).is_some() {
                continue;
            }
            let (record, manifest) = store.load_component_package(&capture)?;
            sources.push(Box::new(ExternalCaptureSource {
                record,
                manifest,
                capture,
            }));
        }
    }
    Ok(sources)
}

pub struct ExternalConnector {
    name: String,
    record: ExtensionPackageRecord,
    manifest: ExtensionManifest,
    connector: ConnectorComponent,
    store: ExtensionRegistryStore,
    cached_gather: Mutex<Option<GatherResponse>>,
}

impl ExternalConnector {
    pub fn new(
        name: String,
        record: ExtensionPackageRecord,
        manifest: ExtensionManifest,
        connector_id: String,
        store: ExtensionRegistryStore,
    ) -> Result<Self> {
        let connector = find_connector(&manifest, &connector_id)?;
        Ok(Self {
            name,
            record,
            manifest,
            connector,
            store,
            cached_gather: Mutex::new(None),
        })
    }

    fn gather_once(&self, capture_dir: &Path) -> Result<GatherResponse> {
        if let Some(cached) = self.cached_gather.lock().unwrap().clone() {
            return Ok(cached);
        }
        let managed = ManagedExtension::start(
            &self.manifest,
            &self.record.package_dir,
            &extension_log_dir(),
            None,
        )?;
        let response = managed.client().gather(&GatherRequest {
            connector: self.connector.id.clone(),
            capture_dir: capture_dir.to_path_buf(),
        })?;
        for warning in &response.warnings {
            events::emit(Event::Warning {
                source: format!("extension/{}/gather", self.manifest.id),
                message: warning.clone(),
            });
        }
        *self.cached_gather.lock().unwrap() = Some(response.clone());
        Ok(response)
    }
}

impl Connector for ExternalConnector {
    fn name(&self) -> &str {
        &self.name
    }

    fn capture_sources(&self) -> Vec<Box<dyn CaptureSource>> {
        self.connector
            .routes
            .iter()
            .map(|route| route.from.component.clone())
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
            .filter(|capture| builtin_components::capture_component(capture).is_none())
            .filter_map(|capture| {
                match self.store.load_component_package(&capture) {
                    Ok((record, manifest)) => Some(Box::new(ExternalCaptureSource {
                        record,
                        manifest,
                        capture,
                    }) as Box<dyn CaptureSource>),
                    Err(e) => {
                        tracing::warn!(component = %capture, error = %e, "failed to resolve external capture component");
                        None
                    }
                }
            })
            .collect()
    }

    fn processors(&self) -> Vec<Box<dyn Processor>> {
        self.connector
            .routes
            .iter()
            .flat_map(|route| {
                route.to.iter().filter_map(move |processor| {
                    match self.store.load_component_package(processor) {
                        Ok((record, manifest)) => Some(Box::new(ExternalProcessor {
                            record,
                            manifest,
                            processor: processor.clone(),
                            selector: route.from.clone(),
                        }) as Box<dyn Processor>),
                        Err(e) => {
                            tracing::warn!(component = %processor, error = %e, "failed to resolve external processor component");
                            None
                        }
                    }
                })
            })
            .collect()
    }

    fn gather_data_refs(&self, capture_dir: &Path) -> Result<Vec<DataRef>> {
        Ok(self.gather_once(capture_dir)?.data_refs)
    }

    fn gather_observations(&self, capture_dir: &Path) -> Result<Vec<Observation>> {
        Ok(self.gather_once(capture_dir)?.observations)
    }

    fn expected_source_names(&self) -> Vec<String> {
        let mut out = Vec::new();
        let routed_captures = self
            .connector
            .routes
            .iter()
            .map(|route| route.from.component.clone())
            .collect::<std::collections::BTreeSet<_>>();
        for capture_id in routed_captures {
            if let Some(capture) = builtin_components::capture_component(&capture_id) {
                out.extend(
                    capture
                        .sources
                        .into_iter()
                        .filter(|source| source.expected)
                        .map(|source| source.id),
                );
                continue;
            }
            let Ok((_record, manifest)) = self.store.load_component_package(&capture_id) else {
                continue;
            };
            let Some((_package, local_id)) = capture_id.split_once('/') else {
                continue;
            };
            out.extend(
                manifest
                    .captures
                    .iter()
                    .filter(|capture| capture.id == local_id)
                    .flat_map(|capture| {
                        capture
                            .sources
                            .iter()
                            .filter(|source| source.expected)
                            .map(|source| source.id.clone())
                    }),
            );
        }
        out
    }
}

pub struct ExternalProcessor {
    record: ExtensionPackageRecord,
    manifest: ExtensionManifest,
    processor: String,
    selector: RouteSelector,
}

#[async_trait]
impl Processor for ExternalProcessor {
    fn name(&self) -> &str {
        &self.processor
    }

    fn handles(&self) -> Vec<String> {
        [
            Some(self.selector.component.clone()),
            self.selector.source.clone(),
            self.selector.mime.clone(),
            self.selector.schema.clone(),
        ]
        .into_iter()
        .flatten()
        .collect()
    }

    fn accepts(&self, data_ref: &DataRef) -> bool {
        selector_matches(&self.selector, data_ref)
    }

    async fn process(&self, data_refs: &[DataRef], capture_dir: &Path) -> Result<Vec<Observation>> {
        let record = self.record.clone();
        let manifest = self.manifest.clone();
        let processor = self.processor.clone();
        let capture_dir = capture_dir.to_path_buf();
        let data_refs = data_refs.to_vec();
        tokio::task::spawn_blocking(move || {
            let managed = ManagedExtension::start(
                &manifest,
                &record.package_dir,
                &extension_log_dir(),
                None,
            )?;
            let response = managed.client().process(&ProcessRequest {
                processor,
                data_refs,
                capture_dir,
            })?;
            for warning in &response.warnings {
                events::emit(Event::Warning {
                    source: format!("extension/{}/process", manifest.id),
                    message: warning.clone(),
                });
            }
            Ok(response.observations)
        })
        .await?
    }
}

pub struct ExternalCaptureSource {
    record: ExtensionPackageRecord,
    manifest: ExtensionManifest,
    capture: String,
}

#[async_trait]
impl CaptureSource for ExternalCaptureSource {
    fn name(&self) -> &str {
        &self.capture
    }

    async fn run(&self, capture_dir: &Path, mut shutdown: watch::Receiver<bool>) -> Result<()> {
        let managed = ManagedExtension::start(
            &self.manifest,
            &self.record.package_dir,
            &extension_log_dir(),
            None,
        )?;
        let started = managed.client().capture_start(&CaptureStartRequest {
            capture: self.capture.clone(),
            capture_dir: capture_dir.to_path_buf(),
        })?;
        let _ = shutdown.changed().await;
        managed.client().capture_stop(&CaptureStopRequest {
            capture: self.capture.clone(),
            run_id: started.run_id,
        })?;
        Ok(())
    }
}

fn find_connector(manifest: &ExtensionManifest, connector_id: &str) -> Result<ConnectorComponent> {
    manifest
        .connectors
        .iter()
        .find(|connector| connector.id == connector_id)
        .cloned()
        .with_context(|| format!("connector {connector_id} not found in {}", manifest.id))
}

fn selector_matches(selector: &RouteSelector, data_ref: &DataRef) -> bool {
    component_matches(selector, data_ref)
        && selector
            .source
            .as_ref()
            .map(|source| source == &data_ref.source)
            .unwrap_or(true)
        && selector
            .mime
            .as_ref()
            .map(|mime| mime == &data_ref.mime)
            .unwrap_or(true)
        && selector
            .schema
            .as_ref()
            .map(|schema| schema == &data_ref.schema)
            .unwrap_or(true)
}

fn component_matches(selector: &RouteSelector, data_ref: &DataRef) -> bool {
    if !data_ref.producer.is_empty() {
        return selector.component == data_ref.producer;
    }
    selector
        .source
        .as_ref()
        .map(|source| source == &data_ref.source)
        .unwrap_or(false)
}

fn extension_log_dir() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".alvum")
        .join("runtime")
        .join("logs")
        .join("extensions")
}

#[cfg(test)]
mod tests {
    use super::*;
    use alvum_core::extension::{ExtensionPackageRecord, ExtensionRegistry, MANIFEST_FILE};
    use std::collections::BTreeMap;

    fn package_record(
        root: &Path,
        id: &str,
        manifest: &serde_json::Value,
    ) -> ExtensionPackageRecord {
        let package_dir = root.join(id);
        std::fs::create_dir_all(&package_dir).unwrap();
        let manifest_path = package_dir.join(MANIFEST_FILE);
        std::fs::write(&manifest_path, serde_json::to_vec_pretty(manifest).unwrap()).unwrap();
        ExtensionPackageRecord {
            id: id.into(),
            manifest_path,
            package_dir,
            enabled: true,
            install_source: None,
        }
    }

    #[test]
    fn route_selector_matches_producer_schema_mime_and_source() {
        let selector = RouteSelector {
            component: "fixture/capture".into(),
            source: Some("fixture".into()),
            mime: Some("application/json".into()),
            schema: Some("fixture.event.v1".into()),
        };
        let data_ref = DataRef::new(
            "2026-04-11T10:15:00Z".parse().unwrap(),
            "fixture",
            "event.json",
            "application/json",
        )
        .with_routing("fixture/capture", "fixture.event.v1");

        assert!(selector_matches(&selector, &data_ref));
    }

    #[test]
    fn route_selector_rejects_wrong_schema() {
        let selector = RouteSelector {
            component: "fixture/capture".into(),
            source: None,
            mime: None,
            schema: Some("fixture.event.v2".into()),
        };
        let data_ref = DataRef::new(
            "2026-04-11T10:15:00Z".parse().unwrap(),
            "fixture",
            "event.json",
            "application/json",
        )
        .with_routing("fixture/capture", "fixture.event.v1");

        assert!(!selector_matches(&selector, &data_ref));
    }

    #[test]
    fn route_selector_requires_explicit_source_for_legacy_refs() {
        let selector = RouteSelector {
            component: "fixture/capture".into(),
            source: None,
            mime: None,
            schema: None,
        };
        let legacy_ref = DataRef::new(
            "2026-04-11T10:15:00Z".parse().unwrap(),
            "fixture",
            "event.json",
            "application/json",
        );

        assert!(!selector_matches(&selector, &legacy_ref));

        let selector = RouteSelector {
            component: "fixture/capture".into(),
            source: Some("fixture".into()),
            mime: None,
            schema: None,
        };

        assert!(selector_matches(&selector, &legacy_ref));
    }

    #[test]
    fn external_connector_resolves_cross_package_components() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("extensions");
        let capture_manifest = serde_json::json!({
            "schema_version": 1,
            "id": "capture_pkg",
            "name": "Capture package",
            "version": "0.1.0",
            "server": {"start": ["node", "server.js"]},
            "captures": [{
                "id": "events",
                "display_name": "Events",
                "sources": [{"id": "events", "display_name": "Events", "expected": true}]
            }]
        });
        let processor_manifest = serde_json::json!({
            "schema_version": 1,
            "id": "processor_pkg",
            "name": "Processor package",
            "version": "0.1.0",
            "server": {"start": ["node", "server.js"]},
            "processors": [{"id": "summarize", "display_name": "Summarize"}]
        });
        let connector_manifest_json = serde_json::json!({
            "schema_version": 1,
            "id": "connector_pkg",
            "name": "Connector package",
            "version": "0.1.0",
            "server": {"start": ["node", "server.js"]},
            "connectors": [{
                "id": "main",
                "display_name": "Main",
                "routes": [{
                    "from": {"component": "capture_pkg/events"},
                    "to": ["processor_pkg/summarize"]
                }]
            }]
        });
        let capture_record = package_record(&root, "capture_pkg", &capture_manifest);
        let processor_record = package_record(&root, "processor_pkg", &processor_manifest);
        let connector_record = package_record(&root, "connector_pkg", &connector_manifest_json);
        let mut packages = BTreeMap::new();
        packages.insert(capture_record.id.clone(), capture_record);
        packages.insert(processor_record.id.clone(), processor_record);
        packages.insert(connector_record.id.clone(), connector_record.clone());
        let store = ExtensionRegistryStore::new(root);
        store.save(&ExtensionRegistry { packages }).unwrap();
        let connector_manifest =
            ExtensionManifest::from_json_str(&connector_manifest_json.to_string()).unwrap();

        let connector = ExternalConnector::new(
            "fixture".into(),
            connector_record,
            connector_manifest,
            "main".into(),
            store,
        )
        .unwrap();

        let captures = connector.capture_sources();
        let processors = connector.processors();

        assert_eq!(captures.len(), 1);
        assert_eq!(captures[0].name(), "capture_pkg/events");
        assert_eq!(processors.len(), 1);
        assert_eq!(processors[0].name(), "processor_pkg/summarize");
        assert_eq!(
            connector.expected_source_names(),
            vec!["events".to_string()]
        );
    }

    #[test]
    fn external_connector_routes_builtin_capture_to_external_processor_without_owning_capture() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("extensions");
        let processor_manifest = serde_json::json!({
            "schema_version": 1,
            "id": "processor_pkg",
            "name": "Processor package",
            "version": "0.1.0",
            "server": {"start": ["node", "server.js"]},
            "processors": [{"id": "summarize", "display_name": "Summarize"}]
        });
        let connector_manifest_json = serde_json::json!({
            "schema_version": 1,
            "id": "connector_pkg",
            "name": "Connector package",
            "version": "0.1.0",
            "server": {"start": ["node", "server.js"]},
            "connectors": [{
                "id": "main",
                "display_name": "Main",
                "routes": [{
                    "from": {"component": "alvum.audio/audio-mic", "schema": "alvum.audio.opus.v1"},
                    "to": ["processor_pkg/summarize"]
                }]
            }]
        });
        let processor_record = package_record(&root, "processor_pkg", &processor_manifest);
        let connector_record = package_record(&root, "connector_pkg", &connector_manifest_json);
        let mut packages = BTreeMap::new();
        packages.insert(processor_record.id.clone(), processor_record);
        packages.insert(connector_record.id.clone(), connector_record.clone());
        let store = ExtensionRegistryStore::new(root);
        store.save(&ExtensionRegistry { packages }).unwrap();
        let connector_manifest =
            ExtensionManifest::from_json_str(&connector_manifest_json.to_string()).unwrap();

        let connector = ExternalConnector::new(
            "fixture".into(),
            connector_record,
            connector_manifest,
            "main".into(),
            store,
        )
        .unwrap();

        let captures = connector.capture_sources();
        let processors = connector.processors();

        assert!(captures.is_empty());
        assert_eq!(processors.len(), 1);
        assert_eq!(processors[0].name(), "processor_pkg/summarize");
        assert_eq!(
            connector.expected_source_names(),
            vec!["audio-mic".to_string()]
        );
    }
}
