//! Generic level-reduction + cross-correlation primitives.
//!
//! Every upward transition in the distillation tree uses
//! [`distill_level`]: chunked LLM calls with byte-budgeted batches,
//! parse-retry against a strict-JSON prompt on failure, and the same
//! `<wrapper>` defang protection threading already uses.
//!
//! Every cross-correlation pass uses [`correlate_level`]: a single LLM
//! call (or chunked, when the parent count is large) producing
//! `Vec<Edge>` between siblings, with the forward-reference guard from
//! Phase 3.4 enforced uniformly.
//!
//! Concrete level configurations live in sibling modules
//! (`tree::thread`, `tree::cluster`, `tree::domain`, `tree::day`) — each
//! provides a `LevelConfig` + parent struct + `LevelChild`/`LevelParent`
//! impls and delegates the actual work here.

use alvum_core::decision::Edge;
use alvum_core::llm::{LlmProvider, complete_observed};
use alvum_core::pipeline_events::{self as events, Event};
use alvum_core::util::{defang_wrapper_tag, strip_markdown_fences, truncate_chars};
use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use serde::Serialize;
use serde::de::DeserializeOwned;
use std::collections::HashMap;
use tracing::{info, warn};

// ─────────────────────────────────────────────────────────── traits

/// A node feeding INTO an upward reduction. The level primitive only
/// reads `id`, the text it produces for `summary_for_parent`, and the
/// timestamp used to keep batches in chronological order.
pub trait LevelChild: Serialize + Send + Sync {
    fn id(&self) -> &str;
    fn summary_for_parent(&self) -> String;
    fn timestamp(&self) -> DateTime<Utc>;
}

/// A node EMITTED by an upward reduction. The level primitive only
/// touches `id` and `timestamp`; level-specific fields live on the
/// concrete struct.
pub trait LevelParent: DeserializeOwned + Serialize + Send + Sync + Clone {
    fn id(&self) -> &str;
    fn timestamp(&self) -> DateTime<Utc>;
}

// ─────────────────────────────────────────────────────────── configs

/// Per-level distillation configuration. Levels build one of these as a
/// `const` and pass it into [`distill_level`]. The wrapper tag must
/// match the children's domain (e.g. `"observations"` for thread,
/// `"threads"` for cluster, etc.) — the generic primitive uses it for
/// both prompt formatting and breakout defanging.
pub struct LevelConfig {
    /// Stable name surfaced in `pipeline_events` (`"thread"`, `"cluster"`, …).
    pub level_name: &'static str,
    /// System prompt used for the upward distillation call.
    pub system_prompt: &'static str,
    /// System prompt used on parse failure for the single retry attempt.
    pub strict_retry_prompt: &'static str,
    /// XML-style tag wrapping the children block in the user message.
    pub wrapper_tag: &'static str,
    /// Per-batch user-message ceiling. Children are packed into batches
    /// up to this byte count. Default ~100 KB to leave headroom for
    /// system prompt + response within Claude's context.
    pub child_byte_budget: usize,
    /// `pipeline_events` `call_site` prefix. The chunk index is appended
    /// (`thread/chunk_3`, `cluster/chunk_0`).
    pub call_site_prefix: &'static str,
    /// Optional pipeline-generated context blocks prepended to every
    /// batch. These blocks are data, not user instructions.
    pub context_blocks: Vec<LevelContextBlock>,
}

pub struct LevelContextBlock {
    pub tag: &'static str,
    pub body: String,
}

/// Per-level cross-correlation configuration.
pub struct EdgeConfig {
    pub level_name: &'static str,
    pub system_prompt: &'static str,
    pub strict_retry_prompt: &'static str,
    pub wrapper_tag: &'static str,
    pub call_site: &'static str,
    pub context_blocks: Vec<LevelContextBlock>,
}

// ─────────────────────────────────────────────────────────── distill_level

/// Reduce `children` upward into `Vec<P>` parents using the level's
/// configured prompts. Children are batched to fit `child_byte_budget`,
/// each batch is one observed LLM call, parse failures retry once with
/// the strict-JSON prompt before bubbling up.
///
/// The generic primitive owns:
///  - chunking by byte budget
///  - wrapper-tag defang against breakout injection
///  - markdown-fence-tolerant JSON parsing
///  - strict-JSON retry on parse failure (one extra call max)
///  - per-batch `LlmCallStart` / `LlmCallEnd` events via `complete_observed`
///  - parse-failure event emission
///
/// The caller (level-specific module) owns:
///  - the prompt content
///  - the parent type (struct + traits)
///  - any post-merge dedup or ordering across batches
pub async fn distill_level<C, P>(
    children: &[C],
    config: &LevelConfig,
    provider: &dyn LlmProvider,
) -> Result<Vec<P>>
where
    C: LevelChild,
    P: LevelParent,
{
    let no_repair = |_response: &str, _children: &[&C]| Ok(None);
    distill_level_repairing(children, config, provider, &no_repair).await
}

pub async fn distill_level_repairing<C, P, F>(
    children: &[C],
    config: &LevelConfig,
    provider: &dyn LlmProvider,
    repair: &F,
) -> Result<Vec<P>>
where
    C: LevelChild,
    P: LevelParent,
    F: Fn(&str, &[&C]) -> Result<Option<Vec<P>>>,
{
    if children.is_empty() {
        return Ok(Vec::new());
    }

    // Sort children by timestamp so batches honor temporal order. The
    // upper-level prompts assume chronological children — a thread
    // looking at out-of-order time-blocks would be confused.
    let mut sorted: Vec<&C> = children.iter().collect();
    sorted.sort_by_key(|c| c.timestamp());

    // Pack into byte-budget batches. Each child's
    // `summary_for_parent()` plus a small per-child framing overhead
    // counts toward the budget.
    let mut batches: Vec<Vec<&C>> = Vec::new();
    let mut current_batch: Vec<&C> = Vec::new();
    let mut current_size: usize = 0;
    for child in sorted {
        let summary = child.summary_for_parent();
        // 32 bytes of framing overhead per child (id markers + separators).
        let item_size = summary.len() + child.id().len() + 32;
        if !current_batch.is_empty() && current_size + item_size > config.child_byte_budget {
            batches.push(std::mem::take(&mut current_batch));
            current_size = 0;
        }
        current_batch.push(child);
        current_size += item_size;
    }
    if !current_batch.is_empty() {
        batches.push(current_batch);
    }

    info!(
        level = config.level_name,
        child_count = children.len(),
        batch_count = batches.len(),
        budget_bytes = config.child_byte_budget,
        "distill_level: prepared batches"
    );

    let mut all_parents: Vec<P> = Vec::new();
    for (i, batch) in batches.iter().enumerate() {
        let call_site = format!("{}/chunk_{i}", config.call_site_prefix);
        let parents =
            call_one_batch::<C, P, F>(batch, config, provider, &call_site, repair).await?;
        all_parents.extend(parents);
    }

    Ok(all_parents)
}

async fn call_one_batch<C, P, F>(
    children: &[&C],
    config: &LevelConfig,
    provider: &dyn LlmProvider,
    call_site: &str,
    repair: &F,
) -> Result<Vec<P>>
where
    C: LevelChild,
    P: LevelParent,
    F: Fn(&str, &[&C]) -> Result<Option<Vec<P>>>,
{
    let formatted = format_children_for_prompt(children);
    let (safe_formatted, defanged) = defang_wrapper_tag(&formatted, config.wrapper_tag);
    if defanged > 0 {
        events::emit(Event::InputFiltered {
            processor: format!("{}/wrapper-guard", config.level_name),
            file: None,
            kept: formatted.len(),
            dropped: 0,
            reasons: serde_json::json!({"wrapper_breakout_defanged": defanged}),
        });
    }

    let mut user_message = String::new();
    for block in &config.context_blocks {
        let (safe_body, block_defanged) = defang_wrapper_tag(&block.body, block.tag);
        if block_defanged > 0 {
            events::emit(Event::InputFiltered {
                processor: format!("{}/{}-wrapper-guard", config.level_name, block.tag),
                file: None,
                kept: block.body.len(),
                dropped: 0,
                reasons: serde_json::json!({"wrapper_breakout_defanged": block_defanged}),
            });
        }
        user_message.push_str(&format!(
            "<{tag}>\n{body}\n</{tag}>\n\n",
            tag = block.tag,
            body = safe_body,
        ));
    }
    user_message.push_str(&format!(
        "<{tag}>\n{body}\n</{tag}>\n\nReturn the JSON array now.",
        tag = config.wrapper_tag,
        body = safe_formatted,
    ));

    let response = complete_observed(provider, config.system_prompt, &user_message, call_site)
        .await
        .with_context(|| format!("{} batch LLM call failed", config.level_name))?;

    match parse_array_or_repair::<C, P, F>(&response, children, repair) {
        Ok(parents) => Ok(parents),
        Err(first_err) => {
            warn!(
                level = config.level_name,
                preview = %&response[..response.len().min(200)],
                error = %first_err,
                "parse failed; retrying once with strict-JSON prompt"
            );
            events::emit(Event::LlmParseFailed {
                call_site: call_site.to_string(),
                preview: response[..response.len().min(500)].to_string(),
            });

            let retry_call_site = format!("{call_site}/retry");
            let retry_user_message = retry_user_message(&user_message, &first_err);
            let retry_response = complete_observed(
                provider,
                config.strict_retry_prompt,
                &retry_user_message,
                &retry_call_site,
            )
            .await
            .with_context(|| format!("{} retry LLM call failed", config.level_name))?;

            parse_array_or_repair::<C, P, F>(&retry_response, children, repair).with_context(|| {
                format!(
                    "{} parse failed even after retry. First preview: {}\nRetry preview: {}",
                    config.level_name,
                    &response[..response.len().min(200)],
                    &retry_response[..retry_response.len().min(200)],
                )
            })
        }
    }
}

fn parse_array_or_repair<C, P, F>(response: &str, children: &[&C], repair: &F) -> Result<Vec<P>>
where
    C: LevelChild,
    P: LevelParent,
    F: Fn(&str, &[&C]) -> Result<Option<Vec<P>>>,
{
    match parse_array::<P>(response) {
        Ok(parsed) => Ok(parsed),
        Err(parse_error) => {
            if let Some(repaired) = repair(response, children)
                .with_context(|| "failed to repair malformed level response")?
            {
                Ok(repaired)
            } else {
                Err(parse_error)
            }
        }
    }
}

fn retry_user_message(user_message: &str, error: &anyhow::Error) -> String {
    let error_string = error
        .chain()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join("\n");
    let error_text = truncate_chars(&error_string, 1200);
    let (safe_error, _) = defang_wrapper_tag(error_text, "parse_error");
    format!(
        "{user_message}\n\n<parse_error>\n{safe_error}\n</parse_error>\n\nReturn a corrected JSON array now."
    )
}

/// Format children for prompt input. Each child is rendered as its
/// `summary_for_parent` text prefixed with its id; the LLM sees the id
/// as a stable handle for cross-references.
fn format_children_for_prompt<C: LevelChild>(children: &[&C]) -> String {
    let mut s = String::new();
    for c in children {
        s.push_str(&format!("[id:{}]\n", c.id()));
        s.push_str(&c.summary_for_parent());
        s.push_str("\n\n");
    }
    s
}

fn parse_array<P: DeserializeOwned>(response: &str) -> Result<Vec<P>> {
    let json_str = strip_markdown_fences(response);
    serde_json::from_str(json_str).with_context(|| {
        format!(
            "failed to parse level response as JSON array. First 500 chars:\n{}",
            &response[..response.len().min(500)]
        )
    })
}

pub(crate) fn is_level_json_parse_error(error: &anyhow::Error) -> bool {
    error
        .chain()
        .any(|cause| cause.downcast_ref::<serde_json::Error>().is_some())
}

// ─────────────────────────────────────────────────────────── correlate_level

/// Cross-correlate siblings at a single tree level. Single LLM call
/// when `parents.len()` fits the budget; chunked otherwise (with the
/// caveat that cross-batch edges are missed — that's the cost of
/// bounded prompts at large fan-out, which is rare at upper levels).
///
/// Forward-reference guard runs over the result: edges where
/// `from.timestamp > to.timestamp` are dropped with an `InputFiltered`
/// event tagged `forward_reference`. Caller-supplied `relation`
/// vocabulary is otherwise free-form.
pub async fn correlate_level<P>(
    parents: &[P],
    config: &EdgeConfig,
    provider: &dyn LlmProvider,
) -> Result<Vec<Edge>>
where
    P: LevelParent,
{
    if parents.len() <= 1 {
        // Zero or one parent — no edges possible. Skip the LLM call to
        // save round-trip latency on light days.
        return Ok(Vec::new());
    }

    // Build (id → timestamp) lookup for the forward-ref guard.
    let ts_by_id: HashMap<String, DateTime<Utc>> = parents
        .iter()
        .map(|p| (p.id().to_string(), p.timestamp()))
        .collect();

    let formatted = parents
        .iter()
        .map(|p| {
            let json = serde_json::to_string(p).unwrap_or_default();
            format!("[id:{}]\n{}", p.id(), json)
        })
        .collect::<Vec<_>>()
        .join("\n\n");
    let (safe_formatted, defanged) = defang_wrapper_tag(&formatted, config.wrapper_tag);
    if defanged > 0 {
        events::emit(Event::InputFiltered {
            processor: format!("{}/wrapper-guard", config.level_name),
            file: None,
            kept: formatted.len(),
            dropped: 0,
            reasons: serde_json::json!({"wrapper_breakout_defanged": defanged}),
        });
    }

    let mut user_message = String::new();
    for block in &config.context_blocks {
        let (safe_body, block_defanged) = defang_wrapper_tag(&block.body, block.tag);
        if block_defanged > 0 {
            events::emit(Event::InputFiltered {
                processor: format!("{}/{}-wrapper-guard", config.level_name, block.tag),
                file: None,
                kept: block.body.len(),
                dropped: 0,
                reasons: serde_json::json!({"wrapper_breakout_defanged": block_defanged}),
            });
        }
        user_message.push_str(&format!(
            "<{tag}>\n{body}\n</{tag}>\n\n",
            tag = block.tag,
            body = safe_body,
        ));
    }
    user_message.push_str(&format!(
        "<{tag}>\n{body}\n</{tag}>\n\nReturn the JSON array of edges now.",
        tag = config.wrapper_tag,
        body = safe_formatted,
    ));

    let response = complete_observed(
        provider,
        config.system_prompt,
        &user_message,
        config.call_site,
    )
    .await
    .with_context(|| format!("{} correlate LLM call failed", config.level_name))?;

    let mut edges: Vec<Edge> = match parse_array::<Edge>(&response) {
        Ok(v) => v,
        Err(first_err) => {
            warn!(
                level = config.level_name,
                preview = %&response[..response.len().min(200)],
                error = %first_err,
                "edge-parse failed; retrying once"
            );
            events::emit(Event::LlmParseFailed {
                call_site: config.call_site.to_string(),
                preview: response[..response.len().min(500)].to_string(),
            });
            let retry_call_site = format!("{}/retry", config.call_site);
            let retry_response = complete_observed(
                provider,
                config.strict_retry_prompt,
                &user_message,
                &retry_call_site,
            )
            .await
            .with_context(|| format!("{} edge retry call failed", config.level_name))?;
            parse_array::<Edge>(&retry_response).with_context(|| {
                format!(
                    "{} edge parse failed after retry. First preview: {}\nRetry preview: {}",
                    config.level_name,
                    &response[..response.len().min(200)],
                    &retry_response[..retry_response.len().min(200)],
                )
            })?
        }
    };

    // Forward-reference guard. The "alignment_break" / "alignment_honor"
    // / "thematic" relations may legitimately span the same direction —
    // we accept equal timestamps; only strictly-later causes get dropped.
    let before = edges.len();
    edges.retain(|e| {
        let Some(&from_ts) = ts_by_id.get(&e.from_id) else {
            return false; // dangling reference — drop
        };
        let Some(&to_ts) = ts_by_id.get(&e.to_id) else {
            return false;
        };
        from_ts <= to_ts
    });
    let dropped = before - edges.len();
    if dropped > 0 {
        events::emit(Event::InputFiltered {
            processor: format!("{}/correlate", config.level_name),
            file: None,
            kept: edges.len(),
            dropped,
            reasons: serde_json::json!({"forward_reference_or_dangling": dropped}),
        });
    }

    info!(
        level = config.level_name,
        kept = edges.len(),
        dropped,
        "correlate_level: emitted edges"
    );

    Ok(edges)
}

// ─────────────────────────────────────────────────────────── re-exports

pub use alvum_core::decision::{Edge as LevelEdge, EdgeStrength as LevelEdgeStrength};

#[cfg(test)]
mod tests {
    use super::*;
    use alvum_core::decision::EdgeStrength;
    use serde::Deserialize;
    use std::sync::{Mutex, OnceLock};

    /// Same pattern as `alvum-episode/src/threading.rs` — tests that
    /// invoke `complete_observed` (directly or via the level
    /// primitives) must redirect `ALVUM_PIPELINE_EVENTS_FILE` so they
    /// don't pollute the user's real `~/.alvum/runtime/pipeline.events`.
    fn observed_call_guard() -> std::sync::MutexGuard<'static, ()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        let guard = LOCK
            .get_or_init(|| Mutex::new(()))
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let tmp = std::env::temp_dir().join(format!(
            "alvum-test-events-pipeline-{}-{:?}.jsonl",
            std::process::id(),
            std::thread::current().id(),
        ));
        let _ = std::fs::write(&tmp, b"");
        // SAFETY: serialised through the LOCK above. See the matching
        // comment in alvum-episode for the full reasoning.
        unsafe { std::env::set_var("ALVUM_PIPELINE_EVENTS_FILE", tmp) };
        guard
    }

    // Minimal fake child + parent for the primitive tests. Each fake
    // child contributes ~120 bytes of summary text, so the budget tests
    // can predict batch counts.
    #[derive(Serialize)]
    struct FakeChild {
        id: String,
        ts: DateTime<Utc>,
    }
    impl LevelChild for FakeChild {
        fn id(&self) -> &str {
            &self.id
        }
        fn summary_for_parent(&self) -> String {
            format!(
                "child {} — {}",
                self.id,
                "lorem ipsum dolor sit amet, ".repeat(3)
            )
        }
        fn timestamp(&self) -> DateTime<Utc> {
            self.ts
        }
    }

    #[derive(Serialize, Deserialize, Clone)]
    struct FakeParent {
        id: String,
        ts: DateTime<Utc>,
    }
    impl LevelParent for FakeParent {
        fn id(&self) -> &str {
            &self.id
        }
        fn timestamp(&self) -> DateTime<Utc> {
            self.ts
        }
    }

    #[test]
    fn empty_children_returns_empty_without_llm() {
        let _g = observed_call_guard();
        // Tokio-free: this branch returns before any await.
        let rt = tokio::runtime::Runtime::new().unwrap();
        let provider = scripted::ScriptedProvider::new(&[]);
        let cfg = LevelConfig {
            level_name: "test",
            system_prompt: "",
            strict_retry_prompt: "",
            wrapper_tag: "items",
            child_byte_budget: 1000,
            call_site_prefix: "test",
            context_blocks: Vec::new(),
        };
        let result = rt
            .block_on(async { distill_level::<FakeChild, FakeParent>(&[], &cfg, &provider).await });
        assert!(result.is_ok());
        assert!(result.unwrap().is_empty());
    }

    #[test]
    fn correlate_level_skips_llm_for_zero_or_one_parent() {
        let _g = observed_call_guard();
        let rt = tokio::runtime::Runtime::new().unwrap();
        let provider = scripted::ScriptedProvider::new(&[]);
        let cfg = EdgeConfig {
            level_name: "test",
            system_prompt: "",
            strict_retry_prompt: "",
            wrapper_tag: "items",
            call_site: "test/correlate",
            context_blocks: Vec::new(),
        };

        let zero: Vec<FakeParent> = vec![];
        let one = vec![FakeParent {
            id: "a".into(),
            ts: Utc::now(),
        }];

        let r0 = rt.block_on(async { correlate_level::<FakeParent>(&zero, &cfg, &provider).await });
        assert!(r0.unwrap().is_empty());

        let r1 = rt.block_on(async { correlate_level::<FakeParent>(&one, &cfg, &provider).await });
        assert!(r1.unwrap().is_empty());
    }

    #[test]
    fn forward_ref_guard_drops_edges_to_earlier_targets() {
        let _g = observed_call_guard();
        // Direct unit test against the in-memory filter logic by
        // calling correlate_level with a scripted response containing
        // both legal and illegal edges.
        let rt = tokio::runtime::Runtime::new().unwrap();
        let edges_json = r#"[
            {"from_id":"a","to_id":"b","relation":"caused","mechanism":"ok","strength":"primary"},
            {"from_id":"b","to_id":"a","relation":"caused","mechanism":"forward-ref","strength":"primary"}
        ]"#;
        let provider = scripted::ScriptedProvider::new(&[edges_json]);
        let cfg = EdgeConfig {
            level_name: "test",
            system_prompt: "",
            strict_retry_prompt: "",
            wrapper_tag: "items",
            call_site: "test/correlate",
            context_blocks: Vec::new(),
        };
        let parents = vec![
            FakeParent {
                id: "a".into(),
                ts: "2026-04-22T10:00:00Z".parse().unwrap(),
            },
            FakeParent {
                id: "b".into(),
                ts: "2026-04-22T11:00:00Z".parse().unwrap(),
            },
        ];

        let edges = rt
            .block_on(async { correlate_level::<FakeParent>(&parents, &cfg, &provider).await })
            .unwrap();
        assert_eq!(edges.len(), 1, "forward-ref edge should be dropped");
        assert_eq!(edges[0].from_id, "a");
        assert_eq!(edges[0].to_id, "b");
    }

    /// Suppress unused-import warnings on `EdgeStrength` when no test in
    /// this module references it directly. `LevelEdgeStrength` is the
    /// public re-export; `EdgeStrength` is the underlying type.
    #[allow(dead_code)]
    fn _assert_strength_aliases_match() {
        let _: EdgeStrength = LevelEdgeStrength::Primary;
    }

    mod scripted {
        //! Same scripted provider pattern used in
        //! `alvum-episode/src/threading.rs` retry tests. Pulled inline
        //! here so this file is self-contained.

        use super::*;
        use std::sync::Mutex;

        pub struct ScriptedProvider {
            responses: Mutex<Vec<String>>,
        }

        impl ScriptedProvider {
            pub fn new(responses: &[&str]) -> Self {
                Self {
                    responses: Mutex::new(responses.iter().rev().map(|s| s.to_string()).collect()),
                }
            }
        }

        #[async_trait::async_trait]
        impl LlmProvider for ScriptedProvider {
            async fn complete(&self, _system: &str, _user: &str) -> anyhow::Result<String> {
                self.responses
                    .lock()
                    .unwrap()
                    .pop()
                    .ok_or_else(|| anyhow::anyhow!("scripted provider exhausted"))
            }
            fn name(&self) -> &str {
                "scripted"
            }
        }
    }
}
