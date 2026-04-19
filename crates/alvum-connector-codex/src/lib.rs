//! OpenAI Codex CLI session log connector.
//!
//! Parses Codex session rollout JSONL files — typically found at
//! `~/.codex/sessions/YYYY/MM/DD/rollout-*.jsonl` — into
//! [`alvum_core::observation::Observation`] values. Extracts user and assistant
//! message turns from `response_item` records, skipping system/developer prompts,
//! reasoning-only blocks, and non-message event types.

pub mod parser;
pub mod connector;
pub use connector::CodexConnector;
