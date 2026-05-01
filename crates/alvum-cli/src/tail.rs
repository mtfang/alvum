use anyhow::{Context, Result};
use std::path::PathBuf;

fn pipeline_events_path() -> Result<PathBuf> {
    if let Some(p) = std::env::var_os("ALVUM_PIPELINE_EVENTS_FILE") {
        return Ok(p.into());
    }
    let home = dirs::home_dir().context("could not resolve $HOME for pipeline events file")?;
    Ok(home.join(".alvum/runtime/pipeline.events"))
}

pub(crate) async fn run(follow: bool, filter: Option<String>) -> Result<()> {
    use tokio::io::{AsyncBufReadExt, AsyncSeekExt, BufReader};

    let path = pipeline_events_path()?;
    if !path.exists() {
        // Touch the parent so a freshly-installed system tails cleanly
        // before the first run has created the file.
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        eprintln!(
            "(no events file yet at {} — start a briefing to populate it)",
            path.display()
        );
        if !follow {
            return Ok(());
        }
    }

    // Read whatever already exists, then optionally watch for appends.
    // Open with tokio so the loop integrates cleanly with `--follow`.
    let mut file = if path.exists() {
        Some(
            tokio::fs::File::open(&path)
                .await
                .with_context(|| format!("failed to open {}", path.display()))?,
        )
    } else {
        None
    };

    if let Some(f) = file.as_mut() {
        let mut reader = BufReader::new(f);
        let mut line = String::new();
        loop {
            line.clear();
            let n = reader.read_line(&mut line).await?;
            if n == 0 {
                break;
            }
            print_event_line(&line, filter.as_deref());
        }
    }

    if !follow {
        return Ok(());
    }

    // Tail loop: poll the file every 250 ms. The events file is
    // truncated at run-start (init()), so we also re-open if the size
    // shrinks below our cursor.
    let mut cursor: u64 = match file.as_mut() {
        Some(f) => f.seek(std::io::SeekFrom::Current(0)).await?,
        None => 0,
    };
    drop(file);

    loop {
        tokio::time::sleep(std::time::Duration::from_millis(250)).await;
        if !path.exists() {
            cursor = 0;
            continue;
        }
        let metadata = tokio::fs::metadata(&path).await?;
        let size = metadata.len();
        if size < cursor {
            // File was truncated (new run started). Reset.
            cursor = 0;
        }
        if size == cursor {
            continue;
        }
        let mut f = tokio::fs::File::open(&path).await?;
        f.seek(std::io::SeekFrom::Start(cursor)).await?;
        let mut reader = BufReader::new(f);
        let mut line = String::new();
        loop {
            line.clear();
            let n = reader.read_line(&mut line).await?;
            if n == 0 {
                break;
            }
            print_event_line(&line, filter.as_deref());
            cursor += n as u64;
        }
    }
}

/// Pretty-print one JSONL line. Falls back to raw output on parse
/// failure — better to see something than nothing while debugging.
fn print_event_line(line: &str, filter: Option<&str>) {
    let trimmed = line.trim_end();
    if trimmed.is_empty() {
        return;
    }
    let value: serde_json::Value = match serde_json::from_str(trimmed) {
        Ok(v) => v,
        Err(_) => {
            println!("{trimmed}");
            return;
        }
    };
    let kind = value.get("kind").and_then(|v| v.as_str()).unwrap_or("");
    if let Some(f) = filter
        && !kind.contains(f)
    {
        return;
    }
    let ts = value
        .get("ts")
        .and_then(|v| v.as_i64())
        .map(format_ts)
        .unwrap_or_else(|| "??:??:??.???".into());

    let detail = format_event_detail(kind, &value);
    println!("[{ts}] {kind:<18} {detail}");
}

fn format_ts(ts_millis: i64) -> String {
    use chrono::TimeZone;
    chrono::Local
        .timestamp_millis_opt(ts_millis)
        .single()
        .map(|dt| dt.format("%H:%M:%S%.3f").to_string())
        .unwrap_or_else(|| "??:??:??.???".into())
}

/// Render the event-specific fields. Stage/LLM events get a compact
/// per-shape summary; everything else falls back to the JSON tail.
fn format_event_detail(kind: &str, value: &serde_json::Value) -> String {
    match kind {
        "stage_enter" => str_field(value, "stage").to_string(),
        "stage_exit" => format!(
            "stage={} elapsed_ms={} ok={} extras={}",
            str_field(value, "stage"),
            value
                .get("elapsed_ms")
                .map(|v| v.to_string())
                .unwrap_or_default(),
            value.get("ok").map(|v| v.to_string()).unwrap_or_default(),
            value
                .get("extras")
                .map(|v| v.to_string())
                .unwrap_or_default(),
        ),
        "input_inventory" => format!(
            "{}/{} ref_count={}",
            str_field(value, "connector"),
            str_field(value, "source"),
            value
                .get("ref_count")
                .map(|v| v.to_string())
                .unwrap_or_default(),
        ),
        "llm_call_start" => format!(
            "provider={} call_site={} prompt_chars={} prompt_tokens≈{}",
            str_field(value, "provider"),
            str_field(value, "call_site"),
            value
                .get("prompt_chars")
                .map(|v| v.to_string())
                .unwrap_or_default(),
            value
                .get("prompt_tokens_estimate")
                .map(|v| v.to_string())
                .unwrap_or_default(),
        ),
        "llm_call_end" => format!(
            "provider={} call_site={} latency_ms={} output_tokens={} output_tokens≈{} tok_sec={} tok_sec≈{} stop_reason={} content_blocks={} attempts={} ok={}",
            str_field(value, "provider"),
            str_field(value, "call_site"),
            value
                .get("latency_ms")
                .map(|v| v.to_string())
                .unwrap_or_default(),
            value
                .get("output_tokens")
                .map(|v| v.to_string())
                .unwrap_or_default(),
            value
                .get("response_tokens_estimate")
                .map(|v| v.to_string())
                .unwrap_or_default(),
            value
                .get("tokens_per_sec")
                .map(|v| v.to_string())
                .unwrap_or_default(),
            value
                .get("tokens_per_sec_estimate")
                .map(|v| v.to_string())
                .unwrap_or_default(),
            str_field(value, "stop_reason"),
            value
                .get("content_block_kinds")
                .and_then(|v| v.as_array())
                .map(|items| {
                    items
                        .iter()
                        .filter_map(|item| item.as_str())
                        .collect::<Vec<_>>()
                        .join("+")
                })
                .unwrap_or_default(),
            value
                .get("attempts")
                .map(|v| v.to_string())
                .unwrap_or_default(),
            value.get("ok").map(|v| v.to_string()).unwrap_or_default(),
        ),
        "llm_parse_failed" => format!(
            "call_site={} preview={:?}",
            str_field(value, "call_site"),
            str_field(value, "preview"),
        ),
        "input_filtered" => format!(
            "processor={} kept={} dropped={} reasons={}",
            str_field(value, "processor"),
            value.get("kept").map(|v| v.to_string()).unwrap_or_default(),
            value
                .get("dropped")
                .map(|v| v.to_string())
                .unwrap_or_default(),
            value
                .get("reasons")
                .map(|v| v.to_string())
                .unwrap_or_default(),
        ),
        "warning" | "error" => format!(
            "{}: {}",
            str_field(value, "source"),
            str_field(value, "message"),
        ),
        _ => value.to_string(),
    }
}

fn str_field<'a>(v: &'a serde_json::Value, key: &str) -> &'a str {
    v.get(key).and_then(|x| x.as_str()).unwrap_or("")
}
