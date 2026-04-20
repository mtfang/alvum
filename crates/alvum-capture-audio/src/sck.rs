//! ScreenCaptureKit-driven system-audio capture. Owns an SCStream and
//! forwards decoded 16 kHz mono samples to the caller's `SampleCallback`.
//!
//! Resilient to output-device changes (AirPods, AirPlay, HDMI) because
//! SCK taps audio at the system process graph, not a specific device.

use crate::capture::SampleCallback;
use crate::sck_decode::decode_audio;
use anyhow::{anyhow, Context, Result};
use block2::RcBlock;
use dispatch2::{DispatchQueue, DispatchRetained};
use objc2::rc::Retained;
use objc2::runtime::{NSObject, NSObjectProtocol, ProtocolObject};
use objc2::{define_class, msg_send, AllocAnyThread, DefinedClass};
use objc2_core_media::CMSampleBuffer;
use objc2_foundation::{NSArray, NSError};
use objc2_screen_capture_kit::{
    SCContentFilter, SCShareableContent, SCStream, SCStreamConfiguration, SCStreamOutput,
    SCStreamOutputType,
};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;
use tracing::{error, info};

/// Sample rate SCK natively delivers for our configuration (NSInteger).
const SCK_SAMPLE_RATE: isize = 48_000;
/// Always stereo; we mix down to mono in the decoder.
const SCK_CHANNEL_COUNT: isize = 2;
/// Hard limit on how long to wait for SCK async operations.
const SCK_WAIT_TIMEOUT: Duration = Duration::from_secs(5);

/// Shared ivars owned by the Objective-C delegate class.
struct DelegateState {
    callback: SampleCallback,
    phase: Mutex<f64>,
}

define_class!(
    #[unsafe(super(NSObject))]
    #[name = "AlvumSCKAudioOutput"]
    #[ivars = Arc<DelegateState>]
    struct SckAudioOutput;

    unsafe impl NSObjectProtocol for SckAudioOutput {}

    unsafe impl SCStreamOutput for SckAudioOutput {
        #[unsafe(method(stream:didOutputSampleBuffer:ofType:))]
        #[allow(non_snake_case)]
        fn stream_didOutputSampleBuffer_ofType(
            &self,
            _stream: &SCStream,
            sample_buffer: &CMSampleBuffer,
            of_type: SCStreamOutputType,
        ) {
            if of_type != SCStreamOutputType::Audio {
                return;
            }
            let state = self.ivars();
            let mut phase = match state.phase.lock() {
                Ok(p) => p,
                Err(_) => return,
            };
            match decode_audio(sample_buffer, &mut *phase) {
                Ok(out) if !out.is_empty() => {
                    if let Ok(mut cb) = state.callback.lock() {
                        cb(&out);
                    }
                }
                Ok(_) => {}
                Err(e) => error!(error = %e, "SCK audio decode failed"),
            }
        }
    }
);

impl SckAudioOutput {
    fn new(state: Arc<DelegateState>) -> Retained<Self> {
        let this = Self::alloc().set_ivars(state);
        unsafe { msg_send![super(this), init] }
    }
}

/// Guard that keeps the SCStream alive. Dropping it stops the capture.
pub struct SckAudioStream {
    stream: Retained<SCStream>,
    // Keep the delegate + queue alive for the stream's lifetime — SCK holds
    // only weak-ish references into them via the ObjC runtime.
    _output: Retained<SckAudioOutput>,
    _queue: DispatchRetained<DispatchQueue>,
}

impl Drop for SckAudioStream {
    fn drop(&mut self) {
        // Stop without waiting on completion — we're tearing down anyway.
        unsafe { self.stream.stopCaptureWithCompletionHandler(None) };
        info!("SCK system-audio capture stopped");
    }
}

/// Start SCK system-audio capture. Blocks until the stream is live, or
/// returns an error if anything in the setup chain fails.
pub fn start_capture(callback: SampleCallback) -> Result<SckAudioStream> {
    info!("starting SCK system-audio capture");

    let content = get_shareable_content_blocking()
        .context("failed to obtain SCShareableContent")?;

    let displays = unsafe { content.displays() };
    if displays.count() == 0 {
        return Err(anyhow!("no displays available for SCK filter"));
    }
    let display = displays.objectAtIndex(0);

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
        config.setCapturesAudio(true);
        config.setSampleRate(SCK_SAMPLE_RATE);
        config.setChannelCount(SCK_CHANNEL_COUNT);
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
        callback,
        phase: Mutex::new(0.0_f64),
    });
    let output = SckAudioOutput::new(state);
    let output_proto: &ProtocolObject<dyn SCStreamOutput> = ProtocolObject::from_ref(&*output);

    let queue = DispatchQueue::new("com.alvum.sck-audio", None);

    unsafe {
        stream.addStreamOutput_type_sampleHandlerQueue_error(
            output_proto,
            SCStreamOutputType::Audio,
            Some(&queue),
        )
    }
    .map_err(|e| anyhow!("SCStream addStreamOutput failed: {:?}", e))?;

    start_capture_blocking(&stream)?;
    info!("SCK system-audio capture started");

    Ok(SckAudioStream {
        stream,
        _output: output,
        _queue: queue,
    })
}

/// `SCShareableContent.getShareableContentWithCompletionHandler:` is async-only.
/// Bridge to sync with a Condvar-signaled slot written from the ObjC block.
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
    let mut done = lock.lock().unwrap();
    let (guard, _res) = cvar.wait_timeout(done, SCK_WAIT_TIMEOUT).unwrap();
    done = guard;
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

/// `startCaptureWithCompletionHandler:` is async; we need to know synchronously
/// whether start succeeded so the caller can surface errors to the daemon.
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
    let mut finished = lock.lock().unwrap();
    let (guard, _) = cvar.wait_timeout(finished, SCK_WAIT_TIMEOUT).unwrap();
    finished = guard;
    if !*finished {
        return Err(anyhow!("SCStream start did not complete within {:?}", SCK_WAIT_TIMEOUT));
    }
    if !ok_flag.load(Ordering::SeqCst) {
        let msg = err_slot.lock().unwrap().take().unwrap_or_else(|| "unknown".into());
        return Err(anyhow!("SCStream start error: {}", msg));
    }
    Ok(())
}
