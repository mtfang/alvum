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
    SCContentFilter, SCDisplay, SCShareableContent, SCStream, SCStreamConfiguration,
    SCStreamOutput, SCStreamOutputType, SCWindow,
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

/// If the frontmost window has moved to a different display than the one
/// the SCK filter currently captures, swap the filter to that display.
/// Drops any stale frame already in the slot (it was from the old display).
///
/// Returns `Ok(true)` if a swap happened, `Ok(false)` if not needed. Called
/// opportunistically by the screen source before each trigger-driven frame
/// pop, so multi-monitor users see the display they're actively working on.
pub fn sync_active_display() -> Result<bool> {
    let Some(shared) = SHARED.get() else { return Ok(false); };
    let guard = shared.lock().expect("SHARED poisoned");
    let Some(stream) = guard.as_ref() else { return Ok(false); };

    let content = get_shareable_content_blocking()?;
    let Some(target_display) = find_active_display(&content) else {
        // No focused window on any known display — keep current filter.
        return Ok(false);
    };
    let target_id = unsafe { target_display.displayID() };

    {
        let current = stream.state.current_display_id.lock().unwrap();
        if *current == target_id {
            return Ok(false);
        }
    }

    let empty: Retained<NSArray<_>> = NSArray::new();
    let new_filter = unsafe {
        SCContentFilter::initWithDisplay_excludingWindows(
            SCContentFilter::alloc(),
            &target_display,
            &empty,
        )
    };

    update_content_filter_blocking(&stream._stream, &new_filter)?;

    *stream.state.current_display_id.lock().unwrap() = target_id;
    // Drop any frame still in the slot — it was captured from the old
    // display and would misrepresent where the user actually is.
    *stream.state.latest_png.lock().unwrap() = None;

    info!(display_id = target_id, "SCK filter swapped to active display");
    Ok(true)
}

// ──────────────────────────── internals ────────────────────────────

const SCK_AUDIO_SAMPLE_RATE: u32 = 16_000;
const SCK_AUDIO_CHANNEL_COUNT: isize = 2;

const SCK_WAIT_TIMEOUT: Duration = Duration::from_secs(5);

static SHARED: OnceLock<Mutex<Option<SharedStream>>> = OnceLock::new();

struct SharedState {
    audio_callback: Mutex<Option<SampleCallback>>,
    latest_png: Mutex<Option<Vec<u8>>>,
    /// CGDirectDisplayID of the display whose content is currently flowing
    /// through the stream's SCContentFilter. Updated by `sync_active_display`.
    current_display_id: Mutex<u32>,
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
        // Seed from the display the frontmost window is currently on — for
        // single-display users this is trivially displays[0], for multi-
        // monitor users it starts on whichever display they're actively
        // working in instead of a fixed index.
        let initial_display = find_active_display(&content)
            .unwrap_or_else(|| displays.objectAtIndex(0));
        let initial_display_id = unsafe { initial_display.displayID() };
        let w = unsafe { initial_display.width() } as usize;
        let h = unsafe { initial_display.height() } as usize;

        let empty: Retained<NSArray<_>> = NSArray::new();
        let filter = unsafe {
            SCContentFilter::initWithDisplay_excludingWindows(
                SCContentFilter::alloc(),
                &initial_display,
                &empty,
            )
        };

        let config = unsafe { SCStreamConfiguration::new() };
        unsafe {
            // Audio side — ask SCK for 16 kHz directly. Apple's internal
            // resampler has proper anti-alias filtering, unlike the naive
            // linear decimator we used to run in-process.
            config.setCapturesAudio(true);
            config.setSampleRate(SCK_AUDIO_SAMPLE_RATE as isize);
            config.setChannelCount(SCK_AUDIO_CHANNEL_COUNT);

            // Video side — capture at display resolution, 1fps. Triggers
            // drive disk writes, so higher rates only burn CPU on frames
            // we discard. 1fps is the lowest useful floor (the trigger
            // loop expects a fresh-ish frame to be waiting when it fires).
            config.setWidth(w);
            config.setHeight(h);
            config.setPixelFormat(kCVPixelFormatType_32BGRA);
            config.setShowsCursor(true);
            config.setMinimumFrameInterval(CMTime::new(1, 1));
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
            latest_png: Mutex::new(None),
            current_display_id: Mutex::new(initial_display_id),
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

    let samples = match decode_audio(sample) {
        Ok(s) if !s.is_empty() => s,
        Ok(_) => return,
        Err(e) => {
            warn!(error = %e, "SCK audio decode failed");
            return;
        }
    };

    if let Ok(mut cb) = cb_arc.lock() {
        cb(&samples);
    }
}

fn decode_audio(sample: &CMSampleBuffer) -> Result<Vec<f32>> {
    let interleaved = extract_f32_stereo(sample)
        .context("failed to extract f32 stereo from CMSampleBuffer")?;
    if interleaved.is_empty() {
        return Ok(Vec::new());
    }
    Ok(stereo_to_mono(&interleaved))
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

/// Return the frontmost on-screen regular-app window's bounding rect's
/// center, or None if no such window exists.
fn frontmost_window_center(content: &SCShareableContent) -> Option<(f64, f64)> {
    let windows = unsafe { content.windows() };
    for i in 0..windows.count() {
        let window = windows.objectAtIndex(i);
        if !is_frontmost_candidate(&window) {
            continue;
        }
        let frame = unsafe { window.frame() };
        let cx = frame.origin.x + frame.size.width / 2.0;
        let cy = frame.origin.y + frame.size.height / 2.0;
        return Some((cx, cy));
    }
    None
}

/// Same predicate frontmost_window() uses for picking the reported window.
fn is_frontmost_candidate(window: &SCWindow) -> bool {
    if !unsafe { window.isOnScreen() } {
        return false;
    }
    if unsafe { window.windowLayer() } != 0 {
        return false;
    }
    let Some(app) = (unsafe { window.owningApplication() }) else {
        return false;
    };
    let name = unsafe { app.applicationName() }.to_string();
    !(name.is_empty() || name == "Window Server")
}

/// Find the SCDisplay whose frame contains the frontmost window's center.
/// None if no frontmost window, or none of the displays contain it.
fn find_active_display(content: &SCShareableContent) -> Option<Retained<SCDisplay>> {
    let (cx, cy) = frontmost_window_center(content)?;
    let displays = unsafe { content.displays() };
    for i in 0..displays.count() {
        let d = displays.objectAtIndex(i);
        let f = unsafe { d.frame() };
        let x0 = f.origin.x;
        let y0 = f.origin.y;
        let x1 = x0 + f.size.width;
        let y1 = y0 + f.size.height;
        if cx >= x0 && cx < x1 && cy >= y0 && cy < y1 {
            return Some(d);
        }
    }
    None
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

fn update_content_filter_blocking(
    stream: &SCStream,
    new_filter: &SCContentFilter,
) -> Result<()> {
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

    unsafe { stream.updateContentFilter_completionHandler(new_filter, Some(&block)) };

    let (lock, cvar, ok_flag) = &*done;
    let finished = lock.lock().unwrap();
    let (guard, _res) = cvar.wait_timeout(finished, SCK_WAIT_TIMEOUT).unwrap();
    let finished = guard;
    if !*finished {
        return Err(anyhow!(
            "SCStream updateContentFilter did not complete within {:?}",
            SCK_WAIT_TIMEOUT
        ));
    }
    if !ok_flag.load(Ordering::SeqCst) {
        let msg = err_slot.lock().unwrap().take().unwrap_or_else(|| "unknown".into());
        return Err(anyhow!("SCStream updateContentFilter error: {}", msg));
    }
    Ok(())
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
    fn stereo_to_mono_passthrough_does_not_mutate_length() {
        // SCK delivers 16 kHz stereo directly; decode_audio just downmixes.
        // Half the sample count, no resampling involved.
        let stereo: Vec<f32> = (0..3200).map(|i| (i as f32) * 0.001).collect();
        let mono = stereo_to_mono(&stereo);
        assert_eq!(mono.len(), 1600);
    }
}
