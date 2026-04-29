//! DataRef — what connectors produce. A pointer to raw data, not the data itself.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// A reference to a captured data file. Connectors emit these as JSONL.
/// The connector's only job is to say "here's a file from this time."
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DataRef {
    /// When this data was captured.
    pub ts: DateTime<Utc>,
    /// Which connector produced this (e.g., "audio-mic", "screen", "git").
    pub source: String,
    /// Fully qualified capture component that produced the ref.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub producer: String,
    /// Payload contract for routing to compatible processors.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub schema: String,
    /// File path (relative to capture dir or absolute).
    pub path: String,
    /// MIME type of the file (e.g., "audio/opus", "image/webp", "text/plain").
    pub mime: String,
    /// Connector-specific context (any shape).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata: Option<serde_json::Value>,
}

impl DataRef {
    pub fn new(
        ts: DateTime<Utc>,
        source: impl Into<String>,
        path: impl Into<String>,
        mime: impl Into<String>,
    ) -> Self {
        Self {
            ts,
            source: source.into(),
            producer: String::new(),
            schema: String::new(),
            path: path.into(),
            mime: mime.into(),
            metadata: None,
        }
    }

    pub fn with_routing(mut self, producer: impl Into<String>, schema: impl Into<String>) -> Self {
        self.producer = producer.into();
        self.schema = schema.into();
        self
    }

    pub fn with_metadata(mut self, metadata: serde_json::Value) -> Self {
        self.metadata = Some(metadata);
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serialize_data_ref() {
        let dr = DataRef {
            ts: "2026-04-11T10:15:00Z".parse().unwrap(),
            source: "audio-mic".into(),
            producer: "alvum.audio/audio-mic".into(),
            schema: "alvum.audio.opus.v1".into(),
            path: "capture/audio/mic/10-15-00.opus".into(),
            mime: "audio/opus".into(),
            metadata: None,
        };
        let json = serde_json::to_string(&dr).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed["source"], "audio-mic");
        assert_eq!(parsed["mime"], "audio/opus");
        assert!(parsed.get("metadata").is_none());
    }

    #[test]
    fn serialize_data_ref_with_metadata() {
        let dr = DataRef {
            ts: "2026-04-11T10:15:00Z".parse().unwrap(),
            source: "git".into(),
            producer: "alvum.git/log".into(),
            schema: "alvum.git.diff.v1".into(),
            path: "abc123.patch".into(),
            mime: "text/x-diff".into(),
            metadata: Some(serde_json::json!({"author": "michael", "branch": "main"})),
        };
        let json = serde_json::to_string(&dr).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed["metadata"]["author"], "michael");
    }

    #[test]
    fn roundtrip_data_ref() {
        let dr = DataRef {
            ts: "2026-04-11T10:15:00Z".parse().unwrap(),
            source: "screen".into(),
            producer: "alvum.screen/snapshot".into(),
            schema: "alvum.screen.image.v1".into(),
            path: "capture/snapshots/10-15-00.webp".into(),
            mime: "image/webp".into(),
            metadata: None,
        };
        let json = serde_json::to_string(&dr).unwrap();
        let deserialized: DataRef = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized, dr);
    }

    #[test]
    fn old_data_refs_default_routing_identity() {
        let json = r#"{
            "ts": "2026-04-11T10:15:00Z",
            "source": "audio-mic",
            "path": "capture/audio/mic/10-15-00.opus",
            "mime": "audio/opus"
        }"#;
        let dr: DataRef = serde_json::from_str(json).unwrap();

        assert_eq!(dr.producer, "");
        assert_eq!(dr.schema, "");
    }

    #[test]
    fn data_refs_include_routing_identity_when_present() {
        let dr = DataRef {
            ts: "2026-04-11T10:15:00Z".parse().unwrap(),
            source: "audio-mic".into(),
            producer: "alvum.audio/audio-mic".into(),
            schema: "alvum.audio.opus.v1".into(),
            path: "capture/audio/mic/10-15-00.opus".into(),
            mime: "audio/opus".into(),
            metadata: None,
        };

        let json = serde_json::to_value(&dr).unwrap();

        assert_eq!(json["producer"], "alvum.audio/audio-mic");
        assert_eq!(json["schema"], "alvum.audio.opus.v1");
    }
}
