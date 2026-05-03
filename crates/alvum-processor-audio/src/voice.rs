use alvum_core::artifact::Artifact;
use alvum_core::data_ref::DataRef;
use std::collections::BTreeMap;

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct FingerprintRef {
    pub model: String,
    pub digest: String,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct SpeakerTurn {
    pub start_secs: f32,
    pub end_secs: f32,
    pub text: String,
    pub speaker_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub speaker_label: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider_speaker: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub confidence: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fingerprint_ref: Option<FingerprintRef>,
}

#[derive(Debug, Clone)]
pub struct AudioIntelligenceArtifact {
    data_ref: DataRef,
    text: String,
    turns: Vec<SpeakerTurn>,
    transcript_source: String,
    fingerprint_model: String,
}

impl AudioIntelligenceArtifact {
    pub fn new(
        data_ref: DataRef,
        text: String,
        turns: Vec<SpeakerTurn>,
        transcript_source: impl Into<String>,
        fingerprint_model: impl Into<String>,
    ) -> Self {
        Self {
            data_ref,
            text,
            turns,
            transcript_source: transcript_source.into(),
            fingerprint_model: fingerprint_model.into(),
        }
    }

    pub fn into_artifact(self) -> Artifact {
        let text = speaker_text(&self.text, &self.turns);
        let primary_speaker = self.turns.first().map(|turn| {
            turn.speaker_label
                .as_deref()
                .unwrap_or(&turn.speaker_id)
                .to_string()
        });
        let speaker_layer = speaker_layer(&self.turns);
        let diarization_layer = serde_json::json!({
            "source": self.transcript_source,
            "turn_count": self.turns.len(),
            "speaker_count": speaker_layer["speakers"].as_array().map(|speakers| speakers.len()).unwrap_or(0),
            "fingerprint_model": self.fingerprint_model,
        });
        let structured = serde_json::json!({
            "schema": "alvum.audio.intelligence.v2",
            "transcript_source": self.transcript_source,
            "fingerprint_model": self.fingerprint_model,
            "speaker": primary_speaker,
            "speaker_id": self.turns.first().map(|turn| turn.speaker_id.clone()),
            "turns": self.turns,
            "diarization": diarization_layer,
            "speakers": speaker_layer["speakers"],
        });
        let mut artifact = Artifact::with_text(self.data_ref, text);
        artifact.add_layer("structured", structured.clone());
        artifact.add_layer("structured.audio.v2", structured);
        artifact.add_layer("structured.diarization", diarization_layer);
        artifact.add_layer("structured.speakers", speaker_layer);
        artifact
    }
}

fn speaker_text(fallback: &str, turns: &[SpeakerTurn]) -> String {
    let lines = turns
        .iter()
        .filter_map(|turn| {
            let text = turn.text.trim();
            if text.is_empty() {
                return None;
            }
            let speaker = turn
                .speaker_label
                .as_deref()
                .unwrap_or(&turn.speaker_id)
                .trim();
            if speaker.is_empty() || speaker == "voice_unassigned" {
                Some(text.to_string())
            } else {
                Some(format!("{speaker}: {text}"))
            }
        })
        .collect::<Vec<_>>();
    if lines.is_empty() {
        fallback.trim().to_string()
    } else {
        lines.join("\n")
    }
}

fn speaker_layer(turns: &[SpeakerTurn]) -> serde_json::Value {
    let mut speakers: BTreeMap<String, serde_json::Value> = BTreeMap::new();
    for turn in turns {
        let entry = speakers.entry(turn.speaker_id.clone()).or_insert_with(|| {
            serde_json::json!({
                "speaker_id": turn.speaker_id,
                "speaker_label": turn.speaker_label,
                "provider_speakers": [],
                "fingerprint_refs": [],
                "turn_count": 0,
            })
        });
        entry["turn_count"] = serde_json::json!(entry["turn_count"].as_u64().unwrap_or(0) + 1);
        if let Some(label) = turn.speaker_label.as_deref() {
            entry["speaker_label"] = serde_json::json!(label);
        }
        if let Some(provider_speaker) = turn.provider_speaker.as_deref() {
            push_unique(
                &mut entry["provider_speakers"],
                serde_json::json!(provider_speaker),
            );
        }
        if let Some(fingerprint_ref) = turn.fingerprint_ref.as_ref() {
            push_unique(
                &mut entry["fingerprint_refs"],
                serde_json::json!({
                    "model": fingerprint_ref.model,
                    "digest": fingerprint_ref.digest,
                }),
            );
        }
    }
    serde_json::json!({
        "schema": "alvum.audio.speakers.v1",
        "speakers": speakers.into_values().collect::<Vec<_>>(),
    })
}

fn push_unique(array_value: &mut serde_json::Value, value: serde_json::Value) {
    if let Some(array) = array_value.as_array_mut() {
        if !array.iter().any(|existing| existing == &value) {
            array.push(value);
        }
    }
}
