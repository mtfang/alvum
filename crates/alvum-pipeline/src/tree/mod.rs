//! Recursive hierarchical distillation tree.
//!
//! Five-level reduction: observations → time-blocks → threads →
//! clusters → domains → day briefing. Each upper-level transition uses
//! the [`level::distill_level`] generic primitive; cross-correlation
//! passes at L2/L3/L4 use [`level::correlate_level`].
//!
//! See `~/.claude/plans/serene-soaring-oasis.md` for prompts, schemas,
//! and the orchestration outline.

pub mod blocks;
pub mod cluster;
pub mod day;
pub mod domain;
pub mod level;
pub mod thread;
