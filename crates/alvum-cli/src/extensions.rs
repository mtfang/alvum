use anyhow::{Context, Result, bail};
use clap::Subcommand;
use std::path::PathBuf;

use crate::providers;

mod connectors;

pub(crate) use connectors::{Action as ConnectorAction, run as run_connectors};

#[derive(Subcommand)]
pub(crate) enum ExtensionAction {
    /// List installed extension packages.
    List {
        /// Emit machine-readable JSON for app/front-end integrations.
        #[arg(long)]
        json: bool,
    },

    /// Install an extension package from a local path, git:<url>, or npm:<package>.
    Install { source: String },

    /// Create a starter external HTTP extension package.
    Scaffold {
        path: PathBuf,
        #[arg(long)]
        id: String,
        #[arg(long)]
        name: String,
    },

    /// Reinstall an extension package from its original source.
    Update { id: String },

    /// Remove an installed extension package.
    Remove { id: String },

    /// Enable an installed package and write a connector config entry.
    Enable {
        id: String,
        #[arg(long)]
        connector: Option<String>,
    },

    /// Disable a package and its connector config entries.
    Disable { id: String },

    /// Validate installed package manifests.
    Doctor {
        /// Emit machine-readable JSON for app/front-end integrations.
        #[arg(long)]
        json: bool,
    },

    /// Run an analysis lens on demand.
    Run {
        package: String,
        analysis: String,
        #[arg(long)]
        date: Option<String>,
        #[arg(long)]
        output: Option<PathBuf>,
        #[arg(long)]
        capture_dir: Option<PathBuf>,
        #[arg(long)]
        provider: Option<String>,
        #[arg(long, default_value = "claude-sonnet-4-6")]
        model: String,
    },
}

pub(crate) async fn run_extensions(action: ExtensionAction) -> Result<()> {
    use alvum_connector_external::{ExtensionInstallSource, ExtensionRegistryStore};

    let store = ExtensionRegistryStore::default();
    match action {
        ExtensionAction::List { json } => cmd_extensions_list(&store, json),
        ExtensionAction::Install { source } => {
            let record = store.install(ExtensionInstallSource::parse(&source))?;
            println!("Installed extension package: {}", record.id);
            println!("Enable it with: alvum extensions enable {}", record.id);
            Ok(())
        }
        ExtensionAction::Scaffold { path, id, name } => cmd_extensions_scaffold(&path, &id, &name),
        ExtensionAction::Update { id } => {
            let registry = store.load()?;
            let record = registry
                .packages
                .get(&id)
                .with_context(|| format!("extension package not installed: {id}"))?;
            let was_enabled = record.enabled;
            let source = record
                .install_source
                .clone()
                .with_context(|| format!("extension package {id} has no install source"))?;
            let updated = store.install(ExtensionInstallSource::parse(&source))?;
            if was_enabled {
                store.set_enabled(&updated.id, true)?;
            }
            println!("Updated extension package: {}", updated.id);
            Ok(())
        }
        ExtensionAction::Remove { id } => {
            store.remove(&id)?;
            disable_extension_config(&id)?;
            println!("Removed extension package: {id}");
            Ok(())
        }
        ExtensionAction::Enable { id, connector } => {
            let record = store.set_enabled(&id, true)?;
            let manifest = ExtensionRegistryStore::load_manifest(&record)?;
            let connector = connector.or_else(|| manifest.connectors.first().map(|c| c.id.clone()));
            if let Some(connector) = connector {
                if !manifest.connectors.iter().any(|c| c.id == connector) {
                    anyhow::bail!("extension package {id} has no connector {connector}");
                }
                write_external_connector_config(&id, &connector, true)?;
                println!("Enabled extension connector: {id}/{connector}");
            } else {
                println!("Enabled extension package: {id}");
            }
            Ok(())
        }
        ExtensionAction::Disable { id } => {
            store.set_enabled(&id, false)?;
            disable_extension_config(&id)?;
            println!("Disabled extension package: {id}");
            Ok(())
        }
        ExtensionAction::Doctor { json } => {
            let store = store.clone();
            tokio::task::spawn_blocking(move || cmd_extensions_doctor(&store, json)).await?
        }
        ExtensionAction::Run {
            package,
            analysis,
            date,
            output,
            capture_dir,
            provider,
            model,
        } => {
            let registry = store.load()?;
            let record = registry
                .packages
                .get(&package)
                .with_context(|| format!("extension package not installed: {package}"))?
                .clone();
            if !record.enabled {
                anyhow::bail!("extension package is disabled: {package}");
            }
            let manifest = ExtensionRegistryStore::load_manifest(&record)?;
            let date = date.unwrap_or_else(|| chrono::Utc::now().format("%Y-%m-%d").to_string());
            let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("."));
            let output = output.unwrap_or_else(|| {
                home.join(".alvum")
                    .join("generated")
                    .join("briefings")
                    .join(&date)
            });
            let capture_dir =
                capture_dir.unwrap_or_else(|| home.join(".alvum").join("capture").join(&date));
            let provider = provider.unwrap_or_else(|| {
                alvum_core::config::AlvumConfig::load()
                    .map(|config| config.pipeline.provider)
                    .unwrap_or_else(|_| "auto".into())
            });
            let provider_box =
                alvum_pipeline::llm::create_provider_async(&provider, &model).await?;
            let provider: std::sync::Arc<dyn alvum_core::llm::LlmProvider> = provider_box.into();
            let response = alvum_connector_external::run_analysis(
                record,
                manifest,
                &analysis,
                &date,
                &capture_dir,
                &output,
                provider,
            )
            .await?;
            println!(
                "Analysis {package}/{analysis} wrote {} artifact(s), {} graph overlay(s)",
                response.artifacts.len(),
                response.graph_overlays.len()
            );
            Ok(())
        }
    }
}

#[derive(serde::Serialize)]
struct ExtensionListOutput {
    extensions: Vec<ExtensionSummary>,
    core: Vec<ExtensionSummary>,
}

#[derive(serde::Serialize)]
struct ExtensionSummary {
    id: String,
    kind: String,
    enabled: bool,
    read_only: bool,
    manifest_path: String,
    package_dir: String,
    install_source: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    version: Option<String>,
    captures: Vec<ComponentSummary>,
    processors: Vec<ComponentSummary>,
    analyses: Vec<ComponentSummary>,
    connectors: Vec<ConnectorSummary>,
}

#[derive(serde::Serialize)]
struct ComponentSummary {
    id: String,
    component_id: String,
    display_name: String,
}

#[derive(serde::Serialize)]
struct ConnectorSummary {
    id: String,
    component_id: String,
    display_name: String,
    route_count: usize,
    analysis_count: usize,
}

fn extension_summary(record: &alvum_core::extension::ExtensionPackageRecord) -> ExtensionSummary {
    let base = || ExtensionSummary {
        id: record.id.clone(),
        kind: "external".into(),
        enabled: record.enabled,
        read_only: false,
        manifest_path: record.manifest_path.display().to_string(),
        package_dir: record.package_dir.display().to_string(),
        install_source: record.install_source.clone(),
        error: None,
        name: None,
        version: None,
        captures: Vec::new(),
        processors: Vec::new(),
        analyses: Vec::new(),
        connectors: Vec::new(),
    };
    let manifest = match alvum_connector_external::ExtensionRegistryStore::load_manifest(record) {
        Ok(manifest) => manifest,
        Err(e) => {
            return ExtensionSummary {
                error: Some(format!("{e:#}")),
                ..base()
            };
        }
    };
    extension_summary_from_manifest(
        &manifest,
        "external",
        record.enabled,
        false,
        record.manifest_path.display().to_string(),
        record.package_dir.display().to_string(),
        record.install_source.clone(),
    )
}

fn extension_summary_from_manifest(
    manifest: &alvum_core::extension::ExtensionManifest,
    kind: &str,
    enabled: bool,
    read_only: bool,
    manifest_path: String,
    package_dir: String,
    install_source: Option<String>,
) -> ExtensionSummary {
    let component = |id: &str, display_name: &str| ComponentSummary {
        id: id.to_string(),
        component_id: manifest.component_id(id),
        display_name: display_name.to_string(),
    };
    ExtensionSummary {
        id: manifest.id.clone(),
        kind: kind.into(),
        enabled,
        read_only,
        manifest_path,
        package_dir,
        install_source,
        error: None,
        name: Some(manifest.name.clone()),
        version: Some(manifest.version.clone()),
        captures: manifest
            .captures
            .iter()
            .map(|c| component(&c.id, &c.display_name))
            .collect(),
        processors: manifest
            .processors
            .iter()
            .map(|p| component(&p.id, &p.display_name))
            .collect(),
        analyses: manifest
            .analyses
            .iter()
            .map(|a| component(&a.id, &a.display_name))
            .collect(),
        connectors: manifest
            .connectors
            .iter()
            .map(|c| ConnectorSummary {
                id: c.id.clone(),
                component_id: manifest.component_id(&c.id),
                display_name: c.display_name.clone(),
                route_count: c.routes.len(),
                analysis_count: c.analyses.len(),
            })
            .collect(),
    }
}

fn core_extension_summaries(config: &alvum_core::config::AlvumConfig) -> Vec<ExtensionSummary> {
    alvum_core::builtin_components::manifests()
        .into_iter()
        .map(|manifest| {
            let enabled = match manifest.id.as_str() {
                "alvum.audio" => {
                    config
                        .connector("audio")
                        .map(|connector| connector.enabled)
                        .unwrap_or(false)
                        || config
                            .capture_source("audio-mic")
                            .map(|source| source.enabled)
                            .unwrap_or(false)
                        || config
                            .capture_source("audio-system")
                            .map(|source| source.enabled)
                            .unwrap_or(false)
                }
                "alvum.screen" => {
                    config
                        .connector("screen")
                        .map(|connector| connector.enabled)
                        .unwrap_or(false)
                        || config
                            .capture_source("screen")
                            .map(|source| source.enabled)
                            .unwrap_or(false)
                }
                "alvum.session" => {
                    config
                        .connector("claude-code")
                        .map(|connector| connector.enabled)
                        .unwrap_or(false)
                        || config
                            .connector("codex")
                            .map(|connector| connector.enabled)
                            .unwrap_or(false)
                }
                _ => false,
            };
            extension_summary_from_manifest(
                &manifest,
                "core",
                enabled,
                true,
                format!("builtin://{}", manifest.id),
                format!("builtin://{}", manifest.id),
                None,
            )
        })
        .collect()
}

fn cmd_extensions_list(
    store: &alvum_connector_external::ExtensionRegistryStore,
    json: bool,
) -> Result<()> {
    let registry = store.load()?;
    let summaries: Vec<ExtensionSummary> =
        registry.packages.values().map(extension_summary).collect();
    let config = alvum_core::config::AlvumConfig::load()
        .unwrap_or_else(|_| alvum_core::config::AlvumConfig::default());
    let core = core_extension_summaries(&config);
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&ExtensionListOutput {
                extensions: summaries,
                core,
            })?
        );
        return Ok(());
    }
    if summaries.is_empty() {
        println!("No external extensions installed.");
    } else {
        for summary in summaries {
            let status = if summary.enabled {
                "enabled"
            } else {
                "disabled"
            };
            println!("{} ({})", summary.id, status);
            if let Some(name) = &summary.name {
                println!("  name: {name}");
            }
            println!("  manifest: {}", summary.manifest_path);
            if let Some(source) = &summary.install_source {
                println!("  source: {source}");
            }
            if let Some(error) = &summary.error {
                println!("  error: {error}");
            }
        }
    }
    println!("Core components:");
    for summary in core {
        let status = if summary.enabled {
            "enabled"
        } else {
            "disabled"
        };
        println!("  {} (core, {status})", summary.id);
        if let Some(name) = &summary.name {
            println!("  name: {name}");
        }
    }
    Ok(())
}

fn extension_doctor_summaries(
    store: &alvum_connector_external::ExtensionRegistryStore,
) -> Result<Vec<DoctorSummary>> {
    let registry = store.load()?;
    let log_dir = dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".alvum")
        .join("runtime")
        .join("logs")
        .join("extensions");
    Ok(registry
        .packages
        .values()
        .map(|record| extension_doctor_summary(record, &log_dir))
        .collect())
}

#[derive(serde::Serialize)]
struct GlobalDoctorOutput {
    ok: bool,
    error_count: usize,
    warning_count: usize,
    checks: Vec<GlobalDoctorCheck>,
}

#[derive(serde::Serialize)]
struct GlobalDoctorCheck {
    id: &'static str,
    label: &'static str,
    level: &'static str,
    message: String,
}

fn doctor_check(
    id: &'static str,
    label: &'static str,
    level: &'static str,
    message: impl Into<String>,
) -> GlobalDoctorCheck {
    GlobalDoctorCheck {
        id,
        label,
        level,
        message: message.into(),
    }
}

fn load_config_for_doctor(checks: &mut Vec<GlobalDoctorCheck>) -> alvum_core::config::AlvumConfig {
    let path = alvum_core::config::config_path();
    if !path.exists() {
        checks.push(doctor_check(
            "config",
            "Config",
            "ok",
            format!("No config file at {}; using defaults.", path.display()),
        ));
        return alvum_core::config::AlvumConfig::default();
    }

    match alvum_core::config::AlvumConfig::load() {
        Ok(config) => {
            checks.push(doctor_check(
                "config",
                "Config",
                "ok",
                format!("Loaded {}.", path.display()),
            ));
            config
        }
        Err(e) => {
            checks.push(doctor_check("config", "Config", "error", format!("{e:#}")));
            alvum_core::config::AlvumConfig::default()
        }
    }
}

fn diagnose_connectors(
    config: &alvum_core::config::AlvumConfig,
    store: &alvum_connector_external::ExtensionRegistryStore,
    checks: &mut Vec<GlobalDoctorCheck>,
) {
    match connectors::records(config, store) {
        Ok(records) => {
            if records.is_empty() {
                checks.push(doctor_check(
                    "connectors",
                    "Connectors",
                    "warning",
                    "No connectors are available.",
                ));
                return;
            }

            let route_issues = records
                .iter()
                .flat_map(|record| {
                    record
                        .issues
                        .iter()
                        .map(move |issue| format!("{}: {issue}", record.component_id))
                })
                .collect::<Vec<_>>();
            let disabled_sources = records
                .iter()
                .filter(|record| {
                    record.enabled && record.source_count > 0 && record.enabled_source_count == 0
                })
                .map(|record| record.component_id.clone())
                .collect::<Vec<_>>();

            if !route_issues.is_empty() {
                checks.push(doctor_check(
                    "connectors",
                    "Connectors",
                    "error",
                    format!(
                        "{} route issue{}: {}",
                        route_issues.len(),
                        if route_issues.len() == 1 { "" } else { "s" },
                        route_issues
                            .iter()
                            .take(3)
                            .cloned()
                            .collect::<Vec<_>>()
                            .join("; ")
                    ),
                ));
            } else if !disabled_sources.is_empty() {
                let connector_word = if disabled_sources.len() == 1 {
                    "connector has"
                } else {
                    "connectors have"
                };
                checks.push(doctor_check(
                    "connectors",
                    "Connectors",
                    "warning",
                    format!(
                        "{} enabled {connector_word} all sources off: {}.",
                        disabled_sources.len(),
                        disabled_sources
                            .iter()
                            .take(3)
                            .cloned()
                            .collect::<Vec<_>>()
                            .join(", ")
                    ),
                ));
            } else {
                let enabled = records.iter().filter(|record| record.enabled).count();
                checks.push(doctor_check(
                    "connectors",
                    "Connectors",
                    "ok",
                    format!(
                        "{enabled}/{} connectors enabled; route matrix is valid.",
                        records.len()
                    ),
                ));
            }
        }
        Err(e) => checks.push(doctor_check(
            "connectors",
            "Connectors",
            "error",
            format!("{e:#}"),
        )),
    }
}

fn diagnose_extensions(
    store: &alvum_connector_external::ExtensionRegistryStore,
    checks: &mut Vec<GlobalDoctorCheck>,
) {
    match extension_doctor_summaries(store) {
        Ok(summaries) => {
            if summaries.is_empty() {
                checks.push(doctor_check(
                    "extensions",
                    "Extensions",
                    "ok",
                    "No external extensions installed.",
                ));
                return;
            }
            let failed = summaries
                .iter()
                .filter(|summary| !summary.ok)
                .collect::<Vec<_>>();
            if failed.is_empty() {
                checks.push(doctor_check(
                    "extensions",
                    "Extensions",
                    "ok",
                    format!(
                        "{} external extension packages passed health checks.",
                        summaries.len()
                    ),
                ));
            } else {
                checks.push(doctor_check(
                    "extensions",
                    "Extensions",
                    "error",
                    format!(
                        "{} extension package{} failed: {}.",
                        failed.len(),
                        if failed.len() == 1 { "" } else { "s" },
                        failed
                            .iter()
                            .take(3)
                            .map(|summary| format!("{} ({})", summary.id, summary.message))
                            .collect::<Vec<_>>()
                            .join("; ")
                    ),
                ));
            }
        }
        Err(e) => checks.push(doctor_check(
            "extensions",
            "Extensions",
            "error",
            format!("{e:#}"),
        )),
    }
}

fn diagnose_providers(
    config: &alvum_core::config::AlvumConfig,
    checks: &mut Vec<GlobalDoctorCheck>,
) {
    let configured = providers::normalize_name(&config.pipeline.provider);
    let entries = providers::entries(config);
    let available = entries
        .iter()
        .filter(|entry| entry.available && config.provider_enabled(entry.name))
        .map(|entry| entry.name)
        .collect::<Vec<_>>();

    if configured == "auto" {
        if let Some(provider) = available.first() {
            checks.push(doctor_check(
                "providers",
                "Providers",
                "ok",
                format!("Auto provider can use {provider}."),
            ));
        } else {
            checks.push(doctor_check(
                "providers",
                "Providers",
                "warning",
                "No LLM providers were detected on PATH or in the environment.",
            ));
        }
        return;
    }

    match entries.iter().find(|entry| entry.name == configured) {
        Some(entry) if !config.provider_enabled(entry.name) => checks.push(doctor_check(
            "providers",
            "Providers",
            "warning",
            format!("Configured provider {configured} is removed from Alvum's provider list."),
        )),
        Some(entry) if entry.available => checks.push(doctor_check(
            "providers",
            "Providers",
            "ok",
            format!("Configured provider {configured} is available."),
        )),
        Some(entry) => checks.push(doctor_check(
            "providers",
            "Providers",
            "warning",
            format!(
                "Configured provider {configured} is not detected; {}.",
                entry.auth_hint
            ),
        )),
        None => checks.push(doctor_check(
            "providers",
            "Providers",
            "warning",
            format!("Configured provider {configured} is not recognized."),
        )),
    }
}

fn global_doctor_output() -> GlobalDoctorOutput {
    let mut checks = Vec::new();
    let store = alvum_connector_external::ExtensionRegistryStore::default();
    let config = load_config_for_doctor(&mut checks);

    diagnose_connectors(&config, &store, &mut checks);
    diagnose_extensions(&store, &mut checks);
    diagnose_providers(&config, &mut checks);

    let error_count = checks.iter().filter(|check| check.level == "error").count();
    let warning_count = checks
        .iter()
        .filter(|check| check.level == "warning")
        .count();
    GlobalDoctorOutput {
        ok: error_count == 0,
        error_count,
        warning_count,
        checks,
    }
}

pub(crate) fn run_doctor(json: bool) -> Result<()> {
    let output = global_doctor_output();
    if json {
        println!("{}", serde_json::to_string_pretty(&output)?);
        return Ok(());
    }

    for check in &output.checks {
        println!("[{}] {}: {}", check.level, check.label, check.message);
    }
    if output.ok {
        println!(
            "Diagnostics completed with {} warning{}.",
            output.warning_count,
            if output.warning_count == 1 { "" } else { "s" }
        );
    } else {
        println!(
            "Diagnostics found {} error{} and {} warning{}.",
            output.error_count,
            if output.error_count == 1 { "" } else { "s" },
            output.warning_count,
            if output.warning_count == 1 { "" } else { "s" }
        );
    }
    Ok(())
}
#[derive(serde::Serialize)]
struct DoctorOutput {
    extensions: Vec<DoctorSummary>,
}

#[derive(Clone, serde::Serialize)]
struct DoctorSummary {
    id: String,
    ok: bool,
    enabled: bool,
    connector_count: usize,
    message: String,
}

fn extension_doctor_summary(
    record: &alvum_core::extension::ExtensionPackageRecord,
    log_dir: &std::path::Path,
) -> DoctorSummary {
    let manifest = match alvum_connector_external::ExtensionRegistryStore::load_manifest(record)
        .and_then(|m| m.validate().map(|_| m))
    {
        Ok(manifest) => manifest,
        Err(e) => {
            return DoctorSummary {
                id: record.id.clone(),
                ok: false,
                enabled: record.enabled,
                connector_count: 0,
                message: format!("{e:#}"),
            };
        }
    };
    let health = alvum_connector_external::ManagedExtension::start(
        &manifest,
        &record.package_dir,
        log_dir,
        None,
    )
    .and_then(|managed| {
        let remote = managed.client().manifest()?;
        if remote.id != manifest.id {
            bail!(
                "/v1/manifest reported {}, expected {}",
                remote.id,
                manifest.id
            );
        }
        Ok(())
    });
    match health {
        Ok(()) => DoctorSummary {
            id: record.id.clone(),
            ok: true,
            enabled: record.enabled,
            connector_count: manifest.connectors.len(),
            message: "ok".into(),
        },
        Err(e) => DoctorSummary {
            id: record.id.clone(),
            ok: false,
            enabled: record.enabled,
            connector_count: manifest.connectors.len(),
            message: format!("{e:#}"),
        },
    }
}

fn cmd_extensions_doctor(
    store: &alvum_connector_external::ExtensionRegistryStore,
    json: bool,
) -> Result<()> {
    let registry = store.load()?;
    if registry.packages.is_empty() {
        if json {
            println!(
                "{}",
                serde_json::to_string_pretty(&DoctorOutput { extensions: vec![] })?
            );
        } else {
            println!("No extensions installed.");
        }
        return Ok(());
    }
    let log_dir = dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".alvum")
        .join("runtime")
        .join("logs")
        .join("extensions");
    let summaries: Vec<DoctorSummary> = registry
        .packages
        .values()
        .map(|record| extension_doctor_summary(record, &log_dir))
        .collect();
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&DoctorOutput {
                extensions: summaries
            })?
        );
        return Ok(());
    }
    for summary in summaries {
        if summary.ok {
            println!(
                "{}: ok ({} connector(s))",
                summary.id, summary.connector_count
            );
        } else {
            println!("{}: error: {}", summary.id, summary.message);
        }
    }
    Ok(())
}

fn cmd_extensions_scaffold(path: &std::path::Path, id: &str, name: &str) -> Result<()> {
    if path.exists() && path.read_dir()?.next().is_some() {
        bail!("target directory is not empty: {}", path.display());
    }
    let manifest = serde_json::json!({
        "schema_version": 1,
        "id": id,
        "name": name,
        "version": "0.1.0",
        "description": "Starter Alvum external extension.",
        "server": {
            "start": ["node", "src/server.mjs"],
            "health_path": "/v1/health",
            "startup_timeout_ms": 5000
        },
        "captures": [{
            "id": "capture",
            "display_name": "Starter capture",
            "sources": [{"id": id, "display_name": name, "expected": false}],
            "schemas": [format!("{id}.event.v1")]
        }],
        "processors": [{
            "id": "processor",
            "display_name": "Starter processor",
            "accepts": [{"component": format!("{id}/capture"), "schema": format!("{id}.event.v1")}]
        }],
        "analyses": [{
            "id": "analysis",
            "display_name": "Starter analysis",
            "scopes": ["observations", "briefing"],
            "output": "artifact"
        }],
        "connectors": [{
            "id": "main",
            "display_name": name,
            "routes": [{
                "from": {"component": format!("{id}/capture"), "schema": format!("{id}.event.v1")},
                "to": [format!("{id}/processor")]
            }],
            "analyses": [format!("{id}/analysis")]
        }],
        "permissions": [{
            "kind": "network",
            "description": "Declare any external APIs this package calls."
        }]
    });
    let manifest_json = serde_json::to_string_pretty(&manifest)?;
    alvum_core::extension::ExtensionManifest::from_json_str(&manifest_json)?;

    std::fs::create_dir_all(path.join("src"))?;
    std::fs::write(path.join("alvum.extension.json"), manifest_json)?;
    std::fs::write(
        path.join("package.json"),
        serde_json::to_string_pretty(&serde_json::json!({
            "name": format!("alvum-extension-{id}"),
            "version": "0.1.0",
            "type": "module",
            "private": true,
            "scripts": {
                "start": "node src/server.mjs"
            }
        }))?,
    )?;
    std::fs::write(path.join("README.md"), scaffold_readme(id, name))?;
    std::fs::write(path.join("src/server.mjs"), scaffold_server(id)?)?;
    println!("Scaffolded extension package: {}", path.display());
    println!("Try it with: alvum extensions install {}", path.display());
    Ok(())
}

fn scaffold_readme(id: &str, name: &str) -> String {
    format!(
        "# {name}\n\nStarter Alvum external extension package.\n\n## Run locally\n\n```bash\nnpm start\n```\n\n## Install into Alvum\n\n```bash\nalvum extensions install .\nalvum extensions enable {id}\nalvum extensions doctor\n```\n"
    )
}

fn scaffold_server(id: &str) -> Result<String> {
    let component = format!("{id}/capture");
    let schema = format!("{id}.event.v1");
    Ok(format!(
        r#"import http from 'node:http';
import fs from 'node:fs/promises';

const port = Number(process.env.ALVUM_EXTENSION_PORT || 0);
const token = process.env.ALVUM_EXTENSION_TOKEN || '';
const manifest = JSON.parse(await fs.readFile(new URL('../alvum.extension.json', import.meta.url), 'utf8'));

function send(res, status, body) {{
  const text = typeof body === 'string' ? body : JSON.stringify(body);
  res.writeHead(status, {{ 'content-type': typeof body === 'string' ? 'text/plain' : 'application/json' }});
  res.end(text);
}}

async function readJson(req) {{
  const chunks = [];
  for await (const chunk of req) chunks.push(chunk);
  const text = Buffer.concat(chunks).toString('utf8') || '{{}}';
  return JSON.parse(text);
}}

function authorized(req) {{
  return req.headers.authorization === `Bearer ${{token}}`;
}}

const server = http.createServer(async (req, res) => {{
  if (req.url !== '/v1/health' && !authorized(req)) return send(res, 401, {{ error: 'unauthorized' }});
  if (req.method === 'GET' && req.url === '/v1/health') return send(res, 200, 'ok');
  if (req.method === 'GET' && req.url === '/v1/manifest') return send(res, 200, manifest);

  if (req.method === 'POST' && req.url === '/v1/gather') {{
    const body = await readJson(req);
    const ts = new Date().toISOString();
    return send(res, 200, {{
      data_refs: [{{
        ts,
        source: '{id}',
        producer: '{component}',
        schema: '{schema}',
        path: 'starter-events.jsonl',
        mime: 'application/x-jsonl',
        metadata: {{ connector: body.connector }}
      }}],
      observations: [],
      warnings: []
    }});
  }}

  if (req.method === 'POST' && req.url === '/v1/process') {{
    const body = await readJson(req);
    return send(res, 200, {{
      observations: (body.data_refs || []).map((ref) => ({{
        ts: ref.ts,
        source: ref.source,
        kind: 'custom',
        content: `Starter observation from ${{ref.path}}`,
        confidence: 0.5,
        refs: [ref]
      }})),
      warnings: []
    }});
  }}

  if (req.method === 'POST' && req.url === '/v1/capture/start') return send(res, 200, {{ run_id: 'starter' }});
  if (req.method === 'POST' && req.url === '/v1/capture/stop') return send(res, 200, {{ ok: true }});

  if (req.method === 'POST' && req.url === '/v1/analyze') {{
    const body = await readJson(req);
    return send(res, 200, {{
      artifacts: [{{
        relative_path: 'starter-analysis.md',
        mime: 'text/markdown',
        content: `# Starter analysis\n\nRan ${{body.analysis}} for ${{body.date}}.`
      }}],
      graph_overlays: [],
      warnings: []
    }});
  }}

  send(res, 404, {{ error: 'not found' }});
}});

server.listen(port, '127.0.0.1', () => {{
  console.log(`{id} listening on 127.0.0.1:${{port}}`);
}});
"#
    ))
}

fn write_external_connector_config(package: &str, connector: &str, enabled: bool) -> Result<()> {
    let config_path = alvum_core::config::config_path();
    let mut doc: toml::Table = if config_path.exists() {
        std::fs::read_to_string(&config_path)?.parse()?
    } else {
        toml::to_string(&alvum_core::config::AlvumConfig::default())?.parse()?
    };
    let connectors = doc
        .entry("connectors".to_string())
        .or_insert_with(|| toml::Value::Table(toml::Table::new()))
        .as_table_mut()
        .context("connectors is not a table")?;
    let key = if connector == "main" {
        package.to_string()
    } else {
        format!("{package}-{connector}")
    };
    let mut table = toml::Table::new();
    table.insert("enabled".into(), toml::Value::Boolean(enabled));
    table.insert("kind".into(), toml::Value::String("external-http".into()));
    table.insert("package".into(), toml::Value::String(package.into()));
    table.insert("connector".into(), toml::Value::String(connector.into()));
    connectors.insert(key, toml::Value::Table(table));
    if let Some(parent) = config_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(config_path, toml::to_string_pretty(&doc)?)?;
    Ok(())
}

fn disable_extension_config(package: &str) -> Result<()> {
    let config_path = alvum_core::config::config_path();
    if !config_path.exists() {
        return Ok(());
    }
    let mut doc: toml::Table = std::fs::read_to_string(&config_path)?.parse()?;
    if let Some(connectors) = doc.get_mut("connectors").and_then(|v| v.as_table_mut()) {
        for (_name, value) in connectors.iter_mut() {
            let Some(table) = value.as_table_mut() else {
                continue;
            };
            if table.get("package").and_then(|v| v.as_str()) == Some(package) {
                table.insert("enabled".into(), toml::Value::Boolean(false));
            }
        }
    }
    std::fs::write(config_path, toml::to_string_pretty(&doc)?)?;
    Ok(())
}
