//! Knowledge corpus: accumulated entities, patterns, and facts.
//!
//! The system's long-term semantic memory. Extracted from observations,
//! fed back into every pipeline stage for context.

pub mod types;
pub mod extract;
pub mod store;
