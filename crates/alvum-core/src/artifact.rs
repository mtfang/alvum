//! Artifact — what processors produce. A DataRef plus typed output layers.
//!
//! Layers are a string-keyed map of JSON values. Each processor adds its own
//! layers without knowing about other processors. Convention:
//! - `text`: human/LLM-readable content (pipeline reads this)
//! - `embedding`: vector + model info (embedding index reads this)
//! - `structured`: parsed data like timestamps, speakers, entities
//! - `structured.*`: namespaced structured data (e.g., `structured.sentiment`)

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use crate::data_ref::DataRef;

/// A processed data reference with typed output layers.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Artifact {
    /// The original data file this was processed from.
    pub data_ref: DataRef,
    /// Typed output layers. Keys are namespaced strings (e.g., "text", "embedding").
    pub layers: HashMap<String, serde_json::Value>,
}

impl Artifact {
    /// Create an artifact with a single text layer (the common case).
    pub fn with_text(data_ref: DataRef, text: impl Into<String>) -> Self {
        let mut layers = HashMap::new();
        layers.insert("text".into(), serde_json::Value::String(text.into()));
        Self { data_ref, layers }
    }

    /// Get the text layer (what the LLM reads).
    pub fn text(&self) -> Option<&str> {
        self.layers.get("text")?.as_str()
    }

    /// Get any layer by key.
    pub fn layer(&self, key: &str) -> Option<&serde_json::Value> {
        self.layers.get(key)
    }

    /// Check if a specific layer exists.
    pub fn has_layer(&self, key: &str) -> bool {
        self.layers.contains_key(key)
    }

    /// Add a layer. Overwrites if key already exists.
    pub fn add_layer(&mut self, key: impl Into<String>, value: serde_json::Value) {
        self.layers.insert(key.into(), value);
    }

    /// Convenience: get the timestamp from the underlying DataRef.
    pub fn ts(&self) -> chrono::DateTime<chrono::Utc> {
        self.data_ref.ts
    }

    /// Convenience: get the source from the underlying DataRef.
    pub fn source(&self) -> &str {
        &self.data_ref.source
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::data_ref::DataRef;

    fn sample_data_ref() -> DataRef {
        DataRef {
            ts: "2026-04-11T10:15:00Z".parse().unwrap(),
            source: "audio-mic".into(),
            path: "capture/audio/mic/10-15-00.opus".into(),
            mime: "audio/opus".into(),
            metadata: None,
        }
    }

    #[test]
    fn with_text_creates_text_layer() {
        let artifact = Artifact::with_text(
            sample_data_ref(),
            "I think we should defer the migration",
        );
        assert_eq!(artifact.text(), Some("I think we should defer the migration"));
        assert!(artifact.has_layer("text"));
        assert!(!artifact.has_layer("embedding"));
    }

    #[test]
    fn add_multiple_layers() {
        let mut artifact = Artifact::with_text(sample_data_ref(), "transcript text");
        artifact.add_layer("structured", serde_json::json!({
            "segments": [{"start": 0.0, "end": 2.5, "text": "transcript text"}],
            "language": "en"
        }));
        artifact.add_layer("embedding", serde_json::json!({
            "model": "gemini-embedding-2",
            "vector": [0.1, 0.2, 0.3],
            "dims": 3
        }));

        assert!(artifact.has_layer("text"));
        assert!(artifact.has_layer("structured"));
        assert!(artifact.has_layer("embedding"));
        assert_eq!(artifact.layer("structured").unwrap()["language"], "en");
    }

    #[test]
    fn roundtrip_artifact() {
        let mut artifact = Artifact::with_text(sample_data_ref(), "hello");
        artifact.add_layer("structured", serde_json::json!({"key": "value"}));

        let json = serde_json::to_string(&artifact).unwrap();
        let deserialized: Artifact = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.text(), Some("hello"));
        assert_eq!(deserialized.layer("structured").unwrap()["key"], "value");
    }

    #[test]
    fn convenience_methods() {
        let artifact = Artifact::with_text(sample_data_ref(), "text");
        assert_eq!(artifact.source(), "audio-mic");
        assert_eq!(artifact.ts().to_rfc3339(), "2026-04-11T10:15:00+00:00");
    }
}
