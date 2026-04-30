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
    ProviderCapabilities, ProviderSelectedModels, default_image_model_for, provider_capabilities,
    provider_selected_models, static_provider_capabilities,
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
    /// omitted, picks a sensible default per provider.
    Test {
        #[arg(long)]
        provider: String,
        #[arg(long)]
        model: Option<String>,
    },

    /// Output JSON model options for a provider. Uses live provider
    /// catalogs when available, with safe defaults as fallback options.
    Models {
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
        Action::Test { provider, model } => {
            let model = match model {
                Some(model) => model,
                None => default_model_for_config(&provider).await,
            };
            cmd_providers_test(&provider, &model).await
        }
        Action::Models { provider } => cmd_providers_models(&provider).await,
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
        "codex" | "codex-cli" => "", // let codex pick from its config
        "ollama" => "",
        "bedrock" => "anthropic.claude-sonnet-4-20250514-v1:0",
        // claude-cli / anthropic-api / cli / api / auto / unknown
        _ => "claude-sonnet-4-6",
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
    provider_setting_string(&config, &normalized, "text_model")
        .or_else(|| provider_setting_string(&config, &normalized, "model"))
        .unwrap_or_else(|| default_model_for(&normalized).into())
}

/// Each entry the popover renders. `available` reflects the cheap
/// detection check; an entry that's `available` may still fail at call
/// time if the user hasn't actually completed `claude login` etc. —
/// the Test action proves end-to-end auth.
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
    config_fields: Vec<ProviderConfigField>,
    selected_models: ProviderSelectedModels,
    capabilities: ProviderCapabilities,
    readiness: ProviderReadiness,
    active: bool,
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
    options: Vec<ProviderModelOption>,
}

#[derive(Clone, serde::Serialize)]
struct ProviderModelOption {
    value: String,
    label: String,
}

#[derive(Clone, serde::Serialize)]
struct ProviderInstallableModel {
    value: String,
    label: String,
    detail: String,
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
    let selected_models = selected_models_for_provider(&config, p.name).await;
    let capabilities = provider_capabilities(&config, p.name, &selected_models).await;
    let readiness = provider_readiness(p.available, config.provider_enabled(p.name));
    let config_fields =
        provider_config_fields_with_selected_models(&config, p.name, &selected_models).await;
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
        config_fields,
        selected_models,
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
    if provider != "ollama" {
        return provider_selected_models(config, provider);
    }
    match tokio::time::timeout(PROVIDER_MODELS_TIMEOUT, ollama_model_catalog(config)).await {
        Ok(Ok(catalog)) => resolve_ollama_selected_models(config, &catalog),
        _ => provider_selected_models(config, provider),
    }
}

async fn provider_config_fields_with_selected_models(
    config: &alvum_core::config::AlvumConfig,
    provider: &str,
    selected: &ProviderSelectedModels,
) -> Vec<ProviderConfigField> {
    let mut fields = provider_config_fields(config, provider);
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
            description: "Uses the Claude Code subscription already logged in on this Mac.",
            available: cli_binary_on_path("claude"),
            auth_hint: "subscription via `claude login`",
            setup_kind: "terminal",
            setup_label: "Login",
            setup_hint: "Opens Terminal and runs `claude login`.",
            setup_command: Some("claude login"),
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
            setup_hint: "Choose an AWS profile and region. Credentials still come from the standard AWS chain.",
            setup_command: Some("aws configure"),
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

fn config_field(
    config: &alvum_core::config::AlvumConfig,
    provider: &str,
    key: &'static str,
    label: &'static str,
    kind: &'static str,
    detail: &'static str,
    placeholder: &'static str,
) -> ProviderConfigField {
    let value = provider_setting_string(config, provider, key);
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
        configured: value.is_some(),
        value,
        placeholder,
        detail,
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
                "Optional model override. Leave blank to use the CLI default.",
                default_model_for(provider),
            ),
            config_field(
                config,
                provider,
                "image_model",
                "Image model",
                "text",
                "Tracked for model capability display; this adapter is text-only today.",
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
    }
}

fn installable_model(
    value: impl Into<String>,
    label: impl Into<String>,
    detail: impl Into<String>,
) -> ProviderInstallableModel {
    ProviderInstallableModel {
        value: value.into(),
        label: label.into(),
        detail: detail.into(),
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

fn ollama_installable_models() -> Vec<ProviderInstallableModel> {
    vec![
        installable_model(
            "gemma4:e2b",
            "Gemma 4 E2B",
            "Small edge model; good first Ollama download for laptops.",
        ),
        installable_model(
            "gemma4:e4b",
            "Gemma 4 E4B",
            "Stronger edge model when you have more memory available.",
        ),
        installable_model("gemma4", "Gemma 4", "Default Gemma 4 local model."),
        installable_model(
            "llama3.2",
            "Llama 3.2",
            "Compact general-purpose local model.",
        ),
        installable_model(
            "qwen3:4b",
            "Qwen 3 4B",
            "Small reasoning-oriented local model.",
        ),
        installable_model(
            "mistral",
            "Mistral",
            "Reliable lightweight general-purpose model.",
        ),
    ]
}

fn static_model_options(provider: &str) -> Vec<ProviderModelOption> {
    match provider {
        "claude-cli" => vec![
            model_option("sonnet", "Sonnet"),
            model_option("opus", "Opus"),
            model_option(default_model_for(provider), default_model_for(provider)),
        ],
        "codex-cli" => vec![model_option("", "CLI default")],
        "anthropic-api" => vec![model_option(
            default_model_for(provider),
            default_model_for(provider),
        )],
        "bedrock" => vec![model_option(
            default_model_for(provider),
            default_model_for(provider),
        )],
        "ollama" => vec![],
        _ => vec![],
    }
}

fn static_model_options_for_field(provider: &str, key: &str) -> Vec<ProviderModelOption> {
    if key == "image_model" {
        return match provider {
            "codex-cli" => vec![model_option("", "CLI default")],
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
        return vec![];
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

fn aws_credentials_present(config: &alvum_core::config::AlvumConfig) -> bool {
    std::env::var("AWS_PROFILE").is_ok()
        || std::env::var("AWS_ACCESS_KEY_ID").is_ok()
        || std::env::var("AWS_SESSION_TOKEN").is_ok()
        || provider_setting_string(config, "bedrock", "aws_profile").is_some()
        || dirs::home_dir()
            .map(|h| h.join(".aws/credentials").exists() || h.join(".aws/config").exists())
            .unwrap_or(false)
}

const PROVIDER_TEST_TIMEOUT: Duration = Duration::from_secs(25);

#[derive(Clone, serde::Serialize)]
struct ProviderTestReport {
    provider: String,
    status: String,
    ok: bool,
    elapsed_ms: u128,
    response_preview: Option<String>,
    error: Option<String>,
}

async fn provider_test_report(provider_name: &str, model: &str) -> ProviderTestReport {
    // Tiny prompt. The expected response is "OK" — anything containing
    // it counts as success. Some providers may include leading
    // whitespace or quote marks, hence the contains() check.
    const TEST_SYSTEM: &str =
        "You are a connectivity probe. Reply with the exact word OK and nothing else.";
    const TEST_USER: &str = "ping";
    let started = std::time::Instant::now();
    let normalized = normalize_name(provider_name);

    if !known_provider_name(&normalized) || normalized == "auto" {
        return ProviderTestReport {
            provider: normalized,
            status: "unknown_provider".into(),
            ok: false,
            elapsed_ms: started.elapsed().as_millis(),
            response_preview: None,
            error: Some(format!("unknown provider: {provider_name}")),
        };
    }

    if normalized == "ollama" {
        return ollama_provider_test_report(model, started).await;
    }

    let probe = async {
        let provider = alvum_pipeline::llm::create_provider_async(&normalized, model)
            .await
            .with_context(|| format!("provider construction failed for {normalized}"))?;
        provider.complete(TEST_SYSTEM, TEST_USER).await
    };

    match tokio::time::timeout(PROVIDER_TEST_TIMEOUT, probe).await {
        Err(_) => ProviderTestReport {
            provider: normalized,
            status: "timeout".into(),
            ok: false,
            elapsed_ms: started.elapsed().as_millis(),
            response_preview: None,
            error: Some(format!(
                "provider probe timed out after {}s",
                PROVIDER_TEST_TIMEOUT.as_secs()
            )),
        },
        Ok(Ok(text)) => {
            let preview: String = text.chars().take(80).collect();
            let ok = text.to_uppercase().contains("OK");
            ProviderTestReport {
                provider: normalized,
                status: if ok {
                    "available".into()
                } else {
                    "unexpected_response".into()
                },
                ok,
                elapsed_ms: started.elapsed().as_millis(),
                response_preview: Some(preview),
                error: if ok {
                    None
                } else {
                    Some(format!("response did not contain 'OK': {text:?}"))
                },
            }
        }
        Ok(Err(e)) => ProviderTestReport {
            provider: normalized,
            status: alvum_pipeline::llm::classify_provider_error_status(&e).into(),
            ok: false,
            elapsed_ms: started.elapsed().as_millis(),
            response_preview: None,
            error: Some(format!("{e:#}")),
        },
    }
}

async fn ollama_provider_test_report(
    model: &str,
    started: std::time::Instant,
) -> ProviderTestReport {
    let config = alvum_core::config::AlvumConfig::load()
        .unwrap_or_else(|_| alvum_core::config::AlvumConfig::default());
    match tokio::time::timeout(PROVIDER_TEST_TIMEOUT, ollama_model_options(&config)).await {
        Err(_) => ProviderTestReport {
            provider: "ollama".into(),
            status: "timeout".into(),
            ok: false,
            elapsed_ms: started.elapsed().as_millis(),
            response_preview: None,
            error: Some(format!(
                "Ollama model list timed out after {}s",
                PROVIDER_TEST_TIMEOUT.as_secs()
            )),
        },
        Ok(Err(e)) => ProviderTestReport {
            provider: "ollama".into(),
            status: "unavailable".into(),
            ok: false,
            elapsed_ms: started.elapsed().as_millis(),
            response_preview: None,
            error: Some(format!("{e:#}")),
        },
        Ok(Ok((source, options))) => {
            let requested = model.trim();
            let installed = options.iter().any(|option| option.value == requested);
            let has_models = !options.is_empty();
            let ok = has_models && (requested.is_empty() || installed);
            ProviderTestReport {
                provider: "ollama".into(),
                status: if ok {
                    "available".into()
                } else if has_models {
                    "model_not_installed".into()
                } else {
                    "no_models".into()
                },
                ok,
                elapsed_ms: started.elapsed().as_millis(),
                response_preview: Some(format!(
                    "{} installed model(s) from {source}",
                    options.len()
                )),
                error: if ok {
                    None
                } else if has_models {
                    Some(format!(
                        "Ollama is running, but model {requested:?} is not installed. Choose an installed model or download it."
                    ))
                } else {
                    Some("Ollama is running, but no local models are installed.".into())
                },
            }
        }
    }
}

async fn cmd_providers_test(provider_name: &str, model: &str) -> Result<()> {
    let report = provider_test_report(provider_name, model).await;

    println!("{}", serde_json::to_string_pretty(&report)?);
    Ok(())
}

const PROVIDER_MODELS_TIMEOUT: Duration = Duration::from_secs(8);

#[derive(serde::Serialize)]
struct ProviderModelsReport {
    ok: bool,
    provider: String,
    source: String,
    options: Vec<ProviderModelOption>,
    options_by_modality: ProviderModelOptionsByModality,
    installable_options: Vec<ProviderInstallableModel>,
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

fn model_options_with_config(
    config: &alvum_core::config::AlvumConfig,
    provider: &str,
    options: Vec<ProviderModelOption>,
) -> Vec<ProviderModelOption> {
    if provider == "ollama" {
        return dedupe_model_options(options);
    }
    let mut merged = Vec::new();
    if let Some(current) = provider_setting_string(config, provider, "text_model")
        .or_else(|| provider_setting_string(config, provider, "model"))
    {
        merged.push(model_option(current.clone(), current));
    }
    if provider == "codex-cli" {
        merged.push(model_option("", "CLI default"));
    }
    merged.extend(options);
    dedupe_model_options(merged)
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

fn ollama_modalities_from_show_json(json: &serde_json::Value) -> (bool, bool, bool) {
    let mut text = false;
    let mut image = false;
    let mut audio = false;
    if let Some(values) = json.get("capabilities").and_then(|value| value.as_array()) {
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
    }
    (text, image, audio)
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
        let show_json: serde_json::Value = client
            .post(format!("{base_url}/api/show"))
            .json(&serde_json::json!({ "model": option.value }))
            .send()
            .await
            .with_context(|| format!("failed to query Ollama model details for {}", option.value))?
            .error_for_status()
            .with_context(|| format!("Ollama model details request failed for {}", option.value))?
            .json()
            .await
            .with_context(|| {
                format!(
                    "Ollama returned malformed model details JSON for {}",
                    option.value
                )
            })?;
        let (text, image, audio) = ollama_modalities_from_show_json(&show_json);
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

async fn bedrock_models_json(
    config: &alvum_core::config::AlvumConfig,
) -> Result<serde_json::Value> {
    let mut args = vec![
        "bedrock".to_string(),
        "list-foundation-models".to_string(),
        "--by-provider".to_string(),
        "Anthropic".to_string(),
        "--output".to_string(),
        "json".to_string(),
    ];
    if let Some(region) = provider_setting_string(config, "bedrock", "aws_region") {
        args.push("--region".into());
        args.push(region);
    }
    if let Some(profile) = provider_setting_string(config, "bedrock", "aws_profile") {
        args.push("--profile".into());
        args.push(profile);
    }
    run_json_command("aws", &args, PROVIDER_MODELS_TIMEOUT).await
}

async fn bedrock_model_options(
    config: &alvum_core::config::AlvumConfig,
) -> Result<Vec<ProviderModelOption>> {
    let json = bedrock_models_json(config).await?;
    let options = json
        .get("modelSummaries")
        .and_then(|models| models.as_array())
        .into_iter()
        .flatten()
        .filter_map(|model| {
            let id = model.get("modelId").and_then(|value| value.as_str())?;
            Some(model_option(id, id))
        })
        .collect::<Vec<_>>();
    Ok(options)
}

async fn live_model_options(
    config: &alvum_core::config::AlvumConfig,
    provider: &str,
) -> Result<(String, Vec<ProviderModelOption>)> {
    match provider {
        "claude-cli" => Ok(("static".into(), static_model_options(provider))),
        "codex-cli" => Ok(("codex-cli".into(), codex_model_options().await?)),
        "anthropic-api" => Ok(("anthropic-api".into(), anthropic_model_options().await?)),
        "bedrock" => Ok(("aws-bedrock".into(), bedrock_model_options(config).await?)),
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
                error: Some(format!("unknown provider: {provider_name}")),
            })?
        );
        return Ok(());
    }

    let config = alvum_core::config::AlvumConfig::load()
        .unwrap_or_else(|_| alvum_core::config::AlvumConfig::default());
    let fallback =
        model_options_with_config(&config, &normalized, static_model_options(&normalized));
    let installable_options = if normalized == "ollama" {
        ollama_installable_models()
    } else {
        vec![]
    };
    let report = if normalized == "ollama" {
        match ollama_model_catalog(&config).await {
            Ok(catalog) if !catalog.models.is_empty() => ProviderModelsReport {
                ok: true,
                provider: normalized.clone(),
                source: catalog.source.clone(),
                options: catalog.all_options(),
                options_by_modality: catalog.options_by_modality(),
                installable_options,
                error: None,
            },
            Ok(catalog) => ProviderModelsReport {
                ok: false,
                provider: normalized.clone(),
                source: catalog.source,
                options: vec![],
                options_by_modality: ProviderModelOptionsByModality::default(),
                installable_options,
                error: Some("provider returned no installed models".into()),
            },
            Err(e) => ProviderModelsReport {
                ok: false,
                provider: normalized.clone(),
                source: "fallback".into(),
                options: vec![],
                options_by_modality: ProviderModelOptionsByModality::default(),
                installable_options,
                error: Some(format!("{e:#}")),
            },
        }
    } else {
        match live_model_options(&config, &normalized).await {
            Ok((source, options)) if !options.is_empty() => ProviderModelsReport {
                ok: true,
                provider: normalized.clone(),
                source,
                options: model_options_with_config(&config, &normalized, options.clone()),
                options_by_modality: ProviderModelOptionsByModality {
                    text: model_options_with_config(&config, &normalized, options.clone()),
                    image: model_options_with_config(&config, &normalized, options),
                    audio: vec![],
                },
                installable_options,
                error: None,
            },
            Ok((source, _)) => ProviderModelsReport {
                ok: false,
                provider: normalized.clone(),
                source,
                options: fallback.clone(),
                options_by_modality: ProviderModelOptionsByModality {
                    text: fallback.clone(),
                    image: fallback,
                    audio: vec![],
                },
                installable_options,
                error: Some("provider returned no model options".into()),
            },
            Err(e) => ProviderModelsReport {
                ok: false,
                provider: normalized.clone(),
                source: "fallback".into(),
                options: fallback.clone(),
                options_by_modality: ProviderModelOptionsByModality {
                    text: fallback.clone(),
                    image: fallback,
                    audio: vec![],
                },
                installable_options,
                error: Some(format!("{e:#}")),
            },
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
    let mut reports = Vec::new();
    for entry in &entries {
        let report = if entry.available {
            provider_test_report(entry.name, default_model_for(entry.name)).await
        } else {
            ProviderTestReport {
                provider: entry.name.into(),
                status: "not_installed".into(),
                ok: false,
                elapsed_ms: 0,
                response_preview: None,
                error: Some(entry.auth_hint.into()),
            }
        };
        reports.push(report);
    }

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
