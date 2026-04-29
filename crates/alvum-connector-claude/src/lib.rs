//! Claude Code session log connector.
//!
//! Parses Claude Code JSONL conversation logs into [`alvum_core::observation::Observation`]
//! values, filtering out system messages, metadata, and thinking blocks.

pub mod connector;
pub mod parser;
pub use connector::{ClaudeCodeConnector, from_config};
