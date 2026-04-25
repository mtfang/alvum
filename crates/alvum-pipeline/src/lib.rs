//! Decision extraction pipeline: distill, link, and brief.
//!
//! Takes [`alvum_core::observation::Observation`] values from any connector and produces
//! [`alvum_core::decision::Decision`] values with causal links and a morning briefing.
//! Uses [`llm::LlmProvider`] for model-agnostic LLM access (Claude CLI, API, or Ollama).

pub mod llm;
pub mod distill;
pub mod causal;
pub mod briefing;
pub mod extract;
pub mod processed_index;
pub mod processor_runner;
pub mod util;
