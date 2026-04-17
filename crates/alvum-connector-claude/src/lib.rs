//! Claude Code session log connector.
//!
//! Parses Claude Code JSONL conversation logs into [`alvum_core::observation::Observation`]
//! values, filtering out system messages, metadata, and thinking blocks.

pub mod parser;
pub mod connector;
pub use connector::ClaudeCodeConnector;
