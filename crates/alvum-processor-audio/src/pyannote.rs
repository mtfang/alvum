use crate::fingerprint::AudioFingerprint;
use crate::transcriber::Segment;
use anyhow::{Context, Result, bail};

#[derive(Debug, Clone, PartialEq)]
pub struct PyannoteTurn {
    pub start_secs: f32,
    pub end_secs: f32,
    pub speaker: String,
    pub confidence: Option<f32>,
    pub embedding: Option<Vec<f32>>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PyannoteDiarization {
    pub turns: Vec<PyannoteTurn>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct AlignedDiarizedSegment {
    pub segment: Segment,
    pub provider_speaker: Option<String>,
    pub confidence: Option<f32>,
    pub fingerprint: AudioFingerprint,
}

impl PyannoteDiarization {
    pub fn from_value(value: serde_json::Value) -> Result<Self> {
        let turns_value = value
            .get("turns")
            .or_else(|| value.get("segments"))
            .and_then(|value| value.as_array())
            .context("pyannote diarization JSON is missing turns")?;
        let mut turns = Vec::new();
        for turn in turns_value {
            let start_secs = number_field(turn, &["start", "start_secs"])
                .context("pyannote turn missing start")?;
            let end_secs =
                number_field(turn, &["end", "end_secs"]).context("pyannote turn missing end")?;
            if end_secs <= start_secs {
                continue;
            }
            let speaker = turn
                .get("speaker")
                .or_else(|| turn.get("label"))
                .and_then(|value| value.as_str())
                .unwrap_or("speaker")
                .to_string();
            let confidence = number_field(turn, &["confidence", "score"]);
            let embedding = turn
                .get("embedding")
                .and_then(|value| value.as_array())
                .map(|values| {
                    values
                        .iter()
                        .filter_map(|value| value.as_f64().map(|number| number as f32))
                        .collect::<Vec<_>>()
                })
                .filter(|values| !values.is_empty());
            turns.push(PyannoteTurn {
                start_secs,
                end_secs,
                speaker,
                confidence,
                embedding,
            });
        }
        if turns.is_empty() {
            bail!("pyannote diarization JSON contained no usable turns");
        }
        Ok(Self { turns })
    }
}

pub fn align_segments_to_diarization(
    segments: &[Segment],
    diarization: &PyannoteDiarization,
) -> Vec<AlignedDiarizedSegment> {
    segments
        .iter()
        .map(|segment| {
            let turn = best_turn_for_segment(segment, &diarization.turns);
            let fingerprint = turn
                .and_then(|turn| turn.embedding.clone())
                .map(|embedding| {
                    AudioFingerprint::from_vector("pyannote.embedding", 16_000, embedding)
                })
                .unwrap_or_else(|| {
                    AudioFingerprint::from_vector(
                        "pyannote.unassigned",
                        16_000,
                        vec![segment.start_secs, segment.end_secs],
                    )
                });
            AlignedDiarizedSegment {
                segment: segment.clone(),
                provider_speaker: turn.map(|turn| turn.speaker.clone()),
                confidence: turn.and_then(|turn| turn.confidence),
                fingerprint,
            }
        })
        .collect()
}

fn best_turn_for_segment<'a>(
    segment: &Segment,
    turns: &'a [PyannoteTurn],
) -> Option<&'a PyannoteTurn> {
    turns
        .iter()
        .filter_map(|turn| {
            let overlap = overlap_secs(
                segment.start_secs,
                segment.end_secs,
                turn.start_secs,
                turn.end_secs,
            );
            (overlap > 0.0).then_some((turn, overlap))
        })
        .max_by(|left, right| left.1.total_cmp(&right.1))
        .map(|(turn, _)| turn)
}

fn overlap_secs(left_start: f32, left_end: f32, right_start: f32, right_end: f32) -> f32 {
    left_end.min(right_end) - left_start.max(right_start)
}

fn number_field(value: &serde_json::Value, keys: &[&str]) -> Option<f32> {
    keys.iter()
        .find_map(|key| value.get(*key).and_then(|value| value.as_f64()))
        .map(|number| number as f32)
}
