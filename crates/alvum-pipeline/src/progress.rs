//! Briefing progress IPC. The pipeline appends JSON lines to
//! `~/.alvum/runtime/briefing.progress`; the running Alvum.app polls
//! it and surfaces the stage + ASCII progress bar in the tray menu.
//!
//! Same single-file-queue pattern as the notification queue
//! (`alvum_notify` / `notify.queue`) so the IPC story stays uniform:
//! out-of-process producer appends, in-process consumer polls and
//! truncates implicitly via byte-cursor.
//!
//! Failures are silent — progress is observability scaffolding, not a
//! correctness contract. A pipeline run must not be aborted by an
//! ENOSPC on the tracking file.

use std::io::Write;

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
/// never confuses last-run leftovers with current progress.
pub fn init() {
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

    #[test]
    fn report_appends_jsonl_line() {
        let tmp = tempfile::NamedTempFile::new().unwrap();
        // SAFETY: single-threaded test; env-var mutation is contained.
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
        let tmp = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(tmp.path(), "stale data\n").unwrap();
        unsafe { std::env::set_var(PATH_ENV, tmp.path()) };
        init();
        assert_eq!(std::fs::read_to_string(tmp.path()).unwrap(), "");
        unsafe { std::env::remove_var(PATH_ENV) };
    }
}
