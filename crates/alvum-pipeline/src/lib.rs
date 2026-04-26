//! Decision extraction pipeline: recursive hierarchical distillation tree.
//!
//! Takes [`alvum_core::observation::Observation`] values from any connector
//! and walks them up a five-level tree (block → thread → cluster → domain
//! → day) to produce [`alvum_core::decision::Decision`] values with causal
//! edges and a gap-narrative briefing. Uses [`llm::LlmProvider`] for
//! model-agnostic LLM access (Claude CLI, API, Ollama, Bedrock).
//!
//! See `~/.claude/plans/serene-soaring-oasis.md` for the architecture.

pub mod extract;
pub mod llm;
pub mod processed_index;
pub mod processor_runner;
pub mod tree;
pub mod util;

// Progress IPC moved to alvum-core so processor crates can call
// tick_stage without a circular dep. Keep this re-export for any
// in-flight callers that still reference alvum_pipeline::progress.
pub use alvum_core::progress;
