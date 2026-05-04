use super::thresholds::{
    HIGH_PERSON_VOICE_HOLDOUT_ACCURACY, HIGH_PERSON_VOICE_MARGIN, HIGH_PERSON_VOICE_MODEL_SUPPORT,
    HIGH_PERSON_VOICE_RADIUS, HIGH_PERSON_VOICE_SOURCE_COUNT, HIGH_PERSON_VOICE_SOURCE_SUPPORT,
    MEDIUM_PERSON_VOICE_HOLDOUT_ACCURACY, MEDIUM_PERSON_VOICE_MARGIN, MEDIUM_PERSON_VOICE_RADIUS,
    MIN_PERSON_VOICE_MODEL_SUPPORT, PERSON_VOICE_AUTO_PREDICT_THRESHOLD,
    PERSON_VOICE_CANDIDATE_THRESHOLD, PERSON_VOICE_MATCH_FLOOR,
};
use super::{
    PersonVoiceModelSummary, SpeakerProfile, TrackedInterestCandidate, VoiceModelSourceStat,
    VoiceSample, interest_summary, is_person_interest,
};
use crate::fingerprint::AudioFingerprint;
use alvum_core::synthesis_profile::{SynthesisInterest, SynthesisProfile};
use std::collections::{HashMap, HashSet};

#[derive(Debug, Clone)]
pub(super) struct PersonVoiceModel {
    interest: SynthesisInterest,
    model: String,
    centroid: Vec<f32>,
    support_count: usize,
    confidence_radius: f32,
    mean_similarity: f32,
    confidence: VoiceModelConfidence,
    source_stats: Vec<VoiceModelSourceStat>,
    holdout_accuracy: f32,
    holdout_margin: f32,
}

#[derive(Debug, Clone)]
struct VerifiedVoiceFingerprint {
    fingerprint: AudioFingerprint,
    source: String,
}

#[derive(Debug, Clone)]
struct VoiceModelStats {
    centroid: Vec<f32>,
    support_count: usize,
    confidence_radius: f32,
    mean_similarity: f32,
}

#[derive(Debug, Clone, Default)]
struct VoiceModelVerification {
    accuracy: f32,
    margin: f32,
    source_accuracy: HashMap<String, f32>,
    source_margin: HashMap<String, f32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum VoiceModelConfidence {
    Low,
    Medium,
    High,
}

impl VoiceModelConfidence {
    fn as_str(self) -> &'static str {
        match self {
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
        }
    }
}

pub(super) fn person_voice_models(
    profile: Option<&SynthesisProfile>,
    _speakers: &[SpeakerProfile],
    samples: &[VoiceSample],
) -> Vec<PersonVoiceModel> {
    let Some(profile) = profile else {
        return Vec::new();
    };
    let people = profile
        .interests
        .iter()
        .filter(|interest| is_person_interest(interest))
        .map(|interest| (interest.id.clone(), interest.clone()))
        .collect::<HashMap<_, _>>();
    if people.is_empty() {
        return Vec::new();
    }

    let mut grouped: HashMap<(String, String), Vec<VerifiedVoiceFingerprint>> = HashMap::new();
    let mut seen: HashMap<(String, String), HashSet<String>> = HashMap::new();
    for sample in samples {
        if sample
            .quality_flags
            .iter()
            .any(|flag| flag == "ignored_by_user")
        {
            continue;
        }
        if sample.assignment_source != "user_confirmed_sample" {
            continue;
        }
        let (Some(interest_id), Some(fingerprint)) = (
            sample.linked_interest_id.as_deref(),
            sample.fingerprint.as_ref(),
        ) else {
            continue;
        };
        if people.contains_key(interest_id) {
            add_verified_person_fingerprint(
                &mut grouped,
                &mut seen,
                interest_id,
                &sample.source,
                fingerprint,
            );
        }
    }

    let mut models = grouped
        .iter()
        .filter_map(|((interest_id, model), fingerprints)| {
            let interest = people.get(interest_id)?;
            person_voice_model(interest, model, fingerprints, &grouped)
        })
        .collect::<Vec<_>>();
    models.sort_by(|left, right| {
        left.interest
            .name
            .cmp(&right.interest.name)
            .then(left.model.cmp(&right.model))
    });
    models
}

fn add_verified_person_fingerprint(
    grouped: &mut HashMap<(String, String), Vec<VerifiedVoiceFingerprint>>,
    seen: &mut HashMap<(String, String), HashSet<String>>,
    interest_id: &str,
    source: &str,
    fingerprint: &AudioFingerprint,
) {
    let key = (interest_id.to_string(), fingerprint.model.clone());
    if !seen
        .entry(key.clone())
        .or_default()
        .insert(format!("{source}:{}", fingerprint.digest))
    {
        return;
    }
    grouped
        .entry(key)
        .or_default()
        .push(VerifiedVoiceFingerprint {
            fingerprint: fingerprint.clone(),
            source: source.to_string(),
        });
}

fn person_voice_model(
    interest: &SynthesisInterest,
    model: &str,
    fingerprints: &[VerifiedVoiceFingerprint],
    grouped: &HashMap<(String, String), Vec<VerifiedVoiceFingerprint>>,
) -> Option<PersonVoiceModel> {
    let stats = voice_model_stats(
        &fingerprints
            .iter()
            .map(|verified| verified.fingerprint.clone())
            .collect::<Vec<_>>(),
    )?;
    let verification = verify_voice_model(&interest.id, model, fingerprints, grouped);
    let source_stats = voice_model_source_stats(fingerprints, &verification);
    let confidence = voice_model_confidence(&stats, &source_stats, &verification);
    Some(PersonVoiceModel {
        interest: interest.clone(),
        model: model.to_string(),
        centroid: stats.centroid,
        support_count: stats.support_count,
        confidence_radius: stats.confidence_radius,
        mean_similarity: stats.mean_similarity,
        confidence,
        source_stats,
        holdout_accuracy: verification.accuracy,
        holdout_margin: verification.margin,
    })
}

fn voice_model_stats(fingerprints: &[AudioFingerprint]) -> Option<VoiceModelStats> {
    let model = fingerprints.first()?.model.clone();
    let vectors = fingerprints
        .iter()
        .filter(|fingerprint| fingerprint.model == model)
        .filter_map(|fingerprint| normalized_vector(&fingerprint.vector))
        .collect::<Vec<_>>();
    let centroid = centroid_vector(&vectors)?;
    let similarities = vectors
        .iter()
        .filter(|vector| vector.len() == centroid.len())
        .map(|vector| cosine_unit_vectors(vector, &centroid))
        .collect::<Vec<_>>();
    if similarities.is_empty() {
        return None;
    }
    let support_count = similarities.len();
    let mean_similarity = similarities.iter().sum::<f32>() / support_count as f32;
    let empirical_radius = (similarities
        .iter()
        .map(|similarity| {
            let distance = 1.0 - similarity.clamp(-1.0, 1.0);
            distance * distance
        })
        .sum::<f32>()
        / support_count as f32)
        .sqrt();
    let confidence_radius =
        (empirical_radius + 0.25 / (support_count as f32).sqrt()).clamp(0.0, 1.0);
    Some(VoiceModelStats {
        centroid,
        support_count,
        confidence_radius,
        mean_similarity,
    })
}

fn verify_voice_model(
    interest_id: &str,
    model: &str,
    fingerprints: &[VerifiedVoiceFingerprint],
    grouped: &HashMap<(String, String), Vec<VerifiedVoiceFingerprint>>,
) -> VoiceModelVerification {
    let mut total = 0usize;
    let mut correct = 0usize;
    let mut min_margin: Option<f32> = None;
    let mut source_total: HashMap<String, usize> = HashMap::new();
    let mut source_correct: HashMap<String, usize> = HashMap::new();
    let mut source_min_margin: HashMap<String, f32> = HashMap::new();

    for (index, held_out) in fingerprints.iter().enumerate() {
        let Some(query) = normalized_vector(&held_out.fingerprint.vector) else {
            continue;
        };
        let Some(own_centroid) = held_out_centroid(fingerprints, Some(index)) else {
            continue;
        };
        let own_similarity = cosine_unit_vectors(&query, &own_centroid);
        let best_other_similarity = grouped
            .iter()
            .filter(|((candidate_interest_id, candidate_model), _)| {
                candidate_interest_id != interest_id && candidate_model == model
            })
            .filter_map(|(_, other_fingerprints)| held_out_centroid(other_fingerprints, None))
            .filter(|centroid| centroid.len() == query.len())
            .map(|centroid| cosine_unit_vectors(&query, &centroid))
            .fold(PERSON_VOICE_MATCH_FLOOR, f32::max);
        let margin = own_similarity - best_other_similarity;
        let is_correct = own_similarity >= PERSON_VOICE_MATCH_FLOOR && margin >= 0.0;

        total += 1;
        if is_correct {
            correct += 1;
        }
        min_margin = Some(min_margin.map_or(margin, |existing| existing.min(margin)));
        *source_total.entry(held_out.source.clone()).or_insert(0) += 1;
        if is_correct {
            *source_correct.entry(held_out.source.clone()).or_insert(0) += 1;
        }
        source_min_margin
            .entry(held_out.source.clone())
            .and_modify(|existing| *existing = existing.min(margin))
            .or_insert(margin);
    }

    if total == 0 {
        return VoiceModelVerification::default();
    }
    let source_accuracy = source_total
        .iter()
        .map(|(source, total)| {
            let correct = source_correct.get(source).copied().unwrap_or(0);
            (source.clone(), correct as f32 / *total as f32)
        })
        .collect::<HashMap<_, _>>();

    VoiceModelVerification {
        accuracy: correct as f32 / total as f32,
        margin: min_margin.unwrap_or(0.0),
        source_accuracy,
        source_margin: source_min_margin,
    }
}

fn held_out_centroid(
    fingerprints: &[VerifiedVoiceFingerprint],
    excluded_index: Option<usize>,
) -> Option<Vec<f32>> {
    let vectors = fingerprints
        .iter()
        .enumerate()
        .filter(|(index, _)| Some(*index) != excluded_index)
        .filter_map(|(_, verified)| normalized_vector(&verified.fingerprint.vector))
        .collect::<Vec<_>>();
    centroid_vector(&vectors)
}

fn voice_model_source_stats(
    fingerprints: &[VerifiedVoiceFingerprint],
    verification: &VoiceModelVerification,
) -> Vec<VoiceModelSourceStat> {
    let mut by_source: HashMap<String, Vec<AudioFingerprint>> = HashMap::new();
    for verified in fingerprints {
        by_source
            .entry(verified.source.clone())
            .or_default()
            .push(verified.fingerprint.clone());
    }
    let mut stats = by_source
        .into_iter()
        .filter_map(|(source, fingerprints)| {
            let stats = voice_model_stats(&fingerprints)?;
            Some(VoiceModelSourceStat {
                source: source.clone(),
                support_count: stats.support_count,
                confidence_radius: stats.confidence_radius,
                mean_similarity: stats.mean_similarity,
                holdout_accuracy: verification.source_accuracy.get(&source).copied(),
                holdout_margin: verification.source_margin.get(&source).copied(),
            })
        })
        .collect::<Vec<_>>();
    stats.sort_by(|left, right| left.source.cmp(&right.source));
    stats
}

fn voice_model_confidence(
    stats: &VoiceModelStats,
    source_stats: &[VoiceModelSourceStat],
    verification: &VoiceModelVerification,
) -> VoiceModelConfidence {
    let source_count = source_stats.len();
    let sources_have_high_support = source_stats
        .iter()
        .filter(|source| source.support_count >= HIGH_PERSON_VOICE_SOURCE_SUPPORT)
        .count()
        >= HIGH_PERSON_VOICE_SOURCE_COUNT;
    if stats.support_count >= HIGH_PERSON_VOICE_MODEL_SUPPORT
        && source_count >= HIGH_PERSON_VOICE_SOURCE_COUNT
        && sources_have_high_support
        && stats.confidence_radius <= HIGH_PERSON_VOICE_RADIUS
        && verification.accuracy >= HIGH_PERSON_VOICE_HOLDOUT_ACCURACY
        && verification.margin >= HIGH_PERSON_VOICE_MARGIN
    {
        return VoiceModelConfidence::High;
    }
    if stats.support_count >= MIN_PERSON_VOICE_MODEL_SUPPORT
        && stats.confidence_radius <= MEDIUM_PERSON_VOICE_RADIUS
        && verification.accuracy >= MEDIUM_PERSON_VOICE_HOLDOUT_ACCURACY
        && verification.margin >= MEDIUM_PERSON_VOICE_MARGIN
    {
        return VoiceModelConfidence::Medium;
    }
    VoiceModelConfidence::Low
}

pub(super) fn voice_model_candidates(
    fingerprints: &[AudioFingerprint],
    person_models: &[PersonVoiceModel],
    query_source: Option<&str>,
) -> Vec<TrackedInterestCandidate> {
    let mut scored = Vec::new();
    for (index, model) in person_models.iter().enumerate() {
        if model.confidence == VoiceModelConfidence::Low {
            continue;
        }
        let similarity = fingerprints
            .iter()
            .filter(|fingerprint| fingerprint.model == model.model)
            .filter_map(|fingerprint| normalized_vector(&fingerprint.vector))
            .filter(|vector| vector.len() == model.centroid.len())
            .map(|vector| cosine_unit_vectors(&vector, &model.centroid))
            .fold(0.0_f32, f32::max);
        if similarity <= 0.0 {
            continue;
        }
        let score = conservative_voice_match_score(similarity, model);
        if score >= PERSON_VOICE_CANDIDATE_THRESHOLD {
            scored.push((index, similarity, score));
        }
    }
    let mut candidates = Vec::new();
    for (index, similarity, score) in &scored {
        let model = &person_models[*index];
        let runner_up_similarity = scored
            .iter()
            .filter(|(candidate_index, _, _)| candidate_index != index)
            .map(|(_, similarity, _)| *similarity)
            .fold(PERSON_VOICE_MATCH_FLOOR, f32::max);
        let prediction_margin = similarity - runner_up_similarity;
        let source_supported = query_source.is_none_or(|source| {
            model
                .source_stats
                .iter()
                .any(|stat| stat.source == source && stat.support_count > 0)
        });
        let auto_predict = model.confidence == VoiceModelConfidence::High
            && *score >= PERSON_VOICE_AUTO_PREDICT_THRESHOLD
            && prediction_margin >= HIGH_PERSON_VOICE_MARGIN
            && source_supported;
        if model.confidence != VoiceModelConfidence::High || auto_predict {
            candidates.push(distribution_candidate(
                &model.interest,
                *score,
                format!(
                    "{} confidence voice fingerprint distribution match",
                    model.confidence.as_str()
                ),
                model,
                prediction_margin,
                auto_predict,
            ));
        }
    }
    candidates
}

fn conservative_voice_match_score(similarity: f32, model: &PersonVoiceModel) -> f32 {
    let similarity_strength = ((similarity - PERSON_VOICE_MATCH_FLOOR)
        / (1.0 - PERSON_VOICE_MATCH_FLOOR))
        .clamp(0.0, 1.0);
    let support_strength = ((model.support_count as f32) / 4.0).sqrt().min(1.0);
    let radius_strength = (1.0 - model.confidence_radius).clamp(0.0, 1.0);
    (similarity_strength * support_strength * radius_strength).clamp(0.0, 1.0)
}

fn centroid_vector(vectors: &[Vec<f32>]) -> Option<Vec<f32>> {
    let len = vectors.first()?.len();
    if len == 0 {
        return None;
    }
    let mut centroid = vec![0.0_f32; len];
    let mut count = 0usize;
    for vector in vectors {
        if vector.len() != len {
            continue;
        }
        for (index, value) in vector.iter().enumerate() {
            centroid[index] += value;
        }
        count += 1;
    }
    if count == 0 {
        return None;
    }
    for value in &mut centroid {
        *value /= count as f32;
    }
    normalized_vector(&centroid)
}

fn normalized_vector(vector: &[f32]) -> Option<Vec<f32>> {
    let norm = vector.iter().map(|value| value * value).sum::<f32>().sqrt();
    if norm == 0.0 {
        return None;
    }
    Some(vector.iter().map(|value| value / norm).collect())
}

fn cosine_unit_vectors(left: &[f32], right: &[f32]) -> f32 {
    left.iter()
        .zip(right.iter())
        .map(|(left, right)| left * right)
        .sum::<f32>()
        .clamp(-1.0, 1.0)
}

fn distribution_candidate(
    interest: &SynthesisInterest,
    score: f32,
    reason: String,
    model: &PersonVoiceModel,
    prediction_margin: f32,
    auto_predict: bool,
) -> TrackedInterestCandidate {
    TrackedInterestCandidate {
        id: interest.id.clone(),
        interest_type: interest.interest_type.clone(),
        name: interest.name.clone(),
        score,
        reason,
        support_count: Some(model.support_count),
        confidence_radius: Some(model.confidence_radius),
        mean_similarity: Some(model.mean_similarity),
        voice_model_confidence: Some(model.confidence.as_str().into()),
        verified_sample_count: Some(model.support_count),
        source_count: Some(model.source_stats.len()),
        holdout_accuracy: Some(model.holdout_accuracy),
        holdout_margin: Some(model.holdout_margin),
        prediction_margin: Some(prediction_margin),
        auto_predict: Some(auto_predict),
        source_stats: Some(model.source_stats.clone()),
    }
}

pub(super) fn person_voice_model_summary(model: PersonVoiceModel) -> PersonVoiceModelSummary {
    PersonVoiceModelSummary {
        linked_interest: interest_summary(&model.interest),
        model: model.model,
        confidence: model.confidence.as_str().into(),
        verified_sample_count: model.support_count,
        source_count: model.source_stats.len(),
        confidence_radius: model.confidence_radius,
        mean_similarity: model.mean_similarity,
        holdout_accuracy: model.holdout_accuracy,
        holdout_margin: model.holdout_margin,
        auto_predict_ready: model.confidence == VoiceModelConfidence::High,
        source_stats: model.source_stats,
    }
}
