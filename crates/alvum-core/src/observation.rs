//! Observation — what the pipeline consumes. Text content with optional media reference.
//!
//! Observations are typically created from Artifact text layers, but can also be
//! produced directly by simple connectors that don't need the full DataRef → Artifact flow.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// A processed observation ready for the pipeline.
/// The `content` field is always human/LLM-readable text.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Observation {
    pub ts: DateTime<Utc>,
    /// Source identifier (connector name).
    pub source: String,
    /// Free-form kind string, connector/processor-defined.
    /// Examples: "dialogue", "commit", "speech_segment", "app_focus", "email_sent".
    pub kind: String,
    /// Human/LLM-readable text content.
    pub content: String,
    /// Optional structured metadata (any shape, for LLM context enrichment).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata: Option<serde_json::Value>,
    /// Optional reference to the original media file (for multimodal embedding).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub media_ref: Option<MediaRef>,
}

/// A pointer to a raw media file for future multimodal embedding.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct MediaRef {
    pub path: String,
    pub mime: String,
}

impl Observation {
    /// Create a simple dialogue observation (common case for conversation connectors).
    pub fn dialogue(ts: DateTime<Utc>, source: &str, speaker: &str, content: &str) -> Self {
        Self {
            ts,
            source: source.into(),
            kind: "dialogue".into(),
            content: content.into(),
            metadata: Some(serde_json::json!({"speaker": speaker})),
            media_ref: None,
        }
    }

    /// Get the speaker name if this is a dialogue observation.
    pub fn speaker(&self) -> Option<&str> {
        self.metadata.as_ref()?
            .get("speaker")?
            .as_str()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serialize_observation_with_string_kind() {
        let obs = Observation::dialogue(
            "2026-04-02T04:31:55Z".parse().unwrap(),
            "claude-code",
            "user",
            "imagine we have endless context",
        );
        let json = serde_json::to_string(&obs).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed["source"], "claude-code");
        assert_eq!(parsed["kind"], "dialogue");
        assert_eq!(parsed["metadata"]["speaker"], "user");
    }

    #[test]
    fn roundtrip_observation() {
        let obs = Observation {
            ts: "2026-04-11T10:15:00Z".parse().unwrap(),
            source: "git".into(),
            kind: "commit".into(),
            content: "refactored auth middleware".into(),
            metadata: Some(serde_json::json!({"hash": "abc123"})),
            media_ref: None,
        };
        let json = serde_json::to_string(&obs).unwrap();
        let deserialized: Observation = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized, obs);
    }

    #[test]
    fn observation_with_media_ref() {
        let obs = Observation {
            ts: "2026-04-11T10:15:00Z".parse().unwrap(),
            source: "audio-mic".into(),
            kind: "speech_segment".into(),
            content: "I think we should defer the migration".into(),
            metadata: None,
            media_ref: Some(MediaRef {
                path: "capture/audio/mic/10-15-00.opus".into(),
                mime: "audio/opus".into(),
            }),
        };
        let json = serde_json::to_string(&obs).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed["media_ref"]["mime"], "audio/opus");
    }

    #[test]
    fn dialogue_helper() {
        let obs = Observation::dialogue(
            "2026-04-02T04:31:55Z".parse().unwrap(),
            "claude-code",
            "assistant",
            "This is a fascinating problem.",
        );
        assert_eq!(obs.speaker(), Some("assistant"));
        assert_eq!(obs.kind, "dialogue");
    }
}
