//! L0 → L1 of the distillation tree: deterministic temporal quantization.
//!
//! Buckets observations into fixed-duration time-blocks (5 min by
//! default), packs blocks into byte-budgeted chunks for the upper-level
//! LLM calls, and formats blocks as readable text the threading prompt
//! consumes. No LLM calls live here — this is pure aggregation.
//!
//! Migrated from `alvum-episode/src/{time_block,types}.rs` as part of
//! the tree-rewrite refactor (T3+T8). The crate `alvum-episode` is
//! retired by this commit; its prompts and types belong with the rest
//! of the tree levels.

use alvum_core::observation::Observation;
use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};

/// A fixed-duration window containing all observations from all sources.
/// Pure temporal quantization — no LLM involvement.
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

/// Bucket observations into fixed-duration time blocks.
/// Empty blocks (no observations) are omitted.
pub fn assemble_time_blocks(
    observations: &[Observation],
    block_duration: Duration,
) -> Vec<TimeBlock> {
    if observations.is_empty() {
        return vec![];
    }

    let mut sorted: Vec<&Observation> = observations.iter().collect();
    sorted.sort_by_key(|o| o.ts);

    let earliest = sorted.first().unwrap().ts;
    let latest = sorted.last().unwrap().ts;

    // Align block start to the block boundary before the earliest observation
    let block_secs = block_duration.num_seconds();
    let epoch_secs = earliest.timestamp();
    let block_start_epoch = (epoch_secs / block_secs) * block_secs;
    let mut current_start = DateTime::<Utc>::from_timestamp(block_start_epoch, 0).unwrap();

    let mut blocks = Vec::new();

    while current_start <= latest {
        let current_end = current_start + block_duration;

        let block_obs: Vec<Observation> = sorted
            .iter()
            .filter(|o| o.ts >= current_start && o.ts < current_end)
            .cloned()
            .cloned()
            .collect();

        if !block_obs.is_empty() {
            blocks.push(TimeBlock {
                start: current_start,
                end: current_end,
                observations: block_obs,
            });
        }

        current_start = current_end;
    }

    blocks
}

/// Greedy packing of time blocks into chunks whose total formatted length
/// stays under `budget_bytes`. A single block that itself exceeds the
/// budget still gets emitted in its own chunk — we can't subdivide an
/// atomic block, so callers must accept that rare degenerate case.
///
/// Used by the threading layer to split L1→L2 LLM calls so a full-day
/// prompt doesn't blow Claude's context window.
pub fn chunk_time_blocks_by_budget(
    blocks: &[TimeBlock],
    budget_bytes: usize,
) -> Vec<Vec<TimeBlock>> {
    let mut chunks: Vec<Vec<TimeBlock>> = Vec::new();
    let mut current: Vec<TimeBlock> = Vec::new();
    let mut current_size = 0usize;

    for block in blocks {
        let size = format_blocks_for_llm(std::slice::from_ref(block)).len();
        if !current.is_empty() && current_size + size > budget_bytes {
            chunks.push(std::mem::take(&mut current));
            current_size = 0;
        }
        current.push(block.clone());
        current_size += size;
    }
    if !current.is_empty() {
        chunks.push(current);
    }
    chunks
}

/// Format time blocks as a text timeline for LLM consumption.
/// Used as input to the L1→L2 (threading) call.
pub fn format_blocks_for_llm(blocks: &[TimeBlock]) -> String {
    let mut parts = Vec::new();

    for (i, block) in blocks.iter().enumerate() {
        let start = block.start.format("%H:%M");
        let end = block.end.format("%H:%M");
        parts.push(format!("=== Block {i} ({start}-{end}) ==="));

        for obs in &block.observations {
            let ts = obs.ts.format("%H:%M:%S");
            let speaker = obs.speaker().map(|s| format!(" {s}:")).unwrap_or_default();
            let content = if obs.content.chars().count() > 500 {
                let truncated: String = obs.content.chars().take(500).collect();
                format!("{truncated}...")
            } else {
                obs.content.clone()
            };
            parts.push(format!(
                "[{ts}] [{source}/{kind}]{speaker} {content}",
                source = obs.source,
                kind = obs.kind
            ));
        }

        parts.push(String::new()); // blank line between blocks
    }

    parts.join("\n")
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
    fn empty_observations_produces_no_blocks() {
        let blocks = assemble_time_blocks(&[], Duration::minutes(5));
        assert!(blocks.is_empty());
    }

    fn make_block(minute: u32, obs_count: usize, content_size: usize) -> TimeBlock {
        let observations: Vec<Observation> = (0..obs_count)
            .map(|i| {
                obs(
                    &format!("2026-04-11T10:{minute:02}:{i:02}Z"),
                    "audio-mic",
                    "speech",
                    &"x".repeat(content_size),
                )
            })
            .collect();
        assemble_time_blocks(&observations, Duration::minutes(5))
            .into_iter()
            .next()
            .expect("non-empty observation vec must yield at least one block")
    }

    #[test]
    fn chunk_empty_input_returns_empty() {
        let chunks = chunk_time_blocks_by_budget(&[], 1_000);
        assert!(chunks.is_empty());
    }

    #[test]
    fn chunk_all_under_budget_single_chunk() {
        let blocks = vec![
            make_block(0, 1, 10),
            make_block(5, 1, 10),
            make_block(10, 1, 10),
        ];
        let chunks = chunk_time_blocks_by_budget(&blocks, 10_000);
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0].len(), 3);
    }

    #[test]
    fn chunk_exceeding_budget_splits_into_multiple() {
        let blocks: Vec<TimeBlock> = (0..6).map(|i| make_block(i * 5, 4, 200)).collect();
        let chunks = chunk_time_blocks_by_budget(&blocks, 2_000);
        assert!(chunks.len() >= 3, "expected ≥3 chunks, got {}", chunks.len());
        let total: usize = chunks.iter().map(|c| c.len()).sum();
        assert_eq!(total, 6);
    }

    #[test]
    fn chunk_oversized_single_block_still_emitted() {
        let blocks = vec![make_block(0, 20, 1000)];
        let chunks = chunk_time_blocks_by_budget(&blocks, 100);
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0].len(), 1);
    }

    #[test]
    fn chunk_preserves_block_order() {
        let blocks: Vec<TimeBlock> = (0..5).map(|i| make_block(i * 5, 3, 150)).collect();
        let chunks = chunk_time_blocks_by_budget(&blocks, 1_200);
        let flat: Vec<DateTime<Utc>> = chunks.iter().flatten().map(|b| b.start).collect();
        let mut sorted = flat.clone();
        sorted.sort();
        assert_eq!(flat, sorted);
    }

    #[test]
    fn single_observation_produces_one_block() {
        let observations = vec![obs(
            "2026-04-11T10:02:30Z",
            "audio-mic",
            "speech",
            "hello",
        )];
        let blocks = assemble_time_blocks(&observations, Duration::minutes(5));
        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0].observations.len(), 1);
    }

    #[test]
    fn observations_in_same_window_group_together() {
        let observations = vec![
            obs("2026-04-11T10:01:00Z", "audio-mic", "speech", "first"),
            obs("2026-04-11T10:03:00Z", "screen", "app_focus", "Zoom"),
            obs("2026-04-11T10:04:30Z", "audio-mic", "speech", "second"),
        ];
        let blocks = assemble_time_blocks(&observations, Duration::minutes(5));
        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0].observations.len(), 3);
        assert_eq!(blocks[0].source_count(), 2);
    }

    #[test]
    fn observations_in_different_windows_produce_separate_blocks() {
        let observations = vec![
            obs("2026-04-11T10:01:00Z", "audio-mic", "speech", "morning"),
            obs("2026-04-11T10:12:00Z", "audio-mic", "speech", "later"),
        ];
        let blocks = assemble_time_blocks(&observations, Duration::minutes(5));
        assert_eq!(blocks.len(), 2);
        assert_eq!(blocks[0].observations[0].content, "morning");
        assert_eq!(blocks[1].observations[0].content, "later");
    }

    #[test]
    fn empty_gaps_are_skipped() {
        let observations = vec![
            obs("2026-04-11T10:01:00Z", "audio-mic", "speech", "early"),
            obs("2026-04-11T10:31:00Z", "audio-mic", "speech", "late"),
        ];
        let blocks = assemble_time_blocks(&observations, Duration::minutes(5));
        assert_eq!(blocks.len(), 2);
    }

    #[test]
    fn observations_sorted_regardless_of_input_order() {
        let observations = vec![
            obs("2026-04-11T10:04:00Z", "audio-mic", "speech", "second"),
            obs("2026-04-11T10:01:00Z", "audio-mic", "speech", "first"),
        ];
        let blocks = assemble_time_blocks(&observations, Duration::minutes(5));
        assert_eq!(blocks[0].observations[0].content, "first");
        assert_eq!(blocks[0].observations[1].content, "second");
    }

    #[test]
    fn cross_source_observations_in_same_block() {
        let observations = vec![
            obs("2026-04-11T10:00:15Z", "audio-mic", "speech", "let's defer"),
            obs("2026-04-11T10:00:15Z", "screen", "app_focus", "Zoom"),
            obs("2026-04-11T10:01:00Z", "calendar", "event", "Sprint Planning"),
        ];
        let blocks = assemble_time_blocks(&observations, Duration::minutes(5));
        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0].source_count(), 3);
    }

    #[test]
    fn format_blocks_produces_readable_output() {
        let observations = vec![
            obs("2026-04-11T10:00:15Z", "audio-mic", "speech", "hello world"),
            obs("2026-04-11T10:00:20Z", "screen", "app_focus", "Zoom"),
        ];
        let blocks = assemble_time_blocks(&observations, Duration::minutes(5));
        let formatted = format_blocks_for_llm(&blocks);
        assert!(formatted.contains("=== Block 0"));
        assert!(formatted.contains("[audio-mic/speech]"));
        assert!(formatted.contains("[screen/app_focus]"));
        assert!(formatted.contains("hello world"));
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
    fn roundtrip_time_block() {
        let block = TimeBlock {
            start: "2026-04-11T10:00:00Z".parse().unwrap(),
            end: "2026-04-11T10:05:00Z".parse().unwrap(),
            observations: vec![obs(
                "2026-04-11T10:01:00Z",
                "git",
                "commit",
                "fix bug",
            )],
        };
        let json = serde_json::to_string(&block).unwrap();
        let deserialized: TimeBlock = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.observations.len(), 1);
    }
}
