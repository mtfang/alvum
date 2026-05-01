//! Pipeline event channel — richer overlay on top of [`progress`].
//!
//! `progress` answers "how far along is the bar?". This module answers
//! "what is the pipeline doing right now, on what input, with what
//! outcome?". Used by the tray popover live panel and the `alvum tail`
//! CLI to surface stage transitions, input inventory, LLM-call lifecycle,
//! filter counts, warnings, and errors as they happen.
//!
//! Same on-disk pattern as `progress`: append-only JSONL at
//! `~/.alvum/runtime/pipeline.events`, truncated at the top of each run
//! so consumers never confuse the previous run's tail with the current
//! one. Failures are silent — observability is scaffolding, not a
//! correctness contract.

use std::io::Write;

use serde::Serialize;

/// Override the on-disk path. Tests set this; production reads `~/.alvum/...`.
const PATH_ENV: &str = "ALVUM_PIPELINE_EVENTS_FILE";

/// Stage names — kept in sync with [`progress`] so the popover can
/// correlate enter/exit events against the bar's stage ticks.
pub const STAGE_GATHER: &str = "gather";
pub const STAGE_PROCESS: &str = "process";
pub const STAGE_THREAD: &str = "thread";
pub const STAGE_CLUSTER: &str = "cluster";
pub const STAGE_CLUSTER_CORRELATE: &str = "cluster-correlate";
pub const STAGE_DOMAIN: &str = "domain";
pub const STAGE_DOMAIN_CORRELATE: &str = "domain-correlate";
pub const STAGE_DAY: &str = "day";
pub const STAGE_DISTILL: &str = "distill";
pub const STAGE_CAUSAL: &str = "causal";
pub const STAGE_BRIEF: &str = "brief";
pub const STAGE_KNOWLEDGE: &str = "knowledge";

/// One event in the pipeline lifecycle. The `kind` tag is rendered as a
/// snake-case discriminator in the JSONL output so consumers can
/// dispatch on it without parsing the whole shape.
#[derive(Debug, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Event {
    /// A stage of the pipeline began.
    StageEnter { stage: String },
    /// A stage of the pipeline finished. `extras` carries stage-specific
    /// counters (e.g. `process` reports `kept`/`dropped`, `thread`
    /// reports `chunk_count`/`thread_count`). `ok=false` means the
    /// stage exited with an error and downstream stages were skipped.
    StageExit {
        stage: String,
        elapsed_ms: u64,
        ok: bool,
        #[serde(skip_serializing_if = "is_null", default)]
        extras: serde_json::Value,
    },
    /// What a connector found at gather time. One per source. Emitted
    /// even when `ref_count == 0` so silent modalities are visible.
    InputInventory {
        connector: String,
        source: String,
        ref_count: usize,
    },
    /// An LLM call is about to be issued. Paired with `LlmCallEnd`.
    LlmCallStart {
        call_site: String,
        provider: String,
        prompt_chars: usize,
        prompt_tokens_estimate: u64,
    },
    /// An LLM call finished. `attempts` includes any in-provider retries
    /// (transport-level), separate from any pipeline-level retries the
    /// caller may stack on top.
    LlmCallEnd {
        call_site: String,
        provider: String,
        prompt_chars: usize,
        latency_ms: u64,
        response_chars: usize,
        input_tokens: Option<u64>,
        output_tokens: Option<u64>,
        total_tokens: Option<u64>,
        tokens_per_sec: Option<f64>,
        token_source: Option<String>,
        prompt_tokens_estimate: u64,
        response_tokens_estimate: u64,
        total_tokens_estimate: u64,
        tokens_per_sec_estimate: Option<f64>,
        attempts: u32,
        ok: bool,
    },
    /// A response did not parse and the caller is about to retry. Paired
    /// with a subsequent `LlmCallStart` if a retry follows.
    LlmParseFailed {
        call_site: String,
        /// First N chars of the un-parseable response, for debugging.
        preview: String,
    },
    /// A processor filtered some input and emitted observations only for
    /// what survived. `kept`/`dropped` are absolute counts; `reasons`
    /// breaks down `dropped` by category (e.g. `"no_speech_prob"`,
    /// `"low_token_prob"`).
    InputFiltered {
        processor: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        file: Option<String>,
        kept: usize,
        dropped: usize,
        reasons: serde_json::Value,
    },
    /// Soft signal — the run can continue but something unexpected
    /// happened (e.g. a connector found zero refs in its expected
    /// scan path).
    Warning { source: String, message: String },
    /// Hard signal — a stage failed and the run may still recover via
    /// resume, or may abort. Pair with the matching `StageExit { ok: false }`.
    Error { source: String, message: String },
}

/// Always-present envelope around an [`Event`]. `ts` is millis since
/// the Unix epoch; consumers compute relative offsets themselves.
#[derive(Debug, Serialize)]
struct Envelope<'a> {
    ts: i64,
    #[serde(flatten)]
    event: &'a Event,
}

fn events_path() -> Option<std::path::PathBuf> {
    if let Some(p) = std::env::var_os(PATH_ENV) {
        return Some(p.into());
    }
    dirs::home_dir().map(|h| h.join(".alvum/runtime/pipeline.events"))
}

/// Truncate the events file at the start of a run. Mirrors
/// [`progress::init`] — both share the same lifecycle and should be
/// called together.
pub fn init() {
    if let Some(path) = events_path() {
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let _ = std::fs::write(&path, "");
    }
}

/// Append a single event. Best-effort: any IO error is swallowed so a
/// pipeline run is never aborted by an `ENOSPC` on this file.
pub fn emit(event: Event) {
    let _ = emit_inner(&event);
}

fn emit_inner(event: &Event) -> std::io::Result<()> {
    let Some(path) = events_path() else {
        return Ok(());
    };
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let envelope = Envelope {
        ts: chrono::Utc::now().timestamp_millis(),
        event,
    };
    // Compose the line + trailing newline as a single buffer, then issue
    // ONE `write_all` against an `O_APPEND` file. POSIX guarantees
    // append-mode writes ≤ PIPE_BUF (typically 4 KiB on macOS / Linux)
    // are atomic, so concurrent emitters from parallel processors never
    // interleave. `writeln!` issues the body and the newline as two
    // separate writes, which DOES tear under contention — observed in
    // production after the first event-channel rollout.
    let mut payload = serde_json::to_vec(&envelope)?;
    payload.push(b'\n');
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)?;
    f.write_all(&payload)?;
    Ok(())
}

fn is_null(v: &serde_json::Value) -> bool {
    v.is_null()
}

/// RAII timer that emits `StageEnter` on construction and `StageExit` on
/// `finish` (or on drop, in which case `ok` is `false` — i.e. the timer
/// was dropped without an explicit success/failure call, signalling an
/// unwind or early return).
///
/// ```ignore
/// let timer = StageTimer::start(STAGE_THREAD);
/// // ... do the threading work ...
/// timer.finish_ok(serde_json::json!({"chunk_count": 9}));
/// ```
pub struct StageTimer {
    stage: &'static str,
    started: std::time::Instant,
    finished: bool,
}

impl StageTimer {
    /// Begin the stage. Emits `StageEnter` immediately.
    pub fn start(stage: &'static str) -> Self {
        emit(Event::StageEnter {
            stage: stage.to_string(),
        });
        Self {
            stage,
            started: std::time::Instant::now(),
            finished: false,
        }
    }

    /// Mark the stage as completed successfully. `extras` is rendered
    /// inline alongside the standard fields (e.g.
    /// `serde_json::json!({"chunk_count": 9})`).
    pub fn finish_ok(mut self, extras: serde_json::Value) {
        self.emit_exit(true, extras);
        self.finished = true;
    }

    /// Mark the stage as failed. The error message is captured in a
    /// preceding `Error` event by the caller; this one only marks the
    /// timing boundary.
    pub fn finish_err(mut self, extras: serde_json::Value) {
        self.emit_exit(false, extras);
        self.finished = true;
    }

    fn emit_exit(&self, ok: bool, extras: serde_json::Value) {
        emit(Event::StageExit {
            stage: self.stage.to_string(),
            elapsed_ms: self.started.elapsed().as_millis() as u64,
            ok,
            extras,
        });
    }
}

impl Drop for StageTimer {
    fn drop(&mut self) {
        if !self.finished {
            // Implicit drop without finish_ok / finish_err — usually an
            // early return or panic. Record it as a failure so the
            // event log is complete; any preceding `Error` event tells
            // the operator what actually went wrong.
            self.emit_exit(false, serde_json::Value::Null);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::BufRead;
    use std::sync::{Mutex, OnceLock};

    // PATH_ENV mutation is global; cargo test runs cases in parallel by
    // default. Serialise through a single mutex so the env-var override
    // is uncontested for the duration of each test.
    fn lock() -> std::sync::MutexGuard<'static, ()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
            .lock()
            .unwrap_or_else(|e| e.into_inner())
    }

    fn read_events(path: &std::path::Path) -> Vec<serde_json::Value> {
        let f = std::fs::File::open(path).unwrap();
        std::io::BufReader::new(f)
            .lines()
            .filter_map(|l| l.ok())
            .filter(|l| !l.is_empty())
            .map(|l| serde_json::from_str::<serde_json::Value>(&l).unwrap())
            .collect()
    }

    #[test]
    fn emits_jsonl_with_kind_tag() {
        let _g = lock();
        let tmp = tempfile::NamedTempFile::new().unwrap();
        unsafe { std::env::set_var(PATH_ENV, tmp.path()) };
        init();
        emit(Event::StageEnter {
            stage: "thread".into(),
        });
        emit(Event::Warning {
            source: "connector/screen".into(),
            message: "0 refs".into(),
        });

        let events = read_events(tmp.path());
        assert_eq!(events.len(), 2);
        assert_eq!(events[0]["kind"], "stage_enter");
        assert_eq!(events[0]["stage"], "thread");
        assert!(events[0]["ts"].is_number());
        assert_eq!(events[1]["kind"], "warning");
        assert_eq!(events[1]["source"], "connector/screen");
        unsafe { std::env::remove_var(PATH_ENV) };
    }

    #[test]
    fn init_truncates_existing_file() {
        let _g = lock();
        let tmp = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(tmp.path(), "stale\n").unwrap();
        unsafe { std::env::set_var(PATH_ENV, tmp.path()) };
        init();
        assert_eq!(std::fs::read_to_string(tmp.path()).unwrap(), "");
        unsafe { std::env::remove_var(PATH_ENV) };
    }

    #[test]
    fn stage_timer_emits_enter_then_exit_with_elapsed() {
        let _g = lock();
        let tmp = tempfile::NamedTempFile::new().unwrap();
        unsafe { std::env::set_var(PATH_ENV, tmp.path()) };
        init();

        let t = StageTimer::start(STAGE_DISTILL);
        std::thread::sleep(std::time::Duration::from_millis(2));
        t.finish_ok(serde_json::json!({"decisions": 38}));

        let events = read_events(tmp.path());
        assert_eq!(events.len(), 2);
        assert_eq!(events[0]["kind"], "stage_enter");
        assert_eq!(events[0]["stage"], "distill");
        assert_eq!(events[1]["kind"], "stage_exit");
        assert_eq!(events[1]["stage"], "distill");
        assert_eq!(events[1]["ok"], true);
        assert_eq!(events[1]["extras"]["decisions"], 38);
        let elapsed = events[1]["elapsed_ms"].as_u64().unwrap();
        assert!(
            elapsed >= 2,
            "elapsed_ms should reflect the sleep, got {elapsed}"
        );
        unsafe { std::env::remove_var(PATH_ENV) };
    }

    #[test]
    fn stage_timer_drop_without_finish_records_failure() {
        let _g = lock();
        let tmp = tempfile::NamedTempFile::new().unwrap();
        unsafe { std::env::set_var(PATH_ENV, tmp.path()) };
        init();
        {
            let _t = StageTimer::start(STAGE_THREAD);
            // Drop without calling finish_ok/finish_err — simulates
            // an early return or unwind.
        }
        let events = read_events(tmp.path());
        assert_eq!(events.len(), 2);
        assert_eq!(events[1]["kind"], "stage_exit");
        assert_eq!(events[1]["ok"], false);
        unsafe { std::env::remove_var(PATH_ENV) };
    }
}
