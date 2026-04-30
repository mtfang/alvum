use anyhow::{Context, Result};
use std::collections::BTreeSet;

#[derive(Clone, serde::Serialize)]
pub(super) struct ProviderSelectedModels {
    pub(super) text: Option<String>,
    pub(super) image: Option<String>,
    pub(super) audio: Option<String>,
}

#[derive(Clone, serde::Serialize)]
pub(crate) struct ProviderCapabilities {
    pub(crate) text: ProviderCapability,
    pub(crate) image: ProviderCapability,
    pub(crate) audio: ProviderCapability,
}

#[derive(Clone, serde::Serialize)]
pub(crate) struct ProviderCapability {
    pub(crate) supported: bool,
    pub(crate) model_supported: bool,
    pub(crate) adapter_supported: bool,
    pub(crate) provenance: String,
    pub(crate) status: String,
    pub(crate) detail: String,
}

pub(super) fn default_image_model_for(provider: &str) -> &'static str {
    match provider {
        "claude" | "cli" | "claude-cli" => "",
        "codex" | "codex-cli" => "",
        "ollama" => "",
        "bedrock" => "anthropic.claude-sonnet-4-20250514-v1:0",
        _ => "claude-sonnet-4-6",
    }
}

pub(super) fn provider_selected_models(
    config: &alvum_core::config::AlvumConfig,
    provider: &str,
) -> ProviderSelectedModels {
    ProviderSelectedModels {
        text: super::provider_setting_string(config, provider, "text_model")
            .or_else(|| super::provider_setting_string(config, provider, "model"))
            .map(|model| super::display_text_model_for_provider(provider, &model))
            .or_else(|| {
                matches!(provider, "claude-cli" | "codex-cli").then(|| "CLI default".to_string())
            })
            .or_else(|| {
                (provider != "ollama").then(|| super::default_model_for(provider).to_string())
            })
            .filter(|model| !model.trim().is_empty()),
        image: super::provider_setting_string(config, provider, "image_model")
            .map(|model| super::display_modality_model_for_provider(provider, &model))
            .or_else(|| {
                matches!(provider, "claude-cli" | "codex-cli").then(|| "CLI default".to_string())
            })
            .or_else(|| {
                (provider != "ollama").then(|| default_image_model_for(provider).to_string())
            })
            .filter(|model| !model.trim().is_empty()),
        audio: super::provider_setting_string(config, provider, "audio_model")
            .map(|model| super::display_modality_model_for_provider(provider, &model))
            .or_else(|| {
                matches!(provider, "claude-cli" | "codex-cli").then(|| "CLI default".to_string())
            })
            .filter(|model| !model.trim().is_empty()),
    }
}

#[derive(Clone, Copy, Default)]
struct ModelModalities {
    text: bool,
    image: bool,
    audio: bool,
}

impl ModelModalities {
    fn supports(self, modality: &str) -> bool {
        match modality {
            "text" => self.text,
            "image" => self.image,
            "audio" => self.audio,
            _ => false,
        }
    }
}

#[derive(Clone)]
struct ProviderCapabilityEvidence {
    text: Option<ModelModalities>,
    image: Option<ModelModalities>,
    audio: Option<ModelModalities>,
    provenance: String,
}

fn adapter_supports_modality(provider: &str, modality: &str) -> bool {
    match modality {
        "text" => true,
        "image" => matches!(provider, "anthropic-api" | "ollama"),
        "audio" => false,
        _ => false,
    }
}

fn capability_for(
    provider: &str,
    modality: &str,
    selected_model: Option<&str>,
    evidence: Option<ModelModalities>,
    provenance: &str,
) -> ProviderCapability {
    let adapter_supported = adapter_supports_modality(provider, modality);
    let model_supported = evidence
        .map(|modalities| modalities.supports(modality))
        .unwrap_or(false);
    let supported = adapter_supported && model_supported;
    let status = if supported {
        "ready"
    } else if model_supported && !adapter_supported {
        "transport_limited"
    } else {
        "unsupported"
    };
    let detail = match status {
        "ready" => format!(
            "{} input is supported by the selected model and Alvum adapter.",
            modality_label(modality)
        ),
        "transport_limited" => format!(
            "{} input is supported by the selected model, but Alvum's {provider} adapter cannot send it yet.",
            modality_label(modality)
        ),
        _ => match (selected_model, evidence) {
            (Some(model), None) => format!(
                "{} model {model} is not installed or its capability metadata is unavailable.",
                modality_label(modality)
            ),
            (Some(model), Some(_)) => format!(
                "{} input is not supported by selected model {model}.",
                modality_label(modality)
            ),
            (None, _) => format!(
                "No selected {} model is configured.",
                modality_label(modality)
            ),
        },
    };
    ProviderCapability {
        supported,
        model_supported,
        adapter_supported,
        provenance: provenance.into(),
        status: status.into(),
        detail,
    }
}

fn modality_label(modality: &str) -> &'static str {
    match modality {
        "text" => "Text",
        "image" => "Image",
        "audio" => "Audio",
        _ => "Data",
    }
}

pub(super) async fn provider_capabilities(
    config: &alvum_core::config::AlvumConfig,
    provider: &str,
    selected: &ProviderSelectedModels,
) -> ProviderCapabilities {
    let evidence = provider_capability_evidence(config, provider, selected)
        .await
        .unwrap_or_else(|| static_provider_capability_evidence(provider, selected));
    capabilities_from_evidence(provider, selected, evidence)
}

pub(crate) fn static_provider_capabilities(
    provider: &str,
    selected: &ProviderSelectedModels,
) -> ProviderCapabilities {
    capabilities_from_evidence(
        provider,
        selected,
        static_provider_capability_evidence(provider, selected),
    )
}

fn capabilities_from_evidence(
    provider: &str,
    selected: &ProviderSelectedModels,
    evidence: ProviderCapabilityEvidence,
) -> ProviderCapabilities {
    ProviderCapabilities {
        text: capability_for(
            provider,
            "text",
            selected.text.as_deref(),
            evidence.text,
            &evidence.provenance,
        ),
        image: capability_for(
            provider,
            "image",
            selected.image.as_deref(),
            evidence.image,
            &evidence.provenance,
        ),
        audio: capability_for(
            provider,
            "audio",
            selected.audio.as_deref(),
            evidence.audio,
            &evidence.provenance,
        ),
    }
}

fn static_provider_capability_evidence(
    provider: &str,
    selected: &ProviderSelectedModels,
) -> ProviderCapabilityEvidence {
    let text = selected.text.as_deref().map(|_| ModelModalities {
        text: true,
        image: false,
        audio: false,
    });
    let image = selected.image.as_deref().map(|model| match provider {
        "anthropic-api" | "claude-cli" => anthropic_model_modalities(model),
        "bedrock" => anthropic_model_modalities(model),
        "ollama" => ModelModalities {
            text: true,
            image: false,
            audio: false,
        },
        "codex-cli" => ModelModalities {
            text: true,
            image: false,
            audio: false,
        },
        _ => ModelModalities::default(),
    });
    ProviderCapabilityEvidence {
        text,
        image,
        audio: selected
            .audio
            .as_deref()
            .map(|_| ModelModalities::default()),
        provenance: "static_catalog".into(),
    }
}

async fn provider_capability_evidence(
    config: &alvum_core::config::AlvumConfig,
    provider: &str,
    selected: &ProviderSelectedModels,
) -> Option<ProviderCapabilityEvidence> {
    match provider {
        "bedrock" => bedrock_capability_evidence(config, selected).await.ok(),
        "ollama" => ollama_capability_evidence(config, selected).await.ok(),
        "codex-cli" => codex_capability_evidence(selected).await.ok(),
        "anthropic-api" => anthropic_capability_evidence(selected).await.ok(),
        "claude-cli" => Some(static_provider_capability_evidence(provider, selected)),
        _ => None,
    }
}

fn modalities_from_json_strings(values: Option<&Vec<serde_json::Value>>) -> ModelModalities {
    let mut modalities = ModelModalities::default();
    if let Some(values) = values {
        for value in values {
            if let Some(item) = value.as_str().map(|item| item.to_ascii_lowercase()) {
                match item.as_str() {
                    "text" | "completion" | "chat" => modalities.text = true,
                    "image" | "vision" => modalities.image = true,
                    "audio" | "speech" => modalities.audio = true,
                    _ => {}
                }
            }
        }
    }
    modalities
}

fn merge_modalities(mut left: ModelModalities, right: ModelModalities) -> ModelModalities {
    left.text |= right.text;
    left.image |= right.image;
    left.audio |= right.audio;
    left
}

fn bedrock_model_modalities(json: &serde_json::Value, model_id: &str) -> Option<ModelModalities> {
    json.get("modelSummaries")
        .and_then(|models| models.as_array())
        .into_iter()
        .flatten()
        .find(|model| model.get("modelId").and_then(|value| value.as_str()) == Some(model_id))
        .map(|model| {
            let input = modalities_from_json_strings(
                model
                    .get("inputModalities")
                    .and_then(|value| value.as_array()),
            );
            let output = modalities_from_json_strings(
                model
                    .get("outputModalities")
                    .and_then(|value| value.as_array()),
            );
            merge_modalities(input, output)
        })
}

fn ollama_show_modalities(json: &serde_json::Value) -> ModelModalities {
    modalities_from_json_strings(json.get("capabilities").and_then(|value| value.as_array()))
}

#[cfg(test)]
fn ollama_modalities_from_show_result(result: Result<serde_json::Value>) -> ModelModalities {
    result.map_or_else(
        |_| ModelModalities::default(),
        |json| ollama_show_modalities(&json),
    )
}

fn codex_model_modalities(json: &serde_json::Value, model_slug: &str) -> Option<ModelModalities> {
    json.get("models")
        .and_then(|models| models.as_array())
        .into_iter()
        .flatten()
        .find(|model| {
            model
                .get("slug")
                .or_else(|| model.get("id"))
                .and_then(|value| value.as_str())
                == Some(model_slug)
        })
        .map(|model| {
            let mut modalities = modalities_from_json_strings(
                model
                    .get("input_modalities")
                    .or_else(|| model.get("inputModalities"))
                    .and_then(|value| value.as_array()),
            );
            if !modalities.text {
                modalities.text = true;
            }
            modalities
        })
}

fn anthropic_model_modalities(model: &str) -> ModelModalities {
    let model = model.to_ascii_lowercase();
    ModelModalities {
        text: true,
        image: model.starts_with("claude-3")
            || model.starts_with("claude-sonnet-4")
            || model.starts_with("claude-opus-4")
            || model.starts_with("claude-haiku-4"),
        audio: false,
    }
}

fn anthropic_available_model_modalities(
    model: &str,
    available: &BTreeSet<String>,
) -> ModelModalities {
    if !available.contains(model) {
        return ModelModalities::default();
    }
    anthropic_model_modalities(model)
}

async fn bedrock_capability_evidence(
    config: &alvum_core::config::AlvumConfig,
    selected: &ProviderSelectedModels,
) -> Result<ProviderCapabilityEvidence> {
    let json = super::bedrock_models_json(config).await?;
    let text = selected
        .text
        .as_deref()
        .and_then(|model| bedrock_model_modalities(&json, model));
    let image = selected
        .image
        .as_deref()
        .and_then(|model| bedrock_model_modalities(&json, model));
    let audio = selected
        .audio
        .as_deref()
        .and_then(|model| bedrock_model_modalities(&json, model));
    Ok(ProviderCapabilityEvidence {
        text,
        image,
        audio,
        provenance: "native_api".into(),
    })
}

async fn ollama_show_json(
    config: &alvum_core::config::AlvumConfig,
    model: &str,
) -> Result<serde_json::Value> {
    let base_url = super::provider_setting_string(config, "ollama", "base_url")
        .unwrap_or_else(|| "http://localhost:11434".into())
        .trim_end_matches('/')
        .to_string();
    let client = reqwest::Client::builder()
        .timeout(super::PROVIDER_MODELS_TIMEOUT)
        .build()?;
    let json = client
        .post(format!("{base_url}/api/show"))
        .json(&serde_json::json!({ "model": model }))
        .send()
        .await
        .context("failed to query Ollama model details")?
        .error_for_status()
        .context("Ollama model details request failed")?
        .json()
        .await
        .context("Ollama returned malformed model details JSON")?;
    Ok(json)
}

async fn ollama_capability_evidence(
    config: &alvum_core::config::AlvumConfig,
    selected: &ProviderSelectedModels,
) -> Result<ProviderCapabilityEvidence> {
    let (text, image, audio) = tokio::join!(
        ollama_selected_model_modalities(config, selected.text.as_deref()),
        ollama_selected_model_modalities(config, selected.image.as_deref()),
        ollama_selected_model_modalities(config, selected.audio.as_deref())
    );
    Ok(ollama_capability_evidence_from_modalities(
        text, image, audio,
    ))
}

async fn ollama_selected_model_modalities(
    config: &alvum_core::config::AlvumConfig,
    model: Option<&str>,
) -> Option<ModelModalities> {
    let model = model?;
    ollama_show_json(config, model)
        .await
        .ok()
        .map(|json| ollama_show_modalities(&json))
}

fn ollama_capability_evidence_from_modalities(
    text: Option<ModelModalities>,
    image: Option<ModelModalities>,
    audio: Option<ModelModalities>,
) -> ProviderCapabilityEvidence {
    ProviderCapabilityEvidence {
        text,
        image,
        audio,
        provenance: "native_api".into(),
    }
}

async fn anthropic_capability_evidence(
    selected: &ProviderSelectedModels,
) -> Result<ProviderCapabilityEvidence> {
    let options = super::anthropic_model_options().await?;
    let available = options
        .into_iter()
        .map(|option| option.value)
        .collect::<BTreeSet<_>>();
    let text = selected
        .text
        .as_deref()
        .map(|model| anthropic_available_model_modalities(model, &available));
    let image = selected
        .image
        .as_deref()
        .map(|model| anthropic_available_model_modalities(model, &available));
    Ok(ProviderCapabilityEvidence {
        text,
        image,
        audio: selected
            .audio
            .as_deref()
            .map(|_| ModelModalities::default()),
        provenance: "native_api+static_catalog".into(),
    })
}

async fn codex_models_json() -> Result<serde_json::Value> {
    match super::run_json_command(
        "codex",
        &["debug".into(), "models".into(), "--bundled".into()],
        super::PROVIDER_MODELS_TIMEOUT,
    )
    .await
    {
        Ok(json) => Ok(json),
        Err(_) => {
            super::run_json_command(
                "codex",
                &["debug".into(), "models".into()],
                super::PROVIDER_MODELS_TIMEOUT,
            )
            .await
        }
    }
}

async fn codex_capability_evidence(
    selected: &ProviderSelectedModels,
) -> Result<ProviderCapabilityEvidence> {
    let json = codex_models_json().await?;
    let text = selected
        .text
        .as_deref()
        .and_then(|model| codex_model_modalities(&json, model))
        .or(Some(ModelModalities {
            text: true,
            image: false,
            audio: false,
        }));
    let image = selected
        .image
        .as_deref()
        .and_then(|model| codex_model_modalities(&json, model));
    let audio = selected
        .audio
        .as_deref()
        .and_then(|model| codex_model_modalities(&json, model));
    Ok(ProviderCapabilityEvidence {
        text,
        image,
        audio,
        provenance: "cli_catalog".into(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bedrock_modalities_parse_input_and_output_modalities() {
        let json = serde_json::json!({
            "modelSummaries": [
                {
                    "modelId": "text-only",
                    "inputModalities": ["TEXT"],
                    "outputModalities": ["TEXT"]
                },
                {
                    "modelId": "vision",
                    "inputModalities": ["TEXT", "IMAGE"],
                    "outputModalities": ["TEXT"]
                }
            ]
        });
        let text = bedrock_model_modalities(&json, "text-only").unwrap();
        assert!(text.text);
        assert!(!text.image);
        let vision = bedrock_model_modalities(&json, "vision").unwrap();
        assert!(vision.text);
        assert!(vision.image);
        assert!(!vision.audio);
    }

    #[test]
    fn ollama_show_capabilities_map_vision_to_image() {
        let json = serde_json::json!({
            "capabilities": ["completion", "vision"]
        });
        let modalities = ollama_show_modalities(&json);
        assert!(modalities.text);
        assert!(modalities.image);
        assert!(!modalities.audio);
    }

    #[test]
    fn ollama_show_failure_maps_to_unsupported_modalities() {
        let modalities =
            ollama_modalities_from_show_result(Err(anyhow::anyhow!("model not installed")));
        assert!(!modalities.text);
        assert!(!modalities.image);
        assert!(!modalities.audio);
    }

    #[test]
    fn ollama_partial_capability_evidence_keeps_successful_modalities() {
        let text = ollama_modalities_from_show_result(Ok(serde_json::json!({
            "capabilities": ["completion"]
        })));
        let image = ollama_modalities_from_show_result(Err(anyhow::anyhow!("missing image model")));
        let evidence = ollama_capability_evidence_from_modalities(Some(text), Some(image), None);

        let text = evidence.text.unwrap();
        assert!(text.text);
        assert!(!text.image);
        let image = evidence.image.unwrap();
        assert!(!image.text);
        assert!(!image.image);
        assert!(!image.audio);
        assert_eq!(evidence.provenance, "native_api");
    }

    #[test]
    fn codex_catalog_reads_input_modalities() {
        let json = serde_json::json!({
            "models": [
                {
                    "slug": "gpt-5.4",
                    "input_modalities": ["text", "image"]
                }
            ]
        });
        let modalities = codex_model_modalities(&json, "gpt-5.4").unwrap();
        assert!(modalities.text);
        assert!(modalities.image);
        assert!(!modalities.audio);
    }

    #[test]
    fn anthropic_static_catalog_does_not_claim_audio() {
        let modalities = anthropic_model_modalities("claude-sonnet-4-6");
        assert!(modalities.text);
        assert!(modalities.image);
        assert!(!modalities.audio);
    }

    #[test]
    fn anthropic_available_catalog_gates_image_support() {
        let available = BTreeSet::from(["claude-3-5-sonnet-latest".to_string()]);
        let available_model =
            anthropic_available_model_modalities("claude-3-5-sonnet-latest", &available);
        assert!(available_model.text);
        assert!(available_model.image);

        let missing_model = anthropic_available_model_modalities("claude-sonnet-4-6", &available);
        assert!(!missing_model.text);
        assert!(!missing_model.image);
        assert!(!missing_model.audio);
    }
}
