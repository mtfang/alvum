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
    /// File path (relative to capture dir or absolute).
    pub path: String,
    /// MIME type of the file (e.g., "audio/opus", "image/webp", "text/plain").
    pub mime: String,
    /// Connector-specific context (any shape).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata: Option<serde_json::Value>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serialize_data_ref() {
        let dr = DataRef {
            ts: "2026-04-11T10:15:00Z".parse().unwrap(),
            source: "audio-mic".into(),
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
            path: "capture/snapshots/10-15-00.webp".into(),
            mime: "image/webp".into(),
            metadata: None,
        };
        let json = serde_json::to_string(&dr).unwrap();
        let deserialized: DataRef = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized, dr);
    }
}
