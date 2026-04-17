//! Core domain types for alvum: data references, artifacts, observations, decisions,
//! and storage primitives.
//!
//! Data flows through three layers:
//! - [`data_ref::DataRef`] — what connectors produce (file pointers)
//! - [`artifact::Artifact`] — what processors produce (typed output layers)
//! - [`observation::Observation`] — what the pipeline consumes (text for LLM reasoning)

pub mod artifact;
pub mod capture;
pub mod config;
pub mod connector;
pub mod data_ref;
pub mod decision;
pub mod llm;
pub mod observation;
pub mod processor;
pub mod storage;
pub mod util;
