# Screen Capture: Screenshot + Vision Model

macOS screen capture via screenshots + vision model interpretation. The capture layer is intentionally dumb (save images), the processor layer is smart (vision model describes content). This avoids the fragility of accessibility tree parsing and bets on vision models improving over time.

## Problem

Without screen data, episodic alignment has only audio — the "cross" in cross-source threading is hollow. The pipeline can't distinguish TV audio from a real meeting because it can't see that Zoom is on screen. Screen capture provides the visual context that makes relevance scoring confident.

## Architecture

Two new crates, same pattern as audio:

```
alvum-capture-screen              alvum-processor-screen
(dumb capture)                    (smart interpretation)

Triggers:                         Reads DataRefs (screenshots)
  App focus change                Calls vision model → text description
  30s idle timer                  Produces Observations
                                  Future: multimodal embeddings
Screenshot active window
Record app name + window title
Save to disk as DataRef
```

The capture daemon runs as a separate process (`alvum capture-screen`), independent of audio recording. Both can crash without taking the other down. The Electron desktop shell spawns both eventually.

## Capture Crate: `alvum-capture-screen`

### Triggers

Two triggers for MVP:

| Trigger | API | Behavior |
|---|---|---|
| App/window focus change | `NSWorkspaceDidActivateApplicationNotification` via `objc2` | Fires immediately on switch |
| Idle timer | `tokio::time::interval` (30s) | Resets after each app switch trigger |

The idle timer reset prevents double-capture: if an app switch fires at T=0, the next idle fires at T+30s, not T+0s.

### Screenshot

Uses `screencapturekit` crate (ScreenCaptureKit bindings) to capture the **active window only**, not the full display:
- Smaller files (just the app content)
- No desktop clutter or overlapping windows
- Privacy-friendlier (only captures what's actively used)

**Format:** PNG for MVP. Lossless, vision models handle natively, no extra dependency. WebP optimization later when storage matters.

**macOS permission required:** Screen Recording (ScreenCaptureKit). Requested at first launch.

### Metadata

On each trigger, capture also records app name and window title from `NSWorkspace.shared.frontmostApplication`. This is a trivial API call — no accessibility tree walking.

### Output

```
capture/2026-04-12/screen/
├── captures.jsonl              one DataRef per screenshot
└── images/
    ├── 09-00-15.png
    ├── 09-00-45.png
    └── ...
```

Each line in `captures.jsonl` is a `DataRef`:
```jsonl
{"ts":"2026-04-12T09:00:15Z","source":"screen","path":"screen/images/09-00-15.png","mime":"image/png","metadata":{"app":"VS Code","window":"main.rs","trigger":"app_focus"}}
{"ts":"2026-04-12T09:00:45Z","source":"screen","path":"screen/images/09-00-45.png","mime":"image/png","metadata":{"app":"VS Code","window":"main.rs","trigger":"idle"}}
```

### CLI Command

```bash
alvum capture-screen --capture-dir ./capture/2026-04-12
```

Runs until Ctrl-C. Same pattern as `alvum record` for audio.

### Resource Budget

Target: <5% CPU, <50MB RAM. The daemon is idle most of the time — it wakes on NSWorkspace notifications or the 30s timer, captures one screenshot, writes two files, and sleeps.

**Storage:** ~200-300 screenshots/day at ~100-300KB each (PNG, single window) = ~30-90MB/day.

## Processor Crate: `alvum-processor-screen`

### Vision Mode (configurable)

The processor supports multiple strategies, selected via `--vision` flag:

| Mode | What it does | Cost | Quality |
|---|---|---|---|
| `local` | Ollama vision model (llava, llama3.2-vision) | Free | Good — understands scenes, identifies speakers |
| `api` | Anthropic API vision (Sonnet/Opus) | ~$0.01-0.02/image | Best — highest accuracy for attribution |
| `ocr` | macOS Vision framework text extraction | Free | Text only — no scene understanding |
| `off` | Skip processing, save screenshots for later | Free | None — raw files only |

**Default: `local`** — zero cost, good quality. Users without Ollama installed fall back to `ocr` with a warning.

**OCR is the fallback, not the primary.** A vision model understands "Zoom meeting with Sarah as active speaker." OCR dumps all visible text without context. The model output is what makes cross-source threading and actor attribution work. OCR is the degraded mode when no model is available.

### Core Function

```rust
pub async fn process_screen_data_refs(
    provider: &dyn LlmProvider,
    data_refs: &[DataRef],
    vision_mode: VisionMode,
) -> Result<Vec<Observation>>
```

Where `VisionMode` is:
```rust
pub enum VisionMode {
    Local,   // Ollama vision model
    Api,     // Anthropic API vision
    Ocr,     // macOS Vision OCR only
    Off,     // Skip processing
}
```

### Vision Model Path (local or api)

For each screenshot DataRef:
1. Read the image file
2. Send to vision model with description prompt
3. Produce an Observation with the model's scene description

Vision prompt:
```
Describe what is on this screen in 1-3 sentences. Focus on:
- What application is shown and what the user appears to be doing
- Any visible content that indicates work activity (documents, code, messages, forms)
- Any notable state (errors, notifications, loading states)
- If this is a video call, identify the active speaker if visible

Do NOT describe UI chrome (toolbars, menubars, scroll bars).
Be specific about content visible on screen.
```

### OCR Fallback Path

For each screenshot DataRef:
1. Read the image file
2. Call macOS Vision framework `VNRecognizeTextRequest` (via objc2 bindings)
3. Produce an Observation with extracted text as content

OCR output is less structured — just the visible text. The threading LLM still gets useful signal (app name from metadata + visible text content), but no scene understanding or speaker identification.

### Output

One Observation per screenshot regardless of mode:
```rust
Observation {
    ts: capture_timestamp,
    source: "screen",
    kind: "screen_capture",
    content: "VS Code showing main.rs with a Rust function. Terminal panel
              open with cargo test output showing 13 passing tests.",
    metadata: Some(json!({
        "app": "VS Code",
        "window": "main.rs",
        "trigger": "idle",
        "vision_mode": "local"
    })),
    media_ref: Some(MediaRef {
        path: "screen/images/09-00-15.png",
        mime: "image/png"
    }),
}
```

### Parallel Processing

Screenshots are independent — process them in parallel overnight. Use `tokio::spawn` with a concurrency limiter (e.g., 10 concurrent vision calls for model modes, unlimited for OCR).

### Cost

- **local mode:** Free. Ollama runs on Apple Silicon, ~2-5s per image.
- **api mode:** ~200-300 screenshots/day at ~$0.01-0.02 per vision call = ~$2-6/day.
- **ocr mode:** Free. macOS Vision framework, instant.

## LlmProvider Extension

The existing `LlmProvider` trait gains an image method:

```rust
#[async_trait::async_trait]
pub trait LlmProvider: Send + Sync {
    async fn complete(&self, system: &str, user_message: &str) -> Result<String>;
    
    /// Complete with an image attachment. Default falls back to text-only.
    async fn complete_with_image(
        &self,
        system: &str,
        user_message: &str,
        image_path: &Path,
    ) -> Result<String> {
        // Default: ignore image, fall back to text-only
        self.complete(system, user_message).await
    }
    
    fn name(&self) -> &str;
}
```

Provider implementations:
- **OllamaProvider:** Primary vision provider. Ollama's `/api/generate` accepts base64 images via the `images` field for multimodal models (llava, llama3.2-vision). Free, local, default.
- **AnthropicApiProvider:** Premium vision provider. API's image content block (base64-encoded PNG in the messages array). Paid, highest quality.
- **ClaudeCliProvider:** Falls back to text-only (Claude CLI doesn't support image input in `-p` mode).

## Pipeline Integration

The CLI's cross-source extract mode gains screen data scanning. When `alvum extract` runs without `--source`:

1. Scan `audio/` subdirs → transcribe with Whisper → audio Observations
2. **Scan `screen/captures.jsonl` → describe with vision model → screen Observations**
3. Merge all Observations by timestamp
4. Pass 1: time blocks (audio + screen in same 5-min windows)
5. Pass 2: LLM threading (sees "VS Code on screen" + "talking about migration" = same session)
6. Filter by relevance → extract decisions → link → brief → knowledge

No changes to episodic alignment, decision extraction, causal linking, or briefing. They all operate on Observation objects regardless of source.

### Threading Payoff

The threading LLM now sees multi-source blocks:
```
=== Block 10:00-10:05 ===
[10:00:15] [audio-mic/speech] "I think we should defer the migration"
[10:00:15] [screen/screen_capture] Zoom showing "Sprint Planning" meeting window
[10:01:00] [screen/screen_capture] Linear showing INGEST-342 issue detail
[10:03:22] [audio-mic/speech] "Yeah let's push to August"
[10:04:10] [screen/screen_capture] Linear showing status field now reads "Backlog"
```

Multi-source convergence → high relevance → confident decision extraction.

## Evolution Path: Multimodal Embeddings

Today's architecture: screenshot → vision model → text description → text embedding (future).

Tomorrow's architecture: screenshot → multimodal embedding model directly → vector in shared text+image space.

Models like Gemini Embedding support this today — a single embedding model that takes text OR images and places them in the same vector space. When this is integrated:
- No vision-model-to-text step needed for search/retrieval
- Screenshots become directly searchable: "find the screen where I was editing the migration" → vector similarity against screenshot embeddings
- The `Artifact` layers HashMap supports this: `"text"` layer (description) + `"embedding"` layer (multimodal vector)

The processor interface stays the same (`DataRef` in, `Observation` + `Artifact` out). The internals swap from "call vision model for text" to "call embedding model for vector" — or both.

The `media_ref` field on every Observation already points to the raw image. Any future embedding pipeline can access the original screenshot without the processor being involved.

## Implementation Scope

### New crate: `alvum-capture-screen`

```
crates/alvum-capture-screen/
├── Cargo.toml
└── src/
    ├── lib.rs
    ├── trigger.rs          NSWorkspace listener + idle timer
    ├── screenshot.rs       ScreenCaptureKit active window capture
    └── writer.rs           Save PNG + append DataRef to captures.jsonl
```

Dependencies: `screencapturekit`, `objc2`, `objc2-app-kit`, `alvum-core`, `tokio`, `png`.

### New crate: `alvum-processor-screen`

```
crates/alvum-processor-screen/
├── Cargo.toml
└── src/
    ├── lib.rs
    ├── describe.rs         Vision model screenshot description (local + api)
    └── ocr.rs              macOS Vision framework OCR fallback
```

Dependencies: `alvum-core`, `alvum-pipeline` (LlmProvider), `tokio`, `base64`, `objc2`, `objc2-vision` (VNRecognizeTextRequest).

### Modified files

- `crates/alvum-pipeline/src/llm.rs` — add `complete_with_image` to LlmProvider trait + implementations
- `crates/alvum-cli/Cargo.toml` — add `alvum-processor-screen` dependency
- `crates/alvum-cli/src/main.rs` — scan screen data in cross-source mode, add `capture-screen` subcommand
- `Cargo.toml` — add new crates to workspace members
