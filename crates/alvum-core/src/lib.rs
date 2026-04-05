//! Core domain types for alvum: observations, decisions, causal links, and storage primitives.
//!
//! Every connector produces [`observation::Observation`] values. The pipeline transforms
//! them into [`decision::Decision`] values with causal links and actor attributions.
//! [`storage`] provides JSONL persistence.

pub mod observation;
pub mod decision;
pub mod storage;
