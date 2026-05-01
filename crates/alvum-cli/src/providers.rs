use anyhow::{Context, Result, bail};
use chrono::Utc;
use clap::Subcommand;
use std::collections::{BTreeSet, HashMap};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;
use tokio::io::AsyncReadExt;

use crate::config_doc;

mod capabilities;

use capabilities::{
    ProviderCapabilities, ProviderSelectedModels, bedrock_capabilities_from_catalog,
    default_image_model_for, provider_capabilities, provider_selected_models,
    static_provider_capabilities,
};

#[derive(Subcommand)]
pub(crate) enum Action {
    /// Output JSON describing every provider's availability, active status,
    /// selected models, and modality capabilities.
    List {
        /// Accepted for parity with other app-facing list commands. Provider
        /// list output is always JSON.
        #[arg(long)]
        json: bool,
    },

    /// Make a tiny `Reply with OK` call against a provider and report
    /// whether auth + connectivity work end-to-end. When --model is
    /// omitted, provider-native defaults are used where possible.
    Test {
        #[arg(long)]
        provider: String,
        #[arg(long)]
        model: Option<String>,
        /// Provider-side timeout for the live ping. Manual pings default
        /// longer than background probes to allow cold CLI/back-end startup.
        #[arg(long, default_value_t = PROVIDER_MANUAL_TEST_TIMEOUT_SECS)]
        timeout_secs: u64,
    },

    /// Output JSON model options for a provider. Uses live provider
    /// catalogs when available, with safe defaults as fallback options.
    Models {
        #[arg(long)]
        provider: String,
    },

    /// Output JSON identity diagnostics for a provider.
    Identity {
        #[arg(long)]
        provider: String,
    },

    /// Download a provider model through the provider's native tooling.
    /// v1 supports Ollama via `ollama pull <model>`.
    InstallModel {
        #[arg(long)]
        provider: String,
        #[arg(long)]
        model: String,
    },

    /// First-run bootstrap: live-ping detected providers and enable only
    /// the providers that pass. Safe to call repeatedly; it skips after
    /// the first successful bootstrap unless --force is passed.
    Bootstrap {
        #[arg(long)]
        force: bool,
    },

    /// Save provider config from a JSON object on stdin. Secrets are
    /// written to macOS Keychain, not config.toml.
    Configure { provider: String },

    /// Set the [pipeline] provider config key.
    SetActive { provider: String },

    /// Add a built-in provider back to Alvum's managed provider list.
    Enable { provider: String },

    /// Remove a built-in provider from Alvum's managed provider list.
    Disable { provider: String },
}

pub(crate) async fn run(action: Action) -> Result<()> {
    match action {
        Action::List { json } => cmd_providers_list(json).await,
        Action::Test {
            provider,
            model,
            timeout_secs,
        } => {
            let model = match model {
                Some(model) => model,
                None => default_model_for_probe(&provider).await,
            };
            cmd_providers_test(&provider, &model, provider_test_timeout(timeout_secs)).await
        }
        Action::Models { provider } => cmd_providers_models(&provider).await,
        Action::Identity { provider } => cmd_providers_identity(&provider).await,
        Action::InstallModel { provider, model } => {
            cmd_providers_install_model(&provider, &model).await
        }
        Action::Bootstrap { force } => cmd_providers_bootstrap(force).await,
        Action::Configure { provider } => cmd_providers_configure(&provider),
        Action::SetActive { provider } => cmd_providers_set_active(&provider),
        Action::Enable { provider } => cmd_providers_set_enabled(&provider, true),
        Action::Disable { provider } => cmd_providers_set_enabled(&provider, false),
    }
}

pub(crate) fn normalize_name(provider: &str) -> String {
    match provider {
        "claude" => "claude-cli".to_string(),
        "cli" => "claude-cli".to_string(),
        "codex" => "codex-cli".to_string(),
        "api" => "anthropic-api".to_string(),
        other => other.to_string(),
    }
}
/// command can't share a single default across providers.
///
/// Empty string is a valid return — for codex-cli we want to defer
/// entirely to the user's ~/.codex/config.toml default, since model
/// names there can be arbitrary (gpt-5, gpt-5.5, o3, etc.) and we
/// can't pick one that's guaranteed to exist.
fn default_model_for(provider: &str) -> &'static str {
    match provider {
        "claude" | "cli" | "claude-cli" => "", // let Claude CLI use its configured backend default
        "codex" | "codex-cli" => "",           // let codex pick from its config
        "ollama" => "",
        "bedrock" => "",
        // anthropic-api / api / auto / unknown
        _ => "claude-sonnet-4-6",
    }
}

fn canonical_text_model_for_provider(provider: &str, model: &str) -> String {
    let trimmed = model.trim();
    match provider {
        "claude" | "cli" | "claude-cli" if trimmed.is_empty() || trimmed == "claude-sonnet-4-6" => {
            String::new()
        }
        "codex" | "codex-cli" if trimmed.is_empty() || trimmed.starts_with("claude-") => {
            String::new()
        }
        _ => trimmed.to_string(),
    }
}

fn canonical_modality_model_for_provider(provider: &str, model: &str) -> String {
    let trimmed = model.trim();
    match provider {
        "claude" | "cli" | "claude-cli" if trimmed.is_empty() || trimmed == "claude-sonnet-4-6" => {
            String::new()
        }
        "codex" | "codex-cli" if trimmed.is_empty() || trimmed.starts_with("claude-") => {
            String::new()
        }
        _ => trimmed.to_string(),
    }
}

pub(super) fn display_text_model_for_provider(provider: &str, model: &str) -> String {
    let canonical = canonical_text_model_for_provider(provider, model);
    if canonical.is_empty()
        && matches!(
            provider,
            "claude" | "cli" | "claude-cli" | "codex" | "codex-cli"
        )
    {
        "CLI default".into()
    } else {
        canonical
    }
}

pub(super) fn display_modality_model_for_provider(provider: &str, model: &str) -> String {
    let canonical = canonical_modality_model_for_provider(provider, model);
    if canonical.is_empty()
        && matches!(
            provider,
            "claude" | "cli" | "claude-cli" | "codex" | "codex-cli"
        )
    {
        "CLI default".into()
    } else {
        canonical
    }
}

async fn default_model_for_config(provider: &str) -> String {
    let normalized = normalize_name(provider);
    let config = alvum_core::config::AlvumConfig::load()
        .unwrap_or_else(|_| alvum_core::config::AlvumConfig::default());
    if normalized == "ollama" {
        if let Ok(catalog) =
            tokio::time::timeout(PROVIDER_MODELS_TIMEOUT, ollama_model_catalog(&config))
                .await
                .unwrap_or_else(|_| Err(anyhow::anyhow!("Ollama model lookup timed out")))
        {
            if let Some(model) = resolve_ollama_selected_models(&config, &catalog).text {
                return model;
            }
        }
    }
    if let Some(model) = provider_setting_string(&config, &normalized, "text_model")
        .or_else(|| provider_setting_string(&config, &normalized, "model"))
    {
        canonical_text_model_for_provider(&normalized, &model)
    } else {
        default_model_for(&normalized).into()
    }
}

async fn default_model_for_probe(provider: &str) -> String {
    let normalized = normalize_name(provider);
    if normalized == "claude-cli" {
        default_model_for(&normalized).into()
    } else {
        default_model_for_config(&normalized).await
    }
}

/// Each entry the popover renders. `available` reflects the cheap
/// detection check; an entry that's `available` may still fail at call
/// time if the provider's own auth/backend setup is incomplete. The
/// Test action proves end-to-end auth.
#[derive(serde::Serialize)]
struct ProviderInfo {
    pub(crate) name: &'static str,
    display_name: &'static str,
    description: &'static str,
    enabled: bool,
    pub(crate) available: bool,
    pub(crate) auth_hint: &'static str,
    setup_kind: &'static str,
    setup_label: &'static str,
    setup_hint: &'static str,
    setup_command: Option<&'static str>,
    setup_url: Option<&'static str>,
    setup_actions: Vec<ProviderSetupAction>,
    config_fields: Vec<ProviderConfigField>,
    selected_models: ProviderSelectedModels,
    resolved_model: Option<String>,
    resolved_model_source: Option<String>,
    resolved_model_kind: Option<String>,
    capabilities: ProviderCapabilities,
    readiness: ProviderReadiness,
    active: bool,
}

#[derive(Clone, serde::Serialize)]
struct ProviderSetupAction {
    id: &'static str,
    label: &'static str,
    kind: &'static str,
    detail: &'static str,
}

#[derive(Clone, serde::Serialize)]
struct ProviderReadiness {
    status: String,
    detail: String,
}

#[derive(Clone, serde::Serialize)]
struct ProviderConfigField {
    key: &'static str,
    label: &'static str,
    kind: &'static str,
    secret: bool,
    configured: bool,
    value: Option<String>,
    placeholder: &'static str,
    detail: &'static str,
    group: &'static str,
    options: Vec<ProviderModelOption>,
}

#[derive(Clone, serde::Serialize)]
struct ProviderModelOption {
    value: String,
    label: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    detail: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    input_support: Option<ProviderModelInputSupport>,
}

#[derive(Clone, serde::Serialize)]
struct ProviderInstallableModel {
    value: String,
    label: String,
    detail: String,
    input_support: ProviderModelInputSupport,
    provenance: String,
}

#[derive(Clone, Default, serde::Serialize)]
struct ProviderModelInputSupport {
    text: bool,
    image: bool,
    audio: bool,
}

#[derive(Clone, Default, serde::Serialize)]
struct ProviderModelOptionsByModality {
    text: Vec<ProviderModelOption>,
    image: Vec<ProviderModelOption>,
    audio: Vec<ProviderModelOption>,
}

#[derive(serde::Serialize)]
struct ProviderListReport {
    /// Whatever's recorded in `[pipeline] provider` — may be "auto" or a
    /// concrete name. The popover shows this as the user's stated
    /// preference; if it's "auto", `auto_resolved` tells you which
    /// concrete provider auto would pick today.
    configured: String,
    /// Concrete provider auto would currently select. None if none
    /// authenticate.
    auto_resolved: Option<&'static str>,
    providers: Vec<ProviderInfo>,
}

#[derive(Clone)]
pub(crate) struct ProviderModalityReadiness {
    pub(crate) status: String,
    pub(crate) level: String,
    pub(crate) detail: String,
}

const SCREEN_READINESS_CAPABILITY_TIMEOUT: Duration = Duration::from_secs(2);

async fn cmd_providers_list(_json: bool) -> Result<()> {
    let config = alvum_core::config::AlvumConfig::load()
        .unwrap_or_else(|_| alvum_core::config::AlvumConfig::default());
    let configured_raw = config.pipeline.provider.clone();
    // Legacy aliases — old install.sh wrote "cli", we now use canonical
    // provider names everywhere. Treat the legacy short form as equivalent
    // for "is this provider currently active" comparisons.
    let configured = normalize_name(&configured_raw);

    let entries = entries(&config);
    let auto_resolved = entries
        .iter()
        .find(|p| p.available && config.provider_enabled(p.name))
        .map(|p| p.name);

    let mut provider_tasks = tokio::task::JoinSet::new();
    for p in entries {
        let config = config.clone();
        let configured = configured.clone();
        provider_tasks.spawn(async move {
            provider_info_for_entry(config, p, &configured, auto_resolved).await
        });
    }

    let mut provider_infos = Vec::new();
    while let Some(result) = provider_tasks.join_next().await {
        provider_infos.push(result?);
    }
    let provider_order = known_provider_ids();
    provider_infos.sort_by_key(|provider| {
        provider_order
            .iter()
            .position(|name| *name == provider.name)
            .unwrap_or(usize::MAX)
    });

    let report = ProviderListReport {
        configured,
        auto_resolved,
        providers: provider_infos,
    };
    println!("{}", serde_json::to_string_pretty(&report)?);
    Ok(())
}

async fn provider_info_for_entry(
    config: alvum_core::config::AlvumConfig,
    p: Entry,
    configured: &str,
    auto_resolved: Option<&'static str>,
) -> ProviderInfo {
    let bedrock_catalog = if p.name == "bedrock" {
        tokio::time::timeout(PROVIDER_MODELS_TIMEOUT, bedrock_catalog(&config))
            .await
            .ok()
            .and_then(Result::ok)
    } else {
        None
    };
    let selected_models = if p.name == "bedrock" {
        bedrock_catalog
            .as_ref()
            .map(|catalog| resolve_bedrock_selected_models(&config, catalog))
            .unwrap_or_else(|| provider_selected_models(&config, p.name))
    } else {
        selected_models_for_provider(&config, p.name).await
    };
    let capabilities = if let Some(catalog) = bedrock_catalog.as_ref() {
        bedrock_capabilities_from_catalog(catalog, &selected_models)
    } else {
        provider_capabilities(&config, p.name, &selected_models).await
    };
    let readiness = provider_readiness(p.available, config.provider_enabled(p.name));
    let config_fields = if p.name == "bedrock" {
        provider_config_fields_with_bedrock_catalog(
            &config,
            &selected_models,
            bedrock_catalog.as_ref(),
        )
    } else {
        provider_config_fields_with_selected_models(&config, p.name, &selected_models).await
    };
    let resolved_model = if let Some(catalog) = bedrock_catalog.as_ref() {
        let configured = provider_setting_string(&config, "bedrock", "text_model")
            .or_else(|| provider_setting_string(&config, "bedrock", "model"))
            .or_else(|| selected_models.text.clone());
        catalog
            .resolve_invoke_target(configured.as_deref(), "text")
            .ok()
    } else {
        resolved_model_for_provider(&config, p.name, &selected_models).await
    };
    ProviderInfo {
        name: p.name,
        display_name: p.display_name,
        description: p.description,
        enabled: config.provider_enabled(p.name),
        available: p.available,
        auth_hint: p.auth_hint,
        setup_kind: p.setup_kind,
        setup_label: p.setup_label,
        setup_hint: p.setup_hint,
        setup_command: p.setup_command,
        setup_url: p.setup_url,
        setup_actions: provider_setup_actions(&config, p.name),
        config_fields,
        selected_models,
        resolved_model: resolved_model
            .as_ref()
            .map(|target| target.invoke_id.clone()),
        resolved_model_source: resolved_model.as_ref().map(|target| target.source.clone()),
        resolved_model_kind: resolved_model.as_ref().map(|target| match target.kind {
            alvum_pipeline::bedrock::BedrockInvokeTargetKind::BaseModel => "base_model".into(),
            alvum_pipeline::bedrock::BedrockInvokeTargetKind::InferenceProfile => {
                "inference_profile".into()
            }
        }),
        capabilities,
        readiness,
        active: configured == p.name || (configured == "auto" && Some(p.name) == auto_resolved),
    }
}

fn provider_readiness(available: bool, enabled: bool) -> ProviderReadiness {
    if !enabled {
        ProviderReadiness {
            status: "disabled".into(),
            detail: "Provider is disabled in Alvum.".into(),
        }
    } else if available {
        ProviderReadiness {
            status: "available".into(),
            detail: "Provider setup is detectable.".into(),
        }
    } else {
        ProviderReadiness {
            status: "setup_required".into(),
            detail: "Provider setup is not detectable yet.".into(),
        }
    }
}

async fn selected_models_for_provider(
    config: &alvum_core::config::AlvumConfig,
    provider: &str,
) -> ProviderSelectedModels {
    if provider == "bedrock" {
        return match tokio::time::timeout(PROVIDER_MODELS_TIMEOUT, bedrock_catalog(config)).await {
            Ok(Ok(catalog)) => resolve_bedrock_selected_models(config, &catalog),
            _ => provider_selected_models(config, provider),
        };
    }
    if provider != "ollama" {
        return provider_selected_models(config, provider);
    }
    match tokio::time::timeout(PROVIDER_MODELS_TIMEOUT, ollama_model_catalog(config)).await {
        Ok(Ok(catalog)) => resolve_ollama_selected_models(config, &catalog),
        _ => provider_selected_models(config, provider),
    }
}

async fn resolved_model_for_provider(
    config: &alvum_core::config::AlvumConfig,
    provider: &str,
    selected: &ProviderSelectedModels,
) -> Option<alvum_pipeline::bedrock::BedrockInvokeTarget> {
    if provider != "bedrock" {
        return None;
    }
    let configured = provider_setting_string(config, "bedrock", "text_model")
        .or_else(|| provider_setting_string(config, "bedrock", "model"))
        .or_else(|| selected.text.clone());
    tokio::time::timeout(PROVIDER_MODELS_TIMEOUT, bedrock_catalog(config))
        .await
        .ok()
        .and_then(Result::ok)
        .and_then(|catalog| {
            catalog
                .resolve_invoke_target(configured.as_deref(), "text")
                .ok()
        })
}

async fn provider_config_fields_with_selected_models(
    config: &alvum_core::config::AlvumConfig,
    provider: &str,
    selected: &ProviderSelectedModels,
) -> Vec<ProviderConfigField> {
    let mut fields = provider_config_fields(config, provider);
    if provider == "bedrock" {
        let catalog = tokio::time::timeout(PROVIDER_MODELS_TIMEOUT, bedrock_catalog(config))
            .await
            .ok()
            .and_then(Result::ok);
        return provider_config_fields_with_bedrock_catalog(config, selected, catalog.as_ref());
    }

    if provider != "ollama" {
        return fields;
    }

    let catalog = tokio::time::timeout(PROVIDER_MODELS_TIMEOUT, ollama_model_catalog(config))
        .await
        .ok()
        .and_then(Result::ok);
    let options_by_modality = catalog
        .as_ref()
        .map(|catalog| catalog.options_by_modality())
        .unwrap_or_default();
    for field in &mut fields {
        match field.key {
            "text_model" => {
                field.value = selected.text.clone();
                field.configured = provider_setting_string(config, "ollama", "text_model")
                    .or_else(|| provider_setting_string(config, "ollama", "model"))
                    .is_some();
                field.options = options_by_modality.text.clone();
            }
            "image_model" => {
                field.value = selected.image.clone();
                field.configured =
                    provider_setting_string(config, "ollama", "image_model").is_some();
                field.options = options_by_modality.image.clone();
            }
            "audio_model" => {
                field.value = selected.audio.clone();
                field.configured =
                    provider_setting_string(config, "ollama", "audio_model").is_some();
                field.options = options_by_modality.audio.clone();
            }
            _ => {}
        }
    }
    fields
}

fn provider_config_fields_with_bedrock_catalog(
    config: &alvum_core::config::AlvumConfig,
    selected: &ProviderSelectedModels,
    catalog: Option<&alvum_pipeline::bedrock::BedrockCatalog>,
) -> Vec<ProviderConfigField> {
    let mut fields = provider_config_fields(config, "bedrock");
    let options_by_modality = catalog.map(bedrock_options_by_modality).unwrap_or_default();
    for field in &mut fields {
        match field.key {
            "text_model" => {
                field.value = selected.text.clone();
                field.configured = provider_setting_string(config, "bedrock", "text_model")
                    .or_else(|| provider_setting_string(config, "bedrock", "model"))
                    .is_some();
                field.options = model_options_with_config_for_field(
                    config,
                    "bedrock",
                    field.key,
                    options_by_modality.text.clone(),
                );
            }
            "image_model" => {
                field.value = selected.image.clone();
                field.configured =
                    provider_setting_string(config, "bedrock", "image_model").is_some();
                field.options = model_options_with_config_for_field(
                    config,
                    "bedrock",
                    field.key,
                    options_by_modality.image.clone(),
                );
            }
            "audio_model" => {
                field.value = selected.audio.clone();
                field.configured =
                    provider_setting_string(config, "bedrock", "audio_model").is_some();
                field.options = model_options_with_config_for_field(
                    config,
                    "bedrock",
                    field.key,
                    options_by_modality.audio.clone(),
                );
            }
            _ => {}
        }
    }
    fields
}

fn image_adapter_supported(provider: &str) -> bool {
    matches!(provider, "anthropic-api" | "ollama")
}

fn screen_modality_readiness_from_capability(
    provider: &str,
    display_name: &str,
    capabilities: &ProviderCapabilities,
) -> ProviderModalityReadiness {
    let image = &capabilities.image;
    if image.supported {
        return ProviderModalityReadiness {
            status: "ready".into(),
            level: "ok".into(),
            detail: format!(
                "Provider screen mode is ready through {display_name} ({provider}); {}",
                image.detail
            ),
        };
    }
    if !image.adapter_supported {
        return ProviderModalityReadiness {
            status: "unsupported_adapter".into(),
            level: "warning".into(),
            detail: format!(
                "{display_name} ({provider}) cannot receive images through Alvum's provider adapter yet."
            ),
        };
    }
    if !image.model_supported {
        return ProviderModalityReadiness {
            status: "unsupported_model".into(),
            level: "warning".into(),
            detail: image.detail.clone(),
        };
    }
    ProviderModalityReadiness {
        status: "requires_image_provider".into(),
        level: "warning".into(),
        detail: "Provider screen mode requires both an image-capable selected model and an Alvum adapter that can send images.".into(),
    }
}

async fn screen_modality_readiness_for_entry(
    config: &alvum_core::config::AlvumConfig,
    entry: Entry,
) -> ProviderModalityReadiness {
    if !config.provider_enabled(entry.name) {
        return ProviderModalityReadiness {
            status: "provider_disabled".into(),
            level: "warning".into(),
            detail: format!(
                "Provider screen mode is blocked because {} is removed from Alvum's provider list.",
                entry.display_name
            ),
        };
    }
    if !entry.available {
        return ProviderModalityReadiness {
            status: "provider_setup_required".into(),
            level: "warning".into(),
            detail: format!(
                "Provider screen mode is waiting for {} setup: {}.",
                entry.display_name, entry.auth_hint
            ),
        };
    }
    if !image_adapter_supported(entry.name) {
        return ProviderModalityReadiness {
            status: "unsupported_adapter".into(),
            level: "warning".into(),
            detail: format!(
                "{} is available, but Alvum's {} adapter cannot send image input yet.",
                entry.display_name, entry.name
            ),
        };
    }

    let selected = selected_models_for_provider(config, entry.name).await;
    let capabilities = tokio::time::timeout(
        SCREEN_READINESS_CAPABILITY_TIMEOUT,
        provider_capabilities(config, entry.name, &selected),
    )
    .await
    .unwrap_or_else(|_| static_provider_capabilities(entry.name, &selected));
    screen_modality_readiness_from_capability(entry.name, entry.display_name, &capabilities)
}

pub(crate) async fn screen_provider_readiness(
    config: &alvum_core::config::AlvumConfig,
) -> ProviderModalityReadiness {
    let entries = entries(config);
    let configured = normalize_name(&config.pipeline.provider);
    if configured != "auto" {
        if let Some(entry) = entries
            .iter()
            .find(|entry| entry.name == configured)
            .copied()
        {
            return screen_modality_readiness_for_entry(config, entry).await;
        }
        return ProviderModalityReadiness {
            status: "provider_unknown".into(),
            level: "warning".into(),
            detail: format!("Configured provider {configured} is not recognized."),
        };
    }

    let enabled = entries
        .into_iter()
        .filter(|entry| config.provider_enabled(entry.name))
        .collect::<Vec<_>>();
    if enabled.is_empty() {
        return ProviderModalityReadiness {
            status: "requires_image_provider".into(),
            level: "warning".into(),
            detail: "Provider screen mode needs an enabled provider that can send images.".into(),
        };
    }

    let mut adapter_limited = None;
    let mut setup_required = None;
    let mut model_blocked = None;
    for entry in enabled {
        if !entry.available {
            if image_adapter_supported(entry.name) && setup_required.is_none() {
                setup_required = Some(ProviderModalityReadiness {
                    status: "provider_setup_required".into(),
                    level: "warning".into(),
                    detail: format!(
                        "Provider screen mode can use {}, but setup is not detectable yet: {}.",
                        entry.display_name, entry.auth_hint
                    ),
                });
            }
            continue;
        }
        if !image_adapter_supported(entry.name) {
            if adapter_limited.is_none() {
                adapter_limited = Some(ProviderModalityReadiness {
                    status: "unsupported_adapter".into(),
                    level: "warning".into(),
                    detail: format!(
                        "{} is available, but Alvum's {} adapter cannot send image input yet.",
                        entry.display_name, entry.name
                    ),
                });
            }
            continue;
        }

        let readiness = screen_modality_readiness_for_entry(config, entry).await;
        if readiness.status == "ready" {
            return readiness;
        }
        if model_blocked.is_none() {
            model_blocked = Some(readiness);
        }
    }

    model_blocked
        .or(setup_required)
        .or(adapter_limited)
        .unwrap_or_else(|| ProviderModalityReadiness {
            status: "requires_image_provider".into(),
            level: "warning".into(),
            detail: "Provider screen mode needs an enabled provider that can send images.".into(),
        })
}

fn known_provider_name(provider: &str) -> bool {
    provider == "auto" || known_provider_ids().iter().any(|entry| *entry == provider)
}

#[derive(Clone, Copy)]
pub(crate) struct Entry {
    pub(crate) name: &'static str,
    display_name: &'static str,
    description: &'static str,
    pub(crate) available: bool,
    pub(crate) auth_hint: &'static str,
    setup_kind: &'static str,
    setup_label: &'static str,
    setup_hint: &'static str,
    setup_command: Option<&'static str>,
    setup_url: Option<&'static str>,
}

fn known_provider_ids() -> [&'static str; 5] {
    [
        "claude-cli",
        "codex-cli",
        "anthropic-api",
        "bedrock",
        "ollama",
    ]
}

pub(crate) fn entries(config: &alvum_core::config::AlvumConfig) -> Vec<Entry> {
    vec![
        Entry {
            name: "claude-cli",
            display_name: "Claude CLI",
            description: "Uses whichever backend the installed Claude CLI is configured to use.",
            available: cli_binary_on_path("claude"),
            auth_hint: "configure Claude CLI auth/backend",
            setup_kind: "instructions",
            setup_label: "Setup",
            setup_hint: "Configure Claude CLI directly for subscription, API key, Bedrock, Vertex, or another supported backend, then Ping. Alvum uses the CLI default model unless you set an override.",
            setup_command: None,
            setup_url: None,
        },
        Entry {
            name: "codex-cli",
            display_name: "Codex CLI",
            description: "Uses the Codex CLI subscription already logged in on this Mac.",
            available: cli_binary_on_path("codex"),
            auth_hint: "subscription via `codex login`",
            setup_kind: "terminal",
            setup_label: "Login",
            setup_hint: "Opens Terminal and runs `codex login`.",
            setup_command: Some("codex login"),
            setup_url: None,
        },
        Entry {
            name: "anthropic-api",
            display_name: "Anthropic API",
            description: "Uses an Anthropic API key stored in Keychain or the Alvum process environment.",
            available: anthropic_api_key_present(),
            auth_hint: "add an Anthropic API key",
            setup_kind: "inline",
            setup_label: "Setup",
            setup_hint: "Enter an Anthropic API key. Alvum stores it in macOS Keychain.",
            setup_command: None,
            setup_url: Some("https://console.anthropic.com/settings/keys"),
        },
        Entry {
            name: "bedrock",
            display_name: "AWS Bedrock",
            description: "Uses AWS credentials and Anthropic-on-Bedrock models.",
            available: aws_credentials_present(config),
            auth_hint: "configure an AWS profile or credentials",
            setup_kind: "inline",
            setup_label: "Setup",
            setup_hint: "Choose an AWS profile and region. Credentials come from the standard AWS chain, including env vars, profile files, SSO, credential_process, and IAM roles.",
            setup_command: None,
            setup_url: None,
        },
        Entry {
            name: "ollama",
            display_name: "Ollama",
            description: "Uses a local Ollama server and local model.",
            available: cli_binary_on_path("ollama"),
            auth_hint: "install from ollama.com and `ollama run <model>`",
            setup_kind: "inline",
            setup_label: "Setup",
            setup_hint: "Set the local Ollama URL and model. `ollama serve` starts the server; if it says the address is already in use, Ollama is already running.",
            setup_command: Some("ollama serve"),
            setup_url: Some("https://ollama.com/download"),
        },
    ]
}

fn setup_action(
    id: &'static str,
    label: &'static str,
    kind: &'static str,
    detail: &'static str,
) -> ProviderSetupAction {
    ProviderSetupAction {
        id,
        label,
        kind,
        detail,
    }
}

fn provider_setup_actions(
    _config: &alvum_core::config::AlvumConfig,
    provider: &str,
) -> Vec<ProviderSetupAction> {
    match provider {
        "claude-cli" => vec![
            setup_action(
                "claude_doctor",
                "Run Claude doctor",
                "terminal",
                "Open Terminal and run Claude CLI's backend-agnostic diagnostic.",
            ),
            setup_action(
                "edit_extra_path",
                "Set backend PATH",
                "inline",
                "Set extra PATH directories for backend helper tools used by Claude CLI.",
            ),
            setup_action(
                "open_claude_config",
                "Open Claude config",
                "folder",
                "Open ~/.claude so you can inspect whichever Claude CLI backend is configured.",
            ),
        ],
        "codex-cli" => vec![
            setup_action(
                "codex_login",
                "Log in",
                "terminal",
                "Open Terminal and run codex login.",
            ),
            setup_action(
                "codex_models",
                "List models",
                "terminal",
                "Open Terminal and run codex debug models --bundled.",
            ),
            setup_action(
                "open_codex_config",
                "Open Codex config",
                "file",
                "Open ~/.codex/config.toml, or the ~/.codex folder if the file does not exist.",
            ),
        ],
        "anthropic-api" => vec![
            setup_action(
                "anthropic_keys",
                "Open API keys",
                "url",
                "Open the Anthropic console API key page.",
            ),
            setup_action(
                "anthropic_models",
                "Open model docs",
                "url",
                "Open Anthropic model documentation.",
            ),
            setup_action(
                "edit_anthropic_key",
                "Edit API key",
                "inline",
                "Focus the API key field below; Alvum stores the value in macOS Keychain.",
            ),
        ],
        "bedrock" => vec![
            setup_action(
                "open_aws_config",
                "Open AWS config",
                "folder",
                "Open ~/.aws so you can inspect profiles, SSO, credential_process, and credentials.",
            ),
            setup_action(
                "bedrock_refresh_catalog",
                "Refresh catalog",
                "inline",
                "Refresh Bedrock model and inference profile options through the AWS SDK.",
            ),
            setup_action(
                "aws_sts",
                "Check AWS identity",
                "inline",
                "Check AWS caller identity through the AWS SDK using the configured profile, region, and helper PATH.",
            ),
            setup_action(
                "edit_extra_path",
                "Set helper PATH",
                "inline",
                "Set extra PATH directories for AWS credential_process helpers such as isengardcli.",
            ),
            setup_action(
                "bedrock_list_models",
                "List with AWS CLI",
                "terminal",
                "Optional AWS CLI fallback: run aws bedrock list-foundation-models with the configured profile and region.",
            ),
        ],
        "ollama" => vec![
            setup_action(
                "ollama_download",
                "Install Ollama",
                "url",
                "Open the Ollama download page.",
            ),
            setup_action(
                "ollama_serve",
                "Start server",
                "terminal",
                "Open Terminal and run ollama serve.",
            ),
            setup_action(
                "ollama_list",
                "List models",
                "terminal",
                "Open Terminal and run ollama list.",
            ),
            setup_action(
                "ollama_show_text",
                "Inspect text model",
                "terminal",
                "Run ollama show for the selected text model.",
            ),
            setup_action(
                "ollama_show_image",
                "Inspect image model",
                "terminal",
                "Run ollama show for the selected image model.",
            ),
        ],
        _ => vec![],
    }
}

fn provider_setting_string(
    config: &alvum_core::config::AlvumConfig,
    provider: &str,
    key: &str,
) -> Option<String> {
    config
        .provider(provider)
        .and_then(|provider| provider.settings.get(key))
        .and_then(toml_value_to_string)
        .filter(|value| !value.trim().is_empty())
}

fn toml_value_to_string(value: &toml::Value) -> Option<String> {
    match value {
        toml::Value::String(s) => Some(s.clone()),
        toml::Value::Integer(n) => Some(n.to_string()),
        toml::Value::Float(n) => Some(n.to_string()),
        toml::Value::Boolean(v) => Some(v.to_string()),
        _ => None,
    }
}

fn provider_field_group(key: &str) -> &'static str {
    if key == "model" || key.ends_with("_model") {
        "models"
    } else if key == "extra_path" {
        "connection"
    } else {
        "connection"
    }
}

fn config_field(
    config: &alvum_core::config::AlvumConfig,
    provider: &str,
    key: &'static str,
    label: &'static str,
    kind: &'static str,
    detail: &'static str,
    placeholder: &'static str,
) -> ProviderConfigField {
    let configured = provider_setting_string(config, provider, key).is_some();
    let value = provider_setting_string(config, provider, key).map(|value| {
        if key == "text_model" || key == "model" {
            canonical_text_model_for_provider(provider, &value)
        } else if key == "image_model" || key == "audio_model" {
            canonical_modality_model_for_provider(provider, &value)
        } else {
            value
        }
    });
    let options = if key == "model" || key.ends_with("_model") {
        static_model_options_for_field(provider, key)
    } else {
        vec![]
    };
    ProviderConfigField {
        key,
        label,
        kind,
        secret: false,
        configured,
        value,
        placeholder,
        detail,
        group: provider_field_group(key),
        options,
    }
}

fn secret_field(
    provider: &str,
    key: &'static str,
    label: &'static str,
    detail: &'static str,
) -> ProviderConfigField {
    ProviderConfigField {
        key,
        label,
        kind: "secret",
        secret: true,
        configured: provider_secret_present(provider, key),
        value: None,
        placeholder: "Stored in Keychain",
        detail,
        group: provider_field_group(key),
        options: vec![],
    }
}

fn provider_config_fields(
    config: &alvum_core::config::AlvumConfig,
    provider: &str,
) -> Vec<ProviderConfigField> {
    match provider {
        "anthropic-api" => vec![
            secret_field(
                provider,
                "api_key",
                "API key",
                "Stored in macOS Keychain. Environment variable ANTHROPIC_API_KEY still works.",
            ),
            config_field(
                config,
                provider,
                "text_model",
                "Text model",
                "text",
                "Default model for Anthropic API calls.",
                default_model_for(provider),
            ),
            config_field(
                config,
                provider,
                "image_model",
                "Image model",
                "text",
                "Model used when screen processing sends images through Anthropic API.",
                default_image_model_for(provider),
            ),
            config_field(
                config,
                provider,
                "audio_model",
                "Audio model",
                "text",
                "Reserved for provider audio processing; no Alvum audio adapter exists yet.",
                "",
            ),
        ],
        "bedrock" => vec![
            config_field(
                config,
                provider,
                "aws_profile",
                "AWS profile",
                "text",
                "Optional AWS profile name from ~/.aws/config or ~/.aws/credentials.",
                "default",
            ),
            config_field(
                config,
                provider,
                "aws_region",
                "AWS region",
                "text",
                "Optional AWS region for Bedrock.",
                "us-east-1",
            ),
            config_field(
                config,
                provider,
                "extra_path",
                "Credential helper PATH",
                "text",
                "Optional colon-separated directories containing AWS credential_process helpers such as isengardcli.",
                "",
            ),
            config_field(
                config,
                provider,
                "text_model",
                "Text model",
                "text",
                "Bedrock model ID or inference profile ID.",
                default_model_for(provider),
            ),
            config_field(
                config,
                provider,
                "image_model",
                "Image model",
                "text",
                "Model ID checked for image capability; Bedrock image transport is not implemented yet.",
                default_image_model_for(provider),
            ),
            config_field(
                config,
                provider,
                "audio_model",
                "Audio model",
                "text",
                "Reserved for provider audio processing; no Alvum audio adapter exists yet.",
                "",
            ),
        ],
        "ollama" => vec![
            config_field(
                config,
                provider,
                "base_url",
                "Server URL",
                "url",
                "Local Ollama API endpoint.",
                "http://localhost:11434",
            ),
            config_field(
                config,
                provider,
                "text_model",
                "Text model",
                "text",
                "Local model to use for synthesis.",
                default_model_for(provider),
            ),
            config_field(
                config,
                provider,
                "image_model",
                "Image model",
                "text",
                "Local model to use for provider-backed screen processing.",
                default_image_model_for(provider),
            ),
            config_field(
                config,
                provider,
                "audio_model",
                "Audio model",
                "text",
                "Reserved for provider audio processing; no Alvum audio adapter exists yet.",
                "",
            ),
        ],
        "claude-cli" | "codex-cli" => vec![
            config_field(
                config,
                provider,
                "text_model",
                "Text model",
                "text",
                "Advanced override. Leave blank to use the CLI default.",
                default_model_for(provider),
            ),
            config_field(
                config,
                provider,
                "image_model",
                "Image model",
                "text",
                "Advanced override for capability display. Leave blank to use the CLI default; this adapter is text-only today.",
                default_image_model_for(provider),
            ),
            config_field(
                config,
                provider,
                "audio_model",
                "Audio model",
                "text",
                "Advanced override reserved for future provider audio. Leave blank to use the CLI default; no Alvum audio adapter exists yet.",
                "",
            ),
        ]
        .into_iter()
        .chain(if provider == "claude-cli" {
            vec![config_field(
                config,
                provider,
                "extra_path",
                "Backend helper PATH",
                "text",
                "Optional colon-separated directories for helper tools used by the Claude CLI backend.",
                "",
            )]
        } else {
            vec![]
        })
        .collect(),
        _ => vec![],
    }
}

fn provider_config_fields_for_write(
    config: &alvum_core::config::AlvumConfig,
    provider: &str,
) -> Vec<ProviderConfigField> {
    let mut fields = provider_config_fields(config, provider);
    if !fields.iter().any(|field| field.key == "model") {
        fields.push(config_field(
            config,
            provider,
            "model",
            "Legacy text model",
            "text",
            "Legacy alias for text_model.",
            default_model_for(provider),
        ));
    }
    fields
}

fn provider_secret_present(provider: &str, key: &str) -> bool {
    match (provider, key) {
        ("anthropic-api", "api_key") if std::env::var("ANTHROPIC_API_KEY").is_ok() => true,
        _ => alvum_core::keychain::provider_secret_available(provider, key),
    }
}

fn anthropic_api_key_present() -> bool {
    provider_secret_present("anthropic-api", "api_key")
}

fn model_option(value: impl Into<String>, label: impl Into<String>) -> ProviderModelOption {
    ProviderModelOption {
        value: value.into(),
        label: label.into(),
        detail: None,
        input_support: None,
    }
}

fn bedrock_model_option(
    target: alvum_pipeline::bedrock::BedrockInvokeTarget,
) -> ProviderModelOption {
    ProviderModelOption {
        value: target.invoke_id,
        label: target.label,
        detail: Some(target.detail),
        input_support: Some(ProviderModelInputSupport {
            text: target.input_support.text,
            image: target.input_support.image,
            audio: target.input_support.audio,
        }),
    }
}

fn installable_model(
    value: impl Into<String>,
    label: impl Into<String>,
    detail: impl Into<String>,
    input_support: ProviderModelInputSupport,
    provenance: impl Into<String>,
) -> ProviderInstallableModel {
    ProviderInstallableModel {
        value: value.into(),
        label: label.into(),
        detail: detail.into(),
        input_support,
        provenance: provenance.into(),
    }
}

fn dedupe_model_options(options: Vec<ProviderModelOption>) -> Vec<ProviderModelOption> {
    let mut seen = BTreeSet::new();
    let mut deduped = Vec::new();
    for option in options {
        if option.value.trim().is_empty() && seen.contains("") {
            continue;
        }
        if !seen.insert(option.value.clone()) {
            continue;
        }
        deduped.push(option);
    }
    deduped
}

fn static_model_options(provider: &str) -> Vec<ProviderModelOption> {
    match provider {
        "claude-cli" => vec![
            model_option("", "CLI default"),
            model_option("sonnet", "Sonnet"),
            model_option("opus", "Opus"),
        ],
        "codex-cli" => vec![model_option("", "CLI default")],
        "anthropic-api" => vec![model_option(
            default_model_for(provider),
            default_model_for(provider),
        )],
        "bedrock" => vec![],
        "ollama" => vec![],
        _ => vec![],
    }
}

fn static_model_options_for_field(provider: &str, key: &str) -> Vec<ProviderModelOption> {
    if key == "image_model" {
        return match provider {
            "claude-cli" | "codex-cli" => vec![model_option("", "CLI default")],
            "ollama" => vec![],
            _ => {
                let default = default_image_model_for(provider);
                if default.is_empty() {
                    vec![]
                } else {
                    vec![model_option(default, default)]
                }
            }
        };
    }
    if key == "audio_model" {
        return match provider {
            "claude-cli" | "codex-cli" => vec![model_option("", "CLI default")],
            _ => vec![],
        };
    }
    static_model_options(provider)
}

fn cli_binary_on_path(name: &str) -> bool {
    std::process::Command::new("which")
        .arg(name)
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

fn env_var_present(name: &str) -> bool {
    std::env::var(name)
        .map(|value| !value.trim().is_empty())
        .unwrap_or(false)
}

fn aws_credentials_present(config: &alvum_core::config::AlvumConfig) -> bool {
    env_var_present("AWS_PROFILE")
        || env_var_present("AWS_ACCESS_KEY_ID")
        || env_var_present("AWS_SESSION_TOKEN")
        || provider_setting_string(config, "bedrock", "aws_profile").is_some()
        || dirs::home_dir()
            .map(|h| h.join(".aws/credentials").exists() || h.join(".aws/config").exists())
            .unwrap_or(false)
}

const PROVIDER_BACKGROUND_TEST_TIMEOUT_SECS: u64 = 25;
const PROVIDER_MANUAL_TEST_TIMEOUT_SECS: u64 = 90;
const PROVIDER_BACKGROUND_TEST_TIMEOUT: Duration =
    Duration::from_secs(PROVIDER_BACKGROUND_TEST_TIMEOUT_SECS);
const PROVIDER_MANUAL_TEST_TIMEOUT: Duration =
    Duration::from_secs(PROVIDER_MANUAL_TEST_TIMEOUT_SECS);

fn provider_test_timeout(timeout_secs: u64) -> Duration {
    match timeout_secs.clamp(1, 300) {
        PROVIDER_BACKGROUND_TEST_TIMEOUT_SECS => PROVIDER_BACKGROUND_TEST_TIMEOUT,
        PROVIDER_MANUAL_TEST_TIMEOUT_SECS => PROVIDER_MANUAL_TEST_TIMEOUT,
        seconds => Duration::from_secs(seconds),
    }
}

#[derive(Clone, serde::Serialize)]
struct ProviderTestReport {
    provider: String,
    status: String,
    ok: bool,
    elapsed_ms: u128,
    response_preview: Option<String>,
    error: Option<String>,
    resolved_model: Option<String>,
    model_source: String,
    timeout_secs: u64,
    backend_hint: String,
    recommended_setup_actions: Vec<String>,
    diagnosis: ProviderProbeDiagnosis,
}

#[derive(Clone, serde::Serialize)]
struct ProviderProbeDiagnosis {
    resolved_model: Option<String>,
    model_source: String,
    timeout_secs: u64,
    backend_hint: String,
    setup_action_ids: Vec<String>,
    detail: Option<String>,
}

fn provider_probe_model_source(provider: &str, model: &str) -> String {
    let normalized = normalize_name(provider);
    if !known_provider_name(&normalized) || normalized == "auto" {
        return "unknown".into();
    }
    let model = model.trim();
    if model.is_empty() {
        if matches!(normalized.as_str(), "claude-cli" | "codex-cli") {
            "cli_default".into()
        } else {
            "unknown".into()
        }
    } else if model == default_model_for(&normalized)
        || model == default_image_model_for(&normalized)
    {
        "catalog".into()
    } else {
        "configured".into()
    }
}

fn provider_probe_setup_action_ids(
    provider: &str,
    status: &str,
    error: Option<&str>,
) -> Vec<String> {
    let error = error.unwrap_or_default().to_lowercase();
    match provider {
        "claude-cli" => {
            let mut actions = vec!["claude_doctor".into(), "open_claude_config".into()];
            if provider_probe_error_mentions_credential_process(Some(&error)) {
                actions.push("edit_extra_path".into());
            }
            actions
        }
        "codex-cli" => vec![
            "codex_login".into(),
            "codex_models".into(),
            "open_codex_config".into(),
        ],
        "anthropic-api" => vec!["anthropic_keys".into(), "anthropic_models".into()],
        "bedrock" => {
            let mut actions = vec![
                "open_aws_config".into(),
                "bedrock_refresh_catalog".into(),
                "aws_sts".into(),
            ];
            if provider_probe_error_mentions_credential_process(Some(&error)) {
                actions.push("edit_extra_path".into());
            }
            if status != "auth_unavailable"
                || error.contains("bedrock")
                || error.contains("foundation")
                || error.contains("model")
            {
                actions.push("bedrock_list_models".into());
            }
            actions
        }
        "ollama" => {
            let mut actions = vec!["ollama_serve".into(), "ollama_list".into()];
            if status == "model_not_installed" || error.contains("model") {
                actions.push("ollama_show_text".into());
            }
            actions
        }
        _ => vec![],
    }
}

fn provider_probe_error_mentions_credential_process(error: Option<&str>) -> bool {
    let Some(error) = error else {
        return false;
    };
    let error = error.to_ascii_lowercase();
    error.contains("credential_process")
        || error.contains("profilefile provider")
        || error.contains("credentials provider")
        || error.contains("isengardcli")
        || error.contains("siengarcli")
}

fn provider_probe_backend_hint(provider: &str, error: Option<&str>) -> String {
    let credential_process = provider_probe_error_mentions_credential_process(error);
    match provider {
        "claude-cli" if credential_process => "Claude CLI is failing inside its configured backend. If that backend uses AWS credential_process, Alvum must be able to find the credential_process helper on PATH; set Backend helper PATH if the helper is outside the login shell PATH.".into(),
        "claude-cli" => "Claude CLI may be configured through subscription, API, Bedrock, Vertex, or another backend; Alvum uses the CLI default unless you set an override.".into(),
        "codex-cli" => "Codex CLI uses its own login and ~/.codex config; Alvum uses the CLI default unless you set an override.".into(),
        "anthropic-api" => "Anthropic API uses an API key stored in macOS Keychain or ANTHROPIC_API_KEY.".into(),
        "bedrock" if credential_process => "Bedrock credentials are failing while running an AWS credential_process helper. Alvum uses the standard AWS SDK credential chain; set Credential helper PATH if the helper is outside the login shell PATH, then run Check AWS identity.".into(),
        "bedrock" => "Alvum uses the standard AWS SDK credential chain, including env vars, profile files, SSO, credential_process, and IAM roles.".into(),
        "ollama" => "Ollama uses the configured local server URL and installed local models.".into(),
        _ => "Provider setup is controlled by its native tool or credentials.".into(),
    }
}

fn provider_probe_diagnosis(
    provider: &str,
    model: &str,
    timeout: Duration,
    status: &str,
    error: Option<&str>,
) -> ProviderProbeDiagnosis {
    let normalized = normalize_name(provider);
    let display_model = if known_provider_name(&normalized) && normalized != "auto" {
        display_text_model_for_provider(&normalized, model)
    } else {
        String::new()
    };
    ProviderProbeDiagnosis {
        resolved_model: (!display_model.trim().is_empty()).then_some(display_model),
        model_source: provider_probe_model_source(&normalized, model),
        timeout_secs: timeout.as_secs(),
        backend_hint: provider_probe_backend_hint(&normalized, error),
        setup_action_ids: provider_probe_setup_action_ids(&normalized, status, error),
        detail: error.map(str::to_string),
    }
}

fn provider_test_report_with_diagnosis(
    provider: String,
    status: String,
    ok: bool,
    elapsed_ms: u128,
    response_preview: Option<String>,
    error: Option<String>,
    model: &str,
    timeout: Duration,
) -> ProviderTestReport {
    let diagnosis = provider_probe_diagnosis(&provider, model, timeout, &status, error.as_deref());
    ProviderTestReport {
        provider,
        status,
        ok,
        elapsed_ms,
        response_preview,
        error,
        resolved_model: diagnosis.resolved_model.clone(),
        model_source: diagnosis.model_source.clone(),
        timeout_secs: diagnosis.timeout_secs,
        backend_hint: diagnosis.backend_hint.clone(),
        recommended_setup_actions: diagnosis.setup_action_ids.clone(),
        diagnosis,
    }
}

async fn bedrock_probe_resolved_model(model: &str) -> Option<(String, String)> {
    let config = alvum_core::config::AlvumConfig::load()
        .unwrap_or_else(|_| alvum_core::config::AlvumConfig::default());
    let configured = provider_setting_string(&config, "bedrock", "text_model")
        .or_else(|| provider_setting_string(&config, "bedrock", "model"))
        .or_else(|| {
            let model = model.trim();
            (!model.is_empty()).then(|| model.to_string())
        });
    tokio::time::timeout(PROVIDER_MODELS_TIMEOUT, bedrock_catalog(&config))
        .await
        .ok()
        .and_then(Result::ok)
        .and_then(|catalog| {
            catalog
                .resolve_invoke_target(configured.as_deref(), "text")
                .ok()
        })
        .map(|target| (target.invoke_id, target.source))
}

fn apply_bedrock_probe_resolution(
    report: &mut ProviderTestReport,
    resolved: Option<(String, String)>,
) {
    let Some((model, source)) = resolved else {
        return;
    };
    report.resolved_model = Some(model.clone());
    report.model_source = source.clone();
    report.diagnosis.resolved_model = Some(model);
    report.diagnosis.model_source = source;
}

async fn provider_test_report(
    provider_name: &str,
    model: &str,
    timeout: Duration,
) -> ProviderTestReport {
    // Tiny prompt. The expected response is "OK" — anything containing
    // it counts as success. Some providers may include leading
    // whitespace or quote marks, hence the contains() check.
    const TEST_SYSTEM: &str =
        "You are a connectivity probe. Reply with the exact word OK and nothing else.";
    const TEST_USER: &str = "ping";
    let started = std::time::Instant::now();
    let normalized = normalize_name(provider_name);

    if !known_provider_name(&normalized) || normalized == "auto" {
        return provider_test_report_with_diagnosis(
            normalized,
            "unknown_provider".into(),
            false,
            started.elapsed().as_millis(),
            None,
            Some(format!("unknown provider: {provider_name}")),
            model,
            timeout,
        );
    }

    if normalized == "ollama" {
        return ollama_provider_test_report(model, started, timeout).await;
    }

    let bedrock_resolved = if normalized == "bedrock" {
        bedrock_probe_resolved_model(model).await
    } else {
        None
    };
    let probe = async {
        let provider = alvum_pipeline::llm::create_provider_async(&normalized, model)
            .await
            .with_context(|| format!("provider construction failed for {normalized}"))?;
        provider.complete(TEST_SYSTEM, TEST_USER).await
    };

    let mut report = match tokio::time::timeout(timeout, probe).await {
        Err(_) => provider_test_report_with_diagnosis(
            normalized,
            "timeout".into(),
            false,
            started.elapsed().as_millis(),
            None,
            Some(format!(
                "provider probe timed out after {}s",
                timeout.as_secs()
            )),
            model,
            timeout,
        ),
        Ok(Ok(text)) => {
            let preview: String = text.chars().take(80).collect();
            let ok = text.to_uppercase().contains("OK");
            provider_test_report_with_diagnosis(
                normalized,
                if ok {
                    "available".into()
                } else {
                    "unexpected_response".into()
                },
                ok,
                started.elapsed().as_millis(),
                Some(preview),
                if ok {
                    None
                } else {
                    Some(format!("response did not contain 'OK': {text:?}"))
                },
                model,
                timeout,
            )
        }
        Ok(Err(e)) => provider_test_report_with_diagnosis(
            normalized,
            alvum_pipeline::llm::classify_provider_error_status(&e).into(),
            false,
            started.elapsed().as_millis(),
            None,
            Some(format!("{e:#}")),
            model,
            timeout,
        ),
    };
    apply_bedrock_probe_resolution(&mut report, bedrock_resolved);
    report
}

async fn ollama_provider_test_report(
    model: &str,
    started: std::time::Instant,
    timeout: Duration,
) -> ProviderTestReport {
    let config = alvum_core::config::AlvumConfig::load()
        .unwrap_or_else(|_| alvum_core::config::AlvumConfig::default());
    match tokio::time::timeout(timeout, ollama_model_options(&config)).await {
        Err(_) => provider_test_report_with_diagnosis(
            "ollama".into(),
            "timeout".into(),
            false,
            started.elapsed().as_millis(),
            None,
            Some(format!(
                "Ollama model list timed out after {}s",
                timeout.as_secs()
            )),
            model,
            timeout,
        ),
        Ok(Err(e)) => provider_test_report_with_diagnosis(
            "ollama".into(),
            "unavailable".into(),
            false,
            started.elapsed().as_millis(),
            None,
            Some(format!("{e:#}")),
            model,
            timeout,
        ),
        Ok(Ok((source, options))) => {
            let requested = model.trim();
            let installed = options.iter().any(|option| option.value == requested);
            let has_models = !options.is_empty();
            let ok = has_models && (requested.is_empty() || installed);
            provider_test_report_with_diagnosis(
                "ollama".into(),
                if ok {
                    "available".into()
                } else if has_models {
                    "model_not_installed".into()
                } else {
                    "no_models".into()
                },
                ok,
                started.elapsed().as_millis(),
                Some(format!(
                    "{} installed model(s) from {source}",
                    options.len()
                )),
                if ok {
                    None
                } else if has_models {
                    Some(format!(
                        "Ollama is running, but model {requested:?} is not installed. Choose an installed model or download it."
                    ))
                } else {
                    Some("Ollama is running, but no local models are installed.".into())
                },
                model,
                timeout,
            )
        }
    }
}

async fn cmd_providers_test(provider_name: &str, model: &str, timeout: Duration) -> Result<()> {
    let report = provider_test_report(provider_name, model, timeout).await;

    println!("{}", serde_json::to_string_pretty(&report)?);
    Ok(())
}

#[derive(serde::Serialize)]
struct ProviderIdentityReport {
    ok: bool,
    provider: String,
    account: Option<String>,
    arn: Option<String>,
    user_id: Option<String>,
    error: Option<String>,
}

async fn cmd_providers_identity(provider_name: &str) -> Result<()> {
    let normalized = normalize_name(provider_name);
    if normalized != "bedrock" {
        println!(
            "{}",
            serde_json::to_string_pretty(&ProviderIdentityReport {
                ok: false,
                provider: normalized,
                account: None,
                arn: None,
                user_id: None,
                error: Some(format!(
                    "identity diagnostics are not implemented for provider {provider_name}"
                )),
            })?
        );
        return Ok(());
    }

    let config = alvum_core::config::AlvumConfig::load()
        .unwrap_or_else(|_| alvum_core::config::AlvumConfig::default());
    let identity = async {
        let sdk_config = alvum_pipeline::bedrock::sdk_config(
            provider_setting_string(&config, "bedrock", "aws_profile"),
            provider_setting_string(&config, "bedrock", "aws_region"),
            provider_setting_string(&config, "bedrock", "extra_path"),
        )
        .await;
        aws_sdk_sts::Client::new(&sdk_config)
            .get_caller_identity()
            .send()
            .await
            .context("AWS STS GetCallerIdentity failed")
    };

    let report = match tokio::time::timeout(PROVIDER_MODELS_TIMEOUT, identity).await {
        Ok(Ok(output)) => ProviderIdentityReport {
            ok: true,
            provider: normalized,
            account: output.account().map(str::to_string),
            arn: output.arn().map(str::to_string),
            user_id: output.user_id().map(str::to_string),
            error: None,
        },
        Ok(Err(error)) => ProviderIdentityReport {
            ok: false,
            provider: normalized,
            account: None,
            arn: None,
            user_id: None,
            error: Some(format!("{error:#}")),
        },
        Err(_) => ProviderIdentityReport {
            ok: false,
            provider: normalized,
            account: None,
            arn: None,
            user_id: None,
            error: Some(format!(
                "AWS identity check timed out after {}s",
                PROVIDER_MODELS_TIMEOUT.as_secs()
            )),
        },
    };
    println!("{}", serde_json::to_string_pretty(&report)?);
    Ok(())
}

const PROVIDER_MODELS_TIMEOUT: Duration = Duration::from_secs(8);
const OLLAMA_INSTALLABLE_MODEL_LIMIT: usize = 6;

#[derive(serde::Serialize)]
struct ProviderModelsReport {
    ok: bool,
    provider: String,
    source: String,
    options: Vec<ProviderModelOption>,
    options_by_modality: ProviderModelOptionsByModality,
    installable_options: Vec<ProviderInstallableModel>,
    installable_error: Option<String>,
    error: Option<String>,
}

#[derive(Clone)]
struct OllamaModelInfo {
    option: ProviderModelOption,
    text: bool,
    image: bool,
    audio: bool,
}

#[derive(Clone)]
struct OllamaModelCatalog {
    source: String,
    models: Vec<OllamaModelInfo>,
}

impl OllamaModelCatalog {
    fn all_options(&self) -> Vec<ProviderModelOption> {
        dedupe_model_options(
            self.models
                .iter()
                .map(|model| model.option.clone())
                .collect(),
        )
    }

    fn options_by_modality(&self) -> ProviderModelOptionsByModality {
        ProviderModelOptionsByModality {
            text: dedupe_model_options(
                self.models
                    .iter()
                    .filter(|model| model.text)
                    .map(|model| model.option.clone())
                    .collect(),
            ),
            image: dedupe_model_options(
                self.models
                    .iter()
                    .filter(|model| model.image)
                    .map(|model| model.option.clone())
                    .collect(),
            ),
            audio: dedupe_model_options(
                self.models
                    .iter()
                    .filter(|model| model.audio)
                    .map(|model| model.option.clone())
                    .collect(),
            ),
        }
    }
}

#[cfg(test)]
fn ollama_model_info(
    value: impl Into<String>,
    label: impl Into<String>,
    text: bool,
    image: bool,
    audio: bool,
) -> OllamaModelInfo {
    OllamaModelInfo {
        option: model_option(value, label),
        text,
        image,
        audio,
    }
}

fn resolve_ollama_model_for_modality(
    configured: Option<String>,
    catalog: &OllamaModelCatalog,
    modality: &str,
) -> Option<String> {
    if let Some(model) = configured {
        return Some(model);
    }
    catalog
        .models
        .iter()
        .find(|model| match modality {
            "text" => model.text,
            "image" => model.image,
            "audio" => model.audio,
            _ => false,
        })
        .map(|model| model.option.value.clone())
}

fn resolve_ollama_selected_models(
    config: &alvum_core::config::AlvumConfig,
    catalog: &OllamaModelCatalog,
) -> ProviderSelectedModels {
    ProviderSelectedModels {
        text: resolve_ollama_model_for_modality(
            provider_setting_string(config, "ollama", "text_model")
                .or_else(|| provider_setting_string(config, "ollama", "model")),
            catalog,
            "text",
        ),
        image: resolve_ollama_model_for_modality(
            provider_setting_string(config, "ollama", "image_model"),
            catalog,
            "image",
        ),
        audio: resolve_ollama_model_for_modality(
            provider_setting_string(config, "ollama", "audio_model"),
            catalog,
            "audio",
        ),
    }
}

async fn bedrock_catalog(
    config: &alvum_core::config::AlvumConfig,
) -> Result<alvum_pipeline::bedrock::BedrockCatalog> {
    alvum_pipeline::bedrock::BedrockCatalog::load(
        provider_setting_string(config, "bedrock", "aws_profile"),
        provider_setting_string(config, "bedrock", "aws_region"),
        provider_setting_string(config, "bedrock", "extra_path"),
    )
    .await
}

fn resolve_bedrock_model_for_modality(
    configured: Option<String>,
    catalog: &alvum_pipeline::bedrock::BedrockCatalog,
    modality: &str,
) -> Option<String> {
    if configured.is_some() {
        return configured;
    }
    catalog
        .resolve_invoke_target(None, modality)
        .ok()
        .map(|target| target.invoke_id)
}

fn resolve_bedrock_selected_models(
    config: &alvum_core::config::AlvumConfig,
    catalog: &alvum_pipeline::bedrock::BedrockCatalog,
) -> ProviderSelectedModels {
    ProviderSelectedModels {
        text: resolve_bedrock_model_for_modality(
            provider_setting_string(config, "bedrock", "text_model")
                .or_else(|| provider_setting_string(config, "bedrock", "model")),
            catalog,
            "text",
        ),
        image: resolve_bedrock_model_for_modality(
            provider_setting_string(config, "bedrock", "image_model"),
            catalog,
            "image",
        ),
        audio: resolve_bedrock_model_for_modality(
            provider_setting_string(config, "bedrock", "audio_model"),
            catalog,
            "audio",
        ),
    }
}

fn bedrock_options_by_modality(
    catalog: &alvum_pipeline::bedrock::BedrockCatalog,
) -> ProviderModelOptionsByModality {
    ProviderModelOptionsByModality {
        text: dedupe_model_options(
            catalog
                .targets_for_modality("text")
                .into_iter()
                .map(bedrock_model_option)
                .collect(),
        ),
        image: dedupe_model_options(
            catalog
                .targets_for_modality("image")
                .into_iter()
                .map(bedrock_model_option)
                .collect(),
        ),
        audio: dedupe_model_options(
            catalog
                .targets_for_modality("audio")
                .into_iter()
                .map(bedrock_model_option)
                .collect(),
        ),
    }
}

fn configured_model_for_field(
    config: &alvum_core::config::AlvumConfig,
    provider: &str,
    key: &str,
) -> Option<String> {
    if key == "text_model" || key == "model" {
        provider_setting_string(config, provider, "text_model")
            .or_else(|| provider_setting_string(config, provider, "model"))
            .map(|model| canonical_text_model_for_provider(provider, &model))
    } else if key == "image_model" || key == "audio_model" {
        provider_setting_string(config, provider, key)
            .map(|model| canonical_modality_model_for_provider(provider, &model))
    } else {
        provider_setting_string(config, provider, key).map(|model| model.trim().to_string())
    }
}

fn model_options_with_config_for_field(
    config: &alvum_core::config::AlvumConfig,
    provider: &str,
    key: &str,
    options: Vec<ProviderModelOption>,
) -> Vec<ProviderModelOption> {
    if provider == "ollama" {
        return dedupe_model_options(options);
    }
    let mut merged = Vec::new();
    if let Some(current) = configured_model_for_field(config, provider, key) {
        if !current.is_empty() {
            merged.push(model_option(current.clone(), current));
        }
    }
    if (key == "text_model" || key == "model" || key == "image_model" || key == "audio_model")
        && matches!(provider, "claude-cli" | "codex-cli")
    {
        merged.push(model_option("", "CLI default"));
    }
    merged.extend(options);
    dedupe_model_options(merged)
}

fn live_model_options_for_field(
    provider: &str,
    key: &str,
    options: &[ProviderModelOption],
) -> Vec<ProviderModelOption> {
    if key == "audio_model" {
        if matches!(provider, "claude-cli" | "codex-cli") {
            return static_model_options_for_field(provider, key);
        }
        return vec![];
    }
    if provider == "claude-cli" && key == "image_model" {
        return static_model_options_for_field(provider, key);
    }
    options.to_vec()
}

async fn run_json_command(
    command: &str,
    args: &[String],
    timeout: Duration,
) -> Result<serde_json::Value> {
    let mut child = tokio::process::Command::new(command)
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .with_context(|| format!("failed to run {command}"))?;

    let mut stdout = child.stdout.take().context("failed to capture stdout")?;
    let mut stderr = child.stderr.take().context("failed to capture stderr")?;
    let stdout_task = tokio::spawn(async move {
        let mut bytes = Vec::new();
        stdout.read_to_end(&mut bytes).await.map(|_| bytes)
    });
    let stderr_task = tokio::spawn(async move {
        let mut bytes = Vec::new();
        stderr.read_to_end(&mut bytes).await.map(|_| bytes)
    });

    let status = match tokio::time::timeout(timeout, child.wait()).await {
        Ok(status) => status.with_context(|| format!("failed to run {command}"))?,
        Err(_) => {
            let _ = child.start_kill();
            let _ = child.wait().await;
            let _ = stdout_task.await;
            let _ = stderr_task.await;
            bail!("{command} timed out after {}s", timeout.as_secs());
        }
    };
    let stdout = stdout_task
        .await
        .with_context(|| format!("failed to collect {command} stdout"))?
        .with_context(|| format!("failed to run {command}"))?;
    let stderr = stderr_task
        .await
        .with_context(|| format!("failed to collect {command} stderr"))?
        .with_context(|| format!("failed to run {command}"))?;

    if !status.success() {
        bail!(
            "{command} exited {}: {}",
            status,
            String::from_utf8_lossy(&stderr).trim()
        );
    }
    serde_json::from_slice(&stdout).with_context(|| format!("{command} returned malformed JSON"))
}

async fn codex_model_options() -> Result<Vec<ProviderModelOption>> {
    let json = run_json_command(
        "codex",
        &["debug".into(), "models".into()],
        PROVIDER_MODELS_TIMEOUT,
    )
    .await?;
    let options = json
        .get("models")
        .and_then(|models| models.as_array())
        .into_iter()
        .flatten()
        .filter(|model| {
            model
                .get("visibility")
                .and_then(|value| value.as_str())
                .map(|visibility| visibility == "list")
                .unwrap_or(true)
        })
        .filter_map(|model| {
            let slug = model.get("slug").and_then(|value| value.as_str())?;
            let label = model
                .get("display_name")
                .and_then(|value| value.as_str())
                .unwrap_or(slug);
            Some(model_option(slug, label))
        })
        .collect::<Vec<_>>();
    Ok(options)
}

fn html_unescape(value: &str) -> String {
    value
        .replace("&quot;", "\"")
        .replace("&#34;", "\"")
        .replace("&#39;", "'")
        .replace("&apos;", "'")
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
}

fn strip_html_tags(value: &str) -> String {
    let mut output = String::new();
    let mut in_tag = false;
    for ch in value.chars() {
        match ch {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ if !in_tag => output.push(ch),
            _ => {}
        }
    }
    html_unescape(output.trim())
}

fn extract_attr_value(tag: &str, attr: &str) -> Option<String> {
    let marker = format!("{attr}=\"");
    let start = tag.find(&marker)? + marker.len();
    let end = tag[start..].find('"')?;
    Some(html_unescape(&tag[start..start + end]))
}

fn extract_attr_near_marker(block: &str, marker: &str, attr: &str) -> Option<String> {
    let marker_pos = block.find(marker)?;
    let tag_start = block[..marker_pos].rfind('<')?;
    let tag_end = block[marker_pos..].find('>')? + marker_pos + 1;
    extract_attr_value(&block[tag_start..tag_end], attr)
}

fn extract_first_paragraph_after(block: &str, marker: &str) -> Option<String> {
    let start = block.find(marker)?;
    let after_marker = &block[start..];
    let paragraph_start = after_marker.find("<p")?;
    let paragraph = &after_marker[paragraph_start..];
    let content_start = paragraph.find('>')? + 1;
    let content_end = paragraph[content_start..].find("</p>")? + content_start;
    let text = strip_html_tags(&paragraph[content_start..content_end]);
    (!text.is_empty()).then_some(text)
}

fn extract_ollama_library_capabilities(block: &str) -> BTreeSet<String> {
    let mut capabilities = BTreeSet::new();
    let mut cursor = block;
    while let Some(marker_pos) = cursor.find("x-test-capability") {
        let after_marker = &cursor[marker_pos..];
        let Some(tag_end) = after_marker.find('>') else {
            break;
        };
        let after_tag = &after_marker[tag_end + 1..];
        let Some(content_end) = after_tag.find("</span>") else {
            break;
        };
        let capability = strip_html_tags(&after_tag[..content_end]).to_ascii_lowercase();
        if !capability.is_empty() {
            capabilities.insert(capability);
        }
        cursor = &after_tag[content_end + "</span>".len()..];
    }
    capabilities
}

fn ollama_library_models_from_html(html: &str, limit: usize) -> Vec<ProviderInstallableModel> {
    let mut models = Vec::new();
    let mut cursor = html;
    while let Some(start) = cursor.find("<li x-test-model") {
        let after_start = &cursor[start..];
        let end = after_start
            .find("\n    </li>")
            .or_else(|| after_start.find("</li>"))
            .unwrap_or(after_start.len());
        let block = &after_start[..end];
        cursor = &after_start[end.min(after_start.len())..];

        let value = extract_attr_near_marker(block, "x-test-model-title", "title")
            .or_else(|| {
                extract_attr_near_marker(block, "href=\"/library/", "href")
                    .and_then(|href| href.strip_prefix("/library/").map(str::to_string))
            })
            .unwrap_or_default();
        if value.trim().is_empty() {
            continue;
        }

        let capabilities = extract_ollama_library_capabilities(block);
        if capabilities.contains("embedding") {
            continue;
        }

        let input_support = ProviderModelInputSupport {
            text: true,
            image: capabilities.contains("vision") || capabilities.contains("image"),
            audio: capabilities.contains("audio") || capabilities.contains("speech"),
        };
        let detail = extract_first_paragraph_after(block, "x-test-model-title").unwrap_or_default();
        models.push(installable_model(
            value.clone(),
            value,
            detail,
            input_support,
            "ollama_library",
        ));
        if models.len() >= limit {
            break;
        }
    }
    models
}

async fn ollama_installable_models() -> Result<Vec<ProviderInstallableModel>> {
    let base_url = std::env::var("ALVUM_OLLAMA_LIBRARY_BASE_URL")
        .unwrap_or_else(|_| "https://ollama.com".into())
        .trim_end_matches('/')
        .to_string();
    let client = reqwest::Client::builder()
        .timeout(PROVIDER_MODELS_TIMEOUT)
        .user_agent("alvum")
        .build()?;
    let html = client
        .get(format!("{base_url}/library"))
        .send()
        .await
        .context("failed to query Ollama library")?
        .error_for_status()
        .context("Ollama library request failed")?
        .text()
        .await
        .context("Ollama library returned unreadable model list")?;
    let models = ollama_library_models_from_html(&html, OLLAMA_INSTALLABLE_MODEL_LIMIT);
    if models.is_empty() {
        bail!("Ollama library returned no downloadable text or vision models");
    }
    Ok(models)
}

fn ollama_modalities_from_show_json(json: &serde_json::Value) -> (bool, bool, bool) {
    let mut text = false;
    let mut image = false;
    let mut audio = false;
    let Some(values) = json.get("capabilities").and_then(|value| value.as_array()) else {
        return (true, false, false);
    };
    if values.is_empty() {
        return (true, false, false);
    }
    for value in values {
        if let Some(item) = value.as_str().map(|item| item.to_ascii_lowercase()) {
            match item.as_str() {
                "text" | "completion" | "chat" => text = true,
                "image" | "vision" => image = true,
                "audio" | "speech" => audio = true,
                _ => {}
            }
        }
    }
    (text, image, audio)
}

fn ollama_modalities_from_show_result(
    result: std::result::Result<&serde_json::Value, &anyhow::Error>,
) -> (bool, bool, bool) {
    result.map_or((true, false, false), ollama_modalities_from_show_json)
}

async fn ollama_api_model_catalog(
    config: &alvum_core::config::AlvumConfig,
) -> Result<OllamaModelCatalog> {
    let base_url = provider_setting_string(config, "ollama", "base_url")
        .unwrap_or_else(|| "http://localhost:11434".into())
        .trim_end_matches('/')
        .to_string();
    let client = reqwest::Client::builder()
        .timeout(PROVIDER_MODELS_TIMEOUT)
        .build()?;
    let tags_json: serde_json::Value = client
        .get(format!("{base_url}/api/tags"))
        .send()
        .await
        .context("failed to query Ollama models")?
        .error_for_status()
        .context("Ollama model list request failed")?
        .json()
        .await
        .context("Ollama returned malformed model list JSON")?;
    let options = tags_json
        .get("models")
        .and_then(|models| models.as_array())
        .into_iter()
        .flatten()
        .filter_map(|model| {
            let name = model
                .get("model")
                .or_else(|| model.get("name"))
                .and_then(|value| value.as_str())?;
            Some(model_option(name, name))
        })
        .collect::<Vec<_>>();

    let mut models = Vec::new();
    for option in options {
        let show_result = async {
            client
                .post(format!("{base_url}/api/show"))
                .json(&serde_json::json!({ "model": option.value }))
                .send()
                .await
                .with_context(|| {
                    format!("failed to query Ollama model details for {}", option.value)
                })?
                .error_for_status()
                .with_context(|| {
                    format!("Ollama model details request failed for {}", option.value)
                })?
                .json()
                .await
                .with_context(|| {
                    format!(
                        "Ollama returned malformed model details JSON for {}",
                        option.value
                    )
                })
        }
        .await;
        let (text, image, audio) = ollama_modalities_from_show_result(show_result.as_ref());
        models.push(OllamaModelInfo {
            option,
            text,
            image,
            audio,
        });
    }

    Ok(OllamaModelCatalog {
        source: "ollama".into(),
        models,
    })
}

async fn ollama_cli_model_options() -> Result<Vec<ProviderModelOption>> {
    let output = tokio::time::timeout(PROVIDER_MODELS_TIMEOUT, async {
        tokio::process::Command::new("ollama")
            .arg("ls")
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()
            .await
    })
    .await
    .with_context(|| {
        format!(
            "ollama ls timed out after {}s",
            PROVIDER_MODELS_TIMEOUT.as_secs()
        )
    })?
    .context("failed to run ollama ls")?;

    if !output.status.success() {
        bail!(
            "ollama ls exited {}: {}",
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let options = stdout
        .lines()
        .skip(1)
        .filter_map(|line| line.split_whitespace().next())
        .filter(|name| !name.trim().is_empty())
        .map(|name| model_option(name, name))
        .collect::<Vec<_>>();
    Ok(options)
}

async fn ollama_cli_model_catalog() -> Result<OllamaModelCatalog> {
    let models = ollama_cli_model_options()
        .await?
        .into_iter()
        .map(|option| OllamaModelInfo {
            option,
            text: true,
            image: false,
            audio: false,
        })
        .collect();
    Ok(OllamaModelCatalog {
        source: "ollama-cli".into(),
        models,
    })
}

async fn ollama_model_options(
    config: &alvum_core::config::AlvumConfig,
) -> Result<(String, Vec<ProviderModelOption>)> {
    let catalog = ollama_model_catalog(config).await?;
    Ok((catalog.source.clone(), catalog.all_options()))
}

async fn ollama_model_catalog(
    config: &alvum_core::config::AlvumConfig,
) -> Result<OllamaModelCatalog> {
    match ollama_api_model_catalog(config).await {
        Ok(catalog) => Ok(catalog),
        Err(api_error) => match ollama_cli_model_catalog().await {
            Ok(catalog) => Ok(catalog),
            Err(cli_error) => {
                Err(api_error).context(format!("ollama ls fallback failed: {cli_error:#}"))
            }
        },
    }
}

async fn anthropic_model_options() -> Result<Vec<ProviderModelOption>> {
    let api_key = std::env::var("ANTHROPIC_API_KEY")
        .ok()
        .filter(|key| !key.trim().is_empty())
        .or_else(|| {
            alvum_core::keychain::read_provider_secret("anthropic-api", "api_key")
                .ok()
                .flatten()
        })
        .context("Anthropic API key is not configured")?;
    let client = reqwest::Client::builder()
        .timeout(PROVIDER_MODELS_TIMEOUT)
        .build()?;
    let json: serde_json::Value = client
        .get("https://api.anthropic.com/v1/models")
        .header("x-api-key", api_key)
        .header("anthropic-version", "2023-06-01")
        .send()
        .await
        .context("failed to query Anthropic models")?
        .error_for_status()
        .context("Anthropic model list request failed")?
        .json()
        .await
        .context("Anthropic returned malformed model list JSON")?;
    let options = json
        .get("data")
        .and_then(|models| models.as_array())
        .into_iter()
        .flatten()
        .filter_map(|model| {
            let id = model.get("id").and_then(|value| value.as_str())?;
            let label = model
                .get("display_name")
                .and_then(|value| value.as_str())
                .unwrap_or(id);
            Some(model_option(id, label))
        })
        .collect::<Vec<_>>();
    Ok(options)
}

async fn bedrock_model_options(
    config: &alvum_core::config::AlvumConfig,
) -> Result<Vec<ProviderModelOption>> {
    let catalog = bedrock_catalog(config).await?;
    Ok(bedrock_options_by_modality(&catalog).text)
}

async fn live_model_options(
    config: &alvum_core::config::AlvumConfig,
    provider: &str,
) -> Result<(String, Vec<ProviderModelOption>)> {
    match provider {
        "claude-cli" => Ok(("cli_aliases".into(), static_model_options(provider))),
        "codex-cli" => Ok(("codex-cli".into(), codex_model_options().await?)),
        "anthropic-api" => Ok(("anthropic-api".into(), anthropic_model_options().await?)),
        "bedrock" => Ok(("native_api".into(), bedrock_model_options(config).await?)),
        "ollama" => ollama_model_options(config).await,
        _ => bail!("unknown provider: {provider}"),
    }
}

async fn cmd_providers_models(provider_name: &str) -> Result<()> {
    let normalized = normalize_name(provider_name);
    if normalized == "auto" || !known_provider_name(&normalized) {
        println!(
            "{}",
            serde_json::to_string_pretty(&ProviderModelsReport {
                ok: false,
                provider: normalized,
                source: "none".into(),
                options: vec![],
                options_by_modality: ProviderModelOptionsByModality::default(),
                installable_options: vec![],
                installable_error: None,
                error: Some(format!("unknown provider: {provider_name}")),
            })?
        );
        return Ok(());
    }

    let config = alvum_core::config::AlvumConfig::load()
        .unwrap_or_else(|_| alvum_core::config::AlvumConfig::default());
    let report = if normalized == "ollama" {
        let (catalog_result, installable_result) = tokio::join!(
            tokio::time::timeout(PROVIDER_MODELS_TIMEOUT, ollama_model_catalog(&config)),
            tokio::time::timeout(PROVIDER_MODELS_TIMEOUT, ollama_installable_models()),
        );
        let (installable_options, installable_error) = match installable_result {
            Ok(Ok(options)) => (options, None),
            Ok(Err(e)) => (vec![], Some(format!("{e:#}"))),
            Err(_) => (
                vec![],
                Some(format!(
                    "Ollama library query timed out after {}s",
                    PROVIDER_MODELS_TIMEOUT.as_secs()
                )),
            ),
        };
        match catalog_result {
            Ok(Ok(catalog)) if !catalog.models.is_empty() => ProviderModelsReport {
                ok: true,
                provider: normalized.clone(),
                source: catalog.source.clone(),
                options: catalog.all_options(),
                options_by_modality: catalog.options_by_modality(),
                installable_options,
                installable_error,
                error: None,
            },
            Ok(Ok(catalog)) => ProviderModelsReport {
                ok: false,
                provider: normalized.clone(),
                source: catalog.source,
                options: vec![],
                options_by_modality: ProviderModelOptionsByModality::default(),
                installable_options,
                installable_error,
                error: Some("provider returned no installed models".into()),
            },
            Ok(Err(e)) => ProviderModelsReport {
                ok: false,
                provider: normalized.clone(),
                source: "fallback".into(),
                options: vec![],
                options_by_modality: ProviderModelOptionsByModality::default(),
                installable_options,
                installable_error,
                error: Some(format!("{e:#}")),
            },
            Err(_) => ProviderModelsReport {
                ok: false,
                provider: normalized.clone(),
                source: "fallback".into(),
                options: vec![],
                options_by_modality: ProviderModelOptionsByModality::default(),
                installable_options,
                installable_error,
                error: Some(format!(
                    "Ollama model catalog timed out after {}s",
                    PROVIDER_MODELS_TIMEOUT.as_secs()
                )),
            },
        }
    } else if normalized == "bedrock" {
        match tokio::time::timeout(PROVIDER_MODELS_TIMEOUT, bedrock_catalog(&config)).await {
            Ok(Ok(catalog)) => {
                let options_by_modality = bedrock_options_by_modality(&catalog);
                let text = model_options_with_config_for_field(
                    &config,
                    &normalized,
                    "text_model",
                    options_by_modality.text,
                );
                let image = model_options_with_config_for_field(
                    &config,
                    &normalized,
                    "image_model",
                    options_by_modality.image,
                );
                let audio = model_options_with_config_for_field(
                    &config,
                    &normalized,
                    "audio_model",
                    options_by_modality.audio,
                );
                ProviderModelsReport {
                    ok: !text.is_empty(),
                    provider: normalized.clone(),
                    source: "native_api".into(),
                    options: text.clone(),
                    options_by_modality: ProviderModelOptionsByModality { text, image, audio },
                    installable_options: vec![],
                    installable_error: None,
                    error: None,
                }
            }
            Ok(Err(e)) => {
                let text =
                    model_options_with_config_for_field(&config, &normalized, "text_model", vec![]);
                let image = model_options_with_config_for_field(
                    &config,
                    &normalized,
                    "image_model",
                    vec![],
                );
                let audio = model_options_with_config_for_field(
                    &config,
                    &normalized,
                    "audio_model",
                    vec![],
                );
                ProviderModelsReport {
                    ok: false,
                    provider: normalized.clone(),
                    source: "native_api".into(),
                    options: text.clone(),
                    options_by_modality: ProviderModelOptionsByModality { text, image, audio },
                    installable_options: vec![],
                    installable_error: None,
                    error: Some(format!("{e:#}")),
                }
            }
            Err(_) => {
                let text =
                    model_options_with_config_for_field(&config, &normalized, "text_model", vec![]);
                let image = model_options_with_config_for_field(
                    &config,
                    &normalized,
                    "image_model",
                    vec![],
                );
                let audio = model_options_with_config_for_field(
                    &config,
                    &normalized,
                    "audio_model",
                    vec![],
                );
                ProviderModelsReport {
                    ok: false,
                    provider: normalized.clone(),
                    source: "native_api".into(),
                    options: text.clone(),
                    options_by_modality: ProviderModelOptionsByModality { text, image, audio },
                    installable_options: vec![],
                    installable_error: None,
                    error: Some(format!(
                        "Bedrock model catalog timed out after {}s",
                        PROVIDER_MODELS_TIMEOUT.as_secs()
                    )),
                }
            }
        }
    } else {
        match live_model_options(&config, &normalized).await {
            Ok((source, options)) if !options.is_empty() => {
                let text = model_options_with_config_for_field(
                    &config,
                    &normalized,
                    "text_model",
                    live_model_options_for_field(&normalized, "text_model", &options),
                );
                let image = model_options_with_config_for_field(
                    &config,
                    &normalized,
                    "image_model",
                    live_model_options_for_field(&normalized, "image_model", &options),
                );
                let audio = model_options_with_config_for_field(
                    &config,
                    &normalized,
                    "audio_model",
                    live_model_options_for_field(&normalized, "audio_model", &options),
                );
                ProviderModelsReport {
                    ok: true,
                    provider: normalized.clone(),
                    source,
                    options: text.clone(),
                    options_by_modality: ProviderModelOptionsByModality { text, image, audio },
                    installable_options: vec![],
                    installable_error: None,
                    error: None,
                }
            }
            Ok((source, _)) => {
                let text = model_options_with_config_for_field(
                    &config,
                    &normalized,
                    "text_model",
                    static_model_options_for_field(&normalized, "text_model"),
                );
                let image = model_options_with_config_for_field(
                    &config,
                    &normalized,
                    "image_model",
                    static_model_options_for_field(&normalized, "image_model"),
                );
                let audio = model_options_with_config_for_field(
                    &config,
                    &normalized,
                    "audio_model",
                    static_model_options_for_field(&normalized, "audio_model"),
                );
                ProviderModelsReport {
                    ok: false,
                    provider: normalized.clone(),
                    source,
                    options: text.clone(),
                    options_by_modality: ProviderModelOptionsByModality { text, image, audio },
                    installable_options: vec![],
                    installable_error: None,
                    error: Some("provider returned no model options".into()),
                }
            }
            Err(e) => {
                let text = model_options_with_config_for_field(
                    &config,
                    &normalized,
                    "text_model",
                    static_model_options_for_field(&normalized, "text_model"),
                );
                let image = model_options_with_config_for_field(
                    &config,
                    &normalized,
                    "image_model",
                    static_model_options_for_field(&normalized, "image_model"),
                );
                let audio = model_options_with_config_for_field(
                    &config,
                    &normalized,
                    "audio_model",
                    static_model_options_for_field(&normalized, "audio_model"),
                );
                ProviderModelsReport {
                    ok: false,
                    provider: normalized.clone(),
                    source: "fallback".into(),
                    options: text.clone(),
                    options_by_modality: ProviderModelOptionsByModality { text, image, audio },
                    installable_options: vec![],
                    installable_error: None,
                    error: Some(format!("{e:#}")),
                }
            }
        }
    };
    println!("{}", serde_json::to_string_pretty(&report)?);
    Ok(())
}

const PROVIDER_MODEL_INSTALL_TIMEOUT: Duration = Duration::from_secs(60 * 60);

#[derive(serde::Serialize)]
struct ProviderInstallModelReport {
    ok: bool,
    provider: String,
    model: String,
    status: String,
    elapsed_ms: u128,
    stdout_tail: Option<String>,
    stderr_tail: Option<String>,
    error: Option<String>,
}

fn tail_string(value: &str, max_chars: usize) -> Option<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return None;
    }
    let char_count = trimmed.chars().count();
    if char_count <= max_chars {
        return Some(trimmed.to_string());
    }
    Some(trimmed.chars().skip(char_count - max_chars).collect())
}

fn valid_ollama_model_ref(model: &str) -> bool {
    let model = model.trim();
    !model.is_empty()
        && model.len() <= 160
        && !model.starts_with('-')
        && model
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '.' | '_' | '-' | ':' | '/'))
}

async fn cmd_providers_install_model(provider_name: &str, model: &str) -> Result<()> {
    let normalized = normalize_name(provider_name);
    let started = std::time::Instant::now();

    if normalized != "ollama" {
        println!(
            "{}",
            serde_json::to_string_pretty(&ProviderInstallModelReport {
                ok: false,
                provider: normalized,
                model: model.into(),
                status: "unsupported_provider".into(),
                elapsed_ms: started.elapsed().as_millis(),
                stdout_tail: None,
                stderr_tail: None,
                error: Some("model downloads are currently supported for Ollama only".into()),
            })?
        );
        return Ok(());
    }

    if !valid_ollama_model_ref(model) {
        println!(
            "{}",
            serde_json::to_string_pretty(&ProviderInstallModelReport {
                ok: false,
                provider: normalized,
                model: model.into(),
                status: "invalid_model".into(),
                elapsed_ms: started.elapsed().as_millis(),
                stdout_tail: None,
                stderr_tail: None,
                error: Some(
                    "Ollama model names may only contain letters, numbers, ., _, -, :, and /"
                        .into()
                ),
            })?
        );
        return Ok(());
    }

    let mut command = tokio::process::Command::new("ollama");
    command
        .args(["pull", model])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    let output = tokio::time::timeout(PROVIDER_MODEL_INSTALL_TIMEOUT, command.output()).await;
    let report = match output {
        Err(_) => ProviderInstallModelReport {
            ok: false,
            provider: normalized,
            model: model.into(),
            status: "timeout".into(),
            elapsed_ms: started.elapsed().as_millis(),
            stdout_tail: None,
            stderr_tail: None,
            error: Some(format!(
                "ollama pull timed out after {}s",
                PROVIDER_MODEL_INSTALL_TIMEOUT.as_secs()
            )),
        },
        Ok(Err(e)) => ProviderInstallModelReport {
            ok: false,
            provider: normalized,
            model: model.into(),
            status: "spawn_error".into(),
            elapsed_ms: started.elapsed().as_millis(),
            stdout_tail: None,
            stderr_tail: None,
            error: Some(format!("failed to run ollama pull: {e}")),
        },
        Ok(Ok(output)) => {
            let stdout = String::from_utf8_lossy(&output.stdout);
            let stderr = String::from_utf8_lossy(&output.stderr);
            ProviderInstallModelReport {
                ok: output.status.success(),
                provider: normalized,
                model: model.into(),
                status: if output.status.success() {
                    "installed".into()
                } else {
                    "failed".into()
                },
                elapsed_ms: started.elapsed().as_millis(),
                stdout_tail: tail_string(&stdout, 1200),
                stderr_tail: tail_string(&stderr, 1200),
                error: if output.status.success() {
                    None
                } else {
                    Some(format!("ollama pull exited {}", output.status))
                },
            }
        }
    };
    println!("{}", serde_json::to_string_pretty(&report)?);
    Ok(())
}

#[derive(serde::Serialize)]
struct ProviderBootstrapReport {
    ok: bool,
    skipped: bool,
    reason: Option<String>,
    enabled: Vec<String>,
    providers: Vec<ProviderTestReport>,
}

fn provider_bootstrap_marker_path() -> PathBuf {
    alvum_core::config::config_path()
        .parent()
        .map(|p| p.join("provider-bootstrap.json"))
        .unwrap_or_else(|| PathBuf::from("provider-bootstrap.json"))
}

fn provider_bootstrap_done() -> bool {
    provider_bootstrap_marker_path().exists()
}

fn provider_config_looks_uninitialized(config_path: &Path, doc: &toml::Table) -> bool {
    if !config_path.exists() {
        return true;
    }
    let configured = doc
        .get("pipeline")
        .and_then(|v| v.as_table())
        .and_then(|pipeline| pipeline.get("provider"))
        .and_then(|v| v.as_str())
        .map(normalize_name)
        .unwrap_or_else(|| "auto".into());
    if configured != "auto" {
        return false;
    }

    let Some(providers) = doc.get("providers").and_then(|v| v.as_table()) else {
        return true;
    };

    known_provider_ids().iter().all(|provider| {
        let Some(value) = providers.get(*provider) else {
            return true;
        };
        let Some(table) = value.as_table() else {
            return false;
        };
        let enabled = table
            .get("enabled")
            .and_then(|v| v.as_bool())
            .unwrap_or(true);
        enabled && table.keys().all(|key| key == "enabled")
    })
}

fn write_provider_bootstrap_marker(report: &ProviderBootstrapReport) -> Result<()> {
    let path = provider_bootstrap_marker_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(
        path,
        serde_json::to_string_pretty(&serde_json::json!({
            "bootstrapped_at": Utc::now().to_rfc3339(),
            "enabled": &report.enabled,
        }))?,
    )?;
    Ok(())
}

async fn cmd_providers_bootstrap(force: bool) -> Result<()> {
    let (config_path, mut doc) = config_doc::load()?;
    if !force && provider_bootstrap_done() {
        let report = ProviderBootstrapReport {
            ok: true,
            skipped: true,
            reason: Some("provider bootstrap already completed".into()),
            enabled: vec![],
            providers: vec![],
        };
        println!("{}", serde_json::to_string_pretty(&report)?);
        return Ok(());
    }
    if !force && !provider_config_looks_uninitialized(&config_path, &doc) {
        let report = ProviderBootstrapReport {
            ok: true,
            skipped: true,
            reason: Some("provider config already customized".into()),
            enabled: vec![],
            providers: vec![],
        };
        write_provider_bootstrap_marker(&report)?;
        println!("{}", serde_json::to_string_pretty(&report)?);
        return Ok(());
    }

    let config_for_entries: alvum_core::config::AlvumConfig =
        toml::from_str(&toml::to_string(&doc)?)?;
    let entries = entries(&config_for_entries);
    let mut bootstrap_tasks = tokio::task::JoinSet::new();
    for (index, entry) in entries.iter().copied().enumerate() {
        bootstrap_tasks.spawn(async move {
            let report = if entry.available {
                provider_test_report(
                    entry.name,
                    default_model_for(entry.name),
                    PROVIDER_BACKGROUND_TEST_TIMEOUT,
                )
                .await
            } else {
                provider_test_report_with_diagnosis(
                    entry.name.into(),
                    "not_installed".into(),
                    false,
                    0,
                    None,
                    Some(entry.auth_hint.into()),
                    default_model_for(entry.name),
                    PROVIDER_BACKGROUND_TEST_TIMEOUT,
                )
            };
            (index, report)
        });
    }
    let mut indexed_reports = Vec::new();
    while let Some(result) = bootstrap_tasks.join_next().await {
        indexed_reports.push(result?);
    }
    indexed_reports.sort_by_key(|(index, _)| *index);
    let reports = indexed_reports
        .into_iter()
        .map(|(_, report)| report)
        .collect::<Vec<_>>();

    let enabled = reports
        .iter()
        .filter(|report| report.ok)
        .map(|report| report.provider.clone())
        .collect::<Vec<_>>();
    for entry in &entries {
        config_doc::set_value(
            &mut doc,
            &format!("providers.{}.enabled", entry.name),
            toml::Value::Boolean(enabled.iter().any(|name| name == entry.name)),
        )?;
    }
    config_doc::set_value(
        &mut doc,
        "pipeline.provider",
        toml::Value::String("auto".into()),
    )?;
    config_doc::save(&config_path, &doc)?;

    let report = ProviderBootstrapReport {
        ok: true,
        skipped: false,
        reason: None,
        enabled,
        providers: reports,
    };
    write_provider_bootstrap_marker(&report)?;
    println!("{}", serde_json::to_string_pretty(&report)?);
    Ok(())
}

#[derive(serde::Serialize)]
struct ProviderMutationReport {
    ok: bool,
    provider: String,
    configured: String,
    enabled: Option<bool>,
}

#[derive(serde::Deserialize)]
struct ProviderConfigureRequest {
    #[serde(default)]
    settings: HashMap<String, serde_json::Value>,
    #[serde(default)]
    secrets: HashMap<String, String>,
    enabled: Option<bool>,
}

#[derive(serde::Serialize)]
struct ProviderConfigureReport {
    ok: bool,
    provider: String,
    configured: String,
    enabled: bool,
    saved_settings: Vec<String>,
    saved_secrets: Vec<String>,
}

fn json_value_to_toml(value: serde_json::Value) -> Result<toml::Value> {
    Ok(match value {
        serde_json::Value::Null => toml::Value::String(String::new()),
        serde_json::Value::Bool(v) => toml::Value::Boolean(v),
        serde_json::Value::Number(n) => {
            if let Some(v) = n.as_i64() {
                toml::Value::Integer(v)
            } else if let Some(v) = n.as_f64() {
                toml::Value::Float(v)
            } else {
                bail!("unsupported numeric provider setting")
            }
        }
        serde_json::Value::String(v) => toml::Value::String(v),
        serde_json::Value::Array(_) | serde_json::Value::Object(_) => {
            bail!("provider settings must be scalar values")
        }
    })
}

fn cmd_providers_configure(provider: &str) -> Result<()> {
    let normalized = normalize_name(provider);
    if normalized == "auto" || !known_provider_name(&normalized) {
        bail!("unknown provider: {normalized}");
    }

    let mut input = String::new();
    std::io::stdin()
        .read_to_string(&mut input)
        .context("failed to read provider config JSON from stdin")?;
    let request: ProviderConfigureRequest = if input.trim().is_empty() {
        ProviderConfigureRequest {
            settings: HashMap::new(),
            secrets: HashMap::new(),
            enabled: None,
        }
    } else {
        serde_json::from_str(&input).context("failed to parse provider config JSON")?
    };

    let (config_path, mut doc) = config_doc::load()?;
    let config: alvum_core::config::AlvumConfig = toml::from_str(&toml::to_string(&doc)?)?;
    let fields = provider_config_fields_for_write(&config, &normalized);
    let mut saved_settings = Vec::new();
    let mut saved_secrets = Vec::new();

    for (key, value) in request.settings {
        let Some(field) = fields
            .iter()
            .find(|field| field.key == key && !field.secret)
        else {
            bail!("unknown provider setting for {normalized}: {key}");
        };
        config_doc::set_value(
            &mut doc,
            &format!("providers.{normalized}.{}", field.key),
            json_value_to_toml(value)?,
        )?;
        saved_settings.push(field.key.to_string());
    }

    for (key, secret) in request.secrets {
        let Some(field) = fields.iter().find(|field| field.key == key && field.secret) else {
            bail!("unknown provider secret for {normalized}: {key}");
        };
        if !secret.is_empty() {
            alvum_core::keychain::write_provider_secret(&normalized, field.key, &secret)?;
            saved_secrets.push(field.key.to_string());
        }
    }

    if let Some(enabled) = request.enabled {
        config_doc::set_value(
            &mut doc,
            &format!("providers.{normalized}.enabled"),
            toml::Value::Boolean(enabled),
        )?;
    }
    config_doc::save(&config_path, &doc)?;

    let configured = doc
        .get("pipeline")
        .and_then(|v| v.as_table())
        .and_then(|pipeline| pipeline.get("provider"))
        .and_then(|v| v.as_str())
        .map(normalize_name)
        .unwrap_or_else(|| "auto".into());
    let enabled = doc
        .get("providers")
        .and_then(|v| v.as_table())
        .and_then(|providers| providers.get(&normalized))
        .and_then(|v| v.as_table())
        .and_then(|provider| provider.get("enabled"))
        .and_then(|v| v.as_bool())
        .unwrap_or(true);

    println!(
        "{}",
        serde_json::to_string_pretty(&ProviderConfigureReport {
            ok: true,
            provider: normalized,
            configured,
            enabled,
            saved_settings,
            saved_secrets,
        })?
    );
    Ok(())
}

fn cmd_providers_set_active(provider: &str) -> Result<()> {
    let normalized = normalize_name(provider);
    if !known_provider_name(&normalized) {
        bail!("unknown provider: {normalized}");
    }
    let (config_path, mut doc) = config_doc::load()?;
    config_doc::set_value(
        &mut doc,
        "pipeline.provider",
        toml::Value::String(normalized.clone()),
    )?;
    config_doc::save(&config_path, &doc)?;
    println!(
        "{}",
        serde_json::to_string_pretty(&ProviderMutationReport {
            ok: true,
            provider: normalized.clone(),
            configured: normalized,
            enabled: None,
        })?
    );
    Ok(())
}

fn cmd_providers_set_enabled(provider: &str, enabled: bool) -> Result<()> {
    let normalized = normalize_name(provider);
    if normalized == "auto" || !known_provider_name(&normalized) {
        bail!("unknown provider: {normalized}");
    }

    let (config_path, mut doc) = config_doc::load()?;
    config_doc::set_value(
        &mut doc,
        &format!("providers.{normalized}.enabled"),
        toml::Value::Boolean(enabled),
    )?;

    let configured = doc
        .get("pipeline")
        .and_then(|v| v.as_table())
        .and_then(|pipeline| pipeline.get("provider"))
        .and_then(|v| v.as_str())
        .map(normalize_name)
        .unwrap_or_else(|| "auto".into());
    let next_configured = if !enabled && configured == normalized {
        config_doc::set_value(
            &mut doc,
            "pipeline.provider",
            toml::Value::String("auto".into()),
        )?;
        "auto".to_string()
    } else {
        configured
    };

    config_doc::save(&config_path, &doc)?;
    println!(
        "{}",
        serde_json::to_string_pretty(&ProviderMutationReport {
            ok: true,
            provider: normalized,
            configured: next_configured,
            enabled: Some(enabled),
        })?
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn capability(
        supported: bool,
        model_supported: bool,
        adapter_supported: bool,
    ) -> capabilities::ProviderCapability {
        capabilities::ProviderCapability {
            supported,
            model_supported,
            adapter_supported,
            provenance: "test".into(),
            status: if supported {
                "ready".into()
            } else if model_supported && !adapter_supported {
                "transport_limited".into()
            } else {
                "unsupported".into()
            },
            detail: "test detail".into(),
        }
    }

    fn test_bedrock_foundation_model(
        model_id: &str,
        model_name: &str,
        text: bool,
        image: bool,
        on_demand: bool,
    ) -> alvum_pipeline::bedrock::BedrockFoundationModel {
        alvum_pipeline::bedrock::BedrockFoundationModel {
            model_id: model_id.into(),
            model_name: model_name.into(),
            active: true,
            input: alvum_pipeline::bedrock::BedrockModelInputSupport {
                text,
                image,
                audio: false,
            },
            output: alvum_pipeline::bedrock::BedrockModelInputSupport {
                text: true,
                image: false,
                audio: false,
            },
            on_demand,
        }
    }

    fn test_bedrock_system_profile(
        id: &str,
        name: &str,
        source_model_ids: &[&str],
    ) -> alvum_pipeline::bedrock::BedrockInferenceProfile {
        alvum_pipeline::bedrock::BedrockInferenceProfile {
            id: id.into(),
            arn: format!("arn:aws:bedrock:us-east-1::inference-profile/{id}"),
            name: name.into(),
            active: true,
            kind: alvum_pipeline::bedrock::BedrockInferenceProfileKind::System,
            source_model_ids: source_model_ids.iter().map(|item| (*item).into()).collect(),
        }
    }

    #[test]
    fn screen_readiness_distinguishes_ready_model_and_adapter_states() {
        let ready = ProviderCapabilities {
            text: capability(true, true, true),
            image: capability(true, true, true),
            audio: capability(false, false, false),
        };
        assert_eq!(
            screen_modality_readiness_from_capability("ollama", "Ollama", &ready).status,
            "ready"
        );

        let model_blocked = ProviderCapabilities {
            text: capability(true, true, true),
            image: capability(false, false, true),
            audio: capability(false, false, false),
        };
        assert_eq!(
            screen_modality_readiness_from_capability("ollama", "Ollama", &model_blocked).status,
            "unsupported_model"
        );

        let adapter_blocked = ProviderCapabilities {
            text: capability(true, true, true),
            image: capability(false, true, false),
            audio: capability(false, false, false),
        };
        assert_eq!(
            screen_modality_readiness_from_capability("codex-cli", "Codex CLI", &adapter_blocked)
                .status,
            "unsupported_adapter"
        );
    }

    #[test]
    fn claude_cli_metadata_uses_cli_default_without_login_action() {
        let config = alvum_core::config::AlvumConfig::default();
        let claude = entries(&config)
            .into_iter()
            .find(|entry| entry.name == "claude-cli")
            .unwrap();

        assert_eq!(default_model_for("claude-cli"), "");
        assert_eq!(claude.setup_kind, "instructions");
        assert_eq!(claude.setup_command, None);
        assert!(!claude.setup_hint.contains("claude login"));
        assert!(!claude.auth_hint.contains("claude login"));

        let fields = provider_config_fields(&config, "claude-cli");
        let extra_path = fields
            .iter()
            .find(|field| field.key == "extra_path")
            .unwrap();
        assert_eq!(extra_path.label, "Backend helper PATH");
        let text_field = fields
            .iter()
            .find(|field| field.key == "text_model")
            .unwrap();
        assert_eq!(text_field.placeholder, "");
        assert!(
            text_field
                .options
                .iter()
                .any(|option| { option.value.is_empty() && option.label == "CLI default" })
        );

        let selected = provider_selected_models(&config, "claude-cli");
        assert_eq!(selected.text.as_deref(), Some("CLI default"));
        assert_eq!(selected.image.as_deref(), Some("CLI default"));
        assert_eq!(selected.audio.as_deref(), Some("CLI default"));
        assert!(
            static_provider_capabilities("claude-cli", &selected)
                .text
                .supported
        );
    }

    #[test]
    fn provider_setup_actions_cover_native_configuration_surfaces() {
        let config = alvum_core::config::AlvumConfig::default();

        let claude_actions = provider_setup_actions(&config, "claude-cli");
        assert!(
            claude_actions
                .iter()
                .any(|action| action.id == "claude_doctor")
        );
        assert!(
            claude_actions
                .iter()
                .any(|action| action.id == "open_claude_config")
        );
        assert!(
            claude_actions
                .iter()
                .any(|action| action.id == "edit_extra_path")
        );
        assert!(
            claude_actions
                .iter()
                .all(|action| action.id != "claude_login")
        );

        let bedrock_actions = provider_setup_actions(&config, "bedrock");
        assert!(
            bedrock_actions
                .iter()
                .any(|action| action.id == "open_aws_config")
        );
        assert!(
            bedrock_actions
                .iter()
                .any(|action| action.id == "bedrock_refresh_catalog")
        );
        assert!(bedrock_actions.iter().any(|action| action.id == "aws_sts"));
        assert!(
            bedrock_actions
                .iter()
                .any(|action| action.id == "edit_extra_path")
        );
        assert!(
            bedrock_actions
                .iter()
                .any(|action| action.id == "bedrock_list_models")
        );

        let ollama_actions = provider_setup_actions(&config, "ollama");
        assert!(
            ollama_actions
                .iter()
                .any(|action| action.id == "ollama_serve")
        );
        assert!(
            ollama_actions
                .iter()
                .any(|action| action.id == "ollama_list")
        );
        assert!(
            ollama_actions
                .iter()
                .any(|action| action.id == "ollama_show_text")
        );
    }

    #[test]
    fn provider_config_fields_are_grouped_by_connection_and_models() {
        let config = alvum_core::config::AlvumConfig::default();
        let fields = provider_config_fields(&config, "bedrock");

        assert_eq!(
            fields
                .iter()
                .find(|field| field.key == "aws_profile")
                .unwrap()
                .group,
            "connection"
        );
        assert_eq!(
            fields
                .iter()
                .find(|field| field.key == "aws_region")
                .unwrap()
                .group,
            "connection"
        );
        let extra_path = fields
            .iter()
            .find(|field| field.key == "extra_path")
            .unwrap();
        assert_eq!(extra_path.group, "connection");
        assert!(extra_path.detail.contains("credential_process"));
        assert_eq!(
            fields
                .iter()
                .find(|field| field.key == "text_model")
                .unwrap()
                .group,
            "models"
        );
    }

    #[test]
    fn bedrock_options_are_profile_aware_by_modality() {
        let catalog = alvum_pipeline::bedrock::BedrockCatalog::from_test_records(
            vec![
                test_bedrock_foundation_model(
                    "anthropic.claude-opus-4-7-20260101-v1:0",
                    "Claude Opus 4.7",
                    true,
                    true,
                    false,
                ),
                test_bedrock_foundation_model(
                    "anthropic.claude-haiku-4-20260101-v1:0",
                    "Claude Haiku 4",
                    true,
                    false,
                    true,
                ),
            ],
            vec![test_bedrock_system_profile(
                "global.anthropic.claude-opus-4-7-20260101-v1:0",
                "Global Claude Opus 4.7",
                &["anthropic.claude-opus-4-7-20260101-v1:0"],
            )],
        );

        let options = bedrock_options_by_modality(&catalog);

        assert_eq!(
            options.text.first().map(|option| option.value.as_str()),
            Some("global.anthropic.claude-opus-4-7-20260101-v1:0")
        );
        assert!(
            options
                .image
                .iter()
                .any(|option| option.value == "global.anthropic.claude-opus-4-7-20260101-v1:0")
        );
        assert!(
            options
                .image
                .iter()
                .all(|option| option.value != "anthropic.claude-haiku-4-20260101-v1:0")
        );
        let image = options.image.first().unwrap();
        assert!(image.detail.as_deref().unwrap_or("").contains("Global"));
        assert!(image.input_support.as_ref().unwrap().image);
    }

    #[test]
    fn bedrock_selected_models_resolve_defaults_from_catalog() {
        let config = alvum_core::config::AlvumConfig::default();
        let catalog = alvum_pipeline::bedrock::BedrockCatalog::from_test_records(
            vec![test_bedrock_foundation_model(
                "anthropic.claude-opus-4-7-20260101-v1:0",
                "Claude Opus 4.7",
                true,
                true,
                false,
            )],
            vec![test_bedrock_system_profile(
                "global.anthropic.claude-opus-4-7-20260101-v1:0",
                "Global Claude Opus 4.7",
                &["anthropic.claude-opus-4-7-20260101-v1:0"],
            )],
        );

        let selected = resolve_bedrock_selected_models(&config, &catalog);

        assert_eq!(
            selected.text.as_deref(),
            Some("global.anthropic.claude-opus-4-7-20260101-v1:0")
        );
        assert_eq!(
            selected.image.as_deref(),
            Some("global.anthropic.claude-opus-4-7-20260101-v1:0")
        );
        assert_eq!(selected.audio, None);
        assert_eq!(default_model_for("bedrock"), "");
        assert_eq!(default_image_model_for("bedrock"), "");
    }

    #[test]
    fn bedrock_selected_models_preserve_configured_base_model_display() {
        let mut config = alvum_core::config::AlvumConfig::default();
        config.providers.insert(
            "bedrock".into(),
            alvum_core::config::ProviderConfig {
                enabled: true,
                settings: HashMap::from([(
                    "text_model".into(),
                    toml::Value::String("anthropic.claude-opus-4-20260101-v1:0".into()),
                )]),
            },
        );
        let catalog = alvum_pipeline::bedrock::BedrockCatalog::from_test_records(
            vec![test_bedrock_foundation_model(
                "anthropic.claude-opus-4-20260101-v1:0",
                "Claude Opus 4",
                true,
                true,
                false,
            )],
            vec![test_bedrock_system_profile(
                "global.anthropic.claude-opus-4-20260101-v1:0",
                "Global Claude Opus 4",
                &["anthropic.claude-opus-4-20260101-v1:0"],
            )],
        );

        let selected = resolve_bedrock_selected_models(&config, &catalog);
        let resolved = catalog
            .resolve_invoke_target(selected.text.as_deref(), "text")
            .unwrap();

        assert_eq!(
            selected.text.as_deref(),
            Some("anthropic.claude-opus-4-20260101-v1:0")
        );
        assert_eq!(
            resolved.invoke_id,
            "global.anthropic.claude-opus-4-20260101-v1:0"
        );
    }

    #[test]
    fn provider_probe_diagnosis_recommends_provider_specific_setup_actions() {
        let timeout = provider_probe_diagnosis(
            "claude-cli",
            "",
            Duration::from_secs(90),
            "timeout",
            Some("provider probe timed out after 90s"),
        );
        assert_eq!(timeout.resolved_model.as_deref(), Some("CLI default"));
        assert_eq!(timeout.model_source, "cli_default");
        assert_eq!(timeout.timeout_secs, 90);
        assert!(
            timeout
                .setup_action_ids
                .contains(&"claude_doctor".to_string())
        );
        assert!(
            timeout
                .setup_action_ids
                .contains(&"open_claude_config".to_string())
        );
        assert!(
            timeout
                .backend_hint
                .contains("subscription, API, Bedrock, Vertex")
        );

        let bedrock = provider_probe_diagnosis(
            "bedrock",
            "anthropic.claude-sonnet-4-5-20250929-v1:0",
            Duration::from_secs(25),
            "auth_unavailable",
            Some("credentials provider was not properly configured"),
        );
        assert_eq!(bedrock.model_source, "configured");
        assert!(
            bedrock
                .setup_action_ids
                .contains(&"open_aws_config".to_string())
        );
        assert!(bedrock.setup_action_ids.contains(&"aws_sts".to_string()));
        assert!(
            bedrock
                .backend_hint
                .contains("standard AWS SDK credential chain")
        );

        let credential_helper = provider_probe_diagnosis(
            "bedrock",
            "anthropic.claude-sonnet-4-5-20250929-v1:0",
            Duration::from_secs(25),
            "auth_unavailable",
            Some("ProfileFile provider failed to run credential_process: isengardcli not found"),
        );
        assert!(
            credential_helper
                .backend_hint
                .contains("credential_process helper")
        );
        assert!(credential_helper.backend_hint.contains("PATH"));
        assert!(
            credential_helper
                .setup_action_ids
                .contains(&"edit_extra_path".to_string())
        );
    }

    #[tokio::test]
    async fn claude_cli_probe_uses_cli_default_model() {
        assert_eq!(default_model_for_probe("claude-cli").await, "");
    }

    #[test]
    fn provider_probe_timeouts_keep_background_short_and_manual_longer() {
        assert_eq!(PROVIDER_BACKGROUND_TEST_TIMEOUT, Duration::from_secs(25));
        assert_eq!(PROVIDER_MANUAL_TEST_TIMEOUT, Duration::from_secs(90));
    }

    #[test]
    fn cli_provider_legacy_claude_model_config_displays_cli_default() {
        let mut config = alvum_core::config::AlvumConfig::default();
        config.providers.insert(
            "claude-cli".into(),
            alvum_core::config::ProviderConfig {
                enabled: true,
                settings: HashMap::from([(
                    "text_model".into(),
                    toml::Value::String("claude-sonnet-4-6".into()),
                )]),
            },
        );
        config.providers.insert(
            "codex-cli".into(),
            alvum_core::config::ProviderConfig {
                enabled: true,
                settings: HashMap::from([(
                    "text_model".into(),
                    toml::Value::String("claude-sonnet-4-6".into()),
                )]),
            },
        );

        for provider in ["claude-cli", "codex-cli"] {
            let fields = provider_config_fields(&config, provider);
            let text_field = fields
                .iter()
                .find(|field| field.key == "text_model")
                .unwrap();
            assert_eq!(text_field.value.as_deref(), Some(""));
            assert_eq!(
                provider_selected_models(&config, provider).text.as_deref(),
                Some("CLI default")
            );
            assert!(
                model_options_with_config_for_field(
                    &config,
                    provider,
                    "text_model",
                    static_model_options(provider)
                )
                .iter()
                .any(|option| option.value.is_empty() && option.label == "CLI default")
            );
        }
    }

    #[test]
    fn claude_cli_keeps_image_options_separate_from_text_aliases() {
        let config = alvum_core::config::AlvumConfig::default();

        let text_options = model_options_with_config_for_field(
            &config,
            "claude-cli",
            "text_model",
            static_model_options_for_field("claude-cli", "text_model"),
        );
        let image_options = model_options_with_config_for_field(
            &config,
            "claude-cli",
            "image_model",
            static_model_options_for_field("claude-cli", "image_model"),
        );
        let audio_options = model_options_with_config_for_field(
            &config,
            "claude-cli",
            "audio_model",
            static_model_options_for_field("claude-cli", "audio_model"),
        );

        assert!(
            text_options
                .iter()
                .any(|option| option.value.is_empty() && option.label == "CLI default")
        );
        assert!(
            image_options
                .iter()
                .any(|option| option.value.is_empty() && option.label == "CLI default")
        );
        assert!(
            audio_options
                .iter()
                .any(|option| option.value.is_empty() && option.label == "CLI default")
        );
        assert!(
            !image_options
                .iter()
                .any(|option| option.value == "claude-sonnet-4-6")
        );
    }

    #[test]
    fn ollama_show_missing_capabilities_defaults_to_text_only() {
        let json = serde_json::json!({ "model_info": {} });

        assert_eq!(
            ollama_modalities_from_show_json(&json),
            (true, false, false)
        );
    }

    #[test]
    fn ollama_show_failure_keeps_installed_model_as_text_only() {
        let err = anyhow::anyhow!("show failed");

        assert_eq!(
            ollama_modalities_from_show_result(Err(&err)),
            (true, false, false)
        );
    }

    #[test]
    fn ollama_selected_models_auto_resolve_from_installed_capabilities() {
        let config = alvum_core::config::AlvumConfig::default();
        let catalog = OllamaModelCatalog {
            source: "test".into(),
            models: vec![
                ollama_model_info("deepseek-r1:32b", "deepseek-r1:32b", true, false, false),
                ollama_model_info("llava:latest", "llava:latest", true, true, false),
            ],
        };

        let selected = resolve_ollama_selected_models(&config, &catalog);

        assert_eq!(selected.text.as_deref(), Some("deepseek-r1:32b"));
        assert_eq!(selected.image.as_deref(), Some("llava:latest"));
        let options = catalog.options_by_modality();
        assert_eq!(options.text.len(), 2);
        assert_eq!(options.image.len(), 1);
        assert_eq!(options.image[0].value, "llava:latest");
    }

    #[test]
    fn ollama_configured_missing_model_stays_visible_but_not_in_installed_options() {
        let mut config = alvum_core::config::AlvumConfig::default();
        config.providers.insert(
            "ollama".into(),
            alvum_core::config::ProviderConfig {
                enabled: true,
                settings: HashMap::from([
                    ("text_model".into(), toml::Value::String("llama3.2".into())),
                    ("image_model".into(), toml::Value::String("gemma3".into())),
                ]),
            },
        );
        let catalog = OllamaModelCatalog {
            source: "test".into(),
            models: vec![ollama_model_info(
                "deepseek-r1:32b",
                "deepseek-r1:32b",
                true,
                false,
                false,
            )],
        };

        let selected = resolve_ollama_selected_models(&config, &catalog);
        let options = catalog.options_by_modality();

        assert_eq!(selected.text.as_deref(), Some("llama3.2"));
        assert_eq!(selected.image.as_deref(), Some("gemma3"));
        assert!(options.text.iter().all(|option| option.value != "llama3.2"));
        assert!(options.image.is_empty());
    }

    #[test]
    fn ollama_library_parser_uses_ollama_description_and_capability_badges() {
        let html = r#"
        <li x-test-model class="flex">
          <a href="/library/gemma3">
            <div x-test-model-title title="gemma3">
              <p>The current, most capable model that runs on a single GPU.</p>
            </div>
            <span x-test-capability>vision</span>
          </a>
        </li>
        <li x-test-model class="flex">
          <a href="/library/nomic-embed-text">
            <div x-test-model-title title="nomic-embed-text">
              <p>A high-performing open embedding model.</p>
            </div>
            <span x-test-capability>embedding</span>
          </a>
        </li>
        <li x-test-model class="flex">
          <a href="/library/llama3.2">
            <div x-test-model-title title="llama3.2">
              <p>Meta&#39;s Llama 3.2 goes small with 1B and 3B models.</p>
            </div>
          </a>
        </li>
        "#;

        let models = ollama_library_models_from_html(html, 6);

        assert_eq!(models.len(), 2);
        assert_eq!(models[0].value, "gemma3");
        assert_eq!(
            models[0].detail,
            "The current, most capable model that runs on a single GPU."
        );
        assert!(models[0].input_support.text);
        assert!(models[0].input_support.image);
        assert!(!models[0].input_support.audio);
        assert_eq!(models[0].provenance, "ollama_library");
        assert_eq!(models[1].value, "llama3.2");
        assert_eq!(
            models[1].detail,
            "Meta's Llama 3.2 goes small with 1B and 3B models."
        );
        assert!(models[1].input_support.text);
        assert!(!models[1].input_support.image);
    }

    #[tokio::test]
    async fn run_json_command_times_out_without_waiting_for_child_completion() {
        let started = std::time::Instant::now();
        let args = vec!["-c".to_string(), "sleep 5".to_string()];
        let err = run_json_command("/bin/sh", &args, Duration::from_millis(50))
            .await
            .unwrap_err();
        assert!(format!("{err:#}").contains("timed out"));
        assert!(started.elapsed() < Duration::from_secs(2));
    }
}
