//! Recursive hierarchical distillation tree.
//!
//! Five-level reduction: observations → time-blocks → threads →
//! clusters → domains → day briefing. Each upper-level transition uses
//! the [`level::distill_level`] generic primitive; cross-correlation
//! passes at L2/L3/L4 use [`level::correlate_level`].
//!
//! See `~/.claude/plans/serene-soaring-oasis.md` for prompts, schemas,
//! and the orchestration outline.

pub mod level;

// Per-level configurations land here as each is built. Keeping them
// behind explicit module declarations rather than a glob so the
// orchestrator (`extract.rs`) imports just the level configs it needs.
//
// pub mod thread;     // L1 → L2 — under construction
// pub mod cluster;    // L2 → L3
// pub mod domain;     // L3 → L4 (emits Decision atoms)
// pub mod day;        // L4 → L5 (gap-narrative briefing)
