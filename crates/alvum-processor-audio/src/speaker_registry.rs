use crate::fingerprint::AudioFingerprint;
use alvum_core::synthesis_profile::{SynthesisInterest, SynthesisProfile};
use anyhow::{Context, Result, bail};
use chrono::{DateTime, Local};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

mod matching;
mod thresholds;
mod voice_model;

use self::matching::fingerprint_score;
use self::thresholds::{DUPLICATE_CANDIDATE_THRESHOLD, SPEAKER_MATCH_THRESHOLD};
use self::voice_model::{
    PersonVoiceModel, person_voice_model_summary, person_voice_models, voice_model_candidates,
};

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct SpeakerRegistry {
    schema_version: u32,
    #[serde(skip)]
    path: PathBuf,
    speakers: Vec<SpeakerProfile>,
    #[serde(default)]
    samples: Vec<VoiceSample>,
    #[serde(default)]
    future_sync: SpeakerRegistrySyncState,
}

#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
struct SpeakerRegistrySyncState {
    enabled: bool,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct SpeakerProfile {
    speaker_id: String,
    label: Option<String>,
    #[serde(default)]
    linked_interest_id: Option<String>,
    fingerprints: Vec<AudioFingerprint>,
    #[serde(default)]
    samples: Vec<SpeakerSample>,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct SpeakerProfileSummary {
    pub speaker_id: String,
    pub label: Option<String>,
    pub linked_interest_id: Option<String>,
    pub linked_interest: Option<TrackedInterestSummary>,
    pub fingerprint_count: usize,
    pub samples: Vec<SpeakerSample>,
    pub person_candidates: Vec<TrackedInterestCandidate>,
    pub duplicate_candidates: Vec<SpeakerDuplicateCandidate>,
    pub context_interests: Vec<TrackedInterestCandidate>,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct VoiceSampleSummary {
    pub sample_id: String,
    pub cluster_id: String,
    pub text: String,
    pub source: String,
    pub ts: String,
    pub start_secs: f32,
    pub end_secs: f32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub media_path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mime: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fingerprint_ref: Option<VoiceFingerprintRef>,
    pub quality_flags: Vec<String>,
    pub assignment_source: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub linked_interest_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub linked_interest: Option<TrackedInterestSummary>,
    pub person_candidates: Vec<TrackedInterestCandidate>,
    pub context_interests: Vec<TrackedInterestCandidate>,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct VoiceFingerprintRef {
    pub model: String,
    pub digest: String,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct TrackedInterestSummary {
    pub id: String,
    #[serde(rename = "type")]
    pub interest_type: String,
    pub name: String,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct TrackedInterestCandidate {
    pub id: String,
    #[serde(rename = "type")]
    pub interest_type: String,
    pub name: String,
    pub score: f32,
    pub reason: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub support_count: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub confidence_radius: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mean_similarity: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub voice_model_confidence: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub verified_sample_count: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_count: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub holdout_accuracy: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub holdout_margin: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prediction_margin: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub auto_predict: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_stats: Option<Vec<VoiceModelSourceStat>>,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct VoiceModelSourceStat {
    pub source: String,
    pub support_count: usize,
    pub confidence_radius: f32,
    pub mean_similarity: f32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub holdout_accuracy: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub holdout_margin: Option<f32>,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct PersonVoiceModelSummary {
    pub linked_interest: TrackedInterestSummary,
    pub model: String,
    pub confidence: String,
    pub verified_sample_count: usize,
    pub source_count: usize,
    pub confidence_radius: f32,
    pub mean_similarity: f32,
    pub holdout_accuracy: f32,
    pub holdout_margin: f32,
    pub auto_predict_ready: bool,
    pub source_stats: Vec<VoiceModelSourceStat>,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct SpeakerDuplicateCandidate {
    pub speaker_id: String,
    pub label: Option<String>,
    pub linked_interest_id: Option<String>,
    pub score: f32,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct SpeakerSample {
    pub text: String,
    pub source: String,
    pub ts: String,
    pub start_secs: f32,
    pub end_secs: f32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub media_path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mime: Option<String>,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
struct VoiceSample {
    sample_id: String,
    cluster_id: String,
    text: String,
    source: String,
    ts: String,
    start_secs: f32,
    end_secs: f32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    media_path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    mime: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    fingerprint: Option<AudioFingerprint>,
    #[serde(default)]
    quality_flags: Vec<String>,
    #[serde(default = "default_assignment_source")]
    assignment_source: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    linked_interest_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SpeakerMatch {
    pub speaker_id: String,
    pub label: Option<String>,
    pub score: f32,
}

impl SpeakerRegistry {
    pub fn load_or_default(path: &Path) -> Result<Self> {
        if !path.exists() {
            return Ok(Self::empty(path));
        }
        let content = std::fs::read_to_string(path)
            .with_context(|| format!("failed to read speaker registry {}", path.display()))?;
        let mut registry: Self = serde_json::from_str(&content)
            .with_context(|| format!("failed to parse speaker registry {}", path.display()))?;
        registry.path = path.to_path_buf();
        registry.normalize_schema();
        Ok(registry)
    }

    pub fn default_path() -> PathBuf {
        dirs::home_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join(".alvum")
            .join("runtime")
            .join("speakers.json")
    }

    pub fn resolve_existing(&self, fingerprint: &AudioFingerprint) -> Option<SpeakerMatch> {
        self.speakers
            .iter()
            .filter_map(|speaker| {
                let score = speaker
                    .fingerprints
                    .iter()
                    .map(|candidate| fingerprint_score(fingerprint, candidate))
                    .fold(0.0_f32, f32::max);
                (score >= SPEAKER_MATCH_THRESHOLD).then(|| SpeakerMatch {
                    speaker_id: speaker.speaker_id.clone(),
                    label: speaker.label.clone(),
                    score,
                })
            })
            .max_by(|left, right| left.score.total_cmp(&right.score))
    }

    pub fn resolve_or_create(&mut self, fingerprint: &AudioFingerprint) -> String {
        if let Some(existing) = self.resolve_existing(fingerprint) {
            return existing.speaker_id;
        }
        let speaker_id = format!(
            "spk_local_{}",
            &fingerprint.digest[..12.min(fingerprint.digest.len())]
        );
        self.speakers.push(SpeakerProfile {
            speaker_id: speaker_id.clone(),
            label: None,
            linked_interest_id: None,
            fingerprints: vec![fingerprint.clone()],
            samples: Vec::new(),
        });
        speaker_id
    }

    pub fn record_sample_with_fingerprint(
        &mut self,
        speaker_id: &str,
        fingerprint: Option<AudioFingerprint>,
        sample: SpeakerSample,
        assignment_source: &str,
    ) -> Result<()> {
        let linked_interest_id = {
            let Some(speaker) = self
                .speakers
                .iter()
                .find(|speaker| speaker.speaker_id == speaker_id)
            else {
                bail!("unknown speaker id: {speaker_id}");
            };
            speaker.linked_interest_id.clone()
        };
        let voice_sample = VoiceSample::from_speaker_sample(
            speaker_id,
            sample.clone(),
            fingerprint.clone(),
            assignment_source,
            linked_interest_id,
        );
        if !self
            .samples
            .iter()
            .any(|existing| existing.sample_id == voice_sample.sample_id)
        {
            self.samples.push(voice_sample);
        }
        if let Some(fingerprint) = fingerprint {
            if let Some(speaker) = self
                .speakers
                .iter_mut()
                .find(|speaker| speaker.speaker_id == speaker_id)
            {
                if !speaker
                    .fingerprints
                    .iter()
                    .any(|existing| existing.digest == fingerprint.digest)
                {
                    speaker.fingerprints.push(fingerprint);
                }
            }
        }
        self.rebuild_speaker_samples_from_ledger();
        Ok(())
    }

    pub fn label_for(&self, speaker_id: &str) -> Option<String> {
        self.speakers
            .iter()
            .find(|speaker| speaker.speaker_id == speaker_id)
            .and_then(|speaker| speaker.label.clone())
    }

    pub fn label_for_sample(
        &self,
        speaker_id: &str,
        sample: &SpeakerSample,
        profile: Option<&SynthesisProfile>,
    ) -> Option<String> {
        let sample_id = stable_sample_id(
            speaker_id,
            &sample.source,
            &sample.ts,
            sample.start_secs,
            sample.end_secs,
            &sample.text,
        );
        self.samples
            .iter()
            .find(|sample| sample.sample_id == sample_id)
            .and_then(|sample| linked_interest_name(profile, sample.linked_interest_id.as_deref()))
            .or_else(|| self.label_for(speaker_id))
    }

    pub fn label_for_sample_with_fingerprint(
        &self,
        speaker_id: &str,
        sample: &SpeakerSample,
        fingerprint: &AudioFingerprint,
        profile: Option<&SynthesisProfile>,
    ) -> Option<String> {
        self.label_for_sample(speaker_id, sample, profile)
            .or_else(|| {
                self.predict_label_for_fingerprint_with_source(
                    fingerprint,
                    profile,
                    Some(&sample.source),
                )
            })
    }

    pub fn predict_label_for_fingerprint(
        &self,
        fingerprint: &AudioFingerprint,
        profile: Option<&SynthesisProfile>,
    ) -> Option<String> {
        self.predict_label_for_fingerprint_with_source(fingerprint, profile, None)
    }

    pub fn predict_label_for_fingerprint_with_source(
        &self,
        fingerprint: &AudioFingerprint,
        profile: Option<&SynthesisProfile>,
        source: Option<&str>,
    ) -> Option<String> {
        let person_models = person_voice_models(profile, &self.speakers, &self.samples);
        voice_model_candidates(std::slice::from_ref(fingerprint), &person_models, source)
            .into_iter()
            .find(|candidate| candidate.auto_predict == Some(true))
            .map(|candidate| candidate.name)
    }

    pub fn speakers(&self) -> Vec<SpeakerProfileSummary> {
        self.speakers_with_profile(None)
    }

    pub fn speakers_with_profile(
        &self,
        profile: Option<&SynthesisProfile>,
    ) -> Vec<SpeakerProfileSummary> {
        let person_models = person_voice_models(profile, &self.speakers, &self.samples);
        self.speakers
            .iter()
            .map(|speaker| SpeakerProfileSummary {
                speaker_id: speaker.speaker_id.clone(),
                label: speaker.label.clone(),
                linked_interest_id: speaker.linked_interest_id.clone(),
                linked_interest: linked_interest_summary(profile, speaker),
                fingerprint_count: speaker.fingerprints.len(),
                samples: speaker.samples.clone(),
                person_candidates: person_candidates(profile, speaker, &person_models),
                duplicate_candidates: duplicate_candidates(&self.speakers, speaker),
                context_interests: context_interests(profile, speaker),
            })
            .collect()
    }

    pub fn voice_samples_with_profile(
        &self,
        profile: Option<&SynthesisProfile>,
    ) -> Vec<VoiceSampleSummary> {
        let person_models = person_voice_models(profile, &self.speakers, &self.samples);
        let mut samples = self
            .samples
            .iter()
            .map(|sample| sample_summary(profile, sample, &person_models))
            .collect::<Vec<_>>();
        samples.sort_by(|left, right| {
            left.ts
                .cmp(&right.ts)
                .then(left.sample_id.cmp(&right.sample_id))
        });
        samples
    }

    pub fn person_voice_model_summaries(
        &self,
        profile: Option<&SynthesisProfile>,
    ) -> Vec<PersonVoiceModelSummary> {
        person_voice_models(profile, &self.speakers, &self.samples)
            .into_iter()
            .map(person_voice_model_summary)
            .collect()
    }

    pub fn confirm_label(&mut self, speaker_id: &str, label: &str) -> Result<()> {
        self.rename(speaker_id, label)
    }

    pub fn rename(&mut self, speaker_id: &str, label: &str) -> Result<()> {
        let label = label.trim();
        if label.is_empty() {
            bail!("speaker label cannot be empty");
        }
        let Some(speaker) = self
            .speakers
            .iter_mut()
            .find(|speaker| speaker.speaker_id == speaker_id)
        else {
            bail!("unknown speaker id: {speaker_id}");
        };
        speaker.label = Some(label.into());
        Ok(())
    }

    pub fn link_to_interest(
        &mut self,
        speaker_id: &str,
        interest_id: &str,
        profile: &SynthesisProfile,
    ) -> Result<()> {
        let Some(interest) = profile
            .interests
            .iter()
            .find(|interest| interest.id == interest_id)
        else {
            bail!("unknown tracked item id: {interest_id}");
        };
        if !is_person_interest(interest) {
            bail!("voice identities can only link to tracked items with type \"person\"");
        }
        let Some(speaker) = self
            .speakers
            .iter_mut()
            .find(|speaker| speaker.speaker_id == speaker_id)
        else {
            bail!("unknown speaker id: {speaker_id}");
        };
        speaker.linked_interest_id = Some(interest.id.clone());
        speaker.label = Some(interest.name.clone());
        for sample in &mut self.samples {
            if sample.cluster_id == speaker_id {
                if !is_user_sample_identity_override(&sample.assignment_source) {
                    sample.linked_interest_id = Some(interest.id.clone());
                    sample.assignment_source = "user_linked_cluster".into();
                }
            }
        }
        Ok(())
    }

    pub fn unlink_interest(&mut self, speaker_id: &str) -> Result<()> {
        let Some(speaker) = self
            .speakers
            .iter_mut()
            .find(|speaker| speaker.speaker_id == speaker_id)
        else {
            bail!("unknown speaker id: {speaker_id}");
        };
        speaker.linked_interest_id = None;
        speaker.label = None;
        for sample in &mut self.samples {
            if sample.cluster_id == speaker_id {
                sample.linked_interest_id = None;
            }
        }
        Ok(())
    }

    pub fn unlink_interest_id(&mut self, interest_id: &str) -> Result<()> {
        if interest_id.trim().is_empty() {
            bail!("tracked person id is required");
        }
        let mut changed = false;
        for speaker in &mut self.speakers {
            if speaker.linked_interest_id.as_deref() == Some(interest_id) {
                speaker.linked_interest_id = None;
                speaker.label = None;
                changed = true;
            }
        }
        for sample in &mut self.samples {
            if sample.linked_interest_id.as_deref() == Some(interest_id) {
                sample.linked_interest_id = None;
                if is_user_sample_identity_override(&sample.assignment_source) {
                    sample.assignment_source = "user_unassigned_sample".into();
                }
                changed = true;
            }
        }
        if changed {
            self.rebuild_speaker_samples_from_ledger();
        }
        Ok(())
    }

    pub fn link_sample_to_interest(
        &mut self,
        sample_id: &str,
        interest_id: &str,
        profile: &SynthesisProfile,
    ) -> Result<()> {
        let Some(interest) = profile
            .interests
            .iter()
            .find(|interest| interest.id == interest_id)
        else {
            bail!("unknown tracked item id: {interest_id}");
        };
        if !is_person_interest(interest) {
            bail!("voice identities can only link to tracked items with type \"person\"");
        }
        let Some(sample) = self
            .samples
            .iter_mut()
            .find(|sample| sample.sample_id == sample_id)
        else {
            bail!("unknown voice sample id: {sample_id}");
        };
        sample.linked_interest_id = Some(interest.id.clone());
        sample.assignment_source = "user_confirmed_sample".into();
        Ok(())
    }

    pub fn unlink_sample_interest(&mut self, sample_id: &str) -> Result<()> {
        let Some(sample) = self
            .samples
            .iter_mut()
            .find(|sample| sample.sample_id == sample_id)
        else {
            bail!("unknown voice sample id: {sample_id}");
        };
        sample.linked_interest_id = None;
        sample.assignment_source = "user_unassigned_sample".into();
        self.rebuild_speaker_samples_from_ledger();
        Ok(())
    }

    pub fn migrate_legacy_labels(&mut self, profile: &mut SynthesisProfile) -> Result<bool> {
        let mut changed = false;
        let mut cluster_links = Vec::new();
        for speaker in &mut self.speakers {
            if let Some(interest_id) = speaker.linked_interest_id.clone() {
                cluster_links.push((speaker.speaker_id.clone(), interest_id));
                continue;
            }
            let Some(label) = speaker
                .label
                .as_deref()
                .map(str::trim)
                .filter(|value| !value.is_empty())
            else {
                continue;
            };
            let interest_id = ensure_person_interest(profile, label);
            speaker.linked_interest_id = Some(interest_id.clone());
            cluster_links.push((speaker.speaker_id.clone(), interest_id));
            changed = true;
        }
        for (cluster_id, interest_id) in cluster_links {
            for sample in &mut self.samples {
                if sample.cluster_id == cluster_id
                    && !is_user_sample_identity_override(&sample.assignment_source)
                    && sample.linked_interest_id.as_deref() != Some(interest_id.as_str())
                {
                    sample.linked_interest_id = Some(interest_id.clone());
                    changed = true;
                }
            }
        }
        Ok(changed)
    }

    pub fn forget(&mut self, speaker_id: &str) -> Result<()> {
        let before = self.speakers.len();
        self.speakers
            .retain(|speaker| speaker.speaker_id != speaker_id);
        if self.speakers.len() == before {
            bail!("unknown speaker id: {speaker_id}");
        }
        self.samples
            .retain(|sample| sample.cluster_id != speaker_id);
        Ok(())
    }

    pub fn merge(&mut self, source_speaker_id: &str, target_speaker_id: &str) -> Result<()> {
        if source_speaker_id == target_speaker_id {
            return Ok(());
        }
        let Some(source_index) = self
            .speakers
            .iter()
            .position(|speaker| speaker.speaker_id == source_speaker_id)
        else {
            bail!("unknown source speaker id: {source_speaker_id}");
        };
        let Some(target_index) = self
            .speakers
            .iter()
            .position(|speaker| speaker.speaker_id == target_speaker_id)
        else {
            bail!("unknown target speaker id: {target_speaker_id}");
        };
        let source = self.speakers.remove(source_index);
        let adjusted_target_index = if source_index < target_index {
            target_index - 1
        } else {
            target_index
        };
        let target = &mut self.speakers[adjusted_target_index];
        if target.label.is_none() {
            target.label = source.label;
        }
        if target.linked_interest_id.is_none() {
            target.linked_interest_id = source.linked_interest_id;
        }
        let target_linked_interest_id = target.linked_interest_id.clone();
        target.fingerprints.extend(source.fingerprints);
        for sample in &mut self.samples {
            if sample.cluster_id == source_speaker_id {
                sample.cluster_id = target_speaker_id.to_string();
                if !is_user_sample_identity_override(&sample.assignment_source) {
                    sample.assignment_source = "user_merged_cluster".into();
                }
            }
            if sample.cluster_id == target_speaker_id
                && !is_user_sample_identity_override(&sample.assignment_source)
            {
                sample.linked_interest_id = target_linked_interest_id.clone();
            }
        }
        self.rebuild_speaker_samples_from_ledger();
        Ok(())
    }

    pub fn record_sample(&mut self, speaker_id: &str, sample: SpeakerSample) -> Result<()> {
        self.record_sample_with_fingerprint(speaker_id, None, sample, "legacy")
    }

    pub fn move_sample_to_cluster(&mut self, sample_id: &str, cluster_id: &str) -> Result<String> {
        let sample_index = self
            .samples
            .iter()
            .position(|sample| sample.sample_id == sample_id)
            .ok_or_else(|| anyhow::anyhow!("unknown voice sample id: {sample_id}"))?;
        let target_cluster_id = if cluster_id == "new" {
            self.create_cluster_for_sample(sample_index)
        } else {
            if !self
                .speakers
                .iter()
                .any(|speaker| speaker.speaker_id == cluster_id)
            {
                bail!("unknown target cluster id: {cluster_id}");
            }
            cluster_id.to_string()
        };
        let target_linked_interest_id = self
            .speakers
            .iter()
            .find(|speaker| speaker.speaker_id == target_cluster_id)
            .and_then(|speaker| speaker.linked_interest_id.clone());
        let sample_identity_override =
            is_user_sample_identity_override(&self.samples[sample_index].assignment_source);
        let existing_assignment_source = self.samples[sample_index].assignment_source.clone();
        let existing_linked_interest_id = self.samples[sample_index].linked_interest_id.clone();
        self.samples[sample_index].cluster_id = target_cluster_id.clone();
        self.samples[sample_index].assignment_source = if sample_identity_override {
            existing_assignment_source
        } else {
            "user_moved_sample".into()
        };
        self.samples[sample_index].linked_interest_id = if sample_identity_override {
            existing_linked_interest_id
        } else {
            target_linked_interest_id
        };
        self.rebuild_speaker_samples_from_ledger();
        Ok(target_cluster_id)
    }

    pub fn ignore_sample(&mut self, sample_id: &str) -> Result<()> {
        let Some(sample) = self
            .samples
            .iter_mut()
            .find(|sample| sample.sample_id == sample_id)
        else {
            bail!("unknown voice sample id: {sample_id}");
        };
        if !sample
            .quality_flags
            .iter()
            .any(|flag| flag == "ignored_by_user")
        {
            sample.quality_flags.push("ignored_by_user".into());
        }
        sample.assignment_source = "user_ignored_sample".into();
        sample.linked_interest_id = None;
        self.rebuild_speaker_samples_from_ledger();
        Ok(())
    }

    pub fn split_sample_at(
        &mut self,
        sample_id: &str,
        at_secs: f32,
        left_text: &str,
        right_text: &str,
    ) -> Result<Vec<String>> {
        let sample_index = self
            .samples
            .iter()
            .position(|sample| sample.sample_id == sample_id)
            .ok_or_else(|| anyhow::anyhow!("unknown voice sample id: {sample_id}"))?;
        let original = self.samples[sample_index].clone();
        if at_secs <= original.start_secs || at_secs >= original.end_secs {
            bail!("split point must be inside the sample");
        }
        let left_text = left_text.trim();
        let right_text = right_text.trim();
        if left_text.is_empty() || right_text.is_empty() {
            bail!("both split transcript fields are required");
        }

        let left = original.child_turn(original.start_secs, at_secs, left_text);
        let right = original.child_turn(at_secs, original.end_secs, right_text);
        let child_ids = vec![left.sample_id.clone(), right.sample_id.clone()];
        self.samples.remove(sample_index);
        self.samples.insert(sample_index, right);
        self.samples.insert(sample_index, left);
        self.rebuild_speaker_samples_from_ledger();
        Ok(child_ids)
    }

    pub fn split_samples_to_new_cluster(
        &mut self,
        source_cluster_id: &str,
        sample_ids: &[String],
    ) -> Result<String> {
        if !self
            .speakers
            .iter()
            .any(|speaker| speaker.speaker_id == source_cluster_id)
        {
            bail!("unknown source cluster id: {source_cluster_id}");
        }
        let first_sample_id = sample_ids
            .first()
            .ok_or_else(|| anyhow::anyhow!("at least one sample id is required"))?;
        let sample_index = self
            .samples
            .iter()
            .position(|sample| sample.sample_id == *first_sample_id)
            .ok_or_else(|| anyhow::anyhow!("unknown voice sample id: {first_sample_id}"))?;
        let target_cluster_id = self.create_cluster_for_sample(sample_index);
        for sample_id in sample_ids {
            let Some(sample) = self
                .samples
                .iter_mut()
                .find(|sample| sample.sample_id == *sample_id)
            else {
                bail!("unknown voice sample id: {sample_id}");
            };
            if sample.cluster_id != source_cluster_id {
                bail!("voice sample {sample_id} is not in cluster {source_cluster_id}");
            }
            sample.cluster_id = target_cluster_id.clone();
            if !is_user_sample_identity_override(&sample.assignment_source) {
                sample.assignment_source = "user_split_sample".into();
                sample.linked_interest_id = None;
            }
        }
        self.rebuild_speaker_samples_from_ledger();
        Ok(target_cluster_id)
    }

    pub fn day_for_sample(&self, sample_id: &str) -> Result<String> {
        let sample = self
            .samples
            .iter()
            .find(|sample| sample.sample_id == sample_id)
            .ok_or_else(|| anyhow::anyhow!("unknown voice sample id: {sample_id}"))?;
        sample_local_day(&sample.ts)
    }

    pub fn days_for_cluster(&self, speaker_id: &str) -> Result<Vec<String>> {
        if !self
            .speakers
            .iter()
            .any(|speaker| speaker.speaker_id == speaker_id)
        {
            bail!("unknown speaker id: {speaker_id}");
        }
        Ok(unique_days(
            self.samples
                .iter()
                .filter(|sample| sample.cluster_id == speaker_id)
                .filter_map(|sample| sample_local_day(&sample.ts).ok()),
        ))
    }

    pub fn days_for_interest_id(&self, interest_id: &str) -> Vec<String> {
        let linked_clusters = self
            .speakers
            .iter()
            .filter(|speaker| speaker.linked_interest_id.as_deref() == Some(interest_id))
            .map(|speaker| speaker.speaker_id.as_str())
            .collect::<HashSet<_>>();
        unique_days(
            self.samples
                .iter()
                .filter(|sample| {
                    sample.linked_interest_id.as_deref() == Some(interest_id)
                        || linked_clusters.contains(sample.cluster_id.as_str())
                })
                .filter_map(|sample| sample_local_day(&sample.ts).ok()),
        )
    }

    pub fn all_sample_days(&self) -> Vec<String> {
        unique_days(
            self.samples
                .iter()
                .filter_map(|sample| sample_local_day(&sample.ts).ok()),
        )
    }

    pub fn reset(&mut self) {
        self.speakers.clear();
        self.samples.clear();
    }

    pub fn save(&self) -> Result<()> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let content = serde_json::to_string_pretty(self)?;
        std::fs::write(&self.path, content)
            .with_context(|| format!("failed to write speaker registry {}", self.path.display()))
    }

    fn empty(path: &Path) -> Self {
        Self {
            schema_version: 1,
            path: path.to_path_buf(),
            speakers: Vec::new(),
            samples: Vec::new(),
            future_sync: SpeakerRegistrySyncState::default(),
        }
    }

    fn normalize_schema(&mut self) {
        if self.schema_version < 2 || self.samples.is_empty() {
            let mut migrated = Vec::new();
            for speaker in &self.speakers {
                for sample in &speaker.samples {
                    migrated.push(VoiceSample::from_speaker_sample(
                        &speaker.speaker_id,
                        sample.clone(),
                        speaker.fingerprints.first().cloned(),
                        "legacy_cluster",
                        speaker.linked_interest_id.clone(),
                    ));
                }
            }
            for sample in migrated {
                if !self
                    .samples
                    .iter()
                    .any(|existing| existing.sample_id == sample.sample_id)
                {
                    self.samples.push(sample);
                }
            }
        }
        self.schema_version = 2;
        self.rebuild_speaker_samples_from_ledger();
    }

    fn create_cluster_for_sample(&mut self, sample_index: usize) -> String {
        let sample = &self.samples[sample_index];
        let seed = sample
            .fingerprint
            .as_ref()
            .map(|fingerprint| fingerprint.digest.as_str())
            .unwrap_or(&sample.sample_id);
        let mut cluster_id = format!("spk_local_{}", &seed[..12.min(seed.len())]);
        let mut suffix = 2usize;
        while self
            .speakers
            .iter()
            .any(|speaker| speaker.speaker_id == cluster_id)
        {
            cluster_id = format!("spk_local_{}_{suffix}", &seed[..10.min(seed.len())]);
            suffix += 1;
        }
        self.speakers.push(SpeakerProfile {
            speaker_id: cluster_id.clone(),
            label: None,
            linked_interest_id: None,
            fingerprints: sample.fingerprint.clone().into_iter().collect(),
            samples: Vec::new(),
        });
        cluster_id
    }

    fn rebuild_speaker_samples_from_ledger(&mut self) {
        for speaker in &mut self.speakers {
            speaker.samples.clear();
            let mut samples = self
                .samples
                .iter()
                .filter(|sample| sample.cluster_id == speaker.speaker_id)
                .map(VoiceSample::as_speaker_sample)
                .collect::<Vec<_>>();
            samples.sort_by(|left, right| left.ts.cmp(&right.ts));
            for sample in samples
                .into_iter()
                .rev()
                .take(5)
                .collect::<Vec<_>>()
                .into_iter()
                .rev()
            {
                push_sample(speaker, sample);
            }
        }
        self.rebuild_speaker_fingerprints_from_ledger();
    }

    fn rebuild_speaker_fingerprints_from_ledger(&mut self) {
        let sample_fingerprint_digests = self
            .samples
            .iter()
            .filter_map(|sample| sample.fingerprint.as_ref())
            .map(|fingerprint| fingerprint.digest.clone())
            .collect::<HashSet<_>>();
        let mut fingerprints_by_cluster: HashMap<String, Vec<AudioFingerprint>> = HashMap::new();
        for sample in &self.samples {
            if sample
                .quality_flags
                .iter()
                .any(|flag| flag == "ignored_by_user")
            {
                continue;
            }
            let Some(fingerprint) = sample.fingerprint.clone() else {
                continue;
            };
            push_unique_fingerprint(
                fingerprints_by_cluster
                    .entry(sample.cluster_id.clone())
                    .or_default(),
                fingerprint,
            );
        }
        for speaker in &mut self.speakers {
            let mut rebuilt = speaker
                .fingerprints
                .iter()
                .filter(|fingerprint| !sample_fingerprint_digests.contains(&fingerprint.digest))
                .cloned()
                .collect::<Vec<_>>();
            if let Some(sample_fingerprints) = fingerprints_by_cluster.remove(&speaker.speaker_id) {
                for fingerprint in sample_fingerprints {
                    push_unique_fingerprint(&mut rebuilt, fingerprint);
                }
            }
            speaker.fingerprints = rebuilt;
        }
    }
}

fn push_sample(speaker: &mut SpeakerProfile, sample: SpeakerSample) {
    if sample.text.trim().is_empty() {
        return;
    }
    if speaker
        .samples
        .iter()
        .any(|existing| existing.ts == sample.ts && existing.text == sample.text)
    {
        return;
    }
    speaker.samples.push(sample);
    if speaker.samples.len() > 5 {
        speaker.samples.remove(0);
    }
}

fn push_unique_fingerprint(
    fingerprints: &mut Vec<AudioFingerprint>,
    fingerprint: AudioFingerprint,
) {
    if !fingerprints
        .iter()
        .any(|existing| existing.digest == fingerprint.digest)
    {
        fingerprints.push(fingerprint);
    }
}

impl VoiceSample {
    fn from_speaker_sample(
        cluster_id: &str,
        sample: SpeakerSample,
        fingerprint: Option<AudioFingerprint>,
        assignment_source: &str,
        linked_interest_id: Option<String>,
    ) -> Self {
        let sample_id = stable_sample_id(
            cluster_id,
            &sample.source,
            &sample.ts,
            sample.start_secs,
            sample.end_secs,
            &sample.text,
        );
        Self {
            sample_id,
            cluster_id: cluster_id.to_string(),
            text: sample.text,
            source: sample.source,
            ts: sample.ts,
            start_secs: sample.start_secs,
            end_secs: sample.end_secs,
            media_path: sample.media_path,
            mime: sample.mime,
            fingerprint,
            quality_flags: Vec::new(),
            assignment_source: assignment_source.to_string(),
            linked_interest_id,
        }
    }

    fn as_speaker_sample(&self) -> SpeakerSample {
        SpeakerSample {
            text: self.text.clone(),
            source: self.source.clone(),
            ts: self.ts.clone(),
            start_secs: self.start_secs,
            end_secs: self.end_secs,
            media_path: self.media_path.clone(),
            mime: self.mime.clone(),
        }
    }

    fn child_turn(&self, start_secs: f32, end_secs: f32, text: &str) -> Self {
        let sample = SpeakerSample {
            text: text.to_string(),
            source: self.source.clone(),
            ts: self.ts.clone(),
            start_secs,
            end_secs,
            media_path: self.media_path.clone(),
            mime: self.mime.clone(),
        };
        let mut child = VoiceSample::from_speaker_sample(
            &self.cluster_id,
            sample,
            self.fingerprint.clone(),
            "user_split_turn",
            self.linked_interest_id.clone(),
        );
        child.quality_flags = Vec::new();
        child
    }
}

fn sample_summary(
    profile: Option<&SynthesisProfile>,
    sample: &VoiceSample,
    person_models: &[PersonVoiceModel],
) -> VoiceSampleSummary {
    VoiceSampleSummary {
        sample_id: sample.sample_id.clone(),
        cluster_id: sample.cluster_id.clone(),
        text: sample.text.clone(),
        source: sample.source.clone(),
        ts: sample.ts.clone(),
        start_secs: sample.start_secs,
        end_secs: sample.end_secs,
        media_path: sample.media_path.clone(),
        mime: sample.mime.clone(),
        fingerprint_ref: sample
            .fingerprint
            .as_ref()
            .map(|fingerprint| VoiceFingerprintRef {
                model: fingerprint.model.clone(),
                digest: fingerprint.digest.clone(),
            }),
        quality_flags: sample.quality_flags.clone(),
        assignment_source: sample.assignment_source.clone(),
        linked_interest_id: sample.linked_interest_id.clone(),
        linked_interest: sample_linked_interest_summary(profile, sample),
        person_candidates: sample_person_candidates(profile, sample, person_models),
        context_interests: sample_context_interests(profile, sample),
    }
}

fn sample_linked_interest_summary(
    profile: Option<&SynthesisProfile>,
    sample: &VoiceSample,
) -> Option<TrackedInterestSummary> {
    let id = sample.linked_interest_id.as_deref()?;
    let interest = profile?
        .interests
        .iter()
        .find(|interest| interest.id == id)?;
    Some(interest_summary(interest))
}

fn sample_person_candidates(
    profile: Option<&SynthesisProfile>,
    sample: &VoiceSample,
    person_models: &[PersonVoiceModel],
) -> Vec<TrackedInterestCandidate> {
    let mut candidates = sample_candidates(profile, sample, true);
    candidates.extend(sample_fingerprint_person_candidates(sample, person_models));
    dedupe_candidates(candidates)
}

fn sample_context_interests(
    profile: Option<&SynthesisProfile>,
    sample: &VoiceSample,
) -> Vec<TrackedInterestCandidate> {
    sample_candidates(profile, sample, false)
}

fn sample_candidates(
    profile: Option<&SynthesisProfile>,
    sample: &VoiceSample,
    people: bool,
) -> Vec<TrackedInterestCandidate> {
    let Some(profile) = profile else {
        return Vec::new();
    };
    let mut candidates = profile
        .interests
        .iter()
        .filter(|interest| is_person_interest(interest) == people)
        .filter_map(|interest| {
            if people
                && sample
                    .linked_interest_id
                    .as_deref()
                    .is_some_and(|id| id == interest.id)
            {
                return Some(candidate(interest, 1.0, "confirmed sample identity"));
            }
            let terms = std::iter::once(interest.name.as_str())
                .chain(interest.aliases.iter().map(String::as_str))
                .filter(|term| !term.trim().is_empty());
            if contains_any(&sample.text, terms) {
                return Some(candidate(interest, 0.65, "context mentioned nearby"));
            }
            None
        })
        .collect::<Vec<_>>();
    candidates.sort_by(|left, right| right.score.total_cmp(&left.score));
    candidates.truncate(5);
    candidates
}

fn sample_fingerprint_person_candidates(
    sample: &VoiceSample,
    person_models: &[PersonVoiceModel],
) -> Vec<TrackedInterestCandidate> {
    let Some(fingerprint) = sample.fingerprint.as_ref() else {
        return Vec::new();
    };
    if sample.linked_interest_id.is_some() {
        return Vec::new();
    }
    voice_model_candidates(
        std::slice::from_ref(fingerprint),
        person_models,
        Some(&sample.source),
    )
}

fn dedupe_candidates(candidates: Vec<TrackedInterestCandidate>) -> Vec<TrackedInterestCandidate> {
    let mut deduped: Vec<TrackedInterestCandidate> = Vec::new();
    for candidate in candidates {
        if let Some(existing) = deduped.iter_mut().find(|item| item.id == candidate.id) {
            if candidate.score > existing.score {
                *existing = candidate;
            }
        } else {
            deduped.push(candidate);
        }
    }
    deduped.sort_by(|left, right| right.score.total_cmp(&left.score));
    deduped.truncate(5);
    deduped
}

fn default_assignment_source() -> String {
    "legacy".into()
}

fn is_user_sample_identity_override(source: &str) -> bool {
    matches!(source, "user_confirmed_sample" | "user_unassigned_sample")
}

fn stable_sample_id(
    cluster_id: &str,
    source: &str,
    ts: &str,
    start_secs: f32,
    end_secs: f32,
    text: &str,
) -> String {
    let mut hash = 0xcbf29ce484222325u64;
    for byte in
        format!("{cluster_id}\n{source}\n{ts}\n{start_secs:.3}\n{end_secs:.3}\n{text}").bytes()
    {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    format!("vsm_{hash:016x}")
}

fn sample_local_day(ts: &str) -> Result<String> {
    if let Ok(parsed) = DateTime::parse_from_rfc3339(ts) {
        return Ok(parsed.with_timezone(&Local).format("%Y-%m-%d").to_string());
    }
    let Some(day) = ts.get(0..10).filter(|value| {
        value.len() == 10 && value.as_bytes()[4] == b'-' && value.as_bytes()[7] == b'-'
    }) else {
        bail!("voice sample timestamp does not contain a local day: {ts}");
    };
    Ok(day.to_string())
}

fn unique_days(days: impl Iterator<Item = String>) -> Vec<String> {
    let mut days = days.collect::<Vec<_>>();
    days.sort();
    days.dedup();
    days
}

fn linked_interest_summary(
    profile: Option<&SynthesisProfile>,
    speaker: &SpeakerProfile,
) -> Option<TrackedInterestSummary> {
    let id = speaker.linked_interest_id.as_deref()?;
    let interest = profile?
        .interests
        .iter()
        .find(|interest| interest.id == id)?;
    Some(interest_summary(interest))
}

fn linked_interest_name(profile: Option<&SynthesisProfile>, id: Option<&str>) -> Option<String> {
    let id = id?;
    profile?
        .interests
        .iter()
        .find(|interest| interest.id == id)
        .map(|interest| interest.name.clone())
}

fn interest_summary(interest: &SynthesisInterest) -> TrackedInterestSummary {
    TrackedInterestSummary {
        id: interest.id.clone(),
        interest_type: interest.interest_type.clone(),
        name: interest.name.clone(),
    }
}

fn person_candidates(
    profile: Option<&SynthesisProfile>,
    speaker: &SpeakerProfile,
    person_models: &[PersonVoiceModel],
) -> Vec<TrackedInterestCandidate> {
    let Some(profile) = profile else {
        return Vec::new();
    };
    let mut candidates = profile
        .interests
        .iter()
        .filter(|interest| is_person_interest(interest))
        .filter_map(|interest| candidate_for_interest(interest, speaker, true))
        .collect::<Vec<_>>();
    candidates.extend(speaker_fingerprint_person_candidates(
        speaker,
        person_models,
    ));
    dedupe_candidates(candidates)
}

fn context_interests(
    profile: Option<&SynthesisProfile>,
    speaker: &SpeakerProfile,
) -> Vec<TrackedInterestCandidate> {
    let Some(profile) = profile else {
        return Vec::new();
    };
    let mut candidates = profile
        .interests
        .iter()
        .filter(|interest| !is_person_interest(interest))
        .filter_map(|interest| candidate_for_interest(interest, speaker, false))
        .collect::<Vec<_>>();
    candidates.sort_by(|left, right| right.score.total_cmp(&left.score));
    candidates.truncate(5);
    candidates
}

fn candidate_for_interest(
    interest: &SynthesisInterest,
    speaker: &SpeakerProfile,
    include_linked: bool,
) -> Option<TrackedInterestCandidate> {
    if include_linked
        && speaker
            .linked_interest_id
            .as_deref()
            .is_some_and(|id| id == interest.id)
    {
        return Some(candidate(interest, 1.0, "linked voice identity"));
    }
    if speaker
        .label
        .as_deref()
        .is_some_and(|label| same_text(label, &interest.name))
    {
        return Some(candidate(interest, 0.9, "legacy speaker label"));
    }
    let sample_text = speaker
        .samples
        .iter()
        .map(|sample| sample.text.as_str())
        .collect::<Vec<_>>()
        .join(" ");
    let terms = std::iter::once(interest.name.as_str())
        .chain(interest.aliases.iter().map(String::as_str))
        .filter(|term| !term.trim().is_empty());
    if contains_any(&sample_text, terms) {
        return Some(candidate(interest, 0.65, "context mentioned nearby"));
    }
    None
}

fn speaker_fingerprint_person_candidates(
    speaker: &SpeakerProfile,
    person_models: &[PersonVoiceModel],
) -> Vec<TrackedInterestCandidate> {
    if speaker.linked_interest_id.is_some() || speaker.fingerprints.is_empty() {
        return Vec::new();
    }
    voice_model_candidates(&speaker.fingerprints, person_models, None)
}

fn candidate(interest: &SynthesisInterest, score: f32, reason: &str) -> TrackedInterestCandidate {
    TrackedInterestCandidate {
        id: interest.id.clone(),
        interest_type: interest.interest_type.clone(),
        name: interest.name.clone(),
        score,
        reason: reason.into(),
        support_count: None,
        confidence_radius: None,
        mean_similarity: None,
        voice_model_confidence: None,
        verified_sample_count: None,
        source_count: None,
        holdout_accuracy: None,
        holdout_margin: None,
        prediction_margin: None,
        auto_predict: None,
        source_stats: None,
    }
}

fn duplicate_candidates(
    speakers: &[SpeakerProfile],
    speaker: &SpeakerProfile,
) -> Vec<SpeakerDuplicateCandidate> {
    let mut candidates = speakers
        .iter()
        .filter(|candidate| candidate.speaker_id != speaker.speaker_id)
        .filter_map(|candidate| {
            let score = speaker
                .fingerprints
                .iter()
                .flat_map(|left| {
                    candidate
                        .fingerprints
                        .iter()
                        .map(move |right| fingerprint_score(left, right))
                })
                .fold(0.0_f32, f32::max);
            (score >= DUPLICATE_CANDIDATE_THRESHOLD).then(|| SpeakerDuplicateCandidate {
                speaker_id: candidate.speaker_id.clone(),
                label: candidate.label.clone(),
                linked_interest_id: candidate.linked_interest_id.clone(),
                score,
            })
        })
        .collect::<Vec<_>>();
    candidates.sort_by(|left, right| right.score.total_cmp(&left.score));
    candidates.truncate(5);
    candidates
}

fn ensure_person_interest(profile: &mut SynthesisProfile, label: &str) -> String {
    if let Some(existing) = profile.interests.iter().find(|interest| {
        is_person_interest(interest)
            && (same_text(&interest.name, label)
                || interest.aliases.iter().any(|alias| same_text(alias, label)))
    }) {
        return existing.id.clone();
    }
    let id = unique_interest_id(profile, &format!("person_{}", slugify(label)));
    profile.interests.push(SynthesisInterest {
        id: id.clone(),
        interest_type: "person".into(),
        name: label.into(),
        aliases: Vec::new(),
        notes: "Created from a confirmed voice identity.".into(),
        priority: next_priority(profile.interests.iter().map(|interest| interest.priority)),
        enabled: true,
        linked_knowledge_ids: Vec::new(),
    });
    id
}

fn unique_interest_id(profile: &SynthesisProfile, seed: &str) -> String {
    let existing = profile
        .interests
        .iter()
        .map(|interest| interest.id.as_str())
        .collect::<std::collections::BTreeSet<_>>();
    let mut candidate: String = if seed.trim().is_empty() {
        "person_voice".into()
    } else {
        seed.into()
    };
    let mut suffix = 2;
    while existing.contains(candidate.as_str()) {
        candidate = format!("{seed}_{suffix}");
        suffix += 1;
    }
    candidate
}

fn next_priority(values: impl Iterator<Item = i32>) -> i32 {
    values.max().map(|value| value + 1).unwrap_or(0)
}

fn slugify(value: &str) -> String {
    let mut out = String::new();
    let mut last_sep = false;
    for ch in value.chars().flat_map(char::to_lowercase) {
        if ch.is_ascii_alphanumeric() {
            out.push(ch);
            last_sep = false;
        } else if !last_sep {
            out.push('_');
            last_sep = true;
        }
    }
    out.trim_matches('_').to_string()
}

fn is_person_interest(interest: &SynthesisInterest) -> bool {
    interest.interest_type.eq_ignore_ascii_case("person")
}

fn same_text(left: &str, right: &str) -> bool {
    left.trim().eq_ignore_ascii_case(right.trim())
}

fn contains_any<'a>(haystack: &str, mut terms: impl Iterator<Item = &'a str>) -> bool {
    let haystack = haystack.to_ascii_lowercase();
    terms.any(|term| {
        let term = term.trim();
        !term.is_empty() && haystack.contains(&term.to_ascii_lowercase())
    })
}
