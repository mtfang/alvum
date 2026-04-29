//! The shared SCStream itself: ObjC delegate (`SharedOutput`), the
//! per-process `SHARED` slot, and the public lifecycle/access API
//! (start, restart, callback registration, frame pop, display sync).
//!
//! Audio and screen frame handling each live in their own module; this
//! file is just the wiring that owns the SCStream and fans incoming
//! sample buffers out to those handlers.

use anyhow::{Context, Result, anyhow};
use dispatch2::{DispatchQueue, DispatchRetained};
use objc2::rc::Retained;
use objc2::runtime::{NSObject, NSObjectProtocol, ProtocolObject};
use objc2::{AllocAnyThread, DefinedClass, define_class, msg_send};
use objc2_core_media::{CMSampleBuffer, CMTime};
use objc2_core_video::kCVPixelFormatType_32BGRA;
use objc2_screen_capture_kit::{
    SCStream, SCStreamConfiguration, SCStreamOutput, SCStreamOutputType,
};
use std::sync::{Arc, Mutex, OnceLock};
use tracing::{info, warn};

use crate::audio::handle_audio;
use crate::display_watcher;
use crate::filter::{build_filter, current_config};
use crate::helpers::{
    get_shareable_content_blocking, start_capture_blocking, update_content_filter_blocking,
};
use crate::screen::{find_active_display, frontmost_window, handle_screen};

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
///
/// First successful call also starts the display-reconfiguration watcher
/// (once per process). The watcher rebuilds the stream proactively when
/// macOS fires a display event — sleep/wake, monitor connect/disconnect,
/// resolution or mirror change — which otherwise leave the stream in a
/// half-broken state that emits empty CMSampleBuffers forever.
pub fn ensure_started() -> Result<()> {
    let shared = SHARED.get_or_init(|| Mutex::new(None));
    let mut guard = shared.lock().expect("SHARED poisoned");
    if guard.is_some() {
        return Ok(());
    }
    let stream = SharedStream::start()?;
    *guard = Some(stream);
    drop(guard);
    display_watcher::start_once();
    Ok(())
}

/// Tear down and rebuild the shared SCK stream in-place. Preserves the
/// registered audio callback and filter config so subscribers keep
/// receiving samples across the restart.
pub fn restart() -> Result<()> {
    info!("restarting shared SCK stream");
    let shared = SHARED.get_or_init(|| Mutex::new(None));
    // Lift the audio callback before dropping the old state so the new
    // stream picks it up immediately rather than the audio-system
    // source having to re-register.
    let preserved_cb = {
        let guard = shared.lock().expect("SHARED poisoned");
        guard
            .as_ref()
            .and_then(|s| s.state.audio_callback.lock().ok().map(|g| g.clone()))
            .flatten()
    };
    {
        let mut guard = shared.lock().expect("SHARED poisoned");
        *guard = None; // drop the old SharedStream
    }
    // ensure_started re-reads FILTER_CONFIG and rebuilds from scratch.
    ensure_started()?;
    if let Some(cb) = preserved_cb {
        set_audio_callback(Some(cb));
    }
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
    Some(Frame {
        png_bytes,
        app_name,
        window_title,
    })
}

/// If the frontmost window has moved to a different display than the one
/// the SCK filter currently captures, swap the filter to that display.
/// Drops any stale frame already in the slot (it was from the old display).
///
/// Returns `Ok(true)` if a swap happened, `Ok(false)` if not needed. Called
/// opportunistically by the screen source before each trigger-driven frame
/// pop, so multi-monitor users see the display they're actively working on.
pub fn sync_active_display() -> Result<bool> {
    let Some(shared) = SHARED.get() else {
        return Ok(false);
    };
    let guard = shared.lock().expect("SHARED poisoned");
    let Some(stream) = guard.as_ref() else {
        return Ok(false);
    };

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

    info!(
        display_id = target_id,
        "SCK filter swapped to active display"
    );
    Ok(true)
}

// ──────────────────────────── internals ────────────────────────────

const SCK_AUDIO_SAMPLE_RATE: u32 = 16_000;
const SCK_AUDIO_CHANNEL_COUNT: isize = 2;

static SHARED: OnceLock<Mutex<Option<SharedStream>>> = OnceLock::new();

pub(crate) struct SharedState {
    pub(crate) audio_callback: Mutex<Option<SampleCallback>>,
    pub(crate) latest_png: Mutex<Option<Vec<u8>>>,
    /// CGDirectDisplayID of the display whose content is currently flowing
    /// through the stream's SCContentFilter. Updated by `sync_active_display`.
    pub(crate) current_display_id: Mutex<u32>,
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

        let content =
            get_shareable_content_blocking().context("failed to obtain SCShareableContent")?;

        let displays = unsafe { content.displays() };
        if displays.count() == 0 {
            return Err(anyhow!("no displays available for SCK filter"));
        }
        // Seed from the display the frontmost window is currently on — for
        // single-display users this is trivially displays[0], for multi-
        // monitor users it starts on whichever display they're actively
        // working in instead of a fixed index.
        let initial_display =
            find_active_display(&content).unwrap_or_else(|| displays.objectAtIndex(0));
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
        let output_proto: &ProtocolObject<dyn SCStreamOutput> = ProtocolObject::from_ref(&*output);

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
