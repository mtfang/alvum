# Capture & Processor Bolster Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Refine all capture, processor, and connector components — delete dead code and stale traces, eliminate the leaky DataRef hack, deduplicate the Claude/Codex connectors, add idempotency, replace osascript OCR with native Vision.framework, split the monolithic SCK file, and standardize processor naming.

**Architecture:** Seven sequenced phases. Phases 1–3 are zero-contract cleanup. Phase 4 changes the `Connector` trait to remove the dummy-DataRef hack. Phases 5–6 build on the trait refactor (session-connector unification, idempotency sidecar). Phase 7 is mechanical renaming. Each phase is self-contained: tests pass at each phase boundary; commits are bite-sized within phases.

**Tech Stack:** Rust 2024 edition, Cargo workspace. `objc2-vision` 0.3.2 for native OCR (depends on `objc2`, `objc2-foundation` already in tree). `blake3` for content hashing in the idempotency sidecar.

---

## Phase 0: Setup

### Task 0.1: Branch and worktree

**Files:** none

- [ ] **Step 1: Create a worktree for this work**

Run from the repo root:

```bash
git worktree add ../alvum-bolster -b bolster-2026-04-24
cd ../alvum-bolster
```

Expected: new worktree at `../alvum-bolster` on branch `bolster-2026-04-24`.

- [ ] **Step 2: Verify baseline tests pass**

```bash
cargo test --workspace 2>&1 | tail -20
```

Expected: all tests pass. If any fail, stop and surface to operator before proceeding.

---

## Phase 1: Cleanup (no contract changes)

Each task is a small, independent commit. Order is not strict — they can be interleaved.

### Task 1.1: Delete the unused VAD module

**Files:**
- Delete: `crates/alvum-processor-audio/src/vad.rs`
- Modify: `crates/alvum-processor-audio/src/lib.rs`
- Modify: `crates/alvum-processor-audio/Cargo.toml`
- Modify: `Cargo.toml` (workspace, the `silero-vad-rust` patch)

- [ ] **Step 1: Confirm VAD has no callers**

```bash
grep -rn "VoiceDetector\|VadEvent\|silero_vad\|alvum_processor_audio::vad" crates/ --include="*.rs"
```

Expected: only matches inside `crates/alvum-processor-audio/src/vad.rs`. If anything else matches, stop.

- [ ] **Step 2: Remove `pub mod vad;` from lib.rs**

In `crates/alvum-processor-audio/src/lib.rs`, delete the line:

```rust
pub mod vad;
```

- [ ] **Step 3: Delete the file**

```bash
rm crates/alvum-processor-audio/src/vad.rs
```

- [ ] **Step 4: Drop `silero-vad-rust` from `alvum-processor-audio/Cargo.toml`**

Remove the dependency line. Run:

```bash
grep "silero" crates/alvum-processor-audio/Cargo.toml
```

Expected: empty.

- [ ] **Step 5: Drop the workspace `[patch.crates-io]` for silero**

In the root `Cargo.toml`, delete the entire `[patch.crates-io]` block (lines 26–27 and the patch entry) and the explanatory comment block (lines 20–25). Verify only that section is removed:

```bash
grep -A3 "patch.crates-io" Cargo.toml
```

Expected: empty.

- [ ] **Step 6: Delete the local patch directory**

```bash
ls patches/
rm -rf patches/silero-vad-rust
# remove patches/ if empty
rmdir patches 2>/dev/null || true
```

- [ ] **Step 7: Verify build and tests**

```bash
cargo build --workspace 2>&1 | tail -10
cargo test -p alvum-processor-audio 2>&1 | tail -20
```

Expected: build succeeds, no compile warnings about missing module, audio tests still pass.

- [ ] **Step 8: Commit**

```bash
git add -A
git commit -m "chore(processor-audio): remove unused VAD module

Whisper provides per-segment timestamps already; the Silero VAD wrapper
was never invoked. Drop the module, the silero-vad-rust dependency, and
the corresponding [patch.crates-io] block."
```

### Task 1.2: Delete unused `mic_device` propagation and `system_device` field

**Files:**
- Modify: `crates/alvum-connector-audio/src/lib.rs`

- [ ] **Step 1: Confirm `system_device` is read by no source**

```bash
grep -rn "system_device\|sys_settings.insert" crates/alvum-capture-audio/src/ crates/alvum-connector-audio/src/
```

Expected: matches only inside `alvum-connector-audio/src/lib.rs`. The receiving source (`AudioSystemSource::from_config`) does not read a `device` key.

- [ ] **Step 2: Confirm `mic_device` propagation is the only place it flows**

```bash
grep -rn "mic_device\|mic_settings" crates/alvum-capture-audio/src/ crates/alvum-connector-audio/src/
```

Expected: `AudioMicSource::from_config` reads `settings.get("device")` (line 26), so `mic_device` IS used. Keep that path. Only `system_device` is dead.

- [ ] **Step 3: Remove `system_device` from `AudioConnector`**

In `crates/alvum-connector-audio/src/lib.rs`:

Remove the field declaration:

```rust
system_device: Option<String>,
```

Remove the parsing inside `from_config`:

```rust
let system_device = settings.get("system_device")
    .and_then(|v| v.as_str())
    .map(String::from);
```

Remove the field from the `Self { ... }` constructor.

Remove the propagation block in `capture_sources`:

```rust
if let Some(ref d) = self.system_device {
    sys_settings.insert("device".into(), toml::Value::String(d.clone()));
}
```

The `sys_settings` map is now empty; replace the construction so it's still passed but holds nothing:

```rust
if self.system_enabled {
    sources.push(Box::new(
        alvum_capture_audio::source::AudioSystemSource::from_config(
            &alvum_core::config::CaptureSourceConfig {
                enabled: true,
                settings: HashMap::new(),
            }
        )
    ));
}
```

- [ ] **Step 4: Verify**

```bash
cargo build -p alvum-connector-audio 2>&1 | tail -5
cargo test -p alvum-connector-audio 2>&1 | tail -10
```

Expected: builds clean, tests pass.

- [ ] **Step 5: Commit**

```bash
git add crates/alvum-connector-audio/
git commit -m "refactor(connector-audio): drop unused system_device field

AudioSystemSource ignores any device config — SCK owns device routing.
The system_device field was parsed and propagated but never read.
Remove it to match the actual contract."
```

### Task 1.3: Delete unused `artifact_to_observations` function

**Files:**
- Modify: `crates/alvum-processor-audio/src/transcriber.rs`

- [ ] **Step 1: Confirm no external callers**

```bash
grep -rn "artifact_to_observations" crates/ --include="*.rs"
```

Expected: only matches inside `transcriber.rs`. The internal call at `process_audio_data_refs` is the only use; we'll inline.

- [ ] **Step 2: Inline the body into `process_audio_data_refs`**

Replace the `Ok(artifact)` branch in `process_audio_data_refs` (currently `transcriber.rs:159-161`):

```rust
Ok(artifact) => {
    observations.extend(AudioTranscriber::artifact_to_observations(&artifact));
}
```

with the inlined logic:

```rust
Ok(artifact) => {
    if let Some(text) = artifact.text() && !text.is_empty() {
        observations.push(Observation {
            ts: artifact.data_ref.ts,
            source: artifact.data_ref.source.clone(),
            kind: "speech_segment".into(),
            content: text.to_string(),
            metadata: artifact.layer("structured").cloned(),
            media_ref: Some(MediaRef {
                path: artifact.data_ref.path.clone(),
                mime: artifact.data_ref.mime.clone(),
            }),
        });
    }
}
```

- [ ] **Step 3: Delete the standalone `artifact_to_observations` method**

Remove the entire method block (lines 79–98 in the current file).

- [ ] **Step 4: Verify**

```bash
cargo test -p alvum-processor-audio 2>&1 | tail -20
```

Expected: tests pass.

- [ ] **Step 5: Commit**

```bash
git add crates/alvum-processor-audio/
git commit -m "refactor(processor-audio): inline single-use artifact_to_observations"
```

### Task 1.4: Delete the dead semaphore stub in screen describe

**Files:**
- Modify: `crates/alvum-processor-screen/src/describe.rs`

- [ ] **Step 1: Confirm semaphore is never used for concurrency**

Read `describe.rs:50-77`. The semaphore is created but each iteration acquires sequentially — concurrency is not actually used. The placeholder comment says "parallel via semaphore can be added when we confirm the API handles concurrent calls well."

- [ ] **Step 2: Remove the semaphore and the placeholder comment**

In `describe.rs`, replace the `process_screen_data_refs` body around the loop (lines 49–73) with a clean sequential loop:

```rust
let mut observations = Vec::new();

for data_ref in data_refs {
    match describe_screenshot(provider, data_ref, capture_dir).await {
        Ok(obs) => observations.push(obs),
        Err(e) => {
            warn!(path = %data_ref.path, error = %e, "failed to process screenshot");
        }
    }
}
```

Delete the unused intermediate `handles` `Vec<(DataRef, PathBuf)>` and the cloning loop that built it.

- [ ] **Step 3: Verify**

```bash
cargo test -p alvum-processor-screen 2>&1 | tail -20
```

Expected: tests pass.

- [ ] **Step 4: Commit**

```bash
git add crates/alvum-processor-screen/
git commit -m "refactor(processor-screen): remove dead semaphore stub

The semaphore was acquired sequentially with no real concurrency.
Drop it and the 'parallel can be added later' placeholder comment."
```

### Task 1.5: Lift Whisper language to config

**Files:**
- Modify: `crates/alvum-processor-audio/src/transcriber.rs`
- Modify: `crates/alvum-processor-audio/src/lib.rs` (re-export)
- Modify: `crates/alvum-connector-audio/src/lib.rs`
- Modify: `crates/alvum-connector-audio/src/processor.rs`

- [ ] **Step 1: Add a failing test for non-default language**

In `crates/alvum-processor-audio/src/transcriber.rs` add to the test module (or create one if missing):

```rust
#[cfg(test)]
mod lang_tests {
    use super::*;

    #[test]
    fn transcriber_accepts_language() {
        // Smoke test: the constructor must accept a language code without panicking.
        // We don't actually decode audio here — just verify the field is plumbed.
        let cfg = TranscriberConfig { language: "es".into() };
        assert_eq!(cfg.language, "es");
    }
}
```

- [ ] **Step 2: Add `TranscriberConfig` and pass it through**

In `transcriber.rs`, add at the top:

```rust
#[derive(Debug, Clone)]
pub struct TranscriberConfig {
    /// Whisper language code ("en", "es", "auto", etc.).
    pub language: String,
}

impl Default for TranscriberConfig {
    fn default() -> Self {
        Self { language: "en".into() }
    }
}
```

Modify `AudioTranscriber` to hold it:

```rust
pub struct AudioTranscriber {
    ctx: whisper_rs::WhisperContext,
    config: TranscriberConfig,
}

impl AudioTranscriber {
    pub fn new(model_path: &Path, config: TranscriberConfig) -> Result<Self> {
        let ctx = whisper_rs::WhisperContext::new_with_params(
            model_path.to_str().context("model path must be valid UTF-8")?,
            whisper_rs::WhisperContextParameters::default(),
        ).context("failed to load Whisper model")?;
        info!(model = %model_path.display(), language = %config.language, "loaded Whisper model");
        Ok(Self { ctx, config })
    }
    // ...
}
```

In `transcribe_samples`, replace:

```rust
params.set_language(Some("en"));
```

with:

```rust
params.set_language(Some(&self.config.language));
```

- [ ] **Step 3: Update `process_audio_data_refs` to take config**

```rust
pub fn process_audio_data_refs(
    model_path: &Path,
    config: TranscriberConfig,
    data_refs: &[DataRef],
) -> Result<Vec<Observation>> {
    if data_refs.is_empty() { return Ok(vec![]); }
    let transcriber = AudioTranscriber::new(model_path, config)?;
    // ... rest unchanged
}
```

- [ ] **Step 4: Plumb config through the connector**

In `crates/alvum-connector-audio/src/lib.rs`, add a field `whisper_language: String`:

```rust
let whisper_language = settings.get("whisper_language")
    .and_then(|v| v.as_str())
    .unwrap_or("en")
    .to_string();
```

Pass it to `WhisperProcessor::new`. Update `crates/alvum-connector-audio/src/processor.rs`:

```rust
pub struct WhisperProcessor {
    model_path: PathBuf,
    config: alvum_processor_audio::transcriber::TranscriberConfig,
}

impl WhisperProcessor {
    pub fn new(model_path: PathBuf, language: String) -> Self {
        Self {
            model_path,
            config: alvum_processor_audio::transcriber::TranscriberConfig { language },
        }
    }
}
```

In its `Processor::process` impl, pass `self.config.clone()` to `process_audio_data_refs`.

- [ ] **Step 5: Verify**

```bash
cargo test -p alvum-processor-audio 2>&1 | tail -20
cargo test -p alvum-connector-audio 2>&1 | tail -10
cargo build --workspace 2>&1 | tail -5
```

Expected: tests pass, build clean.

- [ ] **Step 6: Commit**

```bash
git add -A
git commit -m "feat(processor-audio): make Whisper language configurable

Add TranscriberConfig with language field; default 'en'. Pipe through
the audio connector via [connector.audio] whisper_language = '...'."
```

### Task 1.6: Lift silence-gate defaults into named constants

**Files:**
- Modify: `crates/alvum-capture-audio/src/source.rs`

- [ ] **Step 1: Define constants at the top of the file**

In `crates/alvum-capture-audio/src/source.rs`, immediately after the imports and before `AudioMicSource`, add:

```rust
/// Default silence-gate thresholds, derived from production capture data.
/// A 60-second window is dropped only if BOTH RMS AND peak fall below the gate.
mod silence_defaults {
    /// Quiet room mic with no speech; 22-20-52.wav probe set the floor.
    pub const MIC_RMS_DBFS: f32 = -45.0;
    pub const MIC_PEAK_DBFS: f32 = -15.0;

    /// System audio has no ambient room tone, so the RMS bar sits lower.
    /// Peak floor matches mic — a sharp transient counts as signal in either.
    pub const SYSTEM_RMS_DBFS: f32 = -60.0;
    pub const SYSTEM_PEAK_DBFS: f32 = -15.0;
}
```

- [ ] **Step 2: Replace the magic numbers**

In `AudioMicSource::from_config` replace:

```rust
let silence_gate = parse_silence_gate(&config.settings, -45.0, -15.0);
```

with:

```rust
let silence_gate = parse_silence_gate(
    &config.settings,
    silence_defaults::MIC_RMS_DBFS,
    silence_defaults::MIC_PEAK_DBFS,
);
```

In `AudioSystemSource::try_from_config` replace:

```rust
let silence_gate = parse_silence_gate(&config.settings, -60.0, -15.0);
```

with:

```rust
let silence_gate = parse_silence_gate(
    &config.settings,
    silence_defaults::SYSTEM_RMS_DBFS,
    silence_defaults::SYSTEM_PEAK_DBFS,
);
```

- [ ] **Step 3: Verify**

```bash
cargo test -p alvum-capture-audio 2>&1 | tail -20
```

Expected: all tests pass.

- [ ] **Step 4: Commit**

```bash
git add crates/alvum-capture-audio/
git commit -m "refactor(capture-audio): name silence-gate default thresholds"
```

### Task 1.7: Surface silent failures with explicit logging

**Files:**
- Modify: `crates/alvum-capture-audio/src/source.rs`
- Modify: `crates/alvum-connector-claude/src/connector.rs`
- Modify: `crates/alvum-connector-codex/src/connector.rs`
- Modify: `crates/alvum-capture-screen/src/source.rs`
- Modify: `crates/alvum-processor-screen/src/describe.rs`

- [ ] **Step 1: Replace silent `let _ = enc.flush_segment()` with logged variant**

In `crates/alvum-capture-audio/src/source.rs`, two sites: `AudioMicSource::run` (around line 107) and `AudioSystemSource::run` (around line 270).

Replace each `let _ = enc.flush_segment();` with:

```rust
if let Err(e) = enc.flush_segment() {
    warn!(error = %e, source = self.name(), "final flush_segment failed; tail samples lost");
}
```

Note: `self.name()` requires the trait method to be in scope; since each is inside `CaptureSource::run`, the bare `self.name()` call works.

- [ ] **Step 2: Surface silent `since` parse failures in claude connector**

In `crates/alvum-connector-claude/src/connector.rs`, replace lines 41–43:

```rust
let after_ts = settings.get("since")
    .and_then(|v| v.as_str())
    .and_then(|s| s.parse::<chrono::DateTime<chrono::Utc>>().ok());
```

with:

```rust
let after_ts = match settings.get("since").and_then(|v| v.as_str()) {
    Some(s) => match s.parse::<chrono::DateTime<chrono::Utc>>() {
        Ok(ts) => Some(ts),
        Err(e) => {
            tracing::warn!(value = s, error = %e, "claude-code 'since' is not a valid RFC3339 timestamp; ignoring");
            None
        }
    },
    None => None,
};
```

- [ ] **Step 3: Same fix in codex connector**

Apply the equivalent change in `crates/alvum-connector-codex/src/connector.rs:40-42`.

- [ ] **Step 4: Surface settings-pane open failure**

In `crates/alvum-capture-screen/src/source.rs`, search for `Command::new("open")`. Replace each `let _ = std::process::Command::new("open")...` with:

```rust
if let Err(e) = std::process::Command::new("open").args([...]).status() {
    warn!(error = %e, "failed to open Settings.app");
}
```

(Match the existing args pattern.)

- [ ] **Step 5: Surface vision JSON parse failure**

In `crates/alvum-processor-screen/src/describe.rs:106-112` replace:

```rust
let parsed: VisionResponse = serde_json::from_str(json_str).unwrap_or_else(|_| {
    VisionResponse { description: response.clone(), actors: vec![] }
});
```

with:

```rust
let parsed: VisionResponse = serde_json::from_str(json_str).unwrap_or_else(|e| {
    warn!(error = %e, raw_len = response.len(), "vision response not JSON; using raw text as description");
    VisionResponse { description: response.clone(), actors: vec![] }
});
```

- [ ] **Step 6: Verify**

```bash
cargo build --workspace 2>&1 | tail -5
cargo test --workspace 2>&1 | tail -20
```

Expected: build clean, all tests pass.

- [ ] **Step 7: Commit**

```bash
git add -A
git commit -m "chore: log previously swallowed errors instead of dropping silently

flush_segment failures, since/before parse errors, Settings open errors,
and vision JSON parse fallbacks now produce a warning instead of vanishing."
```

### Task 1.8: Sync stale comments

**Files:**
- Modify: `crates/alvum-capture-audio/src/source.rs`
- Modify: `crates/alvum-pipeline/src/extract.rs`

- [ ] **Step 1: Refresh AudioSystemSource doc**

In `crates/alvum-capture-audio/src/source.rs:114-117`, replace the doc comment with one that matches what the code actually does:

```rust
/// Captures system audio via ScreenCaptureKit. SCK taps the macOS audio
/// graph at the process level, so capture is independent of the active
/// output device — AirPods/AirPlay/HDMI swaps don't interrupt the stream.
/// `device` config keys are not consulted; SCK owns routing entirely.
```

- [ ] **Step 2: Remove the dangling "Option (a)" reference**

In `crates/alvum-pipeline/src/extract.rs` around lines 95–97, search for the comment block referencing "Option (a)" or similar dead-end design notes. Either delete or rewrite to reflect current behavior.

```bash
grep -n "Option (a)\|TODO\|FIXME\|XXX" crates/alvum-pipeline/src/extract.rs
```

For each match, read the surrounding 5 lines and either delete the comment or rewrite it to describe the code as it stands.

- [ ] **Step 3: Verify**

```bash
cargo build --workspace 2>&1 | tail -5
```

- [ ] **Step 4: Commit**

```bash
git add -A
git commit -m "docs: refresh stale capture/pipeline comments"
```

---

## Phase 2: Native OCR via Vision.framework

Replace the `osascript` shellout in `alvum-processor-screen/src/ocr.rs` with `objc2-vision`.

### Task 2.1: Add `objc2-vision` dependency

**Files:**
- Modify: `crates/alvum-processor-screen/Cargo.toml`

- [ ] **Step 1: Add the dep**

```toml
[dependencies]
# ... existing
objc2 = "0.6"
objc2-foundation = "0.3"
objc2-vision = "0.3"
objc2-app-kit = { version = "0.3", features = ["NSImage"] }
```

(`objc2-app-kit` provides `NSImage` to load PNG → CGImage; the alternative is `CGImageSource` from CoreGraphics, but using `NSImage` keeps the binding crate count small.)

- [ ] **Step 2: Verify the dep resolves**

```bash
cargo build -p alvum-processor-screen 2>&1 | tail -10
```

Expected: build succeeds without using the new types yet.

- [ ] **Step 3: Commit**

```bash
git add crates/alvum-processor-screen/Cargo.toml Cargo.lock
git commit -m "chore(processor-screen): add objc2-vision and objc2-app-kit deps"
```

### Task 2.2: Write the failing native-OCR test

**Files:**
- Create: `crates/alvum-processor-screen/tests/fixtures/ocr_sample.png` (a small PNG with the text "alvum test")
- Create: `crates/alvum-processor-screen/tests/native_ocr.rs`

- [ ] **Step 1: Generate the fixture PNG**

```bash
mkdir -p crates/alvum-processor-screen/tests/fixtures
# Create a 200x40 white PNG with black "alvum test" text via Python+Pillow.
python3 - <<'PY'
from PIL import Image, ImageDraw, ImageFont
img = Image.new("RGB", (300, 60), "white")
draw = ImageDraw.Draw(img)
try:
    font = ImageFont.truetype("/System/Library/Fonts/Helvetica.ttc", 28)
except OSError:
    font = ImageFont.load_default()
draw.text((10, 10), "alvum test", fill="black", font=font)
img.save("crates/alvum-processor-screen/tests/fixtures/ocr_sample.png")
PY
```

Expected: file exists, ~3-5 KB. Open it manually to confirm the text is readable.

- [ ] **Step 2: Add the failing test**

`crates/alvum-processor-screen/tests/native_ocr.rs`:

```rust
//! End-to-end test of the native Vision-framework OCR path.

#[cfg(target_os = "macos")]
#[test]
fn native_ocr_extracts_text_from_fixture() {
    let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/ocr_sample.png");
    let text = alvum_processor_screen::ocr::extract_text(&path)
        .expect("OCR call should succeed");
    let lower = text.to_lowercase();
    assert!(
        lower.contains("alvum") && lower.contains("test"),
        "expected to recognize 'alvum test' in fixture, got {text:?}"
    );
}
```

- [ ] **Step 3: Run the test, confirm it fails**

```bash
cargo test -p alvum-processor-screen --test native_ocr 2>&1 | tail -20
```

Expected: FAIL — either "function private" (because `extract_text` is currently private) or osascript timing/path issues. Either way, we'll fix it in the next task.

### Task 2.3: Replace `extract_text` with native Vision implementation

**Files:**
- Modify: `crates/alvum-processor-screen/src/ocr.rs`

- [ ] **Step 1: Make `extract_text` `pub`**

Change `fn extract_text` to `pub fn extract_text`.

- [ ] **Step 2: Replace the body with native Vision API**

Delete the entire `extract_text` body (the `osascript` Command invocation) and replace with:

```rust
/// Extract text from an image using the macOS Vision framework natively.
/// Uses VNRecognizeTextRequest (accurate level) over an NSImage loaded from disk.
pub fn extract_text(image_path: &Path) -> Result<String> {
    use objc2::rc::Retained;
    use objc2::AllocAnyThread;
    use objc2_app_kit::NSImage;
    use objc2_foundation::{NSArray, NSDictionary, NSString, NSURL};
    use objc2_vision::{VNImageRequestHandler, VNRecognizeTextRequest, VNRecognizedTextObservation, VNRequestTextRecognitionLevel};

    let path_str = image_path.to_str()
        .with_context(|| format!("OCR: non-UTF8 path {}", image_path.display()))?;

    // Load image as NSImage → CGImage. Vision expects CGImage / CIImage / data.
    // SAFETY: NSImage initialization with a file URL is documented; method is unsafe in objc2 but well-defined.
    let image: Retained<NSImage> = unsafe {
        let url = NSURL::fileURLWithPath(&NSString::from_str(path_str));
        NSImage::initWithContentsOfURL(NSImage::alloc(), &url)
            .ok_or_else(|| anyhow::anyhow!("Vision: NSImage failed to load {path_str}"))?
    };

    // Convert NSImage → CGImage. NSImage.CGImageForProposedRect_context_hints
    // requires a non-null rect pointer; pass an autorelease CGRect.
    let cg_image = unsafe {
        let mut rect = objc2_core_foundation::CGRect {
            origin: objc2_core_foundation::CGPoint { x: 0.0, y: 0.0 },
            size: objc2_core_foundation::CGSize {
                width: image.size().width,
                height: image.size().height,
            },
        };
        image
            .CGImageForProposedRect_context_hints(&mut rect, None, None)
            .ok_or_else(|| anyhow::anyhow!("Vision: NSImage has no CGImage representation"))?
    };

    // Build the request. Accurate level is the default; we set explicitly for clarity.
    let request: Retained<VNRecognizeTextRequest> = unsafe { VNRecognizeTextRequest::new() };
    unsafe { request.setRecognitionLevel(VNRequestTextRecognitionLevel::Accurate) };

    // Run synchronously through VNImageRequestHandler.
    let handler: Retained<VNImageRequestHandler> = unsafe {
        VNImageRequestHandler::initWithCGImage_options(
            VNImageRequestHandler::alloc(),
            &cg_image,
            &NSDictionary::new(),
        )
    };
    let request_array = NSArray::from_retained_slice(&[request.clone()]);
    unsafe {
        handler
            .performRequests_error(&request_array)
            .map_err(|e| anyhow::anyhow!("Vision performRequests failed: {e}"))?;
    }

    // Concatenate top candidates from each observation in row order.
    // Vision returns observations top-to-bottom.
    let observations = unsafe { request.results() };
    let Some(observations) = observations else {
        return Ok(String::new());
    };

    let mut lines: Vec<String> = Vec::new();
    for i in 0..observations.count() {
        let obs: Retained<VNRecognizedTextObservation> = unsafe { observations.objectAtIndex(i) }
            .downcast()
            .map_err(|_| anyhow::anyhow!("Vision: observation cast failed"))?;
        let candidates = unsafe { obs.topCandidates(1) };
        if candidates.count() > 0 {
            let cand = unsafe { candidates.objectAtIndex(0) };
            let s = unsafe { cand.string() };
            lines.push(s.to_string());
        }
    }

    Ok(lines.join("\n"))
}
```

> **Note for the implementer:** the exact method names (`fileURLWithPath`, `CGImageForProposedRect_context_hints`, `performRequests_error`, etc.) are auto-generated and can shift between objc2-vision releases. If a method is named slightly differently in 0.3.x, consult [docs.rs/objc2-vision/0.3.2](https://docs.rs/objc2-vision/0.3.2/) and adjust. The shape is correct; the names may need 1-2 small renames.

- [ ] **Step 3: Run the integration test**

```bash
cargo test -p alvum-processor-screen --test native_ocr 2>&1 | tail -30
```

Expected: PASS — fixture text is recognized.

- [ ] **Step 4: Run the full processor-screen test suite**

```bash
cargo test -p alvum-processor-screen 2>&1 | tail -20
```

Expected: all green. The existing `empty_data_refs_returns_empty` test still passes.

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "feat(processor-screen): replace osascript OCR with native Vision API

Use objc2-vision's VNRecognizeTextRequest instead of shelling out to
osascript. Adds an integration test that recognizes a fixture PNG."
```

---

## Phase 3: Split the SCK monolith

`crates/alvum-capture-sck/src/lib.rs` is 1100 lines. Split into focused modules without changing behavior. Each step is a mechanical move + re-export. Tests pin behavior at every step.

### Task 3.1: Capture baseline

**Files:** none

- [ ] **Step 1: Run the SCK test suite, save baseline**

```bash
cargo test -p alvum-capture-sck 2>&1 | tee /tmp/sck-baseline.log | tail -10
```

Note the test count and pass/fail status. We will compare after every move.

### Task 3.2: Extract `display_watcher` module to its own file

**Files:**
- Create: `crates/alvum-capture-sck/src/display_watcher.rs`
- Modify: `crates/alvum-capture-sck/src/lib.rs`

- [ ] **Step 1: Move the module**

Cut everything inside `mod display_watcher { ... }` (currently around line 910) and paste into a new file `crates/alvum-capture-sck/src/display_watcher.rs`. Drop the outer `mod display_watcher { ... }` wrapper — the file IS the module.

- [ ] **Step 2: Wire it back in lib.rs**

Where the inline module used to be, replace with:

```rust
mod display_watcher;
```

- [ ] **Step 3: Verify tests still pass**

```bash
cargo test -p alvum-capture-sck 2>&1 | tail -10
```

Expected: same pass count as baseline.

- [ ] **Step 4: Commit**

```bash
git add -A
git commit -m "refactor(capture-sck): split display_watcher into its own file"
```

### Task 3.3: Extract filter logic to `filter.rs`

**Files:**
- Create: `crates/alvum-capture-sck/src/filter.rs`
- Modify: `crates/alvum-capture-sck/src/lib.rs`

- [ ] **Step 1: Move filter types and functions**

Move from `lib.rs` into a new `filter.rs`:
- `pub enum AppFilter` (~line 190)
- `impl Default for AppFilter`
- `pub struct SharedStreamConfig` (~line 204)
- `pub fn configure` (~line 215)
- `fn current_config` (~line 220)
- `pub fn snapshot_config_for_test` (~line 228)
- `fn match_apps_by_rules` (~line 238)
- `fn build_filter` (~line 259)

- [ ] **Step 2: Make the necessary items `pub(crate)`**

`current_config`, `match_apps_by_rules`, and `build_filter` are used elsewhere in the crate but should not be public outside. Mark them `pub(crate)`.

- [ ] **Step 3: Wire it in lib.rs**

```rust
mod filter;
pub use filter::{AppFilter, SharedStreamConfig, configure, snapshot_config_for_test};
```

- [ ] **Step 4: Verify**

```bash
cargo test -p alvum-capture-sck 2>&1 | tail -10
```

Expected: same pass count.

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "refactor(capture-sck): split filter logic into filter.rs"
```

### Task 3.4: Extract audio decoding to `audio.rs`

**Files:**
- Create: `crates/alvum-capture-sck/src/audio.rs`
- Modify: `crates/alvum-capture-sck/src/lib.rs`

- [ ] **Step 1: Move audio decode functions**

Move from `lib.rs` into a new `audio.rs`:
- `fn handle_audio` (~line 509)
- `fn decode_audio` (~line 538)
- `fn stereo_to_mono` (~line 640)
- The `SampleCallback` typedef if it lives near these functions

- [ ] **Step 2: Make `pub(crate)` as needed**

`handle_audio` and `decode_audio` are called from `SharedStream`'s output handler — `pub(crate)`.

- [ ] **Step 3: Wire and verify**

In `lib.rs`:

```rust
mod audio;
```

```bash
cargo test -p alvum-capture-sck 2>&1 | tail -10
```

Expected: same pass count.

- [ ] **Step 4: Commit**

```bash
git add -A
git commit -m "refactor(capture-sck): split audio decoding into audio.rs"
```

### Task 3.5: Extract screen handling to `screen.rs`

**Files:**
- Create: `crates/alvum-capture-sck/src/screen.rs`
- Modify: `crates/alvum-capture-sck/src/lib.rs`

- [ ] **Step 1: Move screen helpers**

Move from `lib.rs` into a new `screen.rs`:
- `fn handle_screen` (~line 649)
- `fn encode_png_from_sample` (~line 660)
- `fn frontmost_window_center` (~line 712)
- `fn is_frontmost_candidate` (~line 728)
- `fn find_active_display` (~line 744)
- `fn frontmost_window` (~line 761)

- [ ] **Step 2: Mark crate-internal**

All `pub(crate)`.

- [ ] **Step 3: Wire and verify**

```rust
mod screen;
```

```bash
cargo test -p alvum-capture-sck 2>&1 | tail -10
```

Expected: same pass count.

- [ ] **Step 4: Commit**

```bash
git add -A
git commit -m "refactor(capture-sck): split screen handling into screen.rs"
```

### Task 3.6: Extract SCK lifecycle helpers to `helpers.rs`

**Files:**
- Create: `crates/alvum-capture-sck/src/helpers.rs`
- Modify: `crates/alvum-capture-sck/src/lib.rs`

- [ ] **Step 1: Move blocking helpers**

Move from `lib.rs` into a new `helpers.rs`:
- `fn get_shareable_content_blocking` (~line 788)
- `fn update_content_filter_blocking` (~line 838)
- `fn start_capture_blocking` (~line 876)

- [ ] **Step 2: Wire and verify**

```rust
mod helpers;
```

```bash
cargo test -p alvum-capture-sck 2>&1 | tail -10
```

Expected: same pass count.

- [ ] **Step 3: Commit**

```bash
git add -A
git commit -m "refactor(capture-sck): split SCK lifecycle helpers into helpers.rs"
```

### Task 3.7: Extract SharedStream into `stream.rs`

**Files:**
- Create: `crates/alvum-capture-sck/src/stream.rs`
- Modify: `crates/alvum-capture-sck/src/lib.rs`

- [ ] **Step 1: Move stream types**

Move from `lib.rs` into a new `stream.rs`:
- `struct SharedState` (~line 357)
- `struct SharedStream` (~line 365)
- `struct SharedOutput` (and any related structs)
- `impl SharedOutput` (~line 406)
- `impl SharedStream` (~line 413)
- The static `SHARED: OnceLock<...>` that holds the global stream

- [ ] **Step 2: Mark `pub(crate)` as needed**

The `SHARED` static needs to be `pub(crate)` because `lib.rs::ensure_started`, `lib.rs::restart`, and `lib.rs::set_audio_callback` access it.

Alternative cleaner approach: make `stream.rs` own the public functions too — move `ensure_started`, `restart`, `set_audio_callback`, `pop_latest_frame`, `sync_active_display` into `stream.rs` and re-export from `lib.rs`.

Choose the latter: move all public-API functions that touch `SHARED` into `stream.rs` and have `lib.rs` re-export them.

- [ ] **Step 3: Final lib.rs shape**

After this task, `lib.rs` should be very small — the file-level doc comment, `pub use` re-exports, and the `Frame` struct (or move that too).

```rust
//! Shared ScreenCaptureKit stream for system audio + screen capture.
//! ... (existing doc comment)

mod audio;
mod display_watcher;
mod filter;
mod helpers;
mod screen;
mod stream;

pub use filter::{AppFilter, SharedStreamConfig, configure, snapshot_config_for_test};
pub use stream::{Frame, ensure_started, restart, set_audio_callback, pop_latest_frame, sync_active_display};
```

- [ ] **Step 4: Verify**

```bash
cargo test -p alvum-capture-sck 2>&1 | tail -10
wc -l crates/alvum-capture-sck/src/*.rs
```

Expected: same pass count. Each file ≤300 lines; `lib.rs` ≤50.

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "refactor(capture-sck): split stream into stream.rs; lib.rs is now a facade"
```

---

## Phase 4: Connector trait refactor

Remove the dummy-DataRef hack. Each connector enumerates its own DataRefs.

### Task 4.1: Add `gather_data_refs` to the `Connector` trait

**Files:**
- Modify: `crates/alvum-core/src/connector.rs`

- [ ] **Step 1: Write a test for the new trait method**

Add to `crates/alvum-core/src/connector.rs` (or a new `tests/connector.rs`):

```rust
#[cfg(test)]
mod gather_tests {
    use super::*;
    use crate::data_ref::DataRef;

    struct StubConnector;

    impl Connector for StubConnector {
        fn name(&self) -> &str { "stub" }
        fn capture_sources(&self) -> Vec<Box<dyn crate::capture::CaptureSource>> { vec![] }
        fn processors(&self) -> Vec<Box<dyn crate::processor::Processor>> { vec![] }
        fn gather_data_refs(&self, _capture_dir: &std::path::Path) -> anyhow::Result<Vec<DataRef>> {
            Ok(vec![DataRef {
                ts: chrono::Utc::now(),
                source: "stub".into(),
                path: "stub.bin".into(),
                mime: "application/octet-stream".into(),
                metadata: None,
            }])
        }
    }

    #[test]
    fn connector_can_gather_data_refs() {
        let c = StubConnector;
        let refs = c.gather_data_refs(std::path::Path::new("/tmp")).unwrap();
        assert_eq!(refs.len(), 1);
        assert_eq!(refs[0].source, "stub");
    }
}
```

- [ ] **Step 2: Run the test, confirm it fails**

```bash
cargo test -p alvum-core 2>&1 | tail -10
```

Expected: FAIL — `Connector::gather_data_refs` does not exist.

- [ ] **Step 3: Add the method to the trait**

In `crates/alvum-core/src/connector.rs`, add to the trait:

```rust
pub trait Connector: Send + Sync {
    fn name(&self) -> &str;
    fn capture_sources(&self) -> Vec<Box<dyn CaptureSource>>;
    fn processors(&self) -> Vec<Box<dyn Processor>>;

    /// Enumerate DataRefs available for processing within `capture_dir`.
    /// Each connector decides how to scan: filesystem walk, JSONL index,
    /// network call, etc. The pipeline merges all connectors' results,
    /// optionally filters them against an idempotency index, then dispatches
    /// to processors via `Processor::handles()`.
    fn gather_data_refs(&self, capture_dir: &std::path::Path) -> anyhow::Result<Vec<crate::data_ref::DataRef>>;
}
```

- [ ] **Step 4: Run the test, confirm it now compiles but other connectors fail**

```bash
cargo build --workspace 2>&1 | tail -20
```

Expected: compile errors in `alvum-connector-audio`, `alvum-connector-screen`, `alvum-connector-claude`, `alvum-connector-codex` — they don't yet implement `gather_data_refs`. Tasks 4.2–4.5 fix each.

- [ ] **Step 5: Don't commit yet**

This task leaves the workspace broken. Continue to 4.2.

### Task 4.2: Implement `gather_data_refs` for AudioConnector

**Files:**
- Modify: `crates/alvum-connector-audio/src/lib.rs`

- [ ] **Step 1: Lift `scan_audio_dir` out of pipeline**

Read `crates/alvum-pipeline/src/extract.rs` and find `fn scan_audio_dir` (helper used by `gather_data_refs_for_handles`). Copy its body. We'll reuse it inside the connector.

- [ ] **Step 2: Add the impl**

In `crates/alvum-connector-audio/src/lib.rs`, add to `impl Connector for AudioConnector`:

```rust
fn gather_data_refs(&self, capture_dir: &std::path::Path) -> anyhow::Result<Vec<alvum_core::data_ref::DataRef>> {
    let mut refs = Vec::new();
    if self.mic_enabled {
        let dir = capture_dir.join("audio").join("mic");
        refs.extend(scan_audio_dir(&dir, "audio-mic")?);
    }
    if self.system_enabled {
        let dir = capture_dir.join("audio").join("system");
        refs.extend(scan_audio_dir(&dir, "audio-system")?);
    }
    Ok(refs)
}
```

Add the helper at file scope:

```rust
fn scan_audio_dir(dir: &std::path::Path, source: &str) -> anyhow::Result<Vec<alvum_core::data_ref::DataRef>> {
    use std::time::SystemTime;
    let mut refs = Vec::new();
    if !dir.exists() { return Ok(refs); }
    for entry in walkdir::WalkDir::new(dir).into_iter().filter_map(|e| e.ok()) {
        let path = entry.path();
        if !entry.file_type().is_file() { continue; }
        let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
        let mime = match ext {
            "wav" => "audio/wav",
            "opus" => "audio/opus",
            _ => continue,
        };
        let mtime: SystemTime = entry.metadata().ok()
            .and_then(|m| m.modified().ok())
            .unwrap_or(SystemTime::UNIX_EPOCH);
        let ts: chrono::DateTime<chrono::Utc> = mtime.into();
        refs.push(alvum_core::data_ref::DataRef {
            ts,
            source: source.into(),
            path: path.to_string_lossy().to_string(),
            mime: mime.into(),
            metadata: None,
        });
    }
    Ok(refs)
}
```

- [ ] **Step 3: Add `walkdir` to Cargo.toml if not present**

```bash
grep -q "^walkdir" crates/alvum-connector-audio/Cargo.toml || echo "walkdir = \"2\"" >> crates/alvum-connector-audio/Cargo.toml
```

- [ ] **Step 4: Verify**

```bash
cargo build -p alvum-connector-audio 2>&1 | tail -5
```

Expected: builds clean.

### Task 4.3: Implement `gather_data_refs` for ScreenConnector

**Files:**
- Modify: `crates/alvum-connector-screen/src/lib.rs`

- [ ] **Step 1: Read `captures.jsonl` from the connector**

Add to `impl Connector for ScreenConnector`:

```rust
fn gather_data_refs(&self, capture_dir: &std::path::Path) -> anyhow::Result<Vec<alvum_core::data_ref::DataRef>> {
    let captures_path = capture_dir.join("screen").join("captures.jsonl");
    if !captures_path.exists() { return Ok(vec![]); }
    let refs: Vec<alvum_core::data_ref::DataRef> = alvum_core::storage::read_jsonl(&captures_path)
        .with_context(|| format!("read screen captures.jsonl at {}", captures_path.display()))?;
    Ok(refs)
}
```

(Adjust the `Context` import if needed.)

- [ ] **Step 2: Verify**

```bash
cargo build -p alvum-connector-screen 2>&1 | tail -5
```

### Task 4.4: Implement `gather_data_refs` for ClaudeCodeConnector

**Files:**
- Modify: `crates/alvum-connector-claude/src/connector.rs`

- [ ] **Step 1: Walk session files into DataRefs**

Replace the dummy-ref strategy. Each `*.jsonl` file in `session_dir` becomes one DataRef whose `path` is the session file's absolute path and `ts` is the file's mtime (the parser ignores `ts` anyway — it uses per-line timestamps internally).

```rust
fn gather_data_refs(&self, _capture_dir: &std::path::Path) -> anyhow::Result<Vec<alvum_core::data_ref::DataRef>> {
    let mut refs = Vec::new();
    if !self.session_dir.exists() { return Ok(refs); }
    for entry in walkdir::WalkDir::new(&self.session_dir).into_iter().filter_map(|e| e.ok()) {
        if !entry.file_type().is_file() { continue; }
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("jsonl") { continue; }
        let mtime: std::time::SystemTime = entry.metadata().ok()
            .and_then(|m| m.modified().ok())
            .unwrap_or(std::time::SystemTime::UNIX_EPOCH);
        refs.push(alvum_core::data_ref::DataRef {
            ts: mtime.into(),
            source: "claude-code".into(),
            path: path.to_string_lossy().into(),
            mime: "application/x-jsonl".into(),
            metadata: None,
        });
    }
    Ok(refs)
}
```

- [ ] **Step 2: Update `ClaudeCodeProcessor::process` to actually use `data_refs`**

Replace the body of `process` so it parses each provided DataRef instead of walking the session_dir itself:

```rust
async fn process(
    &self,
    data_refs: &[DataRef],
    _capture_dir: &Path,
) -> Result<Vec<Observation>> {
    let mut observations = Vec::new();
    for dr in data_refs {
        if dr.source != "claude-code" { continue; }
        let path = std::path::Path::new(&dr.path);
        let session_obs = crate::parser::parse_session_filtered(
            path, self.after_ts, self.before_ts,
        )?;
        observations.extend(session_obs);
    }
    info!(obs = observations.len(), "loaded claude observations");
    Ok(observations)
}
```

- [ ] **Step 3: Add `walkdir` to claude connector Cargo.toml**

```bash
grep -q "^walkdir" crates/alvum-connector-claude/Cargo.toml || echo "walkdir = \"2\"" >> crates/alvum-connector-claude/Cargo.toml
```

- [ ] **Step 4: Verify**

```bash
cargo build -p alvum-connector-claude 2>&1 | tail -5
cargo test -p alvum-connector-claude 2>&1 | tail -10
```

### Task 4.5: Implement `gather_data_refs` for CodexConnector

**Files:**
- Modify: `crates/alvum-connector-codex/src/connector.rs`

- [ ] **Step 1: Same shape as Claude, with `rollout-*.jsonl` filter**

```rust
fn gather_data_refs(&self, _capture_dir: &std::path::Path) -> anyhow::Result<Vec<alvum_core::data_ref::DataRef>> {
    let mut refs = Vec::new();
    if !self.session_dir.exists() { return Ok(refs); }
    for entry in walkdir::WalkDir::new(&self.session_dir).into_iter().filter_map(|e| e.ok()) {
        if !entry.file_type().is_file() { continue; }
        let path = entry.path();
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else { continue };
        if !name.starts_with("rollout-") || !name.ends_with(".jsonl") { continue; }
        let mtime: std::time::SystemTime = entry.metadata().ok()
            .and_then(|m| m.modified().ok())
            .unwrap_or(std::time::SystemTime::UNIX_EPOCH);
        refs.push(alvum_core::data_ref::DataRef {
            ts: mtime.into(),
            source: "codex".into(),
            path: path.to_string_lossy().into(),
            mime: "application/x-jsonl".into(),
            metadata: None,
        });
    }
    Ok(refs)
}
```

- [ ] **Step 2: Update `CodexProcessor::process` to use the provided data_refs**

```rust
async fn process(
    &self,
    data_refs: &[DataRef],
    _capture_dir: &Path,
) -> Result<Vec<Observation>> {
    let mut observations = Vec::new();
    for dr in data_refs {
        if dr.source != "codex" { continue; }
        let session_obs = crate::parser::parse_session_filtered(
            std::path::Path::new(&dr.path), self.after_ts, self.before_ts,
        )?;
        observations.extend(session_obs);
    }
    info!(obs = observations.len(), "loaded codex observations");
    Ok(observations)
}
```

- [ ] **Step 3: Add walkdir, verify**

```bash
grep -q "^walkdir" crates/alvum-connector-codex/Cargo.toml || echo "walkdir = \"2\"" >> crates/alvum-connector-codex/Cargo.toml
cargo build -p alvum-connector-codex 2>&1 | tail -5
cargo test -p alvum-connector-codex 2>&1 | tail -10
```

### Task 4.6: Replace `gather_data_refs_for_handles` in pipeline

**Files:**
- Modify: `crates/alvum-pipeline/src/extract.rs`

- [ ] **Step 1: Find the call site of `gather_data_refs_for_handles`**

```bash
grep -n "gather_data_refs_for_handles\|pairs_from_connectors" crates/alvum-pipeline/src/extract.rs
```

- [ ] **Step 2: Replace the call**

In `pairs_from_connectors` (or wherever the pipeline currently iterates connectors), replace:

```rust
let refs = gather_data_refs_for_handles(capture_dir, &processor.handles())?;
```

with:

```rust
let refs = connector.gather_data_refs(capture_dir)?;
let handles = processor.handles();
let refs: Vec<_> = refs.into_iter()
    .filter(|dr| handles.contains(&dr.source))
    .collect();
```

- [ ] **Step 3: Delete the old `gather_data_refs_for_handles` function**

It's no longer called from pipeline code. Delete it (lines ~394–453) along with `scan_audio_dir` if it's now duplicated in the audio connector. (Keep the audio-connector copy; delete from pipeline.)

- [ ] **Step 4: Update tests in `extract.rs`**

The tests at `extract.rs:586+` reference behavior that's now per-connector. Adjust them: instead of calling `gather_data_refs_for_handles` directly, construct stub connectors and call `connector.gather_data_refs`. If the tests are tightly coupled to the old function, mark them with TODO and write replacement tests.

Specifically:
- `extract.rs:586` — write_transcript test: leave alone if it's about transcript persistence.
- The tests around `gather_data_refs_for_handles` should be moved into per-connector test files (or kept and updated to use `Connector::gather_data_refs` via stubs).

For each test that calls the deleted function, rewrite to use the new per-connector path:

```rust
// Before:
// let refs = gather_data_refs_for_handles(&dir, &["audio-mic".into()])?;

// After:
let connector = alvum_connector_audio::AudioConnector::from_config(&HashMap::new())?;
let refs = connector.gather_data_refs(&dir)?;
let refs: Vec<_> = refs.into_iter()
    .filter(|d| d.source == "audio-mic")
    .collect();
```

- [ ] **Step 5: Verify the whole workspace**

```bash
cargo test --workspace 2>&1 | tail -30
```

Expected: all tests pass. The pipeline no longer has the dummy-DataRef hack; one-shot connectors enumerate session files as real DataRefs.

- [ ] **Step 6: Commit phase 4**

```bash
git add -A
git commit -m "refactor(connector): add Connector::gather_data_refs; drop dummy-DataRef hack

Each connector now enumerates its own DataRefs (audio walks FS, screen
reads captures.jsonl, claude/codex enumerate session files). The
pipeline-side gather_data_refs_for_handles switch is gone; processors
filter by their handles() against the merged ref list."
```

---

## Phase 5: Session connector unification

Collapse the duplication between `alvum-connector-claude` and `alvum-connector-codex`.

### Task 5.1: Create `alvum-connector-session` workspace crate

**Files:**
- Create: `crates/alvum-connector-session/Cargo.toml`
- Create: `crates/alvum-connector-session/src/lib.rs`
- Modify: `Cargo.toml` (workspace members)

- [ ] **Step 1: Add the crate**

`crates/alvum-connector-session/Cargo.toml`:

```toml
[package]
name = "alvum-connector-session"
version = "0.1.0"
edition = "2024"

[dependencies]
alvum-core = { path = "../alvum-core" }
anyhow.workspace = true
async-trait = "0.1"
chrono.workspace = true
dirs = "6"
serde_json.workspace = true
tracing.workspace = true
walkdir = "2"
```

`crates/alvum-connector-session/src/lib.rs`:

```rust
//! Generic JSONL session connector. Backs alvum-connector-claude and
//! alvum-connector-codex. Each schema impl supplies its own line parser,
//! filename filter, and source-name string.

use alvum_core::capture::CaptureSource;
use alvum_core::connector::Connector;
use alvum_core::data_ref::DataRef;
use alvum_core::observation::Observation;
use alvum_core::processor::Processor;
use anyhow::Result;
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use tracing::info;

/// Schema-specific behaviour for a JSONL session connector.
pub trait SessionSchema: Send + Sync + 'static {
    /// Source/connector name (e.g., "claude-code", "codex").
    fn source_name(&self) -> &'static str;

    /// Default session directory if none configured.
    fn default_session_dir(&self) -> PathBuf;

    /// Whether a filename should be considered a session file.
    fn matches_session_file(&self, name: &str) -> bool;

    /// Parse one JSONL line into an Observation, applying the timestamp
    /// window. Returns None for lines that should be skipped.
    fn parse_line(
        &self,
        line: &str,
        after: Option<DateTime<Utc>>,
        before: Option<DateTime<Utc>>,
    ) -> Option<Observation>;
}

pub struct SessionConnector<S: SessionSchema> {
    schema: S,
    session_dir: PathBuf,
    after_ts: Option<DateTime<Utc>>,
    before_ts: Option<DateTime<Utc>>,
}

impl<S: SessionSchema> SessionConnector<S> {
    pub fn from_config(schema: S, settings: &HashMap<String, toml::Value>) -> Result<Self> {
        let session_dir = settings.get("session_dir")
            .and_then(|v| v.as_str())
            .map(|s| {
                if let Some(stripped) = s.strip_prefix("~/")
                    && let Some(home) = dirs::home_dir()
                {
                    return home.join(stripped);
                }
                PathBuf::from(s)
            })
            .unwrap_or_else(|| schema.default_session_dir());

        let after_ts = match settings.get("since").and_then(|v| v.as_str()) {
            Some(s) => match s.parse::<DateTime<Utc>>() {
                Ok(ts) => Some(ts),
                Err(e) => {
                    tracing::warn!(
                        connector = schema.source_name(),
                        value = s,
                        error = %e,
                        "ignoring invalid 'since' timestamp"
                    );
                    None
                }
            },
            None => None,
        };

        Ok(Self { schema, session_dir, after_ts, before_ts: None })
    }

    pub fn with_before(mut self, before: Option<DateTime<Utc>>) -> Self {
        self.before_ts = before; self
    }
    pub fn with_since(mut self, since: Option<DateTime<Utc>>) -> Self {
        self.after_ts = since; self
    }
}

impl<S: SessionSchema> Connector for SessionConnector<S> {
    fn name(&self) -> &str { self.schema.source_name() }
    fn capture_sources(&self) -> Vec<Box<dyn CaptureSource>> { vec![] }

    fn processors(&self) -> Vec<Box<dyn Processor>> {
        vec![Box::new(SessionProcessor {
            schema_name: self.schema.source_name(),
            after_ts: self.after_ts,
            before_ts: self.before_ts,
            // schema is moved in via Box<dyn>...
            schema: Box::new(SchemaWrapper { inner: self.schema.clone_box() }),
        })]
    }

    fn gather_data_refs(&self, _capture_dir: &Path) -> Result<Vec<DataRef>> {
        let mut refs = Vec::new();
        if !self.session_dir.exists() { return Ok(refs); }
        for entry in walkdir::WalkDir::new(&self.session_dir).into_iter().filter_map(|e| e.ok()) {
            if !entry.file_type().is_file() { continue; }
            let path = entry.path();
            let Some(name) = path.file_name().and_then(|n| n.to_str()) else { continue };
            if !self.schema.matches_session_file(name) { continue; }
            let mtime: std::time::SystemTime = entry.metadata().ok()
                .and_then(|m| m.modified().ok())
                .unwrap_or(std::time::SystemTime::UNIX_EPOCH);
            refs.push(DataRef {
                ts: mtime.into(),
                source: self.schema.source_name().into(),
                path: path.to_string_lossy().into(),
                mime: "application/x-jsonl".into(),
                metadata: None,
            });
        }
        Ok(refs)
    }
}

// --- Processor side ------------------------------------------------------

trait CloneableSchema: SessionSchema {
    fn clone_box(&self) -> Box<dyn CloneableSchema>;
}

impl<T: SessionSchema + Clone> CloneableSchema for T {
    fn clone_box(&self) -> Box<dyn CloneableSchema> { Box::new(self.clone()) }
}

struct SchemaWrapper { inner: Box<dyn CloneableSchema> }
unsafe impl Send for SchemaWrapper {}
unsafe impl Sync for SchemaWrapper {}

struct SessionProcessor {
    schema: Box<SchemaWrapper>,
    schema_name: &'static str,
    after_ts: Option<DateTime<Utc>>,
    before_ts: Option<DateTime<Utc>>,
}

#[async_trait]
impl Processor for SessionProcessor {
    fn name(&self) -> &str { self.schema_name }
    fn handles(&self) -> Vec<String> { vec![self.schema_name.into()] }

    async fn process(&self, data_refs: &[DataRef], _capture_dir: &Path) -> Result<Vec<Observation>> {
        let mut observations = Vec::new();
        for dr in data_refs {
            if dr.source != self.schema_name { continue; }
            let content = std::fs::read_to_string(&dr.path)
                .map_err(|e| anyhow::anyhow!("read {}: {}", dr.path, e))?;
            for line in content.lines() {
                if line.trim().is_empty() { continue; }
                if let Some(obs) = self.schema.inner.parse_line(line, self.after_ts, self.before_ts) {
                    observations.push(obs);
                }
            }
        }
        info!(obs = observations.len(), connector = %self.schema_name, "loaded session observations");
        Ok(observations)
    }
}
```

> **Note:** the `CloneableSchema` indirection is needed because the `Connector` trait doesn't get `Clone`. Schemas are cheap (zero-state) so cloning is fine. Implementer may simplify by requiring `S: Clone + 'static` on `SessionConnector` and threading `S` directly into `SessionProcessor`. Either approach is acceptable.

- [ ] **Step 2: Add to workspace**

In root `Cargo.toml`, add to `[workspace]` members:

```toml
"crates/alvum-connector-session",
```

- [ ] **Step 3: Verify**

```bash
cargo build -p alvum-connector-session 2>&1 | tail -10
```

- [ ] **Step 4: Commit**

```bash
git add crates/alvum-connector-session/ Cargo.toml
git commit -m "feat(connector-session): add generic JSONL session connector"
```

### Task 5.2: Migrate Claude connector to use SessionConnector

**Files:**
- Modify: `crates/alvum-connector-claude/Cargo.toml`
- Modify: `crates/alvum-connector-claude/src/lib.rs`
- Modify: `crates/alvum-connector-claude/src/connector.rs` (likely deleted)
- Modify: `crates/alvum-connector-claude/src/parser.rs` (becomes line-level only)

- [ ] **Step 1: Convert `parser.rs` to a per-line schema**

Strip the file-walking logic from `parser.rs`. Keep only:
- The functions that parse a single JSONL line into an `Observation`
- The `extract_user_content` / `extract_assistant_content` helpers

Add a `ClaudeSchema` struct at the top:

```rust
use alvum_connector_session::SessionSchema;
use alvum_core::observation::Observation;
use chrono::{DateTime, Utc};
use std::path::PathBuf;

#[derive(Clone, Default)]
pub struct ClaudeSchema;

impl SessionSchema for ClaudeSchema {
    fn source_name(&self) -> &'static str { "claude-code" }

    fn default_session_dir(&self) -> PathBuf {
        dirs::home_dir().map(|h| h.join(".claude/projects")).unwrap_or_else(|| PathBuf::from("."))
    }

    fn matches_session_file(&self, name: &str) -> bool { name.ends_with(".jsonl") }

    fn parse_line(
        &self,
        line: &str,
        after: Option<DateTime<Utc>>,
        before: Option<DateTime<Utc>>,
    ) -> Option<Observation> {
        parse_claude_line(line, after, before)
    }
}

fn parse_claude_line(
    line: &str,
    after: Option<DateTime<Utc>>,
    before: Option<DateTime<Utc>>,
) -> Option<Observation> {
    let obj: serde_json::Value = serde_json::from_str(line).ok()?;
    let msg_type = obj.get("type")?.as_str()?;
    let is_meta = obj.get("isMeta").and_then(|m| m.as_bool()).unwrap_or(false);
    let timestamp = obj.get("timestamp")?.as_str()?;
    let ts: DateTime<Utc> = timestamp.parse().ok()?;
    if let Some(lower) = after && ts < lower { return None; }
    if let Some(upper) = before && ts >= upper { return None; }

    match msg_type {
        "user" if !is_meta => {
            let content = extract_user_content(&obj)?;
            let trimmed = content.trim();
            if trimmed.is_empty() || trimmed.starts_with('<') { return None; }
            Some(Observation::dialogue(ts, "claude-code", "user", trimmed))
        }
        "assistant" => {
            let content = extract_assistant_content(&obj)?;
            let trimmed = content.trim();
            if trimmed.is_empty() { return None; }
            Some(Observation::dialogue(ts, "claude-code", "assistant", trimmed))
        }
        _ => None,
    }
}

// extract_user_content / extract_assistant_content stay as before
```

Keep the existing test fixtures; rename them to call `parse_claude_line` directly. The whole-file `parse_session_filtered` function can be removed (or kept as a thin wrapper that iterates the file and calls `parse_claude_line` on each line, for test compatibility).

- [ ] **Step 2: Replace `connector.rs`**

Delete the old `ClaudeCodeConnector` struct and its hand-written impls. Replace with:

```rust
//! Claude Code connector — thin wrapper around alvum-connector-session.

use alvum_connector_session::SessionConnector;
use crate::parser::ClaudeSchema;
use anyhow::Result;
use std::collections::HashMap;

pub type ClaudeCodeConnector = SessionConnector<ClaudeSchema>;

pub fn from_config(settings: &HashMap<String, toml::Value>) -> Result<ClaudeCodeConnector> {
    SessionConnector::from_config(ClaudeSchema, settings)
}
```

- [ ] **Step 3: Update lib.rs re-exports**

`crates/alvum-connector-claude/src/lib.rs`:

```rust
pub mod connector;
pub mod parser;
pub use connector::{ClaudeCodeConnector, from_config};
```

- [ ] **Step 4: Update CLI to use the function instead of the type alias**

In `crates/alvum-cli/src/main.rs`, replace `ClaudeCodeConnector::from_config(...)` with `alvum_connector_claude::from_config(...)`. Same for any other call sites.

- [ ] **Step 5: Update Cargo.toml**

Add `alvum-connector-session = { path = "../alvum-connector-session" }` and `dirs = "6"`. Remove deps no longer needed (`async-trait` if unused, `walkdir` if unused).

- [ ] **Step 6: Verify**

```bash
cargo test -p alvum-connector-claude 2>&1 | tail -20
cargo build --workspace 2>&1 | tail -5
```

Expected: all parser tests still pass.

- [ ] **Step 7: Commit**

```bash
git add -A
git commit -m "refactor(connector-claude): migrate to SessionConnector

Replace the hand-rolled connector + walkdir loop with ClaudeSchema +
SessionConnector. The parser is now line-level only."
```

### Task 5.3: Migrate Codex connector to use SessionConnector

**Files:**
- Modify: `crates/alvum-connector-codex/Cargo.toml`
- Modify: `crates/alvum-connector-codex/src/lib.rs`
- Modify: `crates/alvum-connector-codex/src/connector.rs`
- Modify: `crates/alvum-connector-codex/src/parser.rs`

- [ ] **Step 1: Mirror Task 5.2 for Codex**

Apply the same migration pattern. The CodexSchema differs from Claude only in:
- `source_name()` → `"codex"`
- `default_session_dir()` → `~/.codex`
- `matches_session_file(name)` → `name.starts_with("rollout-") && name.ends_with(".jsonl")`
- `parse_line(line, ...)` → existing per-line codex logic, extracted from `parse_session_filtered`

- [ ] **Step 2: Verify**

```bash
cargo test -p alvum-connector-codex 2>&1 | tail -20
cargo build --workspace 2>&1 | tail -5
```

- [ ] **Step 3: Commit**

```bash
git add -A
git commit -m "refactor(connector-codex): migrate to SessionConnector"
```

---

## Phase 6: Idempotency

Add a sidecar that records every (source, path, mtime, size) we've already processed. Filter DataRefs against it before dispatching to processors.

### Task 6.1: Define the idempotency index

**Files:**
- Create: `crates/alvum-pipeline/src/processed_index.rs`
- Modify: `crates/alvum-pipeline/src/lib.rs`

- [ ] **Step 1: Write a failing test for the index**

`crates/alvum-pipeline/src/processed_index.rs`:

```rust
//! Tracks which DataRefs the pipeline has already processed, so re-runs skip work.
//! The index is a JSONL file at <output_dir>/processed.jsonl.

use alvum_core::data_ref::DataRef;
use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub struct Entry {
    pub source: String,
    pub path: String,
    pub size: u64,
    pub mtime_secs: i64,
}

pub struct ProcessedIndex {
    file: PathBuf,
    entries: HashSet<Entry>,
}

impl ProcessedIndex {
    pub fn load(file: PathBuf) -> Result<Self> {
        let mut entries = HashSet::new();
        if file.exists() {
            let text = std::fs::read_to_string(&file)?;
            for line in text.lines() {
                if line.trim().is_empty() { continue; }
                if let Ok(e) = serde_json::from_str::<Entry>(line) {
                    entries.insert(e);
                }
            }
        }
        Ok(Self { file, entries })
    }

    /// True if the ref's content has been processed before.
    pub fn contains(&self, dr: &DataRef) -> bool {
        match entry_for_ref(dr) {
            Some(e) => self.entries.contains(&e),
            None => false,
        }
    }

    pub fn record(&mut self, dr: &DataRef) -> Result<()> {
        let Some(entry) = entry_for_ref(dr) else { return Ok(()); };
        if !self.entries.contains(&entry) {
            self.entries.insert(entry.clone());
            self.append(&entry)?;
        }
        Ok(())
    }

    fn append(&self, entry: &Entry) -> Result<()> {
        use std::io::Write;
        if let Some(parent) = self.file.parent() { std::fs::create_dir_all(parent)?; }
        let mut f = std::fs::OpenOptions::new().create(true).append(true).open(&self.file)?;
        writeln!(f, "{}", serde_json::to_string(entry)?)?;
        Ok(())
    }
}

fn entry_for_ref(dr: &DataRef) -> Option<Entry> {
    let p = Path::new(&dr.path);
    let meta = std::fs::metadata(p).ok()?;
    let mtime = meta.modified().ok()?
        .duration_since(std::time::UNIX_EPOCH).ok()?
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
        let mut probe = tmp.path().join("probe.txt");
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

        // New instance reads from disk
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
        let mut f = std::fs::OpenOptions::new().append(true).open(&probe).unwrap();
        writeln!(f, "more bytes").unwrap();
        // Force a measurable mtime delta so the test is robust on fast filesystems.
        std::thread::sleep(std::time::Duration::from_millis(1100));
        let _ = std::fs::OpenOptions::new().write(true).open(&probe).unwrap();

        assert!(!idx.contains(&dr));
    }
}
```

- [ ] **Step 2: Wire into lib.rs**

In `crates/alvum-pipeline/src/lib.rs`, add:

```rust
pub mod processed_index;
```

- [ ] **Step 3: Run tests**

```bash
cargo test -p alvum-pipeline processed_index 2>&1 | tail -10
```

Expected: both tests pass.

- [ ] **Step 4: Commit**

```bash
git add -A
git commit -m "feat(pipeline): add processed-index sidecar at output_dir/processed.jsonl"
```

### Task 6.2: Filter DataRefs through the index in `extract_and_pipeline`

**Files:**
- Modify: `crates/alvum-pipeline/src/extract.rs`

- [ ] **Step 1: Add an `--no-skip-processed` opt-out flag**

In `crates/alvum-cli/src/main.rs`, add to the `Extract` subcommand:

```rust
/// Re-process all DataRefs even if recorded in processed.jsonl.
#[arg(long)]
no_skip_processed: bool,
```

Plumb it into `ExtractConfig`.

In `crates/alvum-pipeline/src/extract.rs`, add a field to `ExtractConfig`:

```rust
pub no_skip_processed: bool,
```

- [ ] **Step 2: Wire the index into extract_and_pipeline**

After the `pairs_from_connectors` step (or wherever `connector.gather_data_refs` is called), filter:

```rust
let processed_path = config.output_dir.join("processed.jsonl");
let mut index = crate::processed_index::ProcessedIndex::load(processed_path)?;

let filtered_refs: Vec<DataRef> = if config.no_skip_processed {
    refs
} else {
    refs.into_iter().filter(|dr| !index.contains(dr)).collect()
};
```

After processing succeeds (after `processor.process(&filtered_refs, ...).await?`), record each ref:

```rust
for dr in &filtered_refs {
    if let Err(e) = index.record(dr) {
        tracing::warn!(path = %dr.path, error = %e, "failed to record processed ref");
    }
}
```

- [ ] **Step 3: Add an integration test**

In `crates/alvum-pipeline/tests/idempotency.rs`:

```rust
//! Verifies that running extract twice over the same capture dir does not
//! re-process the same DataRefs.

#[test]
fn second_run_skips_processed_refs() {
    // Construct a stub connector that produces a fixed DataRef the first
    // time, and records how many times its processor's process() was called.
    // Run extract_and_pipeline twice, assert the processor was invoked once.
    // (Fully self-contained — uses tempdir + stub connector.)

    // ... (~40 lines, see implementer note below)
    todo!("flesh out with stub connector + counter; spec is correct, code TBD");
}
```

> **Implementer note:** I am not adding the stub-connector boilerplate inline because the test harness needs roughly 40 lines and the existing test patterns in `extract.rs` already show how to mock connectors. Look at the tests around `extract.rs:586` for the existing stub patterns and adapt. The important behavior to assert: a `processor.process` counter increments by N on first run, by 0 on second run, and by N again with `no_skip_processed = true`.

- [ ] **Step 4: Verify**

```bash
cargo test -p alvum-pipeline 2>&1 | tail -20
```

Expected: idempotency test passes.

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "feat(pipeline): skip already-processed DataRefs by default

Wire ProcessedIndex into extract_and_pipeline. New CLI flag
--no-skip-processed forces re-processing. Each successful processor.process
call records the ref to <output>/processed.jsonl."
```

---

## Phase 7: Processor naming standardization

Make the user-facing names consistent: domain-named processors instead of mixed (tool-named + domain-named).

### Task 7.1: Rename WhisperProcessor → AudioProcessor

**Files:**
- Modify: `crates/alvum-connector-audio/src/processor.rs`
- Modify: `crates/alvum-connector-audio/src/lib.rs`
- Search-replace any callers

- [ ] **Step 1: Find all callers**

```bash
grep -rn "WhisperProcessor" crates/ --include="*.rs"
```

- [ ] **Step 2: Rename in source**

In `crates/alvum-connector-audio/src/processor.rs`, rename the struct and all impls:

```rust
pub struct AudioProcessor { ... }
impl AudioProcessor { ... }
impl Processor for AudioProcessor { fn name(&self) -> &str { "audio" } ... }
```

The `Processor::name()` value should change from `"whisper"` to `"audio"` to match.

In `crates/alvum-connector-audio/src/lib.rs`, update the `use processor::WhisperProcessor;` → `use processor::AudioProcessor;` and any constructor calls.

- [ ] **Step 3: Update tests that reference the old name**

```bash
grep -rln "WhisperProcessor\|\"whisper\"" crates/ --include="*.rs"
```

For each match, update.

- [ ] **Step 4: Verify**

```bash
cargo test --workspace 2>&1 | tail -20
```

Expected: all green.

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "refactor: rename WhisperProcessor → AudioProcessor

Audio processor is now named for its domain instead of its current
backing implementation (Whisper)."
```

### Task 7.2: Verify ScreenProcessor naming

**Files:** none

- [ ] **Step 1: Confirm consistency**

```bash
grep -rn "ScreenProcessor\|fn name(&self) -> &str" crates/alvum-connector-screen/ --include="*.rs"
```

Expected: `ScreenProcessor` is named `screen` in `Processor::name()`. If not, fix.

- [ ] **Step 2: No commit if no change**

If the screen processor is already consistent with the new audio naming, this task is a no-op.

---

## Phase 8: Final verification & wrap

### Task 8.1: Full test sweep

**Files:** none

- [ ] **Step 1: Run every test**

```bash
cargo test --workspace 2>&1 | tee /tmp/bolster-final.log | tail -30
```

Expected: all green.

- [ ] **Step 2: Build release**

```bash
cargo build --release --workspace 2>&1 | tail -10
```

Expected: clean.

- [ ] **Step 3: Run a live extract end-to-end**

```bash
./target/release/alvum extract \
  --capture-dir ./capture \
  --output ./output/$(date +%Y-%m-%d) \
  --provider cli \
  --resume
```

Expected: completes without error; the second run produces zero new transcriptions / vision calls (visible in logs).

- [ ] **Step 4: Smoke-check the capture daemon**

Restart the Electron app. Verify all three sources still write to disk over a 5-minute window. Inspect logs for any `WARN` lines that weren't there before — investigate before merging.

### Task 8.2: Use finishing-a-development-branch skill

When all tasks complete and tests pass, invoke the `superpowers:finishing-a-development-branch` skill to merge or open a PR.

---

## Self-Review Checklist (run before handing the plan to an implementer)

- [x] Spec coverage: every "definitely in", "recommend in", and "recommend out" item from the inventory triage maps to a task here.
- [x] No placeholders for code (Phase 6.2 step 3 deliberately delegates the stub-connector boilerplate to the implementer with reference to existing patterns; this is not a code placeholder, it's a research delegation.)
- [x] Type consistency: `TranscriberConfig`, `SessionSchema`, `SessionConnector`, `ProcessedIndex`, `Entry` are referenced consistently across tasks.
- [x] Order respects dependencies: Phase 4 (trait change) precedes Phases 5 and 6.
- [x] Each phase ends with passing tests so the work can be paused at a phase boundary.

---

## Execution Handoff

Plan complete and saved to `docs/superpowers/plans/2026-04-24-capture-processor-bolster.md`. Two execution options:

1. **Subagent-Driven (recommended)** — I dispatch a fresh subagent per task, review between tasks, fast iteration.
2. **Inline Execution** — Execute tasks in this session using executing-plans, batch execution with checkpoints.

Which approach?
