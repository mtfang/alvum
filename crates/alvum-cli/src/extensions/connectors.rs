use anyhow::{Context, Result, bail};
use clap::Subcommand;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use crate::config_doc;
use crate::providers;

#[derive(Subcommand)]
pub(crate) enum Action {
    /// List user-facing connectors.
    List {
        /// Emit machine-readable JSON for app/front-end integrations.
        #[arg(long)]
        json: bool,
    },

    /// Enable a user-facing connector.
    Enable { id: String },

    /// Disable a user-facing connector.
    Disable { id: String },

    /// Validate connector health.
    Doctor {
        /// Emit machine-readable JSON for app/front-end integrations.
        #[arg(long)]
        json: bool,
    },
}

struct ConnectorPackageSource {
    manifest: alvum_core::extension::ExtensionManifest,
    kind: String,
    record_enabled: bool,
    package_read_only: bool,
    manifest_path: String,
    package_dir: String,
    install_source: Option<String>,
}

#[derive(Clone)]
struct IndexedComponent {
    display_name: String,
    description: String,
    kind: &'static str,
    analysis: Option<alvum_core::extension::AnalysisComponent>,
}

#[derive(Clone)]
struct IndexedCapture {
    capture: alvum_core::extension::CaptureComponent,
    package_kind: String,
}
#[derive(serde::Serialize)]
struct ConnectorListOutput {
    connectors: Vec<ConnectorRecord>,
}

#[derive(Clone, serde::Serialize)]
pub(super) struct ConnectorRecord {
    id: String,
    pub(super) component_id: String,
    package_id: String,
    connector_id: String,
    kind: String,
    pub(super) enabled: bool,
    read_only: bool,
    package_read_only: bool,
    display_name: String,
    description: String,
    package_name: String,
    version: String,
    manifest_path: String,
    package_dir: String,
    install_source: Option<String>,
    aggregate_state: String,
    pub(super) source_count: usize,
    pub(super) enabled_source_count: usize,
    source_controls: Vec<SourceControlSummary>,
    processor_controls: Vec<ProcessorControlSummary>,
    route_count: usize,
    analysis_count: usize,
    captures: Vec<ComponentRefSummary>,
    processors: Vec<ComponentRefSummary>,
    analyses: Vec<AnalysisRefSummary>,
    routes: Vec<RouteSummary>,
    pub(super) issues: Vec<String>,
    #[serde(skip_serializing)]
    config_key: Option<String>,
}

#[derive(Clone, serde::Serialize)]
struct SourceControlSummary {
    id: String,
    label: String,
    component: String,
    kind: String,
    enabled: bool,
    toggleable: bool,
    detail: String,
}

#[derive(Clone, serde::Serialize)]
struct ProcessorControlSummary {
    id: String,
    component: String,
    label: String,
    kind: String,
    detail: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    readiness: Option<ProcessorReadinessSummary>,
    settings: Vec<ProcessorSettingSummary>,
}

#[derive(Clone, serde::Serialize)]
struct ProcessorReadinessSummary {
    status: String,
    level: String,
    detail: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    action: Option<ProcessorActionSummary>,
}

#[derive(Clone, serde::Serialize)]
struct ProcessorActionSummary {
    kind: String,
    label: String,
}

#[derive(Clone, Default)]
struct ProcessorReadinessContext {
    screen_provider: Option<providers::ProviderModalityReadiness>,
}

#[derive(Clone, serde::Serialize)]
struct ProcessorSettingSummary {
    key: String,
    label: String,
    value: Option<String>,
    value_label: String,
    detail: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    options: Vec<SettingOptionSummary>,
}

#[derive(Clone, serde::Serialize)]
struct SettingOptionSummary {
    value: String,
    label: String,
}

#[derive(Clone, serde::Serialize)]
struct ComponentRefSummary {
    component: String,
    display_name: Option<String>,
    kind: Option<String>,
    exists: bool,
}

#[derive(Clone, serde::Serialize)]
struct RouteEndpointSummary {
    component: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    source: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    mime: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    schema: Option<String>,
    display_name: Option<String>,
    exists: bool,
}

#[derive(Clone, serde::Serialize)]
struct RouteSummary {
    from: RouteEndpointSummary,
    to: Vec<RouteEndpointSummary>,
    issues: Vec<String>,
}

#[derive(Clone, serde::Serialize)]
struct AnalysisRefSummary {
    id: String,
    component_id: String,
    display_name: Option<String>,
    output: Option<&'static str>,
    scopes: Vec<&'static str>,
    exists: bool,
}

#[derive(serde::Serialize)]
struct ConnectorDoctorOutput {
    connectors: Vec<ConnectorDoctorSummary>,
}

#[derive(serde::Serialize)]
struct ConnectorDoctorSummary {
    id: String,
    component_id: String,
    ok: bool,
    enabled: bool,
    message: String,
}

fn connector_package_sources(
    store: &alvum_connector_external::ExtensionRegistryStore,
) -> Result<Vec<ConnectorPackageSource>> {
    let mut sources: Vec<ConnectorPackageSource> = alvum_core::builtin_components::manifests()
        .into_iter()
        .map(|manifest| ConnectorPackageSource {
            manifest: manifest.clone(),
            kind: "core".into(),
            record_enabled: true,
            package_read_only: true,
            manifest_path: format!("builtin://{}", manifest.id),
            package_dir: format!("builtin://{}", manifest.id),
            install_source: None,
        })
        .collect();

    let registry = store.load()?;
    for record in registry.packages.values() {
        let manifest = match alvum_connector_external::ExtensionRegistryStore::load_manifest(record)
        {
            Ok(manifest) => manifest,
            Err(_) => continue,
        };
        sources.push(ConnectorPackageSource {
            manifest,
            kind: "external".into(),
            record_enabled: record.enabled,
            package_read_only: false,
            manifest_path: record.manifest_path.display().to_string(),
            package_dir: record.package_dir.display().to_string(),
            install_source: record.install_source.clone(),
        });
    }
    Ok(sources)
}

fn component_index(sources: &[ConnectorPackageSource]) -> BTreeMap<String, IndexedComponent> {
    let mut index = BTreeMap::new();
    for source in sources {
        for capture in &source.manifest.captures {
            index.insert(
                source.manifest.component_id(&capture.id),
                IndexedComponent {
                    display_name: capture.display_name.clone(),
                    description: capture.description.clone(),
                    kind: "capture",
                    analysis: None,
                },
            );
        }
        for processor in &source.manifest.processors {
            index.insert(
                source.manifest.component_id(&processor.id),
                IndexedComponent {
                    display_name: processor.display_name.clone(),
                    description: processor.description.clone(),
                    kind: "processor",
                    analysis: None,
                },
            );
        }
        for analysis in &source.manifest.analyses {
            index.insert(
                source.manifest.component_id(&analysis.id),
                IndexedComponent {
                    display_name: analysis.display_name.clone(),
                    description: analysis.description.clone(),
                    kind: "analysis",
                    analysis: Some(analysis.clone()),
                },
            );
        }
    }
    index
}

fn capture_index(sources: &[ConnectorPackageSource]) -> BTreeMap<String, IndexedCapture> {
    let mut index = BTreeMap::new();
    for source in sources {
        for capture in &source.manifest.captures {
            index.insert(
                source.manifest.component_id(&capture.id),
                IndexedCapture {
                    capture: capture.clone(),
                    package_kind: source.kind.clone(),
                },
            );
        }
    }
    index
}

fn core_connector_config_key(package_id: &str, connector_id: &str) -> Option<&'static str> {
    match (package_id, connector_id) {
        ("alvum.audio", "audio") => Some("audio"),
        ("alvum.screen", "screen") => Some("screen"),
        ("alvum.session", "claude-code") => Some("claude-code"),
        ("alvum.session", "codex") => Some("codex"),
        _ => None,
    }
}

fn external_connector_config_enabled(
    config: &alvum_core::config::AlvumConfig,
    package_id: &str,
    connector_id: &str,
) -> bool {
    config
        .connectors
        .iter()
        .any(|(config_name, connector_cfg)| {
            if connector_cfg.settings.get("kind").and_then(|v| v.as_str()) != Some("external-http")
            {
                return false;
            }
            let configured_package = connector_cfg
                .settings
                .get("package")
                .and_then(|v| v.as_str())
                .unwrap_or(config_name);
            let configured_connector = connector_cfg
                .settings
                .get("connector")
                .and_then(|v| v.as_str())
                .unwrap_or("main");
            configured_package == package_id
                && configured_connector == connector_id
                && connector_cfg.enabled
        })
}

fn component_ref(
    component: &str,
    index: &BTreeMap<String, IndexedComponent>,
) -> ComponentRefSummary {
    let indexed = index.get(component);
    ComponentRefSummary {
        component: component.into(),
        display_name: indexed.map(|component| component.display_name.clone()),
        kind: indexed.map(|component| component.kind.to_string()),
        exists: indexed.is_some(),
    }
}

fn route_endpoint(
    selector: &alvum_core::extension::RouteSelector,
    index: &BTreeMap<String, IndexedComponent>,
) -> RouteEndpointSummary {
    let indexed = index.get(&selector.component);
    RouteEndpointSummary {
        component: selector.component.clone(),
        source: selector.source.clone(),
        mime: selector.mime.clone(),
        schema: selector.schema.clone(),
        display_name: indexed.map(|component| component.display_name.clone()),
        exists: indexed.is_some(),
    }
}

fn source_control_enabled(
    config: &alvum_core::config::AlvumConfig,
    package_kind: &str,
    source_id: &str,
    connector_enabled: bool,
) -> bool {
    if package_kind != "core" {
        return connector_enabled;
    }
    if let Some(capture) = config.capture_source(source_id) {
        return connector_enabled && capture.enabled;
    }
    if let Some(connector) = config.connector(source_id) {
        return connector.enabled;
    }
    connector_enabled
}

fn source_control_toggleable(
    config: &alvum_core::config::AlvumConfig,
    package_kind: &str,
    source_id: &str,
) -> bool {
    package_kind == "core"
        && (config.capture_source(source_id).is_some() || config.connector(source_id).is_some())
}

fn source_controls_for_captures(
    config: &alvum_core::config::AlvumConfig,
    capture_ids: &BTreeSet<String>,
    captures: &BTreeMap<String, IndexedCapture>,
    connector_enabled: bool,
) -> Vec<SourceControlSummary> {
    let mut controls = Vec::new();
    for component_id in capture_ids {
        let Some(indexed) = captures.get(component_id) else {
            continue;
        };
        for source in &indexed.capture.sources {
            controls.push(SourceControlSummary {
                id: source.id.clone(),
                label: source.display_name.clone(),
                component: component_id.clone(),
                kind: "capture".into(),
                enabled: source_control_enabled(
                    config,
                    &indexed.package_kind,
                    &source.id,
                    connector_enabled,
                ),
                toggleable: source_control_toggleable(config, &indexed.package_kind, &source.id),
                detail: indexed.capture.description.clone(),
            });
        }
    }
    controls
}

fn toml_value_summary(value: &toml::Value) -> String {
    match value {
        toml::Value::String(value) => value.clone(),
        toml::Value::Integer(value) => value.to_string(),
        toml::Value::Float(value) => value.to_string(),
        toml::Value::Boolean(value) => value.to_string(),
        _ => value.to_string(),
    }
}

fn configured_processor_value(
    config: &alvum_core::config::AlvumConfig,
    processor_key: &str,
    connector_key: &str,
    setting_key: &str,
) -> Option<String> {
    config
        .processor(processor_key)
        .and_then(|processor| processor.settings.get(setting_key))
        .map(toml_value_summary)
        .or_else(|| {
            config
                .connector(connector_key)
                .and_then(|connector| connector.settings.get(setting_key))
                .map(toml_value_summary)
        })
}

fn processor_setting_summary(
    key: &str,
    label: &str,
    value: Option<String>,
    default_label: &str,
    detail: &str,
    options: Vec<SettingOptionSummary>,
) -> ProcessorSettingSummary {
    let value_label = value
        .as_deref()
        .filter(|value| !value.is_empty())
        .unwrap_or(default_label)
        .to_string();
    ProcessorSettingSummary {
        key: key.into(),
        label: label.into(),
        value,
        value_label,
        detail: detail.into(),
        options,
    }
}

fn setting_option(value: impl Into<String>, label: impl Into<String>) -> SettingOptionSummary {
    SettingOptionSummary {
        value: value.into(),
        label: label.into(),
    }
}

fn screen_mode_label(value: &str) -> String {
    match value {
        "ocr" => "OCR".into(),
        "provider" | "local" | "api" => "Provider".into(),
        "off" => "Off".into(),
        other => other.into(),
    }
}

fn screen_mode_options() -> Vec<SettingOptionSummary> {
    ["ocr", "provider", "off"]
        .into_iter()
        .map(|value| setting_option(value, screen_mode_label(value)))
        .collect()
}

fn audio_mode_label(value: &str) -> String {
    match value {
        "local" => "Local".into(),
        "provider" => "Provider".into(),
        "off" => "Off".into(),
        other => other.into(),
    }
}

fn audio_mode_options() -> Vec<SettingOptionSummary> {
    ["local", "provider", "off"]
        .into_iter()
        .map(|value| setting_option(value, audio_mode_label(value)))
        .collect()
}

fn whisper_language_options() -> Vec<SettingOptionSummary> {
    vec![
        setting_option("en", "English"),
        setting_option("auto", "Auto detect"),
    ]
}

fn path_label(value: &str) -> String {
    Path::new(value)
        .file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty())
        .unwrap_or(value)
        .to_string()
}

fn whisper_model_options(current: Option<&str>) -> Vec<SettingOptionSummary> {
    let mut options = BTreeMap::new();
    if let Some(current) = current.filter(|value| !value.is_empty()) {
        options.insert(current.to_string(), path_label(current));
    }
    if let Some(home) = dirs::home_dir() {
        let model_dir = home.join(".alvum/runtime/models");
        if let Ok(entries) = std::fs::read_dir(model_dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if !path.is_file() {
                    continue;
                }
                let supported = path
                    .extension()
                    .and_then(|ext| ext.to_str())
                    .map(|ext| matches!(ext, "bin" | "gguf"))
                    .unwrap_or(false);
                if !supported {
                    continue;
                }
                let value = path.to_string_lossy().into_owned();
                options
                    .entry(value.clone())
                    .or_insert_with(|| path_label(&value));
            }
        }
    }
    options
        .into_iter()
        .map(|(value, label)| setting_option(value, label))
        .collect()
}

fn builtin_processor_settings(
    config: &alvum_core::config::AlvumConfig,
    package_id: &str,
    connector_id: &str,
    component_id: &str,
) -> Vec<ProcessorSettingSummary> {
    match (package_id, connector_id, component_id) {
        ("alvum.audio", "audio", "alvum.audio/whisper") => {
            let mode = configured_processor_value(config, "audio", "audio", "mode")
                .or_else(|| Some("local".into()));
            let model = configured_processor_value(config, "audio", "audio", "whisper_model");
            let language = configured_processor_value(config, "audio", "audio", "whisper_language")
                .or_else(|| Some("en".into()));
            vec![
                processor_setting_summary(
                    "mode",
                    "Processing mode",
                    mode.clone(),
                    "local",
                    "How audio files are converted into text observations.",
                    audio_mode_options(),
                ),
                processor_setting_summary(
                    "whisper_model",
                    "Whisper model",
                    model.clone(),
                    "Not configured",
                    "Model file used for audio transcription.",
                    whisper_model_options(model.as_deref()),
                ),
                processor_setting_summary(
                    "whisper_language",
                    "Language",
                    language.clone(),
                    "en",
                    "Language hint passed to Whisper.",
                    whisper_language_options(),
                ),
            ]
        }
        ("alvum.screen", "screen", "alvum.screen/vision") => {
            let mode = configured_processor_value(config, "screen", "screen", "mode")
                .or_else(|| configured_processor_value(config, "screen", "screen", "vision"))
                .or_else(|| Some("ocr".into()));
            let value_label = mode
                .as_deref()
                .map(screen_mode_label)
                .unwrap_or_else(|| "OCR".into());
            vec![ProcessorSettingSummary {
                key: "mode".into(),
                label: "Recognition method".into(),
                value: mode,
                value_label,
                detail: "Text and content recognition method for screenshots.".into(),
                options: screen_mode_options(),
            }]
        }
        _ => Vec::new(),
    }
}

fn builtin_processor_readiness(
    config: &alvum_core::config::AlvumConfig,
    context: &ProcessorReadinessContext,
    package_id: &str,
    connector_id: &str,
    component_id: &str,
) -> Option<ProcessorReadinessSummary> {
    match (package_id, connector_id, component_id) {
        ("alvum.audio", "audio", "alvum.audio/whisper") => {
            let mode = configured_processor_value(config, "audio", "audio", "mode")
                .unwrap_or_else(|| "local".into());
            match mode.as_str() {
                "off" => Some(ProcessorReadinessSummary {
                    status: "off".into(),
                    level: "neutral".into(),
                    detail: "Audio processing is off.".into(),
                    action: None,
                }),
                "provider" => Some(ProcessorReadinessSummary {
                    status: "unsupported_adapter".into(),
                    level: "warning".into(),
                    detail: "Provider audio mode is valid config, but no Alvum provider adapter can send audio yet.".into(),
                    action: None,
                }),
                _ => {
                    let model = configured_processor_value(config, "audio", "audio", "whisper_model")
                        .unwrap_or_else(|| {
                            dirs::home_dir()
                                .unwrap_or_else(|| PathBuf::from("~"))
                                .join(".alvum/runtime/models/ggml-base.en.bin")
                                .to_string_lossy()
                                .into_owned()
                        });
                    if Path::new(&model).exists() {
                        Some(ProcessorReadinessSummary {
                            status: "ready".into(),
                            level: "ok".into(),
                            detail: format!("Local Whisper model is installed at {model}."),
                            action: None,
                        })
                    } else {
                        Some(ProcessorReadinessSummary {
                            status: "waiting_on_install".into(),
                            level: "warning".into(),
                            detail: format!("Local audio processing needs Whisper model {model}."),
                            action: Some(ProcessorActionSummary {
                                kind: "install_whisper".into(),
                                label: "Install".into(),
                            }),
                        })
                    }
                }
            }
        }
        ("alvum.screen", "screen", "alvum.screen/vision") => {
            let mode = configured_processor_value(config, "screen", "screen", "mode")
                .or_else(|| configured_processor_value(config, "screen", "screen", "vision"))
                .unwrap_or_else(|| "ocr".into());
            match mode.as_str() {
                "off" => Some(ProcessorReadinessSummary {
                    status: "off".into(),
                    level: "neutral".into(),
                    detail: "Screen processing is off.".into(),
                    action: None,
                }),
                "provider" | "local" | "api" => {
                    let readiness =
                        context
                            .screen_provider
                            .clone()
                            .unwrap_or_else(|| providers::ProviderModalityReadiness {
                                status: "requires_image_provider".into(),
                                level: "warning".into(),
                                detail: "Provider screen mode requires both an image-capable selected model and an Alvum adapter that can send images.".into(),
                            });
                    Some(ProcessorReadinessSummary {
                        status: readiness.status,
                        level: readiness.level,
                        detail: readiness.detail,
                        action: None,
                    })
                }
                _ => Some(ProcessorReadinessSummary {
                    status: "ready".into(),
                    level: "ok".into(),
                    detail: "OCR processing uses the local macOS Vision framework.".into(),
                    action: None,
                }),
            }
        }
        _ => None,
    }
}

fn processor_controls_for_connector(
    config: &alvum_core::config::AlvumConfig,
    context: &ProcessorReadinessContext,
    package_id: &str,
    connector_id: &str,
    processor_ids: &BTreeSet<String>,
    index: &BTreeMap<String, IndexedComponent>,
) -> Vec<ProcessorControlSummary> {
    processor_ids
        .iter()
        .map(|component_id| {
            let indexed = index.get(component_id);
            ProcessorControlSummary {
                id: component_id.clone(),
                component: component_id.clone(),
                label: indexed
                    .map(|component| component.display_name.clone())
                    .unwrap_or_else(|| component_id.clone()),
                kind: "processor".into(),
                detail: indexed
                    .map(|component| component.description.clone())
                    .unwrap_or_default(),
                readiness: builtin_processor_readiness(
                    config,
                    context,
                    package_id,
                    connector_id,
                    component_id,
                ),
                settings: builtin_processor_settings(
                    config,
                    package_id,
                    connector_id,
                    component_id,
                ),
            }
        })
        .collect()
}

fn aggregate_state(enabled: bool, source_controls: &[SourceControlSummary]) -> String {
    if source_controls.is_empty() {
        return if enabled { "all_on" } else { "all_off" }.into();
    }
    let enabled_count = source_controls
        .iter()
        .filter(|control| control.enabled)
        .count();
    if enabled_count == 0 {
        "all_off".into()
    } else if enabled_count == source_controls.len() {
        "all_on".into()
    } else {
        "partial".into()
    }
}

fn output_label(output: &alvum_core::extension::AnalysisOutput) -> &'static str {
    match output {
        alvum_core::extension::AnalysisOutput::Artifact => "artifact",
        alvum_core::extension::AnalysisOutput::GraphOverlay => "graph_overlay",
    }
}

fn scope_label(scope: &alvum_core::extension::DataScope) -> &'static str {
    match scope {
        alvum_core::extension::DataScope::Capture => "capture",
        alvum_core::extension::DataScope::Observations => "observations",
        alvum_core::extension::DataScope::Threads => "threads",
        alvum_core::extension::DataScope::Decisions => "decisions",
        alvum_core::extension::DataScope::Edges => "edges",
        alvum_core::extension::DataScope::Briefing => "briefing",
        alvum_core::extension::DataScope::Knowledge => "knowledge",
        alvum_core::extension::DataScope::RawFiles => "raw_files",
        alvum_core::extension::DataScope::All => "all",
    }
}

fn records_with_context(
    config: &alvum_core::config::AlvumConfig,
    store: &alvum_connector_external::ExtensionRegistryStore,
    context: &ProcessorReadinessContext,
) -> Result<Vec<ConnectorRecord>> {
    let sources = connector_package_sources(store)?;
    let index = component_index(&sources);
    let captures = capture_index(&sources);
    let mut records = Vec::new();

    for source in &sources {
        for connector in &source.manifest.connectors {
            let component_id = source.manifest.component_id(&connector.id);
            let config_key =
                core_connector_config_key(&source.manifest.id, &connector.id).map(str::to_string);
            let enabled = if source.kind == "core" {
                config_key
                    .as_deref()
                    .and_then(|key| config.connector(key))
                    .map(|connector| connector.enabled)
                    .unwrap_or(false)
            } else {
                source.record_enabled
                    && external_connector_config_enabled(config, &source.manifest.id, &connector.id)
            };

            let mut issues = Vec::new();
            let mut capture_ids = BTreeSet::new();
            let mut processor_ids = BTreeSet::new();
            let routes = connector
                .routes
                .iter()
                .map(|route| {
                    capture_ids.insert(route.from.component.clone());
                    let from = route_endpoint(&route.from, &index);
                    let mut route_issues = Vec::new();
                    if !from.exists {
                        route_issues.push(format!(
                            "Capture component {} is not installed",
                            from.component
                        ));
                    }
                    let to = route
                        .to
                        .iter()
                        .map(|target| {
                            processor_ids.insert(target.clone());
                            let endpoint = route_endpoint(
                                &alvum_core::extension::RouteSelector {
                                    component: target.clone(),
                                    source: None,
                                    mime: None,
                                    schema: None,
                                },
                                &index,
                            );
                            if !endpoint.exists {
                                route_issues.push(format!(
                                    "Processor component {} is not installed",
                                    endpoint.component
                                ));
                            }
                            endpoint
                        })
                        .collect();
                    issues.extend(route_issues.iter().cloned());
                    RouteSummary {
                        from,
                        to,
                        issues: route_issues,
                    }
                })
                .collect::<Vec<_>>();

            let analyses = connector
                .analyses
                .iter()
                .map(|analysis_id| {
                    let indexed = index.get(analysis_id);
                    if indexed.is_none() {
                        issues.push(format!("Analysis component {analysis_id} is not installed"));
                    }
                    let analysis = indexed.and_then(|component| component.analysis.as_ref());
                    AnalysisRefSummary {
                        id: analysis_id
                            .split_once('/')
                            .map(|(_, local)| local.to_string())
                            .unwrap_or_else(|| analysis_id.clone()),
                        component_id: analysis_id.clone(),
                        display_name: indexed.map(|component| component.display_name.clone()),
                        output: analysis.map(|analysis| output_label(&analysis.output)),
                        scopes: analysis
                            .map(|analysis| analysis.scopes.iter().map(scope_label).collect())
                            .unwrap_or_default(),
                        exists: indexed.is_some(),
                    }
                })
                .collect::<Vec<_>>();
            let source_controls =
                source_controls_for_captures(config, &capture_ids, &captures, enabled);
            let processor_controls = processor_controls_for_connector(
                config,
                context,
                &source.manifest.id,
                &connector.id,
                &processor_ids,
                &index,
            );
            let enabled_source_count = source_controls
                .iter()
                .filter(|control| control.enabled)
                .count();

            records.push(ConnectorRecord {
                id: component_id.clone(),
                component_id,
                package_id: source.manifest.id.clone(),
                connector_id: connector.id.clone(),
                kind: source.kind.clone(),
                enabled,
                read_only: false,
                package_read_only: source.package_read_only,
                display_name: connector.display_name.clone(),
                description: connector.description.clone(),
                package_name: source.manifest.name.clone(),
                version: source.manifest.version.clone(),
                manifest_path: source.manifest_path.clone(),
                package_dir: source.package_dir.clone(),
                install_source: source.install_source.clone(),
                aggregate_state: aggregate_state(enabled, &source_controls),
                source_count: source_controls.len(),
                enabled_source_count,
                source_controls,
                processor_controls,
                route_count: routes.len(),
                analysis_count: analyses.len(),
                captures: capture_ids
                    .iter()
                    .map(|component| component_ref(component, &index))
                    .collect(),
                processors: processor_ids
                    .iter()
                    .map(|component| component_ref(component, &index))
                    .collect(),
                analyses,
                routes,
                issues,
                config_key,
            });
        }
    }
    Ok(records)
}

pub(super) fn records(
    config: &alvum_core::config::AlvumConfig,
    store: &alvum_connector_external::ExtensionRegistryStore,
) -> Result<Vec<ConnectorRecord>> {
    records_with_context(config, store, &ProcessorReadinessContext::default())
}

async fn records_for_list(
    config: &alvum_core::config::AlvumConfig,
    store: &alvum_connector_external::ExtensionRegistryStore,
) -> Result<Vec<ConnectorRecord>> {
    let context = ProcessorReadinessContext {
        screen_provider: Some(providers::screen_provider_readiness(config).await),
    };
    records_with_context(config, store, &context)
}

fn connector_record_by_id<'a>(
    records: &'a [ConnectorRecord],
    id: &str,
) -> Result<&'a ConnectorRecord> {
    let exact = records
        .iter()
        .find(|record| record.id == id || record.component_id == id);
    if let Some(record) = exact {
        return Ok(record);
    }

    let matches = records
        .iter()
        .filter(|record| {
            record.connector_id == id
                || (record.package_id == id
                    && records
                        .iter()
                        .filter(|candidate| candidate.package_id == id)
                        .count()
                        == 1)
        })
        .collect::<Vec<_>>();
    match matches.as_slice() {
        [record] => Ok(record),
        [] => bail!("connector not found: {id}"),
        _ => bail!("connector id is ambiguous: {id}; use a component id like package/connector"),
    }
}

fn set_table_enabled(
    parent: &mut toml::Table,
    defaults: &toml::Table,
    section_name: &str,
    key: &str,
    enabled: bool,
) -> Result<()> {
    let section = parent
        .entry(section_name.to_string())
        .or_insert_with(|| toml::Value::Table(toml::Table::new()))
        .as_table_mut()
        .with_context(|| format!("{section_name} is not a table"))?;
    if !section.contains_key(key) {
        if let Some(default_value) = defaults
            .get(section_name)
            .and_then(|value| value.as_table())
            .and_then(|table| table.get(key))
        {
            section.insert(key.to_string(), default_value.clone());
        }
    }
    let table = section
        .entry(key.to_string())
        .or_insert_with(|| toml::Value::Table(toml::Table::new()))
        .as_table_mut()
        .with_context(|| format!("{section_name}.{key} is not a table"))?;
    table.insert("enabled".into(), toml::Value::Boolean(enabled));
    Ok(())
}

fn core_capture_config_keys(record: &ConnectorRecord) -> Vec<String> {
    if record.kind != "core" {
        return Vec::new();
    }
    let default_capture_keys = alvum_core::config::AlvumConfig::default()
        .capture
        .keys()
        .cloned()
        .collect::<BTreeSet<_>>();
    let mut keys = BTreeSet::new();
    for capture in &record.captures {
        if let Some(component) =
            alvum_core::builtin_components::capture_component(&capture.component)
        {
            for source in component.sources {
                if default_capture_keys.contains(&source.id) {
                    keys.insert(source.id);
                }
            }
            continue;
        }
        if let Some((_package, local_id)) = capture.component.split_once('/') {
            if default_capture_keys.contains(local_id) {
                keys.insert(local_id.to_string());
            }
        }
    }
    keys.into_iter().collect()
}

fn write_core_connector_enabled(record: &ConnectorRecord, enabled: bool) -> Result<()> {
    let config_key = record
        .config_key
        .as_deref()
        .with_context(|| format!("core connector {} has no config key", record.id))?;
    let mut doc = config_doc::load_table()?;
    let defaults = config_doc::default_table()?;
    set_table_enabled(&mut doc, &defaults, "connectors", config_key, enabled)?;
    for capture_key in core_capture_config_keys(record) {
        set_table_enabled(&mut doc, &defaults, "capture", &capture_key, enabled)?;
    }
    config_doc::write_table(&doc)
}

pub(crate) async fn run(action: Option<Action>) -> Result<()> {
    let store = alvum_connector_external::ExtensionRegistryStore::default();
    match action.unwrap_or(Action::List { json: false }) {
        Action::List { json } => {
            let config = alvum_core::config::AlvumConfig::load()?;
            let records = records_for_list(&config, &store).await?;
            if json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&ConnectorListOutput {
                        connectors: records
                    })?
                );
                return Ok(());
            }
            if records.is_empty() {
                println!("No connectors available.");
                return Ok(());
            }
            for record in records {
                let status = if record.enabled {
                    "enabled"
                } else {
                    "disabled"
                };
                println!("{} ({}, {status})", record.component_id, record.kind);
                println!(
                    "  routes: {}, analyses: {}",
                    record.route_count, record.analysis_count
                );
                if !record.issues.is_empty() {
                    println!("  issues: {}", record.issues.join("; "));
                }
            }
            Ok(())
        }
        Action::Enable { id } => cmd_connector_set_enabled(&store, &id, true),
        Action::Disable { id } => cmd_connector_set_enabled(&store, &id, false),
        Action::Doctor { json } => {
            let config = alvum_core::config::AlvumConfig::load()?;
            let records = records(&config, &store)?;
            let extension_doctor_by_id = super::extension_doctor_summaries(&store)?
                .into_iter()
                .map(|summary| (summary.id.clone(), summary))
                .collect::<BTreeMap<_, _>>();
            let summaries = records
                .into_iter()
                .map(|record| {
                    if record.kind == "core" {
                        ConnectorDoctorSummary {
                            id: record.id,
                            component_id: record.component_id,
                            ok: true,
                            enabled: record.enabled,
                            message: "core connector".into(),
                        }
                    } else {
                        let doctor = extension_doctor_by_id.get(&record.package_id);
                        ConnectorDoctorSummary {
                            id: record.id,
                            component_id: record.component_id,
                            ok: doctor.map(|summary| summary.ok).unwrap_or(false),
                            enabled: record.enabled,
                            message: doctor
                                .map(|summary| summary.message.clone())
                                .unwrap_or_else(|| "extension package not installed".into()),
                        }
                    }
                })
                .collect::<Vec<_>>();
            if json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&ConnectorDoctorOutput {
                        connectors: summaries
                    })?
                );
                return Ok(());
            }
            for summary in summaries {
                if summary.ok {
                    println!("{}: ok", summary.component_id);
                } else {
                    println!("{}: error: {}", summary.component_id, summary.message);
                }
            }
            Ok(())
        }
    }
}

fn cmd_connector_set_enabled(
    store: &alvum_connector_external::ExtensionRegistryStore,
    id: &str,
    enabled: bool,
) -> Result<()> {
    let config = alvum_core::config::AlvumConfig::load()?;
    let records = records(&config, store)?;
    let record = connector_record_by_id(&records, id)?;
    if record.kind == "core" {
        write_core_connector_enabled(record, enabled)?;
    } else {
        if enabled {
            store.set_enabled(&record.package_id, true)?;
        }
        super::write_external_connector_config(&record.package_id, &record.connector_id, enabled)?;
    }
    println!(
        "{} connector: {}",
        if enabled { "Enabled" } else { "Disabled" },
        record.component_id
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[test]
    fn screen_provider_mode_uses_provider_capability_context() {
        let mut config = alvum_core::config::AlvumConfig::default();
        let mut screen_settings = HashMap::new();
        screen_settings.insert("mode".into(), toml::Value::String("provider".into()));
        config.processors.insert(
            "screen".into(),
            alvum_core::config::ProcessorConfig {
                settings: screen_settings,
            },
        );
        let context = ProcessorReadinessContext {
            screen_provider: Some(providers::ProviderModalityReadiness {
                status: "ready".into(),
                level: "ok".into(),
                detail: "Provider screen mode is ready.".into(),
            }),
        };

        let readiness = builtin_processor_readiness(
            &config,
            &context,
            "alvum.screen",
            "screen",
            "alvum.screen/vision",
        )
        .unwrap();

        assert_eq!(readiness.status, "ready");
        assert_eq!(readiness.level, "ok");
    }
}
