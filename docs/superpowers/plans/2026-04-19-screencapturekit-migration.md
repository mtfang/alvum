# ScreenCaptureKit Migration Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Migrate `audio-system` and `screen` capture to macOS ScreenCaptureKit so capture survives output-device changes (AirPods, AirPlay, HDMI) and retires the legacy `CGWindowListCreateImage` screen path.

**Architecture:** Two independent `SCStream` instances — one audio-only in `alvum-capture-audio/src/sck.rs`, one video-only in `alvum-capture-screen/src/sck.rs`. `AudioSystemSource` and `ScreenSource` keep their existing `CaptureSource` trait shapes; only their internals change. Mic capture stays on `cpal`.

**Tech Stack:** Rust, `screencapturekit` crate ≥ 1.5, `core-media-rs` (transitively), `image` (existing), macOS 13+.

**Authoritative spec:** `docs/superpowers/specs/2026-04-19-screencapturekit-migration.md`. Read that first if anything below seems underspecified — the spec carries the "why" and is the tiebreaker.

---

## Task 1: Add screencapturekit dependency

**Files:**
- Modify: `crates/alvum-capture-audio/Cargo.toml` (add dep)
- Modify: `crates/alvum-capture-screen/Cargo.toml` (add dep)
- Modify: `Cargo.lock` (auto, via `cargo build`)

- [ ] **Step 1: Add the crate to both capture crates**

In `crates/alvum-capture-audio/Cargo.toml`, under `[dependencies]`:
```toml
screencapturekit = "1.5"
```

In `crates/alvum-capture-screen/Cargo.toml`, under `[dependencies]`:
```toml
screencapturekit = "1.5"
```

- [ ] **Step 2: Verify it compiles**

Run: `cargo build -p alvum-capture-audio -p alvum-capture-screen`
Expected: Compiles clean. If it doesn't, the crate has a new major version or feature-gate — stop and resolve before proceeding.

- [ ] **Step 3: Commit**

```bash
git add crates/alvum-capture-audio/Cargo.toml crates/alvum-capture-screen/Cargo.toml Cargo.lock
git commit -m "chore(deps): add screencapturekit to audio + screen capture crates"
```

---

## Task 2: SCK smoke-test spike (de-risk API shape)

**Files:**
- Create: `crates/alvum-capture-audio/examples/sck_spike.rs`

Goal of this task: prove on the *actual user's Mac* that (a) SCK audio capture delivers CMSampleBuffers, (b) we can decode them to f32 samples, (c) the stream survives a manual device switch (plug/unplug headphones during the run). This guards against crate-API drift and surfaces decode issues before we touch production code.

- [ ] **Step 1: Write the spike binary**

Create `crates/alvum-capture-audio/examples/sck_spike.rs`:
```rust
//! Throwaway: captures 30 seconds of system audio via SCK and prints
//! per-second sample counts. Switch your audio output (AirPods, headphones)
//! during the run to verify the stream survives device changes.
//!
//! Run: cargo run -p alvum-capture-audio --example sck_spike --release

use screencapturekit::shareable_content::SCShareableContent;
use screencapturekit::stream::configuration::SCStreamConfiguration;
use screencapturekit::stream::content_filter::SCContentFilter;
use screencapturekit::stream::output_trait::SCStreamOutputTrait;
use screencapturekit::stream::output_type::SCStreamOutputType;
use screencapturekit::stream::SCStream;
use core_media_rs::cm_sample_buffer::CMSampleBuffer;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

struct Handler {
    samples: Arc<AtomicU64>,
}

impl SCStreamOutputTrait for Handler {
    fn did_output_sample_buffer(&self, sample: CMSampleBuffer, of_type: SCStreamOutputType) {
        if matches!(of_type, SCStreamOutputType::Audio) {
            let n = sample.get_num_samples().unwrap_or(0) as u64;
            self.samples.fetch_add(n, Ordering::Relaxed);
        }
    }
}

fn main() -> anyhow::Result<()> {
    let content = SCShareableContent::get()?;
    let display = content.displays().into_iter().next()
        .ok_or_else(|| anyhow::anyhow!("no displays"))?;
    let filter = SCContentFilter::new().with_display_excluding_windows(&display, &[]);
    let config = SCStreamConfiguration::new()
        .set_captures_audio(true)?
        .set_sample_rate(48000)?
        .set_channel_count(2)?;

    let samples = Arc::new(AtomicU64::new(0));
    let handler = Handler { samples: samples.clone() };

    let mut stream = SCStream::new(&filter, &config);
    stream.add_output_handler(handler, SCStreamOutputType::Audio);
    stream.start_capture()?;
    eprintln!("started. now switch your audio output a few times. 30s run.");

    let start = Instant::now();
    while start.elapsed() < Duration::from_secs(30) {
        std::thread::sleep(Duration::from_secs(1));
        eprintln!("  t={}s  total_samples={}",
            start.elapsed().as_secs(),
            samples.load(Ordering::Relaxed));
    }

    stream.stop_capture()?;
    let total = samples.load(Ordering::Relaxed);
    eprintln!("done. {} total samples across 30s (~48000 * 30 = 1.44M expected)", total);
    if total < 1_000_000 {
        anyhow::bail!("far too few samples — stream likely broken");
    }
    Ok(())
}
```

**Note:** the method names above (`set_captures_audio`, `with_display_excluding_windows`, `new` vs `create`, etc.) reflect screencapturekit 1.5's documented builder pattern, but the crate has evolved between minor versions. If any method name fails to resolve, run `cargo doc -p screencapturekit --open` and cross-reference the exact names in `SCStreamConfiguration` and `SCContentFilter`. Do not guess.

- [ ] **Step 2: Run it on your Mac**

Run: `cargo run -p alvum-capture-audio --example sck_spike --release`

On first run, macOS will prompt for Screen Recording permission. Grant it. Rerun.

Expected: prints growing sample counts each second; total ≥ 1M samples after 30s; exits 0. While running, switch audio output between built-in speakers and AirPods (or headphones) — the sample count must keep growing across the switch. If it stops, SCK isn't resilient on your config and the whole migration premise is wrong — stop and file a finding.

- [ ] **Step 3: Run it twice in a row**

Run the same command a second time. Expected: no re-prompt; audio starts capturing immediately.

This proves the TCC grant is keyed to the signed binary's identity (which is what the migration relies on).

- [ ] **Step 4: Commit**

```bash
git add crates/alvum-capture-audio/examples/sck_spike.rs
git commit -m "spike(capture-audio): SCK audio capture smoke test"
```

---

## Task 3: CMSampleBuffer audio decoder

**Files:**
- Create: `crates/alvum-capture-audio/src/sck_decode.rs`
- Modify: `crates/alvum-capture-audio/src/lib.rs` (add `mod sck_decode;`)
- Test: inline `#[cfg(test)] mod tests` in `sck_decode.rs`

Goal: pure function from `CMSampleBuffer` (48 kHz stereo) to `Vec<f32>` (16 kHz mono). No SCK state, no Cocoa state — just decode.

- [ ] **Step 1: Write the module skeleton with docs**

Create `crates/alvum-capture-audio/src/sck_decode.rs`:
```rust
//! Decode a ScreenCaptureKit audio `CMSampleBuffer` into 16 kHz mono f32
//! samples, the format the rest of the audio pipeline expects.
//!
//! SCK delivers native 48 kHz stereo interleaved f32. We:
//!   1. Extract the interleaved buffer via CMSampleBuffer's block buffer.
//!   2. Average L+R to mono.
//!   3. Decimate 3:1 to hit 16 kHz. Linear interpolation, matching the
//!      resampling approach already used in `capture.rs`.

use anyhow::{Context, Result};
use core_media_rs::cm_sample_buffer::CMSampleBuffer;

pub const SCK_INPUT_RATE: u32 = 48_000;
pub const TARGET_RATE: u32 = 16_000;

/// Decimation ratio as f64 for precise phase accumulation.
const RATIO: f64 = SCK_INPUT_RATE as f64 / TARGET_RATE as f64;

/// Decode one SCK audio sample buffer.
///
/// `phase` is the sub-sample position carried across callbacks so the
/// decimator doesn't produce a zipper pattern at buffer boundaries.
/// Pass the same `&mut f64` across every call in the same stream.
pub fn decode_audio(sample: &CMSampleBuffer, phase: &mut f64) -> Result<Vec<f32>> {
    let interleaved = extract_f32_stereo(sample)
        .context("failed to extract f32 stereo from CMSampleBuffer")?;
    let mono = stereo_to_mono(&interleaved);
    Ok(resample_linear(&mono, phase))
}

fn extract_f32_stereo(_sample: &CMSampleBuffer) -> Result<Vec<f32>> {
    // Implementation follows in the next step.
    todo!()
}

fn stereo_to_mono(interleaved: &[f32]) -> Vec<f32> {
    interleaved
        .chunks_exact(2)
        .map(|ch| 0.5 * (ch[0] + ch[1]))
        .collect()
}

fn resample_linear(input: &[f32], phase: &mut f64) -> Vec<f32> {
    let mut out = Vec::with_capacity((input.len() as f64 / RATIO) as usize + 1);
    let mut i = *phase;
    while i < input.len() as f64 {
        let idx = i as usize;
        let frac = (i - idx as f64) as f32;
        let s0 = input[idx];
        let s1 = if idx + 1 < input.len() { input[idx + 1] } else { s0 };
        out.push(s0 + (s1 - s0) * frac);
        i += RATIO;
    }
    *phase = i - input.len() as f64;
    out
}
```

- [ ] **Step 2: Write the pure-function tests first (TDD)**

Append to `sck_decode.rs`:
```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stereo_to_mono_averages_channels() {
        let interleaved = [0.2, 0.8, -0.4, 0.4];
        let mono = stereo_to_mono(&interleaved);
        assert_eq!(mono, vec![0.5, 0.0]);
    }

    #[test]
    fn resample_48k_to_16k_drops_two_of_three() {
        // 48 kHz mono → 16 kHz mono: we expect roughly len/3 output samples.
        let input: Vec<f32> = (0..48_000).map(|i| i as f32 * 0.001).collect();
        let mut phase = 0.0;
        let out = resample_linear(&input, &mut phase);
        // Linear interpolation at 3:1 produces exactly 48000/3 = 16000 output.
        // Off-by-one tolerance for phase rounding.
        assert!((15_999..=16_001).contains(&out.len()),
            "expected ~16000 samples, got {}", out.len());
    }

    #[test]
    fn resample_phase_carries_between_calls() {
        // Two back-to-back buffers must produce no discontinuity — the total
        // output count should match a single double-length call.
        let single: Vec<f32> = (0..6_000).map(|i| i as f32).collect();
        let mut phase_a = 0.0;
        let mut phase_b = 0.0;
        let combined = resample_linear(&single, &mut phase_a);

        let half1 = &single[..3_000];
        let half2 = &single[3_000..];
        let mut split = resample_linear(half1, &mut phase_b);
        split.extend(resample_linear(half2, &mut phase_b));

        // Allow off-by-one at the split boundary.
        let diff = (combined.len() as i64 - split.len() as i64).abs();
        assert!(diff <= 1, "split={} vs combined={}", split.len(), combined.len());
    }
}
```

- [ ] **Step 3: Register the module**

In `crates/alvum-capture-audio/src/lib.rs`, add:
```rust
pub mod sck_decode;
```

next to the existing `pub mod capture;` line.

- [ ] **Step 4: Run tests, confirm they pass**

Run: `cargo test -p alvum-capture-audio sck_decode`
Expected: 3 tests passed. `extract_f32_stereo` is `todo!()` — that's fine, no test touches it yet.

- [ ] **Step 5: Implement `extract_f32_stereo`**

Now replace the `todo!()` with the real implementation. The exact Rust API for reaching into a CMSampleBuffer's audio data depends on what `core-media-rs` (the transitive dep) exposes. Check what's available:

Run: `cargo doc -p core-media-rs --open` and find the `CMSampleBuffer` impl. Look for methods like `get_audio_buffer_list`, `data_buffer`, or `copy_into_buffer`.

The likely shape (verify against docs before committing):
```rust
fn extract_f32_stereo(sample: &CMSampleBuffer) -> Result<Vec<f32>> {
    // CMSampleBuffer carries audio in a CMBlockBuffer. We get the contiguous
    // byte slice, reinterpret as f32 pairs (L,R,L,R,...), and return as-is —
    // stereo_to_mono and resample_linear downstream handle the rest.
    let block = sample.get_data_buffer()
        .context("CMSampleBuffer has no data buffer")?;
    let bytes = block.get_data_pointer()
        .context("CMBlockBuffer has no data pointer")?;
    let n_samples = sample.get_num_samples()? as usize;
    // 2 channels, 4 bytes/sample each, interleaved.
    let expected = n_samples * 2 * 4;
    if bytes.len() < expected {
        anyhow::bail!("short buffer: expected {} bytes, got {}", expected, bytes.len());
    }
    let floats: &[f32] = bytemuck::cast_slice(&bytes[..expected]);
    Ok(floats.to_vec())
}
```

If the real method names differ, adjust — but keep the contract: return an f32 interleaved stereo Vec. Add `bytemuck = "1"` to the crate's Cargo.toml dependencies if needed.

- [ ] **Step 6: Run tests again**

Run: `cargo test -p alvum-capture-audio sck_decode`
Expected: still 3 tests pass. (`extract_f32_stereo` is only exercised by the SCK spike once we wire it in — no unit test possible without a real CMSampleBuffer.)

- [ ] **Step 7: Commit**

```bash
git add crates/alvum-capture-audio/src/sck_decode.rs crates/alvum-capture-audio/src/lib.rs crates/alvum-capture-audio/Cargo.toml
git commit -m "feat(capture-audio): CMSampleBuffer → 16kHz mono f32 decoder"
```

---

## Task 4: SCK audio stream wrapper

**Files:**
- Create: `crates/alvum-capture-audio/src/sck.rs`
- Modify: `crates/alvum-capture-audio/src/lib.rs` (add `mod sck;`)

Goal: a struct that owns the SCStream lifecycle and invokes a user-supplied callback with decoded 16 kHz mono f32 samples. Mirrors the shape of the existing `AudioStream` in `capture.rs` but internals are SCK.

- [ ] **Step 1: Write the wrapper**

Create `crates/alvum-capture-audio/src/sck.rs`:
```rust
//! SCK-driven system audio capture. Owns an SCStream and forwards
//! decoded 16 kHz mono samples to the caller's callback.

use crate::capture::SampleCallback;
use crate::sck_decode::{decode_audio, TARGET_RATE};
use anyhow::{Context, Result};
use core_media_rs::cm_sample_buffer::CMSampleBuffer;
use screencapturekit::shareable_content::SCShareableContent;
use screencapturekit::stream::configuration::SCStreamConfiguration;
use screencapturekit::stream::content_filter::SCContentFilter;
use screencapturekit::stream::output_trait::SCStreamOutputTrait;
use screencapturekit::stream::output_type::SCStreamOutputType;
use screencapturekit::stream::SCStream;
use std::sync::{Arc, Mutex};
use tracing::{error, info};

pub struct SckAudioStream {
    stream: SCStream,
}

struct AudioHandler {
    callback: SampleCallback,
    phase: Arc<Mutex<f64>>,
}

impl SCStreamOutputTrait for AudioHandler {
    fn did_output_sample_buffer(&self, sample: CMSampleBuffer, of_type: SCStreamOutputType) {
        if !matches!(of_type, SCStreamOutputType::Audio) {
            return;
        }
        let mut phase = match self.phase.lock() {
            Ok(p) => p,
            Err(_) => return,
        };
        match decode_audio(&sample, &mut *phase) {
            Ok(out) => {
                if !out.is_empty() {
                    if let Ok(mut cb) = self.callback.lock() {
                        cb(&out);
                    }
                }
            }
            Err(e) => error!(error = %e, "SCK audio decode failed"),
        }
    }
}

/// Start system-audio capture via ScreenCaptureKit. Returns a guard that
/// stops the stream when dropped.
pub fn start_capture(callback: SampleCallback) -> Result<SckAudioStream> {
    info!(target_rate = TARGET_RATE, "starting SCK system-audio capture");

    let content = SCShareableContent::get().context("SCShareableContent::get")?;
    let display = content.displays().into_iter().next()
        .context("no displays for SCK capture filter")?;
    let filter = SCContentFilter::new().with_display_excluding_windows(&display, &[]);

    // Audio-only: minimum video config to keep SCK happy but ignore output.
    let config = SCStreamConfiguration::new()
        .set_captures_audio(true)?
        .set_sample_rate(48_000)?
        .set_channel_count(2)?;

    let handler = AudioHandler {
        callback,
        phase: Arc::new(Mutex::new(0.0)),
    };

    let mut stream = SCStream::new(&filter, &config);
    stream.add_output_handler(handler, SCStreamOutputType::Audio);
    stream.start_capture().context("SCStream start_capture")?;
    info!("SCK system-audio capture started");

    Ok(SckAudioStream { stream })
}

impl Drop for SckAudioStream {
    fn drop(&mut self) {
        if let Err(e) = self.stream.stop_capture() {
            error!(error = %e, "SCK audio stop_capture failed");
        } else {
            info!("SCK system-audio capture stopped");
        }
    }
}
```

- [ ] **Step 2: Register the module**

In `crates/alvum-capture-audio/src/lib.rs`, add:
```rust
#[cfg(target_os = "macos")]
pub mod sck;
```

(Gate on macOS so the non-macos doctests / future cross-compiles don't break.)

- [ ] **Step 3: Build-only check (no test — needs a real buffer)**

Run: `cargo build -p alvum-capture-audio`
Expected: clean build. Compiler errors at this stage mean the screencapturekit method names drifted from 1.5 — fix by checking `cargo doc -p screencapturekit --open`.

- [ ] **Step 4: Commit**

```bash
git add crates/alvum-capture-audio/src/sck.rs crates/alvum-capture-audio/src/lib.rs
git commit -m "feat(capture-audio): SCK audio stream wrapper"
```

---

## Task 5: Rewrite `AudioSystemSource` to use SCK

**Files:**
- Modify: `crates/alvum-capture-audio/src/source.rs` (lines 75–153, the `AudioSystemSource` impl)

- [ ] **Step 1: Replace the `AudioSystemSource::run` body**

In `crates/alvum-capture-audio/src/source.rs`, replace the entire `impl CaptureSource for AudioSystemSource` block (lines ~97–153) with:

```rust
#[async_trait::async_trait]
impl CaptureSource for AudioSystemSource {
    fn name(&self) -> &str {
        "audio-system"
    }

    async fn run(&self, capture_dir: &Path, mut shutdown: watch::Receiver<bool>) -> Result<()> {
        let sys_dir = capture_dir.join("audio").join("system");
        let samples_per_chunk = SAMPLE_RATE as usize * self.chunk_duration_secs as usize;

        let encoder = Arc::new(Mutex::new(AudioEncoder::new(sys_dir, SAMPLE_RATE)?));
        let callback = make_chunked_callback(encoder.clone(), samples_per_chunk, "system".into());

        let stream = match crate::sck::start_capture(callback) {
            Ok(s) => s,
            Err(e) => {
                tracing::warn!(error = %e, "SCK system audio unavailable, source will not run");
                while !*shutdown.borrow_and_update() {
                    if shutdown.changed().await.is_err() {
                        break;
                    }
                }
                return Ok(());
            }
        };

        info!("audio-system source started (SCK)");

        while !*shutdown.borrow_and_update() {
            if shutdown.changed().await.is_err() {
                break;
            }
        }

        drop(stream);
        if let Ok(mut enc) = encoder.lock() {
            let _ = enc.flush_segment();
        }

        info!("audio-system source stopped");
        Ok(())
    }
}
```

Note: `self.device_name` is no longer read — SCK captures system audio regardless of output device, so the config key is ignored. Leave the field in the struct for now (removed in a later task) to keep this diff surgical.

- [ ] **Step 2: Build**

Run: `cargo build -p alvum-capture-audio`
Expected: clean. If anything references the old cpal path for system audio, the compiler will flag it.

- [ ] **Step 3: Run existing audio tests**

Run: `cargo test -p alvum-capture-audio`
Expected: all prior tests pass. The mic test path is untouched; the system-audio test was a `#[ignore]`-gated manual integration test and doesn't run in CI.

- [ ] **Step 4: Commit**

```bash
git add crates/alvum-capture-audio/src/source.rs
git commit -m "feat(capture-audio): AudioSystemSource now uses SCK for device-resilient capture"
```

---

## Task 6: Delete dead cpal output-device code

**Files:**
- Modify: `crates/alvum-capture-audio/src/devices.rs` (remove `get_output_device` and its tests)
- Modify: `crates/alvum-capture-audio/src/source.rs` (remove the now-unused `device_name` field on `AudioSystemSource`)

- [ ] **Step 1: Remove `get_output_device`**

Open `crates/alvum-capture-audio/src/devices.rs` and delete:
- The `pub fn get_output_device(...)` function (whole body, lines ~47–61).
- Any test that references `get_output_device` (none currently — grep to confirm).

Run: `grep -rn "get_output_device" crates/` — expected: zero matches after deletion.

- [ ] **Step 2: Remove the `device_name` field from `AudioSystemSource`**

In `crates/alvum-capture-audio/src/source.rs`:

Replace the struct:
```rust
pub struct AudioSystemSource {
    device_name: Option<String>,
    chunk_duration_secs: u32,
}
```
with:
```rust
pub struct AudioSystemSource {
    chunk_duration_secs: u32,
}
```

Replace `from_config`:
```rust
impl AudioSystemSource {
    pub fn from_config(config: &CaptureSourceConfig) -> Self {
        let chunk_duration_secs = config.settings.get("chunk_duration_secs")
            .and_then(|v| v.as_integer())
            .unwrap_or(60) as u32;
        Self { chunk_duration_secs }
    }
}
```

- [ ] **Step 3: Build + test**

Run: `cargo build -p alvum-capture-audio && cargo test -p alvum-capture-audio`
Expected: clean build, all tests pass.

- [ ] **Step 4: Commit**

```bash
git add crates/alvum-capture-audio/src/devices.rs crates/alvum-capture-audio/src/source.rs
git commit -m "refactor(capture-audio): drop cpal output-device binding now that SCK owns system audio"
```

---

## Task 7: SCK screen-capture wrapper

**Files:**
- Create: `crates/alvum-capture-screen/src/sck.rs`
- Modify: `crates/alvum-capture-screen/src/lib.rs` (add `mod sck;`)

Goal: mirror the audio wrapper pattern — a struct owning an `SCStream` (video only) that forwards pixel buffers to a callback. The existing `ScreenSource` trigger loop will drive *when* we snapshot; SCK streams continuously, and we just grab the most-recent frame when a trigger fires.

- [ ] **Step 1: Write the wrapper**

Create `crates/alvum-capture-screen/src/sck.rs`:
```rust
//! SCK-driven screen capture. Owns an SCStream (video-only) and keeps the
//! most recent frame in a shared slot. The ScreenSource trigger loop
//! reads the slot to drive PNG encoding at its own cadence — we do NOT
//! encode every frame.

use anyhow::{Context, Result};
use core_media_rs::cm_sample_buffer::CMSampleBuffer;
use screencapturekit::shareable_content::SCShareableContent;
use screencapturekit::stream::configuration::SCStreamConfiguration;
use screencapturekit::stream::content_filter::SCContentFilter;
use screencapturekit::stream::output_trait::SCStreamOutputTrait;
use screencapturekit::stream::output_type::SCStreamOutputType;
use screencapturekit::stream::SCStream;
use std::sync::{Arc, Mutex};
use tracing::{error, info};

/// A PNG-encoded screenshot with metadata lifted from the foreground window.
pub struct Frame {
    pub png_bytes: Vec<u8>,
    pub app_name: String,
    pub window_title: String,
}

pub struct SckScreenStream {
    stream: SCStream,
    latest: Arc<Mutex<Option<Frame>>>,
}

struct VideoHandler {
    latest: Arc<Mutex<Option<Frame>>>,
    shareable: Arc<SCShareableContent>,
}

impl SCStreamOutputTrait for VideoHandler {
    fn did_output_sample_buffer(&self, sample: CMSampleBuffer, of_type: SCStreamOutputType) {
        if !matches!(of_type, SCStreamOutputType::Screen) {
            return;
        }
        match encode_frame(&sample, &self.shareable) {
            Ok(frame) => {
                if let Ok(mut slot) = self.latest.lock() {
                    *slot = Some(frame);
                }
            }
            Err(e) => error!(error = %e, "SCK frame encode failed"),
        }
    }
}

fn encode_frame(
    sample: &CMSampleBuffer,
    shareable: &SCShareableContent,
) -> Result<Frame> {
    // Pull the CVPixelBuffer out of the sample, read BGRA bytes, convert to
    // RGBA, and PNG-encode via the `image` crate — same conversion the old
    // CGImage path did, just driven by a different pixel-source.
    let pixel_buffer = sample.get_image_buffer()
        .context("CMSampleBuffer has no image buffer")?;
    let width = pixel_buffer.get_width() as u32;
    let height = pixel_buffer.get_height() as u32;
    let bytes_per_row = pixel_buffer.get_bytes_per_row();
    pixel_buffer.lock_base_address(0)?;
    let raw = pixel_buffer.get_base_address_as_slice();

    let mut rgba = Vec::with_capacity((width as usize) * (height as usize) * 4);
    for y in 0..height as usize {
        let row_start = y * bytes_per_row;
        for x in 0..width as usize {
            let off = row_start + x * 4;
            rgba.extend_from_slice(&[raw[off + 2], raw[off + 1], raw[off], raw[off + 3]]);
        }
    }
    pixel_buffer.unlock_base_address(0)?;

    let mut png = Vec::new();
    image::ImageEncoder::write_image(
        image::codecs::png::PngEncoder::new(&mut png),
        &rgba,
        width,
        height,
        image::ExtendedColorType::Rgba8,
    )?;

    // Frontmost app/window metadata: read from SCK's shareable-content
    // snapshot. SCShareableContent.windows() returns a list ordered
    // front-to-back; the first on-screen non-background window is ours.
    let (app_name, window_title) = frontmost_window(shareable);

    Ok(Frame { png_bytes: png, app_name, window_title })
}

fn frontmost_window(shareable: &SCShareableContent) -> (String, String) {
    for window in shareable.windows().iter() {
        if !window.is_on_screen() { continue; }
        let app = window.owning_application()
            .map(|a| a.application_name())
            .unwrap_or_default();
        if app.is_empty() || app == "Window Server" { continue; }
        let title = window.title().unwrap_or_default();
        return (app, title);
    }
    (String::new(), String::new())
}

/// Start the SCK screen stream. Call `.latest()` to pull the most recent frame.
pub fn start_capture() -> Result<SckScreenStream> {
    let content = Arc::new(SCShareableContent::get().context("SCShareableContent::get")?);
    let display = content.displays().into_iter().next()
        .context("no displays available")?;
    let filter = SCContentFilter::new().with_display_excluding_windows(&display, &[]);

    // Config at display resolution, 10 fps (we downsample to trigger cadence).
    let config = SCStreamConfiguration::new()
        .set_width(display.width() as u32)?
        .set_height(display.height() as u32)?
        .set_minimum_frame_interval_secs(0.1)?  // 10fps cap
        .set_pixel_format(screencapturekit::stream::configuration::PixelFormat::BGRA)?;

    let latest = Arc::new(Mutex::new(None));
    let handler = VideoHandler { latest: latest.clone(), shareable: content };

    let mut stream = SCStream::new(&filter, &config);
    stream.add_output_handler(handler, SCStreamOutputType::Screen);
    stream.start_capture().context("SCStream start_capture")?;
    info!("SCK screen capture started");

    Ok(SckScreenStream { stream, latest })
}

impl SckScreenStream {
    /// Pop the most recent frame. Returns None if no frame has arrived yet.
    pub fn latest(&self) -> Option<Frame> {
        self.latest.lock().ok().and_then(|mut s| s.take())
    }
}

impl Drop for SckScreenStream {
    fn drop(&mut self) {
        if let Err(e) = self.stream.stop_capture() {
            error!(error = %e, "SCK screen stop_capture failed");
        } else {
            info!("SCK screen capture stopped");
        }
    }
}
```

- [ ] **Step 2: Register the module**

In `crates/alvum-capture-screen/src/lib.rs`, add:
```rust
#[cfg(target_os = "macos")]
pub mod sck;
```

- [ ] **Step 3: Build**

Run: `cargo build -p alvum-capture-screen`
Expected: clean. If method names drift, `cargo doc -p screencapturekit --open` and fix.

- [ ] **Step 4: Commit**

```bash
git add crates/alvum-capture-screen/src/sck.rs crates/alvum-capture-screen/src/lib.rs
git commit -m "feat(capture-screen): SCK screen stream wrapper"
```

---

## Task 8: Rewrite `ScreenSource` to consume SCK

**Files:**
- Modify: `crates/alvum-capture-screen/src/source.rs` (the entire run body)

- [ ] **Step 1: Replace `ScreenSource::run`**

In `crates/alvum-capture-screen/src/source.rs`, replace the `impl CaptureSource for ScreenSource` block with:

```rust
#[async_trait::async_trait]
impl CaptureSource for ScreenSource {
    fn name(&self) -> &str {
        "screen"
    }

    async fn run(&self, capture_dir: &Path, mut shutdown: watch::Receiver<bool>) -> Result<()> {
        let stream = match crate::sck::start_capture() {
            Ok(s) => s,
            Err(e) => {
                // Surface clearly so detect_permission_issue in lib.sh matches.
                bail!("Screen Recording permission not granted. {}", e);
            }
        };

        let writer = ScreenWriter::new(capture_dir.to_path_buf())
            .context("failed to create screen writer")?;
        let mut triggers = trigger::start_triggers()
            .context("failed to start screen triggers")?;

        info!(capture_dir = %capture_dir.display(),
              idle_secs = self.idle_interval_secs,
              "screen capture started (SCK)");

        let mut count: u64 = 0;
        loop {
            tokio::select! {
                Some(event) = triggers.recv() => {
                    // Grab the most recent SCK frame; skip if nothing landed yet.
                    if let Some(frame) = stream.latest() {
                        match writer.save_screenshot(
                            &frame.png_bytes,
                            event.ts,
                            &frame.app_name,
                            &frame.window_title,
                            event.kind.as_str(),
                        ) {
                            Ok(_) => {
                                count += 1;
                                info!(count, app = %frame.app_name,
                                      trigger = event.kind.as_str(),
                                      "captured screenshot");
                            }
                            Err(e) => warn!(error = %e, "failed to save screenshot"),
                        }
                    }
                }
                _ = shutdown.changed() => {
                    if *shutdown.borrow() { break; }
                }
            }
        }

        info!(total = count, "screen capture stopped");
        Ok(())
    }
}
```

Leave the trigger module (`trigger.rs`) and writer module (`writer.rs`) unchanged. They already work.

- [ ] **Step 2: Build**

Run: `cargo build -p alvum-capture-screen`
Expected: clean.

- [ ] **Step 3: Run existing tests**

Run: `cargo test -p alvum-capture-screen`
Expected: all pass. The #[ignore]'d capture tests that touched the old `capture_frontmost_window` path still exist — fine, they're gated.

- [ ] **Step 4: Commit**

```bash
git add crates/alvum-capture-screen/src/source.rs
git commit -m "feat(capture-screen): ScreenSource uses SCK instead of CGWindowListCreateImage"
```

---

## Task 9: Delete the legacy screen code paths

**Files:**
- Delete: `crates/alvum-capture-screen/src/screenshot.rs` (no callers after Task 8)
- Modify: `crates/alvum-capture-screen/src/lib.rs` (remove `mod screenshot;`)
- Modify: `crates/alvum-capture-screen/Cargo.toml` (drop `core-graphics`, `core-foundation` if unused)

- [ ] **Step 1: Confirm no callers**

Run: `grep -rn "use crate::screenshot\|screenshot::" crates/alvum-capture-screen/src/`
Expected: zero matches (Task 8 removed the last caller).

- [ ] **Step 2: Delete the file + module line**

```bash
rm crates/alvum-capture-screen/src/screenshot.rs
```

In `crates/alvum-capture-screen/src/lib.rs`, remove the `pub mod screenshot;` (or `mod screenshot;`) line.

- [ ] **Step 3: Trim dependencies**

Run: `grep -rn "core_graphics\|core_foundation" crates/alvum-capture-screen/src/`
If zero matches, remove these lines from `crates/alvum-capture-screen/Cargo.toml`:
```toml
core-graphics = "..."
core-foundation = "..."
```
(Keep any that are still referenced.)

- [ ] **Step 4: Build + test**

Run: `cargo build -p alvum-capture-screen && cargo test -p alvum-capture-screen`
Expected: clean.

- [ ] **Step 5: Commit**

```bash
git add -A crates/alvum-capture-screen/
git commit -m "refactor(capture-screen): remove legacy CGWindowListCreateImage path"
```

---

## Task 10: End-to-end verification on the real Mac

**Files:** (none — runtime verification)

- [ ] **Step 1: Build release + install + sign**

Run:
```bash
cargo build --release -p alvum-cli
install -m 755 target/release/alvum /Users/michael/.alvum/runtime/bin/alvum
./scripts/sign-binary.sh
```
Expected: clean build, sign succeeds.

- [ ] **Step 2: Restart the daemon**

Run: `./scripts/capture.sh stop && ./scripts/capture.sh start`
Expected: daemon starts; permission prompt appears on first SCK stream (approve if prompted).

- [ ] **Step 3: Watch the log for 3 minutes**

Run: `tail -f /Users/michael/.alvum/runtime/logs/capture.out`
Expected (within 60s):
- Log lines `SCK system-audio capture started`
- Log lines `SCK screen capture started`
- Log lines `wrote audio segment ... audio/system/...wav`
- Log lines `captured screenshot ... app=...`
- No `CGWindowListCreateImage returned null` lines.
- No `audio stream error` lines.

Leave it running while you do the next steps.

- [ ] **Step 4: Device-switching stress test**

While the daemon runs, in this order:
1. Connect AirPods (or switch default output to a new device). Wait 60s.
2. Disconnect AirPods. Wait 60s.
3. Plug in wired headphones. Wait 60s.
4. Unplug. Wait 60s.

After each switch, confirm:
- New `wrote audio segment` lines continue to appear in the log.
- File count under `~/.alvum/capture/$(date +%Y-%m-%d)/audio/system/` is growing.

If the stream dies at any step, the migration has failed the success criterion — stop, file the specific failure mode, and resolve before proceeding.

- [ ] **Step 5: Screen-capture sanity**

Browse a few different windows over 2 minutes. Expected: PNG files accumulating under `~/.alvum/capture/$(date +%Y-%m-%d)/screen/` with app names matching what you had focused.

- [ ] **Step 6: Commit the verified success**

Only after steps 3–5 all pass, tag the state:
```bash
git tag sck-migration-verified
```

---

## Task 11: Update permission-error detection in scripts

**Files:**
- Modify: `scripts/lib.sh` (the `detect_permission_issue` regex)

Rationale: the current regex looks for `permission not granted` strings emitted by our old code. SCK's errors have different wording. After Task 10 we know exactly what SCK emits — update the regex to match.

- [ ] **Step 1: Capture the SCK permission-denied string**

Temporarily revoke Screen Recording permission (Settings → Privacy & Security → Screen & System Audio Recording → toggle alvum off), then:
```bash
./scripts/capture.sh stop && ./scripts/capture.sh start
sleep 5
grep -iE "permission|SCK" /Users/michael/.alvum/runtime/logs/capture.out | tail -5
```

Record the exact error line(s) SCK emits.

- [ ] **Step 2: Update the regex**

In `scripts/lib.sh`, inside `detect_permission_issue`, update the matching logic to cover both the legacy `permission not granted` phrasing (for back-compat with older capture.out files) AND the SCK phrasing you just observed. Concrete change depends on what SCK emits — typical pattern:

```bash
err_line=$(echo "$stripped" \
  | tail -n "+$start" \
  | grep -E "capture source failed.*source=\"$src\".*(permission not granted|Screen Recording|TCC)" \
  | tail -1 || true)
```

Adjust the alternation to match what you observed in Step 1.

- [ ] **Step 3: Re-grant permission and restart**

Settings → toggle alvum back on.
Run: `./scripts/capture.sh stop && ./scripts/capture.sh start`
Expected: back to healthy.

- [ ] **Step 4: Commit**

```bash
git add scripts/lib.sh
git commit -m "fix(scripts): detect_permission_issue recognizes SCK error phrasing"
```

---

## Task 12: Record the architectural decision in memory

**Files:**
- Modify: `/Users/michael/.claude/projects/-Users-michael-git-alvum/memory/decisions.md`

- [ ] **Step 1: Append the decision entry**

Add an entry dated 2026-04-19 to `decisions.md` with:

- Title: "Capture of system audio and screen moves to ScreenCaptureKit (macOS)"
- Why: first-principles resilience to output-device changes (AirPods, AirPlay, HDMI); retirement of silently-failing `CGWindowListCreateImage`; consolidation onto one TCC permission.
- Rejected alternatives: device-rebind bandaid (A), BlackHole virtual device (B).
- Authoritative spec: `docs/superpowers/specs/2026-04-19-screencapturekit-migration.md`.
- Follow up: mic remains on `cpal`; SCK audio is system audio only.

- [ ] **Step 2: Update MEMORY.md index if needed**

Ensure `MEMORY.md` still points at `decisions.md`. No new index entry needed — the file already exists.

---

## Self-review checklist

Before handing off for execution:

- [ ] Every code step shows the actual code (no "implement similar to...").
- [ ] Exact crate method names are flagged as "verify via `cargo doc`" where screencapturekit/core-media-rs API drift is likely.
- [ ] Type names are consistent across tasks (`SckAudioStream`, `SckScreenStream`, `Frame`, `SampleCallback`).
- [ ] Mic capture is not touched in any task — confirmed by re-reading Tasks 3–6.
- [ ] Success criteria in the spec map to verification steps in Task 10.
- [ ] Commit messages follow the repo's `feat(scope): ...` / `refactor(scope): ...` convention.

## Risks surfaced during planning

1. **`screencapturekit` method name drift** between 1.5.0 and 1.5.4 is plausible; Task 2 spike is the first place this bites. Each code task names the failure mode and points at `cargo doc`.
2. **CMSampleBuffer audio decode** (Task 3 Step 5) has the highest uncertainty — the exact path from sample buffer → f32 slice depends on what `core-media-rs` exposes. Plan B: use `bytemuck::cast_slice` on the raw block buffer; Plan C: FFI to `CMSampleBufferGetAudioBufferListWithRetainedBlockBuffer` if the safe path is insufficient.
3. **Two SCStreams in one process** may be rate-limited by macOS. Task 10 Step 3 verifies both stream's output simultaneously — if either starves, fall back to a single stream with dual handlers as the architectural fix (a ~30-minute refactor).
4. **Permission identity mismatch** — if the binary's code-signing identity changes between builds (already mitigated by `sign-binary.sh`), TCC may re-prompt. Task 10 implicitly verifies this.
