//! Briefing progress IPC. The pipeline appends JSON lines to
//! `~/.alvum/runtime/briefing.progress`; the running Alvum.app polls
//! it and surfaces the stage + progress bar in the tray popover.
//!
//! Lives in alvum-core so parallel processors (Whisper, vision) can
//! call `tick_stage` directly to advance a shared atomic counter —
//! the only way to get per-file granularity for the `process` stage
//! without threading a context object through every Processor::process
//! call.
//!
//! Failures are silent — progress is observability scaffolding, not a
//! correctness contract. A pipeline run must not be aborted by an
//! ENOSPC on the tracking file.

use std::io::Write;
use std::sync::atomic::{AtomicUsize, Ordering};

/// Override the on-disk path. Tests set this; production reads `~/.alvum/...`.
const PATH_ENV: &str = "ALVUM_PROGRESS_FILE";

/// Stage names — must match the JS-side ordering exactly so the tray
/// renders the checklist in pipeline order regardless of which line
/// arrives last.
pub const STAGE_GATHER: &str = "gather";
pub const STAGE_PROCESS: &str = "process";
pub const STAGE_THREAD: &str = "thread";
pub const STAGE_DISTILL: &str = "distill";
pub const STAGE_CAUSAL: &str = "causal";
pub const STAGE_BRIEF: &str = "brief";

fn progress_path() -> Option<std::path::PathBuf> {
    if let Some(p) = std::env::var_os(PATH_ENV) {
        return Some(p.into());
    }
    dirs::home_dir().map(|h| h.join(".alvum/runtime/briefing.progress"))
}

/// Truncate the progress file at the start of a run so the consumer
/// never confuses last-run leftovers with current progress. Also
/// resets the shared atomic counters used by `tick_stage`.
pub fn init() {
    STAGE_TOTAL.store(0, Ordering::SeqCst);
    STAGE_DONE.store(0, Ordering::SeqCst);
    if let Some(path) = progress_path() {
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let _ = std::fs::write(&path, "");
    }
}

/// Append a single progress event. Best-effort: any IO error is swallowed.
pub fn report(stage: &str, current: usize, total: usize) {
    let _ = report_inner(stage, current, total);
}

// === Shared atomic counters ============================================
//
// Parallel work — Whisper across N audio files concurrently with vision
// across M screenshots, all under the `process` stage — needs a single
// monotonically-increasing counter so the bar reflects total work
// completed, not whichever processor reported last. The pipeline calls
// `set_stage_total` before fanning out; each worker calls `tick_stage`
// after one unit of work; the counter increments atomically and emits a
// fresh JSON line on every tick.
static STAGE_TOTAL: AtomicUsize = AtomicUsize::new(0);
static STAGE_DONE: AtomicUsize = AtomicUsize::new(0);

/// Set the stage's total work count, reset done counter, emit the
/// initial 0/total tick. Call once before any parallel `tick_stage`
/// calls for that stage.
pub fn set_stage_total(stage: &str, total: usize) {
    STAGE_TOTAL.store(total, Ordering::SeqCst);
    STAGE_DONE.store(0, Ordering::SeqCst);
    report(stage, 0, total);
}

/// Atomically advance the stage counter and emit a tick. Safe to call
/// from any thread or async task; the increment is SeqCst-ordered so
/// concurrent calls always produce two distinct (current, total) pairs.
/// No-op when total is 0 (`set_stage_total` hasn't been called).
pub fn tick_stage(stage: &str) {
    let total = STAGE_TOTAL.load(Ordering::SeqCst);
    if total == 0 {
        return;
    }
    let done = STAGE_DONE.fetch_add(1, Ordering::SeqCst) + 1;
    report(stage, done.min(total), total);
}

fn report_inner(stage: &str, current: usize, total: usize) -> std::io::Result<()> {
    let Some(path) = progress_path() else {
        return Ok(());
    };
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let line = format!(
        r#"{{"ts":{},"stage":"{}","current":{},"total":{}}}"#,
        chrono::Utc::now().timestamp_millis(),
        stage,
        current,
        total,
    );
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)?;
    writeln!(f, "{line}")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::BufRead;
    use std::sync::{Mutex, OnceLock};

    // The PATH_ENV mutation isn't thread-safe; cargo test runs cases in
    // parallel by default. Serialize them through a single mutex so the
    // env-var override is uncontested for the duration of each test.
    fn lock() -> std::sync::MutexGuard<'static, ()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
            .lock()
            .unwrap_or_else(|e| e.into_inner())
    }

    #[test]
    fn report_appends_jsonl_line() {
        let _g = lock();
        let tmp = tempfile::NamedTempFile::new().unwrap();
        unsafe { std::env::set_var(PATH_ENV, tmp.path()) };
        init();
        report(STAGE_THREAD, 3, 8);
        report(STAGE_THREAD, 4, 8);

        let f = std::fs::File::open(tmp.path()).unwrap();
        let lines: Vec<_> = std::io::BufReader::new(f)
            .lines()
            .filter_map(|l| l.ok())
            .filter(|l| !l.is_empty())
            .collect();
        assert_eq!(lines.len(), 2);
        assert!(lines[0].contains(r#""stage":"thread""#));
        assert!(lines[0].contains(r#""current":3"#));
        assert!(lines[0].contains(r#""total":8"#));
        assert!(lines[1].contains(r#""current":4"#));

        unsafe { std::env::remove_var(PATH_ENV) };
    }

    #[test]
    fn init_truncates_existing_file() {
        let _g = lock();
        let tmp = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(tmp.path(), "stale data\n").unwrap();
        unsafe { std::env::set_var(PATH_ENV, tmp.path()) };
        init();
        assert_eq!(std::fs::read_to_string(tmp.path()).unwrap(), "");
        unsafe { std::env::remove_var(PATH_ENV) };
    }

    #[test]
    fn tick_stage_advances_atomic_counter() {
        let _g = lock();
        let tmp = tempfile::NamedTempFile::new().unwrap();
        unsafe { std::env::set_var(PATH_ENV, tmp.path()) };
        init();
        set_stage_total(STAGE_PROCESS, 3);
        tick_stage(STAGE_PROCESS);
        tick_stage(STAGE_PROCESS);
        tick_stage(STAGE_PROCESS);

        let body = std::fs::read_to_string(tmp.path()).unwrap();
        let lines: Vec<&str> = body.lines().filter(|l| !l.is_empty()).collect();
        // 1 set_stage_total event + 3 ticks = 4 lines, current 0..=3.
        assert_eq!(lines.len(), 4);
        assert!(lines[0].contains(r#""current":0"#));
        assert!(lines[1].contains(r#""current":1"#));
        assert!(lines[2].contains(r#""current":2"#));
        assert!(lines[3].contains(r#""current":3"#));
        unsafe { std::env::remove_var(PATH_ENV) };
    }

    #[test]
    fn tick_stage_clamps_at_total() {
        let _g = lock();
        let tmp = tempfile::NamedTempFile::new().unwrap();
        unsafe { std::env::set_var(PATH_ENV, tmp.path()) };
        init();
        set_stage_total(STAGE_PROCESS, 2);
        tick_stage(STAGE_PROCESS);
        tick_stage(STAGE_PROCESS);
        tick_stage(STAGE_PROCESS); // overshoot — must clamp to total

        let body = std::fs::read_to_string(tmp.path()).unwrap();
        let lines: Vec<&str> = body.lines().filter(|l| !l.is_empty()).collect();
        // Last line's current should still equal total (2), not 3.
        assert!(lines.last().unwrap().contains(r#""current":2"#));
        unsafe { std::env::remove_var(PATH_ENV) };
    }
}
