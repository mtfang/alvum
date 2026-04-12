//! Core types for episodic alignment: time blocks, context threads, and threading results.

use alvum_core::observation::Observation;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// A fixed-duration window containing all observations from all sources.
/// Pass 1 output. Pure temporal quantization.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TimeBlock {
    pub start: DateTime<Utc>,
    pub end: DateTime<Utc>,
    pub observations: Vec<Observation>,
}

impl TimeBlock {
    /// Number of distinct sources in this block.
    pub fn source_count(&self) -> usize {
        let mut sources: Vec<&str> = self.observations.iter().map(|o| o.source.as_str()).collect();
        sources.sort();
        sources.dedup();
        sources.len()
    }

    /// Check if block contains observations from a specific source.
    pub fn has_source(&self, source: &str) -> bool {
        self.observations.iter().any(|o| o.source == source)
    }
}

/// A coherent context spanning one or more TimeBlocks.
/// Pass 2 output. Represents a continuous activity with relevance scoring.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContextThread {
    pub id: String,
    pub label: String,
    pub start: DateTime<Utc>,
    pub end: DateTime<Utc>,
    pub sources: Vec<String>,
    pub observations: Vec<Observation>,
    pub relevance: f32,
    pub relevance_signals: Vec<String>,
    /// Free-form classification. Convention: "conversation", "solo_work",
    /// "media_playback", "ambient", "transition" — any string valid.
    pub thread_type: String,
    pub metadata: Option<serde_json::Value>,
}

impl ContextThread {
    /// Duration in seconds.
    pub fn duration_secs(&self) -> f64 {
        (self.end - self.start).num_milliseconds() as f64 / 1000.0
    }

    /// Whether this thread passes a relevance threshold.
    pub fn is_relevant(&self, threshold: f32) -> bool {
        self.relevance >= threshold
    }
}

/// Complete output of the episodic alignment process.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ThreadingResult {
    pub start: DateTime<Utc>,
    pub end: DateTime<Utc>,
    pub time_blocks: Vec<TimeBlock>,
    pub threads: Vec<ContextThread>,
    pub observation_count: usize,
    pub source_count: usize,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn obs(ts: &str, source: &str, kind: &str, content: &str) -> Observation {
        Observation {
            ts: ts.parse().unwrap(),
            source: source.into(),
            kind: kind.into(),
            content: content.into(),
            metadata: None,
            media_ref: None,
        }
    }

    #[test]
    fn time_block_source_count() {
        let block = TimeBlock {
            start: "2026-04-11T10:00:00Z".parse().unwrap(),
            end: "2026-04-11T10:05:00Z".parse().unwrap(),
            observations: vec![
                obs("2026-04-11T10:00:15Z", "audio-mic", "speech", "hello"),
                obs("2026-04-11T10:00:20Z", "screen", "app_focus", "Zoom"),
                obs("2026-04-11T10:01:00Z", "audio-mic", "speech", "world"),
            ],
        };
        assert_eq!(block.source_count(), 2);
        assert!(block.has_source("audio-mic"));
        assert!(block.has_source("screen"));
        assert!(!block.has_source("calendar"));
    }

    #[test]
    fn context_thread_relevance_filter() {
        let thread = ContextThread {
            id: "thread_001".into(),
            label: "Sprint Planning".into(),
            start: "2026-04-11T10:00:00Z".parse().unwrap(),
            end: "2026-04-11T10:30:00Z".parse().unwrap(),
            sources: vec!["audio-mic".into(), "screen".into()],
            observations: vec![],
            relevance: 0.8,
            relevance_signals: vec!["multi-source convergence".into()],
            thread_type: "conversation".into(),
            metadata: None,
        };
        assert!(thread.is_relevant(0.5));
        assert!(thread.is_relevant(0.8));
        assert!(!thread.is_relevant(0.9));
        assert!((thread.duration_secs() - 1800.0).abs() < 0.1);
    }

    #[test]
    fn roundtrip_time_block() {
        let block = TimeBlock {
            start: "2026-04-11T10:00:00Z".parse().unwrap(),
            end: "2026-04-11T10:05:00Z".parse().unwrap(),
            observations: vec![obs("2026-04-11T10:01:00Z", "git", "commit", "fix bug")],
        };
        let json = serde_json::to_string(&block).unwrap();
        let deserialized: TimeBlock = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.observations.len(), 1);
    }

    #[test]
    fn roundtrip_context_thread() {
        let thread = ContextThread {
            id: "thread_001".into(),
            label: "TV Background".into(),
            start: "2026-04-11T10:05:00Z".parse().unwrap(),
            end: "2026-04-11T11:30:00Z".parse().unwrap(),
            sources: vec!["audio-mic".into()],
            observations: vec![],
            relevance: 0.1,
            relevance_signals: vec!["media dialogue detected".into()],
            thread_type: "media_playback".into(),
            metadata: Some(serde_json::json!({"show": "Breaking Bad"})),
        };
        let json = serde_json::to_string(&thread).unwrap();
        let deserialized: ContextThread = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.thread_type, "media_playback");
        assert_eq!(deserialized.relevance, 0.1);
    }
}
