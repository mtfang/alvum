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
use objc2_core_audio_types::{kAudioFormatFlagIsNonInterleaved, AudioStreamBasicDescription};
use objc2_core_media::{CMAudioFormatDescriptionGetStreamBasicDescription, CMSampleBuffer, CMTime};
use objc2_core_video::{
    kCVPixelFormatType_32BGRA, CVPixelBufferGetBaseAddress, CVPixelBufferGetBytesPerRow,
    CVPixelBufferGetHeight, CVPixelBufferGetWidth, CVPixelBufferLockBaseAddress,
    CVPixelBufferLockFlags, CVPixelBufferUnlockBaseAddress,
};
use objc2_foundation::{NSArray, NSError};
use objc2_screen_capture_kit::{
    SCContentFilter, SCDisplay, SCRunningApplication, SCShareableContent, SCStream,
    SCStreamConfiguration, SCStreamOutput, SCStreamOutputType, SCWindow,
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

    let cfg = current_config();
    let new_filter = build_filter(&content, &target_display, &cfg)
        .context("rebuild SCContentFilter on display swap")?;

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

/// Which apps the SCK content filter should let through.
///
/// - `Exclude { ... }` (default) — capture everything except matching apps.
///   Empty lists = open world (capture all).
/// - `Include { ... }` — whitelist mode: capture ONLY matching apps. Empty
///   lists with Include is a degenerate "capture nothing" configuration;
///   `build_filter` logs a warning and falls back to open-world so the
///   daemon doesn't silently record nothing.
#[derive(Debug, Clone)]
pub enum AppFilter {
    Exclude { names: Vec<String>, bundle_ids: Vec<String> },
    Include { names: Vec<String>, bundle_ids: Vec<String> },
}

impl Default for AppFilter {
    fn default() -> Self {
        AppFilter::Exclude { names: Vec::new(), bundle_ids: Vec::new() }
    }
}

/// Pre-start configuration for the shared SCK stream. Set via
/// [`configure`] before [`ensure_started`] is first called.
#[derive(Debug, Clone, Default)]
pub struct SharedStreamConfig {
    pub filter: AppFilter,
}

static FILTER_CONFIG: OnceLock<Mutex<SharedStreamConfig>> = OnceLock::new();

/// Provide the filter config that [`ensure_started`] will use on first
/// start. Safe to call multiple times before start; last-writer-wins.
/// Idempotent after start, but the filter is not reshaped at runtime —
/// callers that need a live reshape should call `sync_active_display` or
/// trigger a display swap.
pub fn configure(cfg: SharedStreamConfig) {
    let slot = FILTER_CONFIG.get_or_init(|| Mutex::new(SharedStreamConfig::default()));
    *slot.lock().unwrap() = cfg;
}

fn current_config() -> SharedStreamConfig {
    FILTER_CONFIG
        .get()
        .map(|m| m.lock().unwrap().clone())
        .unwrap_or_default()
}

#[doc(hidden)]
pub fn snapshot_config_for_test() -> SharedStreamConfig {
    current_config()
}

/// Pure rule-matching helper used by both include and exclude filter modes.
/// Given name/bundle rule lists and a snapshot of (app_name, bundle_id)
/// tuples, return the indices of matching apps. Name match is
/// case-insensitive; bundle match is exact. Names and bundle IDs are
/// OR'd — an app matching either list is a hit. Each matching app
/// appears exactly once in the result (no duplicate indices).
fn match_apps_by_rules(
    names: &[String],
    bundle_ids: &[String],
    apps: &[(String, String)],
) -> Vec<usize> {
    let names_lower: Vec<String> = names.iter().map(|n| n.to_lowercase()).collect();
    let mut hits: Vec<usize> = Vec::new();
    for (i, (name, bundle)) in apps.iter().enumerate() {
        let name_hit = names_lower.iter().any(|n| n == &name.to_lowercase());
        let bundle_hit = bundle_ids.iter().any(|b| b == bundle);
        if name_hit || bundle_hit {
            hits.push(i);
        }
    }
    hits
}

/// Construct an SCContentFilter according to `cfg`. Returns the
/// wide-open `excludingWindows` filter when no rules apply or no apps
/// match — so a misconfigured exclude/include list can't silently
/// capture nothing.
fn build_filter(
    content: &SCShareableContent,
    display: &SCDisplay,
    cfg: &SharedStreamConfig,
) -> Result<Retained<SCContentFilter>> {
    let empty_windows: Retained<NSArray<SCWindow>> = NSArray::new();

    // Early-exit open-world path: default Exclude with no rules → the
    // existing wide-open filter, no app enumeration needed.
    if let AppFilter::Exclude { names, bundle_ids } = &cfg.filter {
        if names.is_empty() && bundle_ids.is_empty() {
            return Ok(unsafe {
                SCContentFilter::initWithDisplay_excludingWindows(
                    SCContentFilter::alloc(),
                    display,
                    &empty_windows,
                )
            });
        }
    }

    let apps = unsafe { content.applications() };
    let mut tuples: Vec<(String, String)> = Vec::with_capacity(apps.count());
    let mut app_vec: Vec<Retained<SCRunningApplication>> = Vec::with_capacity(apps.count());
    for i in 0..apps.count() {
        let app = apps.objectAtIndex(i);
        let name = unsafe { app.applicationName() }.to_string();
        let bundle = unsafe { app.bundleIdentifier() }.to_string();
        tuples.push((name, bundle));
        app_vec.push(app);
    }

    let (names, bundle_ids, is_include) = match &cfg.filter {
        AppFilter::Exclude { names, bundle_ids } => (names, bundle_ids, false),
        AppFilter::Include { names, bundle_ids } => {
            if names.is_empty() && bundle_ids.is_empty() {
                warn!(
                    "AppFilter::Include with empty rules = capture-nothing; \
                     falling back to open world"
                );
                return Ok(unsafe {
                    SCContentFilter::initWithDisplay_excludingWindows(
                        SCContentFilter::alloc(),
                        display,
                        &empty_windows,
                    )
                });
            }
            (names, bundle_ids, true)
        }
    };

    let indices = match_apps_by_rules(names, bundle_ids, &tuples);
    if indices.is_empty() {
        warn!(
            names = ?names,
            bundles = ?bundle_ids,
            mode = if is_include { "include" } else { "exclude" },
            "no running apps matched SCK filter rules; falling back to open world"
        );
        return Ok(unsafe {
            SCContentFilter::initWithDisplay_excludingWindows(
                SCContentFilter::alloc(),
                display,
                &empty_windows,
            )
        });
    }

    let matched_refs: Vec<&SCRunningApplication> =
        indices.iter().map(|&i| app_vec[i].as_ref()).collect();
    let matched_array: Retained<NSArray<SCRunningApplication>> =
        NSArray::from_slice(&matched_refs);

    let matched_names: Vec<&String> = indices.iter().map(|&i| &tuples[i].0).collect();
    if is_include {
        info!(included = ?matched_names, "SCK filter including only");
        Ok(unsafe {
            SCContentFilter::initWithDisplay_includingApplications_exceptingWindows(
                SCContentFilter::alloc(),
                display,
                &matched_array,
                &empty_windows,
            )
        })
    } else {
        info!(excluded = ?matched_names, "SCK filter excluding apps");
        Ok(unsafe {
            SCContentFilter::initWithDisplay_excludingApplications_exceptingWindows(
                SCContentFilter::alloc(),
                display,
                &matched_array,
                &empty_windows,
            )
        })
    }
}

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

        let cfg = current_config();
        let filter = build_filter(&content, &initial_display, &cfg)
            .context("failed to build SCContentFilter")?;

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

/// Format-aware decode. Reads the CMSampleBuffer's AudioStreamBasicDescription
/// so both interleaved and planar f32 layouts are handled correctly. SCK on
/// macOS 14+ delivers stereo as planar; the old code assumed interleaved,
/// which produced chipmunk-distorted audio (first half of output was
/// decimated L, second half was decimated R, pitched up 2×).
fn decode_audio(sample: &CMSampleBuffer) -> Result<Vec<f32>> {
    let n_frames = unsafe { sample.num_samples() } as usize;
    if n_frames == 0 {
        return Ok(Vec::new());
    }

    let fmt_desc = unsafe { sample.format_description() }
        .context("CMSampleBuffer has no format description")?;
    let asbd_ptr = unsafe {
        CMAudioFormatDescriptionGetStreamBasicDescription(fmt_desc.as_ref())
    };
    if asbd_ptr.is_null() {
        anyhow::bail!("format description has no AudioStreamBasicDescription");
    }
    let asbd: AudioStreamBasicDescription = unsafe { *asbd_ptr };

    // One-shot format log per process to aid diagnostics across macOS versions.
    static LOGGED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    if !LOGGED.swap(true, std::sync::atomic::Ordering::SeqCst) {
        info!(
            sample_rate = asbd.mSampleRate,
            channels = asbd.mChannelsPerFrame,
            bits_per_channel = asbd.mBitsPerChannel,
            bytes_per_frame = asbd.mBytesPerFrame,
            format_flags = format!("{:#x}", asbd.mFormatFlags),
            non_interleaved = (asbd.mFormatFlags & kAudioFormatFlagIsNonInterleaved) != 0,
            "SCK audio format"
        );
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

    let channels = asbd.mChannelsPerFrame as usize;
    let sample_bytes = (asbd.mBitsPerChannel / 8) as usize;
    if sample_bytes != 4 {
        anyhow::bail!(
            "unsupported audio sample size: {} bits (expected 32-bit float)",
            asbd.mBitsPerChannel
        );
    }
    let expected = n_frames * channels * sample_bytes;
    if total_len < expected {
        anyhow::bail!(
            "short CMBlockBuffer: expected ≥{} bytes, got {} (n_frames={} channels={})",
            expected, total_len, n_frames, channels
        );
    }

    let is_non_interleaved = (asbd.mFormatFlags & kAudioFormatFlagIsNonInterleaved) != 0;
    let ptr_f32 = ptr_out as *const f32;

    // Downmix to mono directly so the downstream encoder sees one rate × one channel.
    let mut mono = Vec::with_capacity(n_frames);
    unsafe {
        if channels == 1 {
            let src = std::slice::from_raw_parts(ptr_f32, n_frames);
            mono.extend_from_slice(src);
        } else if is_non_interleaved {
            // Planar: [ch0 × n_frames][ch1 × n_frames]...
            let mut plane_ptrs: Vec<*const f32> = Vec::with_capacity(channels);
            for c in 0..channels {
                plane_ptrs.push(ptr_f32.add(c * n_frames));
            }
            let scale = 1.0 / channels as f32;
            for i in 0..n_frames {
                let mut sum = 0.0_f32;
                for &p in &plane_ptrs {
                    sum += *p.add(i);
                }
                mono.push(sum * scale);
            }
        } else {
            // Interleaved: [ch0,ch1,...,chN,ch0,ch1,...]
            let scale = 1.0 / channels as f32;
            for i in 0..n_frames {
                let mut sum = 0.0_f32;
                for c in 0..channels {
                    sum += *ptr_f32.add(i * channels + c);
                }
                mono.push(sum * scale);
            }
        }
    }

    Ok(mono)
}

/// Retained only for the unit-test fixture; the live decode path no longer
/// needs an intermediate interleaved buffer (decode_audio emits mono directly).
#[cfg(test)]
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

    #[test]
    fn match_apps_empty_rules_returns_empty() {
        let apps = vec![("Music".into(), "com.apple.Music".into())];
        let idx = match_apps_by_rules(&[], &[], &apps);
        assert!(idx.is_empty());
    }

    #[test]
    fn match_apps_by_name_case_insensitive() {
        let apps = vec![
            ("Music".into(), "com.apple.Music".into()),
            ("Safari".into(), "com.apple.Safari".into()),
        ];
        let idx = match_apps_by_rules(&["music".to_string()], &[], &apps);
        assert_eq!(idx, vec![0]);
    }

    #[test]
    fn match_apps_by_bundle_id() {
        let apps = vec![
            ("Music".into(), "com.apple.Music".into()),
            ("Spotify".into(), "com.spotify.client".into()),
        ];
        let idx = match_apps_by_rules(&[], &["com.spotify.client".to_string()], &apps);
        assert_eq!(idx, vec![1]);
    }

    #[test]
    fn match_apps_by_name_and_bundle_unions() {
        let apps = vec![
            ("Music".into(), "com.apple.Music".into()),
            ("Spotify".into(), "com.spotify.client".into()),
            ("Safari".into(), "com.apple.Safari".into()),
        ];
        let idx = match_apps_by_rules(
            &["music".to_string()],
            &["com.spotify.client".to_string()],
            &apps,
        );
        assert_eq!(idx, vec![0, 1]);
    }

    #[test]
    fn match_apps_no_match_returns_empty() {
        let apps = vec![("Safari".into(), "com.apple.Safari".into())];
        let idx = match_apps_by_rules(
            &["music".to_string()],
            &["com.apple.Music".to_string()],
            &apps,
        );
        assert!(idx.is_empty());
    }

    #[test]
    fn match_apps_deduplicates_when_name_and_bundle_both_hit_same_index() {
        let apps = vec![("Music".into(), "com.apple.Music".into())];
        let idx = match_apps_by_rules(
            &["music".to_string()],
            &["com.apple.Music".to_string()],
            &apps,
        );
        assert_eq!(idx, vec![0], "one app should yield one index even if both rules match");
    }
}
