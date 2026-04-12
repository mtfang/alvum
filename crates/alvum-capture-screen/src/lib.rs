//! Screen capture daemon: captures active window screenshots on app focus change
//! and idle timer triggers.
//!
//! Captures are intentionally dumb — save PNG files and record DataRefs.
//! Interpretation (vision model) lives in alvum-processor-screen.

pub mod screenshot;
pub mod writer;
