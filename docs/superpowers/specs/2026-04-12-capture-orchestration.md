# Capture Orchestration: Unified Source Management

Single `alvum capture` command that starts all enabled capture sources from config. Replaces the separate `alvum record` and `alvum capture-screen` commands. Config-driven with CLI overrides.

## Problem

Capture sources (audio mic, audio system, screen) each run as separate CLI commands in separate terminals. There's no unified way to start/stop all capture, no config-driven source management, and processor settings (whisper model path, vision mode) must be passed as CLI flags on every `extract` run.

## Architecture

```
alvum capture
    │
    ├── reads [capture.*] from config
    ├── applies --only / --disable overrides
    │
    ▼
Orchestrator
    │
    ├── AudioMicSource      ── impl CaptureSource
    ├── AudioSystemSource   ── impl CaptureSource
    ├── ScreenSource        ── impl CaptureSource
    │   (future: LocationSource, WearableIngestSource, ...)
    │
    ▼  all run concurrently via tokio
    │
    Ctrl-C → shutdown signal → all sources flush and exit
```

## CaptureSource Trait

Defined in `alvum-core`. The contract between the orchestrator and any capture source.

```rust
#[async_trait]
pub trait CaptureSource: Send + Sync {
    /// Unique name matching the config key (e.g., "audio-mic", "screen").
    fn name(&self) -> &str;

    /// Run the capture source. Writes DataRef JSONL and raw files to capture_dir.
    /// Must exit cleanly when shutdown signal fires.
    async fn run(
        &self,
        capture_dir: &Path,
        shutdown: watch::Receiver<bool>,
    ) -> Result<()>;
}
```

Each source owns its subdirectory under `capture_dir`:
- `AudioMicSource` → `capture/{date}/audio/mic/`
- `AudioSystemSource` → `capture/{date}/audio/system/`
- `ScreenSource` → `capture/{date}/screen/`

Sources write `DataRef` JSONL as they capture. The pipeline reads these files later during `extract`. No runtime coupling between capture and processing.

### Shutdown Protocol

The orchestrator creates a `watch::channel<bool>`. Each source receives a clone of the receiver. On Ctrl-C:
1. Orchestrator sends `true` on the watch channel
2. Each source detects the signal and flushes current state (audio encoder flushes segment, screen writer is already per-event)
3. Sources exit their `run()` method
4. Orchestrator awaits all tasks and exits

This is the same pattern the audio recorder already uses internally.

### Future Extensibility

The trait is designed for a hybrid model:
- **Built-in sources** implement the trait directly in Rust (current approach)
- **External sources** would be wrapped by a `ProcessExecutableSource` adapter that spawns a child process, reads its stdout DataRef JSONL, and implements the same trait. Not built now, clean addition later.
- **Electron app integration** calls the Rust orchestrator via sidecar/IPC. Electron spawns the Rust capture loop, manages desktop permissions, and can delegate platform-specific sources to small native helpers that write DataRefs to the same capture directory.

## Config Structure

```toml
# ~/.config/alvum/config.toml

[pipeline]
provider = "cli"
model = "claude-sonnet-4-6"
output_dir = "output"

# ─── Capture sources: always-on daemons started by `alvum capture` ───

[capture.audio-mic]
enabled = true
device = "default"
chunk_duration_secs = 60

[capture.audio-system]
enabled = true
device = "default"

[capture.screen]
enabled = true
idle_interval_secs = 30

# ─── Connectors: one-shot importers used during `alvum extract` ───

[connectors.claude-code]
enabled = true
session_dir = "~/.claude/projects"

# ─── Processors: configure how raw data is interpreted during extract ───

[processors.audio]
whisper_model = "~/.local/share/alvum/models/ggml-base.en.bin"

[processors.screen]
vision = "local"
```

### Config Sections

| Section | Purpose | Used by |
|---|---|---|
| `[capture.*]` | Always-on daemon settings. Each key is a source name. | `alvum capture` |
| `[connectors.*]` | One-shot importer settings. Read existing data from external systems. | `alvum extract` |
| `[processors.*]` | Processing settings (model paths, modes). Defaults for CLI flags. | `alvum extract` |
| `[pipeline]` | LLM provider, model, output directory. | `alvum extract` |

### Migration from Current Config

The current `[connectors.audio]` is recognized and mapped:
- `[connectors.audio]` with `capture_dir` → `[capture.audio-mic]` + `[capture.audio-system]`
- Deprecation warning printed on first load
- Old config still works, new config takes precedence if both exist

## CLI Command

```bash
# Start all enabled capture sources
alvum capture

# Start only specific sources
alvum capture --only audio-mic,screen

# Start all except specific sources
alvum capture --disable audio-system

# Custom capture directory (overrides config)
alvum capture --capture-dir ./capture/2026-04-12
```

Default capture directory: `./capture/<today>` (same as current behavior).

### Removed Commands

| Old Command | Replacement |
|---|---|
| `alvum record` | `alvum capture` (with audio sources enabled in config) |
| `alvum capture-screen` | `alvum capture` (with screen enabled in config) |

The old commands are removed. `alvum capture --only audio-mic,audio-system` is equivalent to the old `alvum record`. `alvum capture --only screen` is equivalent to the old `alvum capture-screen`.

`alvum devices` stays unchanged — still useful for listing audio hardware when configuring `[capture.audio-mic]` device name.

### Extract Reads Processor Config

`alvum extract` gains defaults from `[processors]` config:

```bash
# Before: must specify every time
alvum extract --capture-dir ./capture/2026-04-12 --whisper-model ~/.local/share/alvum/models/ggml-base.en.bin --vision local --output ./output

# After: config provides defaults, CLI overrides when needed
alvum extract --capture-dir ./capture/2026-04-12 --output ./output
```

CLI flags still override config values for ad-hoc use.

## Source Implementations

### AudioMicSource

Wraps the existing `alvum-capture-audio` recorder for mic input only. Reads `device` and `chunk_duration_secs` from config. Writes WAV chunks to `audio/mic/`.

### AudioSystemSource

Same crate, system audio output capture. Reads `device` from config. Writes WAV chunks to `audio/system/`. Gracefully degrades if system audio device not available (warning, continues without it).

### ScreenSource

Wraps the existing `alvum-capture-screen` daemon. Reads `idle_interval_secs` from config. Checks Screen Recording permission at startup (opens System Settings on failure). Writes PNGs + `captures.jsonl` to `screen/`.

### Future Sources (not built now, same trait)

| Source | What it captures | When |
|---|---|---|
| `LocationSource` | CoreLocation significant changes → `location.jsonl` | When macOS native helper is wired into Electron shell |
| `WearableIngestSource` | HTTP endpoint for ESP32 uploads → `audio/wearable/`, `frames/` | When wearable hardware is ready |
| `CalendarSource` | macOS Calendar events → `calendar.jsonl` | When macOS native helper is wired into Electron shell |
| Mobile/watch sources | iPhone mic, camera, health data | Platform-native helper, writes DataRefs to shared capture dir |

## Relation to Original Spec

The original system design spec (2026-04-03) describes the capture daemon as:
- "Spawned on app launch as a background thread"
- Manages audio streams, screen capture triggers, CoreLocation, wearable ingest endpoint
- "Flushes on sleep/quit, resumes on wake"

This spec implements that vision as a CLI-first orchestrator. The differences:

| Original Spec | This Spec | Reason |
|---|---|---|
| External executable protocol (`--describe`, JSONL stdout) | In-process Rust trait | No community ecosystem yet. Trait designed for hybrid later. |
| Electron app spawns daemons | CLI `alvum capture` command | CLI remains the development interface; Electron becomes the long-term desktop shell. |
| All sources in one `alvum-capture` crate | Each source in its own crate | Follows established pattern. Independent compilation, testing, failure isolation. |
| `[connectors.audio]` for everything | `[capture.*]` / `[connectors.*]` / `[processors.*]` split | Reflects actual lifecycle differences between daemons, importers, and processing. |

## Implementation Scope

### Modified crates

**alvum-core:**
- Add `CaptureSource` trait to `src/capture.rs`
- Update `AlvumConfig` for `[capture.*]` and `[processors.*]` sections
- Migration logic for old `[connectors.audio]` format

**alvum-capture-audio:**
- Refactor `Recorder` internals to expose `AudioMicSource` and `AudioSystemSource` implementing `CaptureSource`
- Each source manages its own stream independently (currently bundled)

**alvum-capture-screen:**
- Implement `CaptureSource` for `ScreenSource`
- Accept `idle_interval_secs` from config
- `daemon::run()` becomes the trait's `run()` implementation

**alvum-cli:**
- Replace `Record` + `CaptureScreen` commands with unified `Capture` command
- Add source registry mapping config keys to implementations
- Add orchestrator that spawns sources, manages shutdown
- Update `Extract` to read `[processors]` config for defaults
