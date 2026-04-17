//! Pass 2: LLM-driven context thread detection.
//! Takes formatted time blocks and produces ContextThreads with relevance scores.

use alvum_core::observation::Observation;
use alvum_core::llm::LlmProvider;
use alvum_core::util::strip_markdown_fences;
use anyhow::{Context, Result};
use chrono::{DateTime, Duration, Utc};
use tracing::info;

use crate::time_block;
use crate::types::{ContextThread, ThreadingResult, TimeBlock};

const THREADING_SYSTEM_PROMPT: &str = r#"You are analyzing a full day of captured data from multiple sensors.
The data is organized into 5-minute time blocks, each containing
observations from various sources (audio transcripts, screen events,
location, calendar, etc.).

Identify CONTEXT THREADS — coherent, continuous activities that
may span multiple time blocks and may run concurrently.

For each thread, output:
- id: sequential (thread_001, thread_002, ...)
- label: human-readable name for this activity
- start: ISO 8601 timestamp (start of first relevant observation)
- end: ISO 8601 timestamp (end of last relevant observation)
- thread_type: free-form classification (e.g., "conversation", "solo_work",
  "media_playback", "ambient", "transition", "phone_call")
- sources: which data sources contribute to this thread
- observations: array of objects with {block_index, obs_index} identifying
  which observations belong to this thread
- relevance: 0.0 to 1.0
- relevance_signals: list of reasons for the score
- metadata: structured context including actor attribution (see below)

THREADING RULES:
1. A time block can participate in MULTIPLE concurrent threads.
2. Each observation belongs to EXACTLY ONE thread. Disambiguate.
3. Trace threads across block boundaries — a meeting spanning
   10:00-10:30 is ONE thread across multiple blocks.
4. Split threads when the context genuinely changes.

ACTOR ATTRIBUTION:
Observations may include actor_hints in their metadata. These are signals
from the capture layer and processors about who is acting. Your job is
to RESOLVE these hints into final attribution using cross-source evidence:

- Fuse signals: if system audio says "unknown_person" and screen shows
  "Sarah Chen" as active speaker in Zoom → resolve to sarah_chen (person).
- Use knowledge corpus: if a name appears in known entities, use that entity ID.
- Resolve ambiguity: mic audio (self, 0.3) + screen shows user typing → self (0.9).
- Detect agents: screen shows Claude Code terminal with AI output → agent (0.8).

In the thread metadata, include:
- "speakers": array of actor identifiers who participated
- "primary_actor": who was mainly driving this activity

RELEVANCE SCORING:
High (0.7-1.0):
  - Multi-source convergence (audio + screen + calendar corroborate)
  - Decision language ("let's do X", "I've decided", "we should")
  - Commitment language ("I'll have it by Friday")
  - References to the person's actual projects, people, goals

Medium (0.3-0.7):
  - Single-source conversation with work content
  - Solo work session with sparse self-talk
  - Thinking aloud about real topics

Low (0.0-0.3):
  - Media playback (TV, movies, podcasts, music)
  - Other people's conversations not involving the user
  - Routine transactions ("large coffee please")
  - Transit with no meaningful conversation

Output ONLY a JSON array of threads. No markdown, no explanation."#;

/// LLM response shape for a single thread.
#[derive(serde::Deserialize)]
struct ThreadRaw {
    id: String,
    label: String,
    start: String,
    end: String,
    thread_type: String,
    sources: Vec<String>,
    observations: Vec<ObsRef>,
    relevance: f32,
    relevance_signals: Vec<String>,
    #[serde(default)]
    metadata: Option<serde_json::Value>,
}

#[derive(serde::Deserialize)]
struct ObsRef {
    block_index: usize,
    obs_index: usize,
}

/// Run Pass 2: identify context threads from time blocks using an LLM.
/// If a knowledge corpus is provided, it's injected into the prompt for better
/// relevance scoring (recognizing known entities, projects, etc.).
pub async fn identify_threads(
    provider: &dyn LlmProvider,
    blocks: &[TimeBlock],
    knowledge: Option<&alvum_knowledge::types::KnowledgeCorpus>,
) -> Result<Vec<ContextThread>> {
    if blocks.is_empty() {
        return Ok(vec![]);
    }

    let formatted = time_block::format_blocks_for_llm(blocks);
    info!(blocks = blocks.len(), formatted_len = formatted.len(), "threading time blocks");

    let mut user_message = String::new();

    // Inject knowledge corpus for better entity recognition and relevance scoring
    if let Some(corpus) = knowledge {
        let summary = corpus.format_for_llm();
        if !summary.is_empty() {
            user_message.push_str(&summary);
            user_message.push_str("\n\n");
        }
    }

    user_message.push_str(&formatted);

    let response = provider
        .complete(THREADING_SYSTEM_PROMPT, &user_message)
        .await
        .context("LLM threading call failed")?;

    let json_str = strip_markdown_fences(&response);
    let raw_threads: Vec<ThreadRaw> = serde_json::from_str(json_str).with_context(|| {
        format!("failed to parse threading response. First 500 chars:\n{}",
            &response[..response.len().min(500)])
    })?;

    // Resolve observation references into actual Observation objects
    let mut threads = Vec::new();
    for raw in raw_threads {
        let mut observations = Vec::new();
        for obs_ref in &raw.observations {
            if let Some(block) = blocks.get(obs_ref.block_index) {
                if let Some(obs) = block.observations.get(obs_ref.obs_index) {
                    observations.push(obs.clone());
                }
            }
        }

        let start = raw.start.parse::<DateTime<Utc>>().unwrap_or_else(|_| {
            observations.first().map(|o| o.ts).unwrap_or_else(Utc::now)
        });
        let end = raw.end.parse::<DateTime<Utc>>().unwrap_or_else(|_| {
            observations.last().map(|o| o.ts).unwrap_or_else(Utc::now)
        });

        threads.push(ContextThread {
            id: raw.id,
            label: raw.label,
            start,
            end,
            sources: raw.sources,
            observations,
            relevance: raw.relevance.clamp(0.0, 1.0),
            relevance_signals: raw.relevance_signals,
            thread_type: raw.thread_type,
            metadata: raw.metadata,
        });
    }

    info!(threads = threads.len(), "identified context threads");
    Ok(threads)
}

/// Full episodic alignment: Pass 1 + Pass 2.
pub async fn align_episodes(
    provider: &dyn LlmProvider,
    observations: &[Observation],
    block_duration: Duration,
    knowledge: Option<&alvum_knowledge::types::KnowledgeCorpus>,
) -> Result<ThreadingResult> {
    // Pass 1: time blocks
    let time_blocks = time_block::assemble_time_blocks(observations, block_duration);
    info!(blocks = time_blocks.len(), "assembled time blocks");

    // Pass 2: context threading
    let threads = identify_threads(provider, &time_blocks, knowledge).await?;

    let mut sources: Vec<String> = observations.iter().map(|o| o.source.clone()).collect();
    sources.sort();
    sources.dedup();

    let start = time_blocks.first().map(|b| b.start).unwrap_or_else(Utc::now);
    let end = time_blocks.last().map(|b| b.end).unwrap_or_else(Utc::now);

    Ok(ThreadingResult {
        start,
        end,
        time_blocks,
        threads,
        observation_count: observations.len(),
        source_count: sources.len(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn threading_prompt_contains_key_instructions() {
        assert!(THREADING_SYSTEM_PROMPT.contains("CONTEXT THREADS"));
        assert!(THREADING_SYSTEM_PROMPT.contains("relevance"));
        assert!(THREADING_SYSTEM_PROMPT.contains("EXACTLY ONE thread"));
        assert!(THREADING_SYSTEM_PROMPT.contains("media_playback"));
    }

    #[test]
    fn threading_prompt_contains_attribution_instructions() {
        assert!(THREADING_SYSTEM_PROMPT.contains("ACTOR ATTRIBUTION"));
        assert!(THREADING_SYSTEM_PROMPT.contains("speakers"));
        assert!(THREADING_SYSTEM_PROMPT.contains("primary_actor"));
        assert!(THREADING_SYSTEM_PROMPT.contains("actor_hints"));
    }
}
