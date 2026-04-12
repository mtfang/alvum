//! Episodic alignment: time blocks + context threads.
//!
//! Two-pass system that groups observations from all sources into time-aligned
//! blocks (Pass 1), then traces coherent context threads across blocks and
//! scores relevance (Pass 2). The pipeline extracts decisions only from
//! high-relevance threads.

pub mod types;
pub mod time_block;
pub mod threading;
