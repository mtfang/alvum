use anyhow::{Context, Result};
use std::ffi::OsString;
use std::path::PathBuf;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize)]
pub struct BedrockModelInputSupport {
    pub text: bool,
    pub image: bool,
    pub audio: bool,
}

impl BedrockModelInputSupport {
    pub fn supports(self, modality: &str) -> bool {
        match modality {
            "text" => self.text,
            "image" => self.image,
            "audio" => self.audio,
            _ => false,
        }
    }
}

#[derive(Clone, Debug)]
pub struct BedrockFoundationModel {
    pub model_id: String,
    pub model_name: String,
    pub active: bool,
    pub input: BedrockModelInputSupport,
    pub output: BedrockModelInputSupport,
    pub on_demand: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BedrockInferenceProfileKind {
    System,
    Application,
    Unknown,
}

#[derive(Clone, Debug)]
pub struct BedrockInferenceProfile {
    pub id: String,
    pub arn: String,
    pub name: String,
    pub active: bool,
    pub kind: BedrockInferenceProfileKind,
    pub source_model_ids: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BedrockInvokeTargetKind {
    BaseModel,
    InferenceProfile,
}

#[derive(Clone, Debug)]
pub struct BedrockInvokeTarget {
    pub invoke_id: String,
    pub label: String,
    pub source: String,
    pub kind: BedrockInvokeTargetKind,
    pub source_model_id: Option<String>,
    pub source_model_name: Option<String>,
    pub input_support: BedrockModelInputSupport,
    pub detail: String,
}

pub fn unverified_configured_target(value: &str) -> BedrockInvokeTarget {
    let value = value.trim().to_string();
    BedrockInvokeTarget {
        invoke_id: value.clone(),
        label: value.clone(),
        source: "configured".into(),
        kind: if looks_like_inference_profile(&value) {
            BedrockInvokeTargetKind::InferenceProfile
        } else {
            BedrockInvokeTargetKind::BaseModel
        },
        source_model_id: (!looks_like_inference_profile(&value)).then_some(value),
        source_model_name: None,
        input_support: BedrockModelInputSupport::default(),
        detail: "Explicit configured Bedrock target; live capability metadata was unavailable."
            .into(),
    }
}

#[derive(Clone, Debug, Default)]
pub struct BedrockCatalog {
    foundation_models: Vec<BedrockFoundationModel>,
    inference_profiles: Vec<BedrockInferenceProfile>,
}

impl BedrockCatalog {
    pub async fn load(
        profile: Option<String>,
        region: Option<String>,
        extra_path: Option<String>,
    ) -> Result<Self> {
        let cfg = sdk_config(profile, region, extra_path).await;
        let client = aws_sdk_bedrock::Client::new(&cfg);
        Self::from_client(&client).await
    }

    pub async fn from_client(client: &aws_sdk_bedrock::Client) -> Result<Self> {
        let foundation_output = client
            .list_foundation_models()
            .by_provider("Anthropic")
            .send()
            .await
            .context("failed to list Bedrock foundation models")?;
        let foundation_models = foundation_output
            .model_summaries()
            .iter()
            .map(BedrockFoundationModel::from_summary)
            .collect::<Vec<_>>();

        let mut inference_profiles = Vec::new();
        let mut next_token = None;
        loop {
            let output = client
                .list_inference_profiles()
                .set_next_token(next_token)
                .send()
                .await
                .context("failed to list Bedrock inference profiles")?;
            inference_profiles.extend(
                output
                    .inference_profile_summaries()
                    .iter()
                    .map(BedrockInferenceProfile::from_summary),
            );
            next_token = output.next_token().map(str::to_string);
            if next_token.is_none() {
                break;
            }
        }

        Ok(Self {
            foundation_models,
            inference_profiles,
        })
    }

    pub fn from_test_records(
        foundation_models: Vec<BedrockFoundationModel>,
        inference_profiles: Vec<BedrockInferenceProfile>,
    ) -> Self {
        Self {
            foundation_models,
            inference_profiles,
        }
    }

    pub fn resolve_invoke_target(
        &self,
        configured: Option<&str>,
        modality: &str,
    ) -> Result<BedrockInvokeTarget> {
        let configured = configured.map(str::trim).filter(|value| !value.is_empty());
        if let Some(configured) = configured {
            if let Some(target) = self.target_for_configured_profile(configured, modality) {
                return Ok(target);
            }
            if looks_like_inference_profile(configured) {
                return Ok(BedrockInvokeTarget {
                    invoke_id: configured.to_string(),
                    label: configured.to_string(),
                    source: "configured".into(),
                    kind: BedrockInvokeTargetKind::InferenceProfile,
                    source_model_id: None,
                    source_model_name: None,
                    input_support: BedrockModelInputSupport::default(),
                    detail: "Explicit inference profile target; live capability metadata was unavailable.".into(),
                });
            }
            return self
                .target_for_model_id(configured, modality)
                .with_context(|| {
                    format!("Bedrock model {configured:?} is not a usable {modality} invoke target")
                });
        }

        self.default_target(modality).with_context(|| {
            format!("No usable Bedrock {modality} model or inference profile was returned by the live Bedrock catalog")
        })
    }

    pub fn targets_for_modality(&self, modality: &str) -> Vec<BedrockInvokeTarget> {
        let mut targets = Vec::new();
        for model in self
            .foundation_models
            .iter()
            .filter(|model| model.usable_for_modality(modality))
        {
            targets.extend(self.profile_targets_for_model(model, modality, true));
            if model.on_demand {
                targets.push(base_target(model));
            }
        }
        targets.sort_by_key(target_sort_key);
        dedupe_targets(targets)
    }

    fn default_target(&self, modality: &str) -> Option<BedrockInvokeTarget> {
        self.foundation_models
            .iter()
            .filter(|model| model.usable_for_modality(modality))
            .flat_map(|model| {
                let mut targets = self.profile_targets_for_model(model, modality, false);
                if model.on_demand {
                    targets.push(base_target(model));
                }
                targets
            })
            .min_by_key(target_sort_key)
    }

    fn target_for_model_id(&self, model_id: &str, modality: &str) -> Option<BedrockInvokeTarget> {
        let model = self
            .foundation_models
            .iter()
            .find(|model| model.model_id == model_id)?;
        if !model.usable_for_modality(modality) {
            return None;
        }
        self.profile_targets_for_model(model, modality, false)
            .into_iter()
            .min_by_key(target_sort_key)
            .or_else(|| model.on_demand.then(|| base_target(model)))
    }

    fn target_for_configured_profile(
        &self,
        configured: &str,
        modality: &str,
    ) -> Option<BedrockInvokeTarget> {
        let profile = self
            .inference_profiles
            .iter()
            .find(|profile| profile.id == configured || profile.arn == configured)?;
        if !profile.active {
            return None;
        }
        profile
            .source_model_ids
            .iter()
            .filter_map(|model_id| {
                self.foundation_models
                    .iter()
                    .find(|model| &model.model_id == model_id)
            })
            .find(|model| model.usable_for_modality(modality))
            .map(|model| {
                let mut target = profile_target(profile, model);
                target.invoke_id = configured.to_string();
                target.source = "configured".into();
                target
            })
    }

    fn profile_targets_for_model(
        &self,
        model: &BedrockFoundationModel,
        _modality: &str,
        include_application: bool,
    ) -> Vec<BedrockInvokeTarget> {
        self.inference_profiles
            .iter()
            .filter(|profile| profile.active)
            .filter(|profile| {
                include_application || profile.kind == BedrockInferenceProfileKind::System
            })
            .filter(|profile| {
                profile
                    .source_model_ids
                    .iter()
                    .any(|source_model| source_model == &model.model_id)
            })
            .map(|profile| profile_target(profile, model))
            .collect()
    }
}

impl BedrockFoundationModel {
    fn from_summary(summary: &aws_sdk_bedrock::types::FoundationModelSummary) -> Self {
        Self {
            model_id: summary.model_id().to_string(),
            model_name: summary
                .model_name()
                .unwrap_or_else(|| summary.model_id())
                .to_string(),
            active: summary
                .model_lifecycle()
                .map(|lifecycle| lifecycle.status().as_str() == "ACTIVE")
                .unwrap_or(true),
            input: modalities(summary.input_modalities().iter().map(|item| item.as_str())),
            output: modalities(summary.output_modalities().iter().map(|item| item.as_str())),
            on_demand: summary
                .inference_types_supported()
                .iter()
                .any(|item| item.as_str() == "ON_DEMAND"),
        }
    }

    pub fn usable_for_modality(&self, modality: &str) -> bool {
        self.active && self.input.supports(modality) && self.output.text
    }

    pub fn modalities_for_alvum(&self) -> BedrockModelInputSupport {
        BedrockModelInputSupport {
            text: self.input.text && self.output.text,
            image: self.input.image && self.output.text,
            audio: self.input.audio && self.output.text,
        }
    }

    #[cfg(test)]
    pub fn test(
        model_id: &str,
        model_name: &str,
        active: bool,
        input: &[&str],
        output: &[&str],
        inference_types: &[&str],
    ) -> Self {
        Self {
            model_id: model_id.into(),
            model_name: model_name.into(),
            active,
            input: modalities(input.iter().copied()),
            output: modalities(output.iter().copied()),
            on_demand: inference_types
                .iter()
                .any(|item| item.eq_ignore_ascii_case("ON_DEMAND")),
        }
    }
}

impl BedrockInferenceProfile {
    fn from_summary(summary: &aws_sdk_bedrock::types::InferenceProfileSummary) -> Self {
        Self {
            id: summary.inference_profile_id().to_string(),
            arn: summary.inference_profile_arn().to_string(),
            name: summary.inference_profile_name().to_string(),
            active: summary.status().as_str() == "ACTIVE",
            kind: match summary.r#type().as_str() {
                "SYSTEM_DEFINED" => BedrockInferenceProfileKind::System,
                "APPLICATION" => BedrockInferenceProfileKind::Application,
                _ => BedrockInferenceProfileKind::Unknown,
            },
            source_model_ids: summary
                .models()
                .iter()
                .filter_map(|model| model.model_arn())
                .filter_map(model_id_from_model_arn)
                .collect(),
        }
    }

    #[cfg(test)]
    pub fn test_system(id: &str, name: &str, source_model_ids: &[&str]) -> Self {
        Self {
            id: id.into(),
            arn: format!("arn:aws:bedrock:us-east-1::inference-profile/{id}"),
            name: name.into(),
            active: true,
            kind: BedrockInferenceProfileKind::System,
            source_model_ids: source_model_ids.iter().map(|item| (*item).into()).collect(),
        }
    }
}

pub async fn sdk_config(
    profile: Option<String>,
    region: Option<String>,
    extra_path: Option<String>,
) -> aws_config::SdkConfig {
    apply_extra_path(extra_path.as_deref());
    let mut loader = aws_config::defaults(aws_config::BehaviorVersion::latest())
        .region(aws_config::Region::new(resolve_region(region.as_deref())));
    if let Some(profile) = profile.filter(|value| !value.trim().is_empty()) {
        loader = loader.profile_name(profile);
    }
    loader.load().await
}

pub fn resolve_region(configured: Option<&str>) -> String {
    configured
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .or_else(|| {
            std::env::var("AWS_REGION")
                .ok()
                .filter(|value| !value.trim().is_empty())
        })
        .or_else(|| {
            std::env::var("AWS_DEFAULT_REGION")
                .ok()
                .filter(|value| !value.trim().is_empty())
        })
        .unwrap_or_else(|| "us-east-1".into())
}

pub fn path_with_extra_path(
    current: Option<OsString>,
    extra_path: Option<&str>,
) -> Option<OsString> {
    let extra_entries = split_path(extra_path.unwrap_or_default());
    if extra_entries.is_empty() {
        return current;
    }
    let mut entries = extra_entries;
    entries.extend(
        current
            .as_deref()
            .map(std::env::split_paths)
            .into_iter()
            .flatten(),
    );
    std::env::join_paths(entries).ok()
}

pub fn apply_extra_path(extra_path: Option<&str>) {
    let Some(path) = path_with_extra_path(std::env::var_os("PATH"), extra_path) else {
        return;
    };
    unsafe { std::env::set_var("PATH", path) };
}

fn split_path(value: &str) -> Vec<PathBuf> {
    value
        .split(':')
        .map(str::trim)
        .filter(|entry| !entry.is_empty())
        .map(PathBuf::from)
        .collect()
}

fn base_target(model: &BedrockFoundationModel) -> BedrockInvokeTarget {
    BedrockInvokeTarget {
        invoke_id: model.model_id.clone(),
        label: model.model_name.clone(),
        source: "base_model".into(),
        kind: BedrockInvokeTargetKind::BaseModel,
        source_model_id: Some(model.model_id.clone()),
        source_model_name: Some(model.model_name.clone()),
        input_support: model.modalities_for_alvum(),
        detail: "On-demand base model.".into(),
    }
}

fn profile_target(
    profile: &BedrockInferenceProfile,
    model: &BedrockFoundationModel,
) -> BedrockInvokeTarget {
    let scope = if profile.id.starts_with("global.") {
        "Global"
    } else if profile.kind == BedrockInferenceProfileKind::System {
        "Geography"
    } else {
        "Application"
    };
    BedrockInvokeTarget {
        invoke_id: profile.id.clone(),
        label: profile.name.clone(),
        source: "inference_profile".into(),
        kind: BedrockInvokeTargetKind::InferenceProfile,
        source_model_id: Some(model.model_id.clone()),
        source_model_name: Some(model.model_name.clone()),
        input_support: model.modalities_for_alvum(),
        detail: format!("{scope} inference profile for {}.", model.model_name),
    }
}

fn target_sort_key(target: &BedrockInvokeTarget) -> (u8, u8, String) {
    (
        target_scope_rank(target),
        target
            .source_model_id
            .as_deref()
            .or(target.source_model_name.as_deref())
            .map(model_family_rank)
            .unwrap_or(9),
        target.label.clone(),
    )
}

fn target_scope_rank(target: &BedrockInvokeTarget) -> u8 {
    match target.kind {
        BedrockInvokeTargetKind::InferenceProfile if target.invoke_id.starts_with("global.") => 0,
        BedrockInvokeTargetKind::InferenceProfile => 1,
        BedrockInvokeTargetKind::BaseModel => 2,
    }
}

fn model_family_rank(model: &str) -> u8 {
    let model = model.to_ascii_lowercase();
    if model.contains("sonnet") {
        0
    } else if model.contains("opus") {
        1
    } else if model.contains("haiku") {
        2
    } else {
        3
    }
}

fn dedupe_targets(targets: Vec<BedrockInvokeTarget>) -> Vec<BedrockInvokeTarget> {
    let mut seen = std::collections::BTreeSet::new();
    targets
        .into_iter()
        .filter(|target| seen.insert(target.invoke_id.clone()))
        .collect()
}

fn modalities<'a>(values: impl IntoIterator<Item = &'a str>) -> BedrockModelInputSupport {
    let mut support = BedrockModelInputSupport::default();
    for value in values {
        match value.to_ascii_lowercase().as_str() {
            "text" | "completion" | "chat" => support.text = true,
            "image" | "vision" => support.image = true,
            "audio" | "speech" => support.audio = true,
            _ => {}
        }
    }
    support
}

fn looks_like_inference_profile(value: &str) -> bool {
    value.contains(":inference-profile/")
        || value.starts_with("global.")
        || value.starts_with("us.")
        || value.starts_with("eu.")
        || value.starts_with("apac.")
        || value.starts_with("jp.")
        || value.starts_with("au.")
        || value.starts_with("ca.")
}

fn model_id_from_model_arn(value: &str) -> Option<String> {
    value
        .rsplit_once("/foundation-model/")
        .map(|(_, model_id)| model_id.to_string())
        .or_else(|| {
            value
                .rsplit_once("foundation-model/")
                .map(|(_, model_id)| model_id.to_string())
        })
        .or_else(|| (!value.trim().is_empty()).then(|| value.to_string()))
}
