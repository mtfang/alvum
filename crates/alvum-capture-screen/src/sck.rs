//! ScreenCaptureKit-driven screen capture. Owns an SCStream (video-only) and
//! keeps the most recent PNG-encoded frame in a shared slot. The existing
//! `ScreenSource` trigger loop reads the slot on focus-change / idle events
//! and drives disk writes at its own cadence — we do NOT encode every frame.
//!
//! Running SCK at a low internal frame rate (2 fps) is deliberate: we only
//! ever snapshot on trigger events, so higher rates waste CPU on PNGs we
//! throw away.

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
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;
use tracing::{info, warn};

/// Readonly-after-fetch frame with metadata. Produced by `SckScreenStream::latest`.
pub struct Frame {
    pub png_bytes: Vec<u8>,
    pub app_name: String,
    pub window_title: String,
}

const SCK_WAIT_TIMEOUT: Duration = Duration::from_secs(5);

/// Shared ivars owned by the Objective-C delegate class.
struct DelegateState {
    latest_png: Mutex<Option<Vec<u8>>>,
}

define_class!(
    #[unsafe(super(NSObject))]
    #[name = "AlvumSCKScreenOutput"]
    #[ivars = Arc<DelegateState>]
    struct SckScreenOutput;

    unsafe impl NSObjectProtocol for SckScreenOutput {}

    unsafe impl SCStreamOutput for SckScreenOutput {
        #[unsafe(method(stream:didOutputSampleBuffer:ofType:))]
        #[allow(non_snake_case)]
        fn stream_didOutputSampleBuffer_ofType(
            &self,
            _stream: &SCStream,
            sample_buffer: &CMSampleBuffer,
            of_type: SCStreamOutputType,
        ) {
            if of_type != SCStreamOutputType::Screen {
                return;
            }
            match encode_png_from_sample(sample_buffer) {
                Ok(png) => {
                    if let Ok(mut slot) = self.ivars().latest_png.lock() {
                        *slot = Some(png);
                    }
                }
                Err(e) => warn!(error = %e, "SCK frame encode failed"),
            }
        }
    }
);

impl SckScreenOutput {
    fn new(state: Arc<DelegateState>) -> Retained<Self> {
        let this = Self::alloc().set_ivars(state);
        unsafe { msg_send![super(this), init] }
    }
}

/// Guard keeping the SCStream alive. Dropping stops the capture.
pub struct SckScreenStream {
    stream: Retained<SCStream>,
    _output: Retained<SckScreenOutput>,
    _queue: DispatchRetained<DispatchQueue>,
    state: Arc<DelegateState>,
}

// SCStream is thread-safe at the ObjC / GCD level; objc2 is conservative.
unsafe impl Send for SckScreenStream {}

impl Drop for SckScreenStream {
    fn drop(&mut self) {
        unsafe { self.stream.stopCaptureWithCompletionHandler(None) };
        info!("SCK screen capture stopped");
    }
}

impl SckScreenStream {
    /// Pop the most recently encoded frame, enriched with frontmost-window
    /// metadata captured at the time of the call. Returns `None` if no
    /// frame has arrived yet since the last `latest()` call.
    pub fn latest(&self) -> Option<Frame> {
        let png_bytes = self.state.latest_png.lock().ok()?.take()?;
        let (app_name, window_title) = match get_shareable_content_blocking() {
            Ok(content) => frontmost_window(&content),
            Err(e) => {
                warn!(error = %e, "SCShareableContent fetch failed — metadata empty");
                (String::new(), String::new())
            }
        };
        Some(Frame { png_bytes, app_name, window_title })
    }
}

/// Start the SCK screen stream at the main display. Blocks until the stream
/// is live; returns an error for permission failure or config errors.
pub fn start_capture() -> Result<SckScreenStream> {
    info!("starting SCK screen capture");

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
        config.setWidth(w);
        config.setHeight(h);
        config.setPixelFormat(kCVPixelFormatType_32BGRA);
        config.setShowsCursor(true);
        // 2 fps — we throw away most frames; triggers drive disk writes.
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

    let state = Arc::new(DelegateState {
        latest_png: Mutex::new(None),
    });
    let output = SckScreenOutput::new(state.clone());
    let output_proto: &ProtocolObject<dyn SCStreamOutput> = ProtocolObject::from_ref(&*output);

    let queue = DispatchQueue::new("com.alvum.sck-screen", None);

    unsafe {
        stream.addStreamOutput_type_sampleHandlerQueue_error(
            output_proto,
            SCStreamOutputType::Screen,
            Some(&queue),
        )
    }
    .map_err(|e| anyhow!("SCStream addStreamOutput failed: {:?}", e))?;

    start_capture_blocking(&stream)?;
    info!(width = w, height = h, "SCK screen capture started");

    Ok(SckScreenStream {
        stream,
        _output: output,
        _queue: queue,
        state,
    })
}

fn encode_png_from_sample(sample: &CMSampleBuffer) -> Result<Vec<u8>> {
    let image = unsafe { sample.image_buffer() }
        .context("CMSampleBuffer has no CVImageBuffer")?;

    // CVPixelBuffer is a typedef of CVImageBuffer — same pointer.
    let pixel_buffer = &*image;

    let lock_status = unsafe {
        CVPixelBufferLockBaseAddress(pixel_buffer, CVPixelBufferLockFlags::ReadOnly)
    };
    if lock_status != 0 {
        return Err(anyhow!(
            "CVPixelBufferLockBaseAddress returned {}",
            lock_status
        ));
    }

    // Wrap in a guard-ish scope so we always unlock, even on early return.
    let result: Result<Vec<u8>> = (|| {
        let width = CVPixelBufferGetWidth(pixel_buffer);
        let height = CVPixelBufferGetHeight(pixel_buffer);
        let bytes_per_row = CVPixelBufferGetBytesPerRow(pixel_buffer);
        let base = CVPixelBufferGetBaseAddress(pixel_buffer);
        if base.is_null() {
            return Err(anyhow!("CVPixelBufferGetBaseAddress returned null"));
        }

        // BGRA (8-bit per channel) → RGBA, stripping any row padding.
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

    unsafe {
        CVPixelBufferUnlockBaseAddress(pixel_buffer, CVPixelBufferLockFlags::ReadOnly);
    }
    result
}

/// Find the frontmost on-screen regular-application window.
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

// ---- ObjC async → sync bridges (reimplemented from audio/sck.rs to avoid
// cross-crate coupling; these are tiny helpers, DRY here is not worth the cost).

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
