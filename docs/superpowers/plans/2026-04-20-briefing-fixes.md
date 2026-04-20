# Briefing Pipeline Fixes Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Fix three independent bugs blocking the daily briefing from incorporating screen + audio context, and one silent cron crash.

**Architecture:** Three surgical fixes — no cross-cutting refactor. Each ships independently and is validated by an explicit end-to-end run in Task 4.

**Tech Stack:** Rust, bash scripts, launchd plist, Whisper (ggml via `whisper-rs`), existing `alvum-pipeline`.

---

## Root causes (audited, with file:line citations)

### 1. Whisper model not configured → audio connector silently produces zero observations

- `crates/alvum-connector-audio/src/lib.rs:41–51` reads `processors.audio.whisper_model` via `settings.get("whisper_model")`. When missing, `whisper_model = None`, no error.
- `crates/alvum-connector-audio/src/lib.rs:107–112` — `processors()` returns an **empty `Vec<Box<dyn Processor>>`** if `whisper_model.is_none()`. The connector still reports enabled, but contributes no processor.
- `crates/alvum-connector-audio/src/processor.rs:36–37` — `WhisperProcessor::process` only runs if instantiated; would bail with `"Whisper model not found"` if the file were missing.
- `scripts/install.sh:31–62` writes the default config but never adds `[processors.audio] whisper_model`. No scripts/ download. No model lives under `~/.alvum/`.

**Effect**: today's 07:33 cron log shows `Running connectors: claude-code, audio, screen, codex` but **no audio processor line follows** — audio silently dropped.

### 2. `--resume` reuses transcripts even when the connector set has changed

- `crates/alvum-pipeline/src/extract.rs:46–51` — `if config.resume && transcript_path.exists() { read transcript } else { re-gather }`. No fingerprint check.
- No sidecar metadata. Transcript is raw JSONL of `Observation`; each only carries its own `source` string, no record of the enabled-connector set.
- `scripts/briefing.sh:36` passes `--resume` unconditionally.

**Effect**: yesterday's 07:00 cron built transcript with only `claude-code + codex` (audio/screen not yet enabled in config). User's 21:45 manual rerun reused that transcript → screen/audio never contributed to yesterday's briefing.

### 3. `claude` CLI not on launchd's PATH

- `launchd/com.alvum.briefing.plist:24–28` sets `PATH = /usr/local/bin:/usr/bin:/bin:/opt/homebrew/bin`.
- Actual install location: `/Users/michael/.local/bin/claude` — **not in the PATH above**.
- `crates/alvum-pipeline/src/llm.rs:45` uses `Command::new("claude")` — bare command, PATH-dependent, no fallback.
- `com.alvum.capture.plist` unaffected because it invokes `@@ALVUM_BIN@@` (full path), not `claude`.

**Effect**: today's 07:33 cron `briefing.err` — `failed to spawn claude — is Claude Code installed?` The full pipeline crashed at the threading stage.

---

## Task 1: Whisper model — autodownload + config wiring

**Files:**
- Create: `scripts/download-whisper-model.sh`
- Modify: `scripts/install.sh`
- Modify: `scripts/lib.sh` (add `ALVUM_MODELS_DIR`, extend `ensure_dirs`)
- Modify: `scripts/install.sh` (add `[processors.audio]` section to default config)

- [ ] **Step 1: Add `ALVUM_MODELS_DIR` to lib.sh and ensure_dirs**

In `scripts/lib.sh`, after `export ALVUM_BRIEFINGS_DIR=...`:
```bash
export ALVUM_MODELS_DIR="$ALVUM_RUNTIME/models"
```

And extend `ensure_dirs()` to also create it:
```bash
ensure_dirs() {
  mkdir -p "$ALVUM_RUNTIME/bin" "$ALVUM_RUNTIME/logs" \
           "$ALVUM_CAPTURE" "$ALVUM_BRIEFINGS_DIR" \
           "$ALVUM_MODELS_DIR" \
           "$ALVUM_LAUNCHAGENTS"
}
```

- [ ] **Step 2: Create `scripts/download-whisper-model.sh`**

Exact content:
```bash
#!/usr/bin/env bash
# Download the ggml Whisper model used by the audio connector.
# Default: base.en (~141MB, English, fast on Apple Silicon).
# Override with MODEL=medium.en ./download-whisper-model.sh for better quality
# (~1.5GB, slower). Skips the download if the target file already exists.

set -euo pipefail
source "$(dirname "$0")/lib.sh"

MODEL="${MODEL:-base.en}"
FILE="ggml-$MODEL.bin"
URL="https://huggingface.co/ggerganov/whisper.cpp/resolve/main/$FILE"
DEST="$ALVUM_MODELS_DIR/$FILE"

ensure_dirs

if [[ -f "$DEST" ]]; then
  echo "    $FILE already present ($(du -h "$DEST" | cut -f1))"
  echo "$DEST"
  exit 0
fi

echo "--> downloading $FILE from Hugging Face (one-time, ~$([ "$MODEL" = base.en ] && echo 141MB || echo 1.5GB))"
# -L follows redirects; -f fails on non-2xx instead of writing an HTML error page.
curl -fL --progress-bar -o "$DEST.tmp" "$URL"
mv "$DEST.tmp" "$DEST"
echo "    $DEST"
```

And make it executable:
```bash
chmod +x scripts/download-whisper-model.sh
```

- [ ] **Step 3: Wire the download into install.sh**

Insert after `ensure_dirs` (around line 14) in `scripts/install.sh`:
```bash
# 1b. Fetch Whisper model for the audio connector. Skipped if ALVUM_SKIP_WHISPER=1.
if [[ "${ALVUM_SKIP_WHISPER:-}" != "1" ]]; then
  echo "--> provisioning Whisper model"
  "$ALVUM_REPO/scripts/download-whisper-model.sh"
fi
```

- [ ] **Step 4: Add `whisper_model` to `[connectors.audio]` in the default config**

`AudioConnector::from_config` reads the model path from the `[connectors.audio]` TOML subtree (see `crates/alvum-connector-audio/src/lib.rs:41`), NOT from `[processors.audio]`. (An earlier research pass suggested `[processors.audio]`, but that was misread from unrelated test-fixture code in `config.rs`. Verified against the actual `from_config` reader.)

In `scripts/install.sh`, inside the `cat > "$ALVUM_CONFIG_FILE" <<EOF ... EOF` block, extend the existing `[connectors.audio]` section so it reads:
```
[connectors.audio]
enabled = true
# Path to the ggml Whisper model. Downloaded by scripts/download-whisper-model.sh.
whisper_model = "$ALVUM_MODELS_DIR/ggml-base.en.bin"
```

Do NOT add a `[processors.audio]` section — nothing reads it.

- [ ] **Step 5: Run install.sh on this machine to provision**

Run: `./scripts/install.sh`
Expected: builds + signs binary as before, new log lines:
```
--> provisioning Whisper model
--> downloading ggml-base.en.bin from Hugging Face (...)
```
and `~/.alvum/runtime/models/ggml-base.en.bin` exists (~141MB).

- [ ] **Step 6: Verify config took effect**

Run: `grep -A2 "^\[connectors\.audio\]" ~/.alvum/runtime/config.toml`
Expected:
```
[connectors.audio]
enabled = true
whisper_model = "/Users/michael/.alvum/runtime/models/ggml-base.en.bin"
```

- [ ] **Step 7: Commit**

```bash
git add scripts/lib.sh scripts/download-whisper-model.sh scripts/install.sh
git commit -m "feat(scripts): provision ggml Whisper model on install for audio connector"
```

---

## Task 2: `--resume` transcript fingerprint

**Files:**
- Modify: `crates/alvum-pipeline/src/extract.rs`
- Modify: `crates/alvum-cli/src/main.rs` (pass fingerprint into `ExtractConfig`)

The fix is a **JSON sidecar** alongside `transcript.jsonl` that records which connectors produced it. On resume, we compare the current enabled-connector set against the sidecar; if different, we invalidate the transcript and re-gather.

No cryptographic hash — a plain JSON equality check is easier to debug ("why did it invalidate?") and has no new dependency.

- [ ] **Step 1: Design the sidecar format**

File: `briefings/<date>/transcript.meta.json` (sits next to `transcript.jsonl`).

Content:
```json
{
  "connectors": ["audio", "claude-code", "codex", "screen"]
}
```

A sorted list of enabled connector **names**. That's the minimum signal that prevents the "enabled screen after transcript was built" bug. Deeper config fingerprinting (since dates, session dirs) is out of scope — not the failure mode we've seen.

- [ ] **Step 2: Write the failing test (TDD)**

Append to `crates/alvum-pipeline/src/extract.rs` inside a `#[cfg(test)] mod tests` block at the bottom (add the block if it doesn't exist):

```rust
#[cfg(test)]
mod resume_tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn write_transcript(dir: &std::path::Path, names: &[&str]) {
        // Minimal transcript — just metadata, so loader can read it.
        fs::write(dir.join("transcript.jsonl"), "").unwrap();
        let meta = serde_json::json!({
            "connectors": names.iter().map(|s| s.to_string()).collect::<Vec<_>>()
        });
        fs::write(
            dir.join("transcript.meta.json"),
            serde_json::to_string(&meta).unwrap(),
        ).unwrap();
    }

    #[test]
    fn resume_invalidates_when_connector_set_changed() {
        let tmp = TempDir::new().unwrap();
        write_transcript(tmp.path(), &["claude-code", "codex"]);

        // Current set adds "screen" — sidecar mismatch should invalidate.
        let current: Vec<String> =
            ["audio", "claude-code", "codex", "screen"].iter().map(|s| s.to_string()).collect();

        assert!(
            transcript_fingerprint_matches(tmp.path(), &current).is_err_or_false(),
            "transcript should be invalidated when connector set differs"
        );
    }

    #[test]
    fn resume_reuses_when_connector_set_matches() {
        let tmp = TempDir::new().unwrap();
        write_transcript(tmp.path(), &["audio", "claude-code"]);

        let current: Vec<String> =
            ["claude-code", "audio"].iter().map(|s| s.to_string()).collect();

        assert!(
            transcript_fingerprint_matches(tmp.path(), &current).unwrap_or(false),
            "transcript should be reused when connector set matches (order-insensitive)"
        );
    }

    trait IsErrOrFalse { fn is_err_or_false(self) -> bool; }
    impl IsErrOrFalse for anyhow::Result<bool> {
        fn is_err_or_false(self) -> bool { !self.unwrap_or(true) }
    }
}
```

- [ ] **Step 3: Run tests to verify they fail (function doesn't exist yet)**

Run: `cargo test -p alvum-pipeline resume_tests`
Expected: compile error — `transcript_fingerprint_matches` not found. Good.

- [ ] **Step 4: Implement `transcript_fingerprint_matches`**

In `crates/alvum-pipeline/src/extract.rs`, add near the top-level functions (not inside any impl):

```rust
/// Compare the enabled-connector set used to build an existing transcript
/// against the set that's active NOW. Returns `Ok(true)` if they match
/// (transcript is reusable), `Ok(false)` if they differ (must re-gather),
/// or `Err(_)` if the sidecar is missing or unreadable (be conservative:
/// treat as "can't trust, re-gather").
fn transcript_fingerprint_matches(
    out_dir: &std::path::Path,
    current_connectors: &[String],
) -> anyhow::Result<bool> {
    let meta_path = out_dir.join("transcript.meta.json");
    if !meta_path.exists() {
        return Ok(false);
    }
    let meta: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&meta_path)?)?;
    let stored_names: Vec<String> = meta
        .get("connectors")
        .and_then(|v| v.as_array())
        .map(|arr| arr.iter().filter_map(|v| v.as_str().map(String::from)).collect())
        .unwrap_or_default();

    let mut a = stored_names;
    let mut b: Vec<String> = current_connectors.to_vec();
    a.sort();
    b.sort();
    Ok(a == b)
}

/// Write the fingerprint sidecar next to the transcript. Call AFTER the
/// transcript has been successfully written.
fn write_transcript_fingerprint(
    out_dir: &std::path::Path,
    connectors: &[String],
) -> anyhow::Result<()> {
    let mut names: Vec<String> = connectors.to_vec();
    names.sort();
    let meta = serde_json::json!({ "connectors": names });
    let bytes = serde_json::to_vec_pretty(&meta)?;
    write_atomic(&out_dir.join("transcript.meta.json"), &bytes)
}
```

- [ ] **Step 5: Integrate into the resume guard**

In `crates/alvum-pipeline/src/extract.rs`, locate the current (lines ~45-51):
```rust
let all_observations: Vec<Observation> = if config.resume && transcript_path.exists() {
    // ...reuse...
} else {
    // ...gather...
};
```

Replace with a guard that also validates the fingerprint:
```rust
let current_connector_names: Vec<String> =
    connectors.iter().map(|c| c.name().to_string()).collect();

let resume_ok = config.resume
    && transcript_path.exists()
    && transcript_fingerprint_matches(&config.output_dir, &current_connector_names)
        .unwrap_or(false);

let all_observations: Vec<Observation> = if resume_ok {
    info!("resume: transcript fingerprint matches, reusing");
    storage::read_jsonl(&transcript_path)?
} else {
    if config.resume && transcript_path.exists() {
        warn!("resume: transcript fingerprint mismatch, re-gathering observations");
    }
    // ... existing re-gather loop, unchanged ...
};
```

(Replace the `...` with the current re-gather loop — don't delete it; just wrap the guard.)

- [ ] **Step 6: Write the fingerprint after successful gather**

Immediately after the re-gather block produces `all_observations` and writes `transcript.jsonl`, add:
```rust
write_transcript_fingerprint(&config.output_dir, &current_connector_names)?;
```

Place this right after the `write_jsonl_atomic(&transcript_path, &all_observations)?` call.

- [ ] **Step 7: Run tests**

Run: `cargo test -p alvum-pipeline resume_tests`
Expected: 2 tests pass.

- [ ] **Step 8: Run the full test suite**

Run: `cargo test --workspace`
Expected: all prior tests pass, plus the 2 new ones.

- [ ] **Step 9: Commit**

```bash
git add crates/alvum-pipeline/src/extract.rs
git commit -m "fix(pipeline): --resume invalidates transcript when connector set changes

Previously --resume unconditionally reused briefings/<date>/transcript.jsonl
if present. When a user enabled a new connector between runs (e.g., screen
between 07:00 cron and 21:45 manual rerun), the stale transcript from the
older run silently skipped the new connector's observations.

Adds transcript.meta.json sidecar recording the sorted enabled-connector
names. --resume now invalidates when the stored set differs from the set
active at the current invocation."
```

---

## Task 3: launchd PATH fix for `claude` CLI

**Files:**
- Modify: `launchd/com.alvum.briefing.plist`

The root cause: plist PATH is missing `$HOME/.local/bin` where `claude` actually lives.

Two candidate fixes were considered:

- **A.** Add `$HOME/.local/bin` to the plist's PATH (simplest, one-line).
- **B.** Resolve `claude` via `which::which` in Rust at runtime with a fallback list (robust, but adds a dep and covers a failure mode the plist fix already handles).

Going with A. It's the actual location `which claude` resolves to on your machine, and `install_plist` already substitutes `@@ALVUM_REPO@@`-style tokens — PATH is just a static string.

- [ ] **Step 1: Read the current plist**

Run: `cat launchd/com.alvum.briefing.plist`
Note the current PATH value around lines 24-28.

- [ ] **Step 2: Update the PATH**

In `launchd/com.alvum.briefing.plist`, replace:
```xml
    <key>PATH</key>
    <string>/usr/local/bin:/usr/bin:/bin:/opt/homebrew/bin</string>
```
with:
```xml
    <key>PATH</key>
    <string>@@HOME@@/.local/bin:/usr/local/bin:/usr/bin:/bin:/opt/homebrew/bin</string>
```

- [ ] **Step 3: Add HOME token substitution to lib.sh**

The `install_plist` helper in `scripts/lib.sh` substitutes `@@ALVUM_ROOT@@`, `@@ALVUM_RUNTIME@@`, `@@ALVUM_BIN@@`, `@@ALVUM_REPO@@`. Extend it with `@@HOME@@`:

```bash
install_plist() {
  local src="$1" dst="$2"
  sed -e "s|@@ALVUM_ROOT@@|$ALVUM_ROOT|g" \
      -e "s|@@ALVUM_RUNTIME@@|$ALVUM_RUNTIME|g" \
      -e "s|@@ALVUM_BIN@@|$ALVUM_BIN|g" \
      -e "s|@@ALVUM_REPO@@|$ALVUM_REPO|g" \
      -e "s|@@HOME@@|$HOME|g" \
      "$src" > "$dst"
  launchctl bootout "gui/$UID" "$dst" 2>/dev/null || true
  launchctl bootstrap "gui/$UID" "$dst"
}
```

- [ ] **Step 4: Re-install the briefing plist**

Run: `./scripts/install.sh`
(The install script re-renders the plist and bootstraps it.)

- [ ] **Step 5: Verify the installed plist**

Run: `cat ~/Library/LaunchAgents/com.alvum.briefing.plist | grep -A1 '<key>PATH</key>'`
Expected: contains `/Users/michael/.local/bin` at the front of the PATH.

- [ ] **Step 6: Verify claude resolves from launchd context**

Run: `launchctl print gui/$UID/com.alvum.briefing | grep -A2 environment`
Expected: PATH listing includes `/Users/michael/.local/bin`.

- [ ] **Step 7: Commit**

```bash
git add launchd/com.alvum.briefing.plist scripts/lib.sh
git commit -m "fix(launchd): include ~/.local/bin in briefing plist PATH

Claude CLI installs to ~/.local/bin/claude (per claude.com/download). The
cron plist's PATH previously listed only the canonical system dirs and
Homebrew, so claude was not resolvable and the 07:00 briefing crashed
with 'failed to spawn claude — is Claude Code installed?'.

Also threads @@HOME@@ through lib.sh::install_plist's substitution list."
```

---

## Task 4: End-to-end verification

**Files:** (none — runtime verification)

- [ ] **Step 1: Clean slate the briefing dir for today**

Run:
```bash
rm -rf ~/.alvum/generated/briefings/$(date +%Y-%m-%d)
```

Rationale: avoid any transcript/fingerprint cross-contamination from the current partial state.

- [ ] **Step 2: Run the briefing manually**

Run: `./scripts/briefing.sh`
Expected in the stdout/stderr:
- `Running connectors: audio, claude-code, codex, screen` (all four).
- Log lines `processor produced observations processor="whisper" count=<N>` with N > 0.
- Log lines `processor produced observations processor="ocr" count=<N>` with N > 0.
- No `failed to spawn claude` errors.
- Final `briefing done -> /Users/michael/.alvum/generated/briefings/<today>/briefing.md`.

- [ ] **Step 3: Verify observation source mix**

Run:
```bash
python3 -c "
import json
counts = {}
p = f'/Users/michael/.alvum/generated/briefings/$(date +%Y-%m-%d)/transcript.jsonl'
for line in open(p):
    try: counts[json.loads(line).get('source','?')] = counts.get(json.loads(line).get('source','?'),0)+1
    except: pass
for k,v in sorted(counts.items(), key=lambda x:-x[1]): print(f'  {k}: {v}')
"
```

Expected: four sources (`claude-code`, `codex`, `audio-mic`, `audio-system`, `screen`) each with N > 0.

- [ ] **Step 4: Re-run briefing with no changes, verify resume kicks in**

Run: `./scripts/briefing.sh`
Expected (in stderr): `resume: transcript fingerprint matches, reusing` — full run completes in < 5 seconds rather than re-gathering everything.

- [ ] **Step 5: Toggle a connector off, re-run, verify invalidation**

Run:
```bash
./scripts/capture.sh toggle codex
./scripts/briefing.sh
```
Expected (in stderr): `resume: transcript fingerprint mismatch, re-gathering observations`. Re-enable codex afterwards:
```bash
./scripts/capture.sh toggle codex
```

- [ ] **Step 6: Inspect the briefing content for audio + screen presence**

Run:
```bash
grep -iE "transcrib|voice|spoken|conversation|window|ghostty|terminal|browser" ~/.alvum/generated/briefings/$(date +%Y-%m-%d)/briefing.md | head -10
```

Expected: at least a few lines showing the briefing cited audio or screen content. If zero lines, audio/screen observations are entering the transcript but not surviving into the briefing — file a follow-up (relevance filter is rejecting them or distill isn't weighting them).

---

## Self-review checklist

- [ ] Every step shows the actual code/command, no `TBD`s.
- [ ] Root causes have file:line citations verified against current code.
- [ ] TDD applied where logic is non-trivial (Task 2 fingerprint).
- [ ] Task 3's plist change is wrapped in the existing `@@`-token substitution pattern, not hardcoded to `/Users/michael/`.
- [ ] Task 4 covers both the happy path and the two resume branches (match + mismatch).

## Risks / open questions

1. **Whisper model choice** — `base.en` is the smallest (~141MB) and usable but not great. Default there to ship quickly; leave `MODEL=medium.en scripts/download-whisper-model.sh` as the escape hatch. If briefings feel like they're missing voice nuance, revisit.
2. **Fingerprint granularity** — only compares connector *names*, not their configs. Changing `connectors.claude-code.since` won't invalidate. That's intentional — `briefing.sh` sets `since` on every run anyway, so invalidating on config changes would thrash. If a more specific failure mode emerges (e.g., swapping `vision = "ocr"` for `"local"` silently reuses OCR observations), extend the sidecar to also compare configs.
3. **Task 3 doesn't touch the capture plist** — no change needed; capture.sh runs `@@ALVUM_BIN@@` directly and doesn't spawn `claude`. Verified via the research pass.
