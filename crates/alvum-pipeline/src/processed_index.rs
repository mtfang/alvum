//! Idempotency sidecar — tracks which DataRefs the pipeline has already
//! processed so re-runs skip work.
//!
//! ## Layout
//!
//! One JSONL line per processed ref, written to `<output_dir>/processed.jsonl`:
//!
//! ```json
//! {"source":"audio-mic","path":"capture/.../mic/22-30-00.wav","size":1234,"mtime_secs":1714000000}
//! ```
//!
//! ## Identity
//!
//! Each entry is keyed by `(source, path, size, mtime_secs)`. We do not hash
//! file contents — touching mtime when content changes (e.g. session JSONL
//! grows) is sufficient to invalidate the entry, and the cost of stating
//! files is bounded.
//!
//! ## Design notes
//!
//! - The index is **append-only**. We never rewrite or compact it; if it
//!   grows unmanageably, the user can delete the file (or pass
//!   `--no-skip-processed`).
//! - Files that fail to stat (deleted, renamed) round-trip as not-contained,
//!   so a re-run that "rediscovers" a missing file will fall through and the
//!   processor will surface its own read error.

use alvum_core::data_ref::DataRef;
use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::path::{Path, PathBuf};

/// On-disk record for a single processed DataRef.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub struct Entry {
    pub source: String,
    pub path: String,
    pub size: u64,
    pub mtime_secs: i64,
}

/// In-memory view of `<output_dir>/processed.jsonl`. Loaded once per run.
pub struct ProcessedIndex {
    file: PathBuf,
    entries: HashSet<Entry>,
}

impl ProcessedIndex {
    /// Load (or create empty) the index at `file`. Tolerates a missing file.
    /// Lines that fail to parse are skipped with a warning — older sidecar
    /// formats won't blow up the pipeline.
    pub fn load(file: PathBuf) -> Result<Self> {
        let mut entries = HashSet::new();
        if file.exists() {
            let text = std::fs::read_to_string(&file)?;
            for line in text.lines() {
                if line.trim().is_empty() {
                    continue;
                }
                match serde_json::from_str::<Entry>(line) {
                    Ok(e) => {
                        entries.insert(e);
                    }
                    Err(e) => {
                        tracing::warn!(error = %e, line, "skipping unparseable processed.jsonl line");
                    }
                }
            }
        }
        Ok(Self { file, entries })
    }

    /// Whether the ref's current on-disk state is recorded.
    /// `false` if the file no longer exists or its size/mtime has changed.
    pub fn contains(&self, dr: &DataRef) -> bool {
        match entry_for_ref(dr) {
            Some(e) => self.entries.contains(&e),
            None => false,
        }
    }

    /// Append the ref's current on-disk state to the index. Idempotent —
    /// duplicates are skipped without rewriting the file. Files that no
    /// longer exist (e.g., the processor moved them) are silently skipped.
    pub fn record(&mut self, dr: &DataRef) -> Result<()> {
        let Some(entry) = entry_for_ref(dr) else {
            return Ok(());
        };
        if self.entries.insert(entry.clone()) {
            self.append(&entry)?;
        }
        Ok(())
    }

    fn append(&self, entry: &Entry) -> Result<()> {
        use std::io::Write;
        if let Some(parent) = self.file.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let mut f = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.file)?;
        writeln!(f, "{}", serde_json::to_string(entry)?)?;
        Ok(())
    }
}

/// Snapshot the file at `dr.path` into an `Entry`. Returns `None` if the file
/// no longer exists, can't be stat'd, or has no modified time.
fn entry_for_ref(dr: &DataRef) -> Option<Entry> {
    let p = Path::new(&dr.path);
    let meta = std::fs::metadata(p).ok()?;
    let mtime = meta
        .modified()
        .ok()?
        .duration_since(std::time::UNIX_EPOCH)
        .ok()?
        .as_secs() as i64;
    Some(Entry {
        source: dr.source.clone(),
        path: dr.path.clone(),
        size: meta.len(),
        mtime_secs: mtime,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use std::io::Write;

    #[test]
    fn round_trips_entries() {
        let tmp = tempfile::tempdir().unwrap();
        let probe = tmp.path().join("probe.txt");
        std::fs::write(&probe, "hello").unwrap();

        let index_path = tmp.path().join("processed.jsonl");
        let mut idx = ProcessedIndex::load(index_path.clone()).unwrap();

        let dr = DataRef {
            ts: Utc::now(),
            source: "test".into(),
            path: probe.to_string_lossy().to_string(),
            mime: "text/plain".into(),
            metadata: None,
        };
        assert!(!idx.contains(&dr));
        idx.record(&dr).unwrap();
        assert!(idx.contains(&dr));

        // New instance reads from disk and sees the same entry.
        let idx2 = ProcessedIndex::load(index_path).unwrap();
        assert!(idx2.contains(&dr));
    }

    #[test]
    fn change_in_size_invalidates_entry() {
        let tmp = tempfile::tempdir().unwrap();
        let probe = tmp.path().join("growing.txt");
        std::fs::write(&probe, "v1").unwrap();

        let mut idx = ProcessedIndex::load(tmp.path().join("p.jsonl")).unwrap();
        let dr = DataRef {
            ts: Utc::now(),
            source: "test".into(),
            path: probe.to_string_lossy().into(),
            mime: "text/plain".into(),
            metadata: None,
        };
        idx.record(&dr).unwrap();
        assert!(idx.contains(&dr));

        // Append → mtime + size differ → not contained.
        // Sleep one second so the mtime resolution captures the change on
        // filesystems with second-granularity timestamps.
        std::thread::sleep(std::time::Duration::from_millis(1100));
        let mut f = std::fs::OpenOptions::new().append(true).open(&probe).unwrap();
        writeln!(f, "more bytes").unwrap();

        assert!(!idx.contains(&dr));
    }

    #[test]
    fn missing_file_not_contained() {
        let tmp = tempfile::tempdir().unwrap();
        let idx = ProcessedIndex::load(tmp.path().join("p.jsonl")).unwrap();
        let dr = DataRef {
            ts: Utc::now(),
            source: "test".into(),
            path: tmp.path().join("does-not-exist.bin").to_string_lossy().into(),
            mime: "application/octet-stream".into(),
            metadata: None,
        };
        assert!(!idx.contains(&dr));
    }

    #[test]
    fn duplicate_record_is_idempotent() {
        let tmp = tempfile::tempdir().unwrap();
        let probe = tmp.path().join("probe.txt");
        std::fs::write(&probe, "hello").unwrap();

        let index_path = tmp.path().join("processed.jsonl");
        let mut idx = ProcessedIndex::load(index_path.clone()).unwrap();
        let dr = DataRef {
            ts: Utc::now(),
            source: "test".into(),
            path: probe.to_string_lossy().to_string(),
            mime: "text/plain".into(),
            metadata: None,
        };
        idx.record(&dr).unwrap();
        idx.record(&dr).unwrap();
        idx.record(&dr).unwrap();

        let body = std::fs::read_to_string(&index_path).unwrap();
        let line_count = body.lines().filter(|l| !l.trim().is_empty()).count();
        assert_eq!(line_count, 1, "expected exactly one persisted line");
    }
}
