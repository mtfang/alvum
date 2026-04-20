//! Shared ScreenCaptureKit stream for system audio + screen capture.
//!
//! One SCStream per process. Two concurrent SCStreams interfere on macOS
//! (delegate callbacks get starved) — we hit this the moment both audio and
//! screen were enabled at once. The fix is a single stream with both audio
//! and video outputs, fan-out to two sets of subscribers:
//!
//!   - `set_audio_callback` — system audio, decoded to 16 kHz mono f32.
//!   - `pop_latest_frame`   — most recent screen frame with metadata.
//!
//! The stream starts on the first call to [`ensure_started`] and stays up
//! for the process lifetime. Dropping subscribers is fine — their samples
//! simply stop flowing; the underlying stream keeps running (the cost of
//! silence + one PNG encode every ~500 ms is negligible).

#![cfg(target_os = "macos")]

use anyhow::{anyhow, Context, Result};
use block2::RcBlock;
use dispatch2::{DispatchQueue, DispatchRetained};
use objc2::rc::Retained;
use objc2::runtime::{NSObject, NSObjectProtocol, ProtocolObject};
use objc2::{define_class, msg_send, AllocAnyThread, DefinedClass};
use objc2_core_media::{CMSampleBuffer, CMTime};
use objc2_core_video::{
    kCVPixelFormatType_32BGRA, CVPixelBufferGetBaseAddress, CVPixelBufferGetBytesPerRow,
    CVPixelBufferGetHeight, CVPixelBufferGetWidth, CVPixelBufferLockBaseAddress,
    CVPixelBufferLockFlags, CVPixelBufferUnlockBaseAddress,
};
use objc2_foundation::{NSArray, NSError};
use objc2_screen_capture_kit::{
    SCContentFilter, SCShareableContent, SCStream, SCStreamConfiguration, SCStreamOutput,
    SCStreamOutputType,
};
use std::ptr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::time::Duration;
use tracing::{info, warn};

// ──────────────────────────── public API ────────────────────────────

/// A captured screen frame plus frontmost-window metadata.
pub struct Frame {
    pub png_bytes: Vec<u8>,
    pub app_name: String,
    pub window_title: String,
}

/// Audio callback type. Receives 16 kHz mono f32 sample chunks.
pub type SampleCallback = Arc<Mutex<dyn FnMut(&[f32]) + Send>>;

/// Start the shared SCK stream (both audio + video configured). Idempotent:
/// subsequent calls return `Ok(())` without reinitializing. Errors here
/// typically mean Screen Recording permission is denied.
pub fn ensure_started() -> Result<()> {
    let shared = SHARED.get_or_init(|| Mutex::new(None));
    let mut guard = shared.lock().expect("SHARED poisoned");
    if guard.is_some() {
        return Ok(());
    }
    let stream = SharedStream::start()?;
    *guard = Some(stream);
    Ok(())
}

/// Set (or clear) the audio callback. When `None`, incoming audio buffers
/// are discarded at minimal cost.
pub fn set_audio_callback(cb: Option<SampleCallback>) {
    let Some(shared) = SHARED.get() else { return };
    let guard = shared.lock().expect("SHARED poisoned");
    if let Some(stream) = guard.as_ref() {
        *stream.state.audio_callback.lock().unwrap() = cb;
    }
}

/// Pop the most recently encoded video frame, enriched with metadata.
/// Returns `None` if no new frame has arrived since the last `pop`.
pub fn pop_latest_frame() -> Option<Frame> {
    let shared = SHARED.get()?;
    let guard = shared.lock().ok()?;
    let stream = guard.as_ref()?;
    let png_bytes = stream.state.latest_png.lock().ok()?.take()?;
    let (app_name, window_title) = match get_shareable_content_blocking() {
        Ok(content) => frontmost_window(&content),
        Err(e) => {
            warn!(error = %e, "SCShareableContent fetch failed — metadata empty");
            (String::new(), String::new())
        }
    };
    Some(Frame { png_bytes, app_name, window_title })
}

// ──────────────────────────── internals ────────────────────────────

const SCK_AUDIO_INPUT_RATE: u32 = 48_000;
const SCK_AUDIO_TARGET_RATE: u32 = 16_000;
const SCK_AUDIO_CHANNEL_COUNT: isize = 2;
const SCK_AUDIO_RESAMPLE_RATIO: f64 =
    SCK_AUDIO_INPUT_RATE as f64 / SCK_AUDIO_TARGET_RATE as f64;

const SCK_WAIT_TIMEOUT: Duration = Duration::from_secs(5);

static SHARED: OnceLock<Mutex<Option<SharedStream>>> = OnceLock::new();

struct SharedState {
    audio_callback: Mutex<Option<SampleCallback>>,
    audio_phase: Mutex<f64>,
    latest_png: Mutex<Option<Vec<u8>>>,
}

struct SharedStream {
    // Objects kept alive for the stream's lifetime. SCK holds handler
    // references via the ObjC runtime; we also pin them here so Rust
    // doesn't drop them.
    _stream: Retained<SCStream>,
    _output: Retained<SharedOutput>,
    _queue: DispatchRetained<DispatchQueue>,
    state: Arc<SharedState>,
}

// SCStream is thread-safe at the ObjC/GCD level; objc2 is conservative.
unsafe impl Send for SharedStream {}
unsafe impl Sync for SharedStream {}

define_class!(
    #[unsafe(super(NSObject))]
    #[name = "AlvumSCKSharedOutput"]
    #[ivars = Arc<SharedState>]
    struct SharedOutput;

    unsafe impl NSObjectProtocol for SharedOutput {}

    unsafe impl SCStreamOutput for SharedOutput {
        #[unsafe(method(stream:didOutputSampleBuffer:ofType:))]
        #[allow(non_snake_case)]
        fn stream_didOutputSampleBuffer_ofType(
            &self,
            _stream: &SCStream,
            sample_buffer: &CMSampleBuffer,
            of_type: SCStreamOutputType,
        ) {
            let state = self.ivars();
            match of_type {
                SCStreamOutputType::Audio => handle_audio(sample_buffer, state),
                SCStreamOutputType::Screen => handle_screen(sample_buffer, state),
                _ => {}
            }
        }
    }
);

impl SharedOutput {
    fn new(state: Arc<SharedState>) -> Retained<Self> {
        let this = Self::alloc().set_ivars(state);
        unsafe { msg_send![super(this), init] }
    }
}

impl SharedStream {
    fn start() -> Result<Self> {
        info!("starting shared SCK stream (audio + video)");

        let content = get_shareable_content_blocking()
            .context("failed to obtain SCShareableContent")?;

        let displays = unsafe { content.displays() };
        if displays.count() == 0 {
            return Err(anyhow!("no displays available for SCK filter"));
        }
        let display = displays.objectAtIndex(0);
        let w = unsafe { display.width() } as usize;
        let h = unsafe { display.height() } as usize;

        let empty: Retained<NSArray<_>> = NSArray::new();
        let filter = unsafe {
            SCContentFilter::initWithDisplay_excludingWindows(
                SCContentFilter::alloc(),
                &display,
                &empty,
            )
        };

        let config = unsafe { SCStreamConfiguration::new() };
        unsafe {
            // Audio side
            config.setCapturesAudio(true);
            config.setSampleRate(SCK_AUDIO_INPUT_RATE as isize);
            config.setChannelCount(SCK_AUDIO_CHANNEL_COUNT);

            // Video side — capture at display resolution, 2fps (triggers
            // drive disk writes; higher rates burn CPU on frames we drop).
            config.setWidth(w);
            config.setHeight(h);
            config.setPixelFormat(kCVPixelFormatType_32BGRA);
            config.setShowsCursor(true);
            config.setMinimumFrameInterval(CMTime::new(1, 2));
        }

        let stream = unsafe {
            SCStream::initWithFilter_configuration_delegate(
                SCStream::alloc(),
                &filter,
                &config,
                None,
            )
        };

        let state = Arc::new(SharedState {
            audio_callback: Mutex::new(None),
            audio_phase: Mutex::new(0.0_f64),
            latest_png: Mutex::new(None),
        });
        let output = SharedOutput::new(state.clone());
        let output_proto: &ProtocolObject<dyn SCStreamOutput> =
            ProtocolObject::from_ref(&*output);

        let queue = DispatchQueue::new("com.alvum.sck-shared", None);

        unsafe {
            stream
                .addStreamOutput_type_sampleHandlerQueue_error(
                    output_proto,
                    SCStreamOutputType::Audio,
                    Some(&queue),
                )
                .map_err(|e| anyhow!("addStreamOutput(audio) failed: {:?}", e))?;
            stream
                .addStreamOutput_type_sampleHandlerQueue_error(
                    output_proto,
                    SCStreamOutputType::Screen,
                    Some(&queue),
                )
                .map_err(|e| anyhow!("addStreamOutput(screen) failed: {:?}", e))?;
        }

        start_capture_blocking(&stream)?;
        info!(width = w, height = h, "shared SCK stream started");

        Ok(SharedStream {
            _stream: stream,
            _output: output,
            _queue: queue,
            state,
        })
    }
}

// ──────────────────────────── audio path ────────────────────────────

fn handle_audio(sample: &CMSampleBuffer, state: &SharedState) {
    // Fast-path: if no subscriber, do nothing.
    let cb_arc = {
        let guard = state.audio_callback.lock().unwrap();
        match &*guard {
            Some(cb) => cb.clone(),
            None => return,
        }
    };

    let mut phase = match state.audio_phase.lock() {
        Ok(p) => p,
        Err(_) => return,
    };
    let samples = match decode_audio(sample, &mut *phase) {
        Ok(s) if !s.is_empty() => s,
        Ok(_) => return,
        Err(e) => {
            warn!(error = %e, "SCK audio decode failed");
            return;
        }
    };
    drop(phase);

    if let Ok(mut cb) = cb_arc.lock() {
        cb(&samples);
    }
}

fn decode_audio(sample: &CMSampleBuffer, phase: &mut f64) -> Result<Vec<f32>> {
    let interleaved = extract_f32_stereo(sample)
        .context("failed to extract f32 stereo from CMSampleBuffer")?;
    if interleaved.is_empty() {
        return Ok(Vec::new());
    }
    let mono = stereo_to_mono(&interleaved);
    Ok(resample_linear(&mono, phase))
}

fn extract_f32_stereo(sample: &CMSampleBuffer) -> Result<Vec<f32>> {
    let n_samples = unsafe { sample.num_samples() } as usize;
    if n_samples == 0 {
        return Ok(Vec::new());
    }
    let block = unsafe { sample.data_buffer() }
        .context("CMSampleBuffer has no CMBlockBuffer")?;

    let mut total_len: usize = 0;
    let mut ptr_out: *mut i8 = ptr::null_mut();
    let status = unsafe {
        block.data_pointer(0, ptr::null_mut(), &mut total_len, &mut ptr_out)
    };
    if status != 0 {
        anyhow::bail!("CMBlockBufferGetDataPointer returned status {}", status);
    }
    if ptr_out.is_null() || total_len == 0 {
        anyhow::bail!("CMBlockBuffer returned null pointer or zero length");
    }

    let expected = n_samples * 2 * 4;
    if total_len < expected {
        anyhow::bail!(
            "short CMBlockBuffer: expected ≥{} bytes, got {} (n_samples={})",
            expected, total_len, n_samples
        );
    }

    let out_len = n_samples * 2;
    let mut out = Vec::<f32>::with_capacity(out_len);
    unsafe {
        ptr::copy_nonoverlapping(ptr_out as *const f32, out.as_mut_ptr(), out_len);
        out.set_len(out_len);
    }
    Ok(out)
}

fn stereo_to_mono(interleaved: &[f32]) -> Vec<f32> {
    interleaved
        .chunks_exact(2)
        .map(|ch| 0.5 * (ch[0] + ch[1]))
        .collect()
}

fn resample_linear(input: &[f32], phase: &mut f64) -> Vec<f32> {
    if input.is_empty() {
        return Vec::new();
    }
    let mut out = Vec::with_capacity((input.len() as f64 / SCK_AUDIO_RESAMPLE_RATIO) as usize + 1);
    let mut i = *phase;
    while i < input.len() as f64 {
        let idx = i as usize;
        let frac = (i - idx as f64) as f32;
        let s0 = input[idx];
        let s1 = if idx + 1 < input.len() { input[idx + 1] } else { s0 };
        out.push(s0 + (s1 - s0) * frac);
        i += SCK_AUDIO_RESAMPLE_RATIO;
    }
    *phase = i - input.len() as f64;
    out
}

// ──────────────────────────── screen path ────────────────────────────

fn handle_screen(sample: &CMSampleBuffer, state: &SharedState) {
    match encode_png_from_sample(sample) {
        Ok(png) => {
            if let Ok(mut slot) = state.latest_png.lock() {
                *slot = Some(png);
            }
        }
        Err(e) => warn!(error = %e, "SCK frame encode failed"),
    }
}

fn encode_png_from_sample(sample: &CMSampleBuffer) -> Result<Vec<u8>> {
    let image = unsafe { sample.image_buffer() }
        .context("CMSampleBuffer has no CVImageBuffer")?;
    let pixel_buffer = &*image;

    let lock_status =
        unsafe { CVPixelBufferLockBaseAddress(pixel_buffer, CVPixelBufferLockFlags::ReadOnly) };
    if lock_status != 0 {
        return Err(anyhow!("CVPixelBufferLockBaseAddress returned {}", lock_status));
    }

    let result: Result<Vec<u8>> = (|| {
        let width = CVPixelBufferGetWidth(pixel_buffer);
        let height = CVPixelBufferGetHeight(pixel_buffer);
        let bytes_per_row = CVPixelBufferGetBytesPerRow(pixel_buffer);
        let base = CVPixelBufferGetBaseAddress(pixel_buffer);
        if base.is_null() {
            return Err(anyhow!("CVPixelBufferGetBaseAddress returned null"));
        }

        let mut rgba = Vec::with_capacity(width * height * 4);
        let src = base as *const u8;
        for y in 0..height {
            let row_start = unsafe { src.add(y * bytes_per_row) };
            for x in 0..width {
                let p = unsafe { row_start.add(x * 4) };
                let b = unsafe { *p };
                let g = unsafe { *p.add(1) };
                let r = unsafe { *p.add(2) };
                let a = unsafe { *p.add(3) };
                rgba.extend_from_slice(&[r, g, b, a]);
            }
        }

        let mut png = Vec::new();
        image::ImageEncoder::write_image(
            image::codecs::png::PngEncoder::new(&mut png),
            &rgba,
            width as u32,
            height as u32,
            image::ExtendedColorType::Rgba8,
        )
        .context("PNG encoding failed")?;
        Ok(png)
    })();

    unsafe { CVPixelBufferUnlockBaseAddress(pixel_buffer, CVPixelBufferLockFlags::ReadOnly) };
    result
}

fn frontmost_window(content: &SCShareableContent) -> (String, String) {
    let windows = unsafe { content.windows() };
    for i in 0..windows.count() {
        let window = windows.objectAtIndex(i);
        if !unsafe { window.isOnScreen() } {
            continue;
        }
        if unsafe { window.windowLayer() } != 0 {
            continue;
        }
        let app_name = match unsafe { window.owningApplication() } {
            Some(app) => unsafe { app.applicationName() }.to_string(),
            None => continue,
        };
        if app_name.is_empty() || app_name == "Window Server" {
            continue;
        }
        let window_title = unsafe { window.title() }
            .map(|s| s.to_string())
            .unwrap_or_default();
        return (app_name, window_title);
    }
    (String::new(), String::new())
}

// ──────────────────────────── async→sync bridges ────────────────────────────

fn get_shareable_content_blocking() -> Result<Retained<SCShareableContent>> {
    type Slot = Arc<Mutex<Option<Result<Retained<SCShareableContent>, String>>>>;
    let slot: Slot = Arc::new(Mutex::new(None));
    let signal = Arc::new((Mutex::new(false), Condvar::new()));

    let slot_cb = slot.clone();
    let signal_cb = signal.clone();
    let block = RcBlock::new(
        move |content: *mut SCShareableContent, error: *mut NSError| {
            let result = if content.is_null() {
                let msg = if !error.is_null() {
                    unsafe { (*error).localizedDescription().to_string() }
                } else {
                    "unknown SCShareableContent error".to_string()
                };
                Err(msg)
            } else {
                match unsafe { Retained::retain(content) } {
                    Some(r) => Ok(r),
                    None => Err("failed to retain SCShareableContent".into()),
                }
            };
            *slot_cb.lock().unwrap() = Some(result);
            let (lock, cvar) = &*signal_cb;
            *lock.lock().unwrap() = true;
            cvar.notify_one();
        },
    );

    unsafe { SCShareableContent::getShareableContentWithCompletionHandler(&block) };

    let (lock, cvar) = &*signal;
    let done = lock.lock().unwrap();
    let (guard, _res) = cvar.wait_timeout(done, SCK_WAIT_TIMEOUT).unwrap();
    let done = guard;
    if !*done {
        return Err(anyhow!(
            "SCShareableContent did not return within {:?}",
            SCK_WAIT_TIMEOUT
        ));
    }
    drop(done);

    slot.lock()
        .unwrap()
        .take()
        .ok_or_else(|| anyhow!("SCShareableContent callback produced no result"))?
        .map_err(|e| anyhow!("SCShareableContent: {}", e))
}

fn start_capture_blocking(stream: &SCStream) -> Result<()> {
    let done = Arc::new((Mutex::new(false), Condvar::new(), AtomicBool::new(true)));
    let err_slot: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));

    let done_cb = done.clone();
    let err_cb = err_slot.clone();
    let block = RcBlock::new(move |err: *mut NSError| {
        if !err.is_null() {
            let msg = unsafe { (*err).localizedDescription().to_string() };
            *err_cb.lock().unwrap() = Some(msg);
            done_cb.2.store(false, Ordering::SeqCst);
        }
        *done_cb.0.lock().unwrap() = true;
        done_cb.1.notify_one();
    });

    unsafe { stream.startCaptureWithCompletionHandler(Some(&block)) };

    let (lock, cvar, ok_flag) = &*done;
    let finished = lock.lock().unwrap();
    let (guard, _res) = cvar.wait_timeout(finished, SCK_WAIT_TIMEOUT).unwrap();
    let finished = guard;
    if !*finished {
        return Err(anyhow!("SCStream start did not complete within {:?}", SCK_WAIT_TIMEOUT));
    }
    if !ok_flag.load(Ordering::SeqCst) {
        let msg = err_slot.lock().unwrap().take().unwrap_or_else(|| "unknown".into());
        return Err(anyhow!("SCStream start error: {}", msg));
    }
    Ok(())
}

// ──────────────────────────── tests ────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stereo_to_mono_averages_channels() {
        let interleaved = [0.2_f32, 0.8_f32, -0.4_f32, 0.4_f32];
        let mono = stereo_to_mono(&interleaved);
        assert_eq!(mono, vec![0.5_f32, 0.0_f32]);
    }

    #[test]
    fn resample_48k_to_16k_drops_two_of_three() {
        let input: Vec<f32> = (0..48_000).map(|i| i as f32 * 0.001).collect();
        let mut phase = 0.0_f64;
        let out = resample_linear(&input, &mut phase);
        assert!((15_999..=16_001).contains(&out.len()),
            "expected ~16000, got {}", out.len());
    }

    #[test]
    fn resample_phase_carries_across_buffers() {
        let single: Vec<f32> = (0..6_000).map(|i| i as f32).collect();
        let mut phase_a = 0.0_f64;
        let mut phase_b = 0.0_f64;

        let combined = resample_linear(&single, &mut phase_a);
        let (half1, half2) = single.split_at(3_000);
        let mut split = resample_linear(half1, &mut phase_b);
        split.extend(resample_linear(half2, &mut phase_b));

        let diff = (combined.len() as i64 - split.len() as i64).abs();
        assert!(diff <= 1, "split={} vs combined={}", split.len(), combined.len());
    }
}
