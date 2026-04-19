//! Throwaway: captures 30 seconds of system audio via ScreenCaptureKit and
//! prints per-second sample counts. Switch your audio output (AirPods, headphones,
//! AirPlay) during the run to verify the stream survives device changes.
//!
//! Run: cargo run -p alvum-capture-audio --example sck_spike --release
//!
//! On first run, macOS will prompt for Screen Recording permission. After
//! granting, re-run. The total sample count should be ~1.44M over 30s
//! (48kHz * 30s) with audio playing. Without audio playing, you'll still see
//! samples (silence), which also proves the stream is live.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use block2::RcBlock;
use dispatch2::DispatchQueue;
use objc2::rc::Retained;
use objc2::runtime::{NSObject, NSObjectProtocol, ProtocolObject};
use objc2::{define_class, msg_send, AllocAnyThread, DefinedClass};
use objc2_core_media::CMSampleBuffer;
use objc2_foundation::{NSArray, NSError};
use objc2_screen_capture_kit::{
    SCContentFilter, SCShareableContent, SCStream, SCStreamConfiguration, SCStreamOutput,
    SCStreamOutputType,
};

// Shared state the delegate class writes into.
struct SpikeState {
    samples: AtomicU64,
}

define_class!(
    #[unsafe(super(NSObject))]
    #[name = "AlvumSpikeOutput"]
    #[ivars = Arc<SpikeState>]
    struct SpikeOutput;

    unsafe impl NSObjectProtocol for SpikeOutput {}

    unsafe impl SCStreamOutput for SpikeOutput {
        #[unsafe(method(stream:didOutputSampleBuffer:ofType:))]
        fn stream_didOutputSampleBuffer_ofType(
            &self,
            _stream: &SCStream,
            sample_buffer: &CMSampleBuffer,
            of_type: SCStreamOutputType,
        ) {
            if of_type != SCStreamOutputType::Audio {
                return;
            }
            let n = unsafe { sample_buffer.num_samples() } as u64;
            self.ivars().samples.fetch_add(n, Ordering::Relaxed);
        }
    }
);

impl SpikeOutput {
    fn new(state: Arc<SpikeState>) -> Retained<Self> {
        let this = Self::alloc().set_ivars(state);
        unsafe { msg_send![super(this), init] }
    }
}

fn get_shareable_content_blocking() -> Result<Retained<SCShareableContent>, String> {
    // getShareableContentWithCompletionHandler: is async only. Bridge to sync
    // using a condvar-signaled slot written from the block.
    let slot: Arc<Mutex<Option<Result<Retained<SCShareableContent>, String>>>> =
        Arc::new(Mutex::new(None));
    let signal = Arc::new((Mutex::new(false), std::sync::Condvar::new()));

    let slot_cb = slot.clone();
    let signal_cb = signal.clone();
    let block = RcBlock::new(
        move |content: *mut SCShareableContent, error: *mut NSError| {
            let result = if content.is_null() {
                let msg = if !error.is_null() {
                    unsafe { (*error).localizedDescription().to_string() }
                } else {
                    "unknown error".to_string()
                };
                Err(msg)
            } else {
                Ok(unsafe { Retained::retain(content).unwrap() })
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
    while !*done {
        done = cvar.wait(done).unwrap();
    }
    drop(done);

    slot.lock().unwrap().take().ok_or_else(|| "no result".to_string())?
}

fn main() -> Result<(), String> {
    eprintln!("requesting shareable content...");
    let content = get_shareable_content_blocking()?;

    let displays = unsafe { content.displays() };
    if displays.count() == 0 {
        return Err("no displays available".into());
    }
    let display = displays.objectAtIndex(0);
    eprintln!(
        "display: {}x{}",
        unsafe { display.width() },
        unsafe { display.height() },
    );

    let empty_windows: Retained<NSArray<_>> = NSArray::new();
    let filter = unsafe {
        SCContentFilter::initWithDisplay_excludingWindows(
            SCContentFilter::alloc(),
            &display,
            &empty_windows,
        )
    };

    let config = unsafe { SCStreamConfiguration::new() };
    unsafe {
        config.setCapturesAudio(true);
        config.setSampleRate(48_000);
        config.setChannelCount(2);
    }

    let stream = unsafe {
        SCStream::initWithFilter_configuration_delegate(SCStream::alloc(), &filter, &config, None)
    };

    let state = Arc::new(SpikeState {
        samples: AtomicU64::new(0),
    });
    let output = SpikeOutput::new(state.clone());
    let output_proto: &ProtocolObject<dyn SCStreamOutput> = ProtocolObject::from_ref(&*output);

    let queue = DispatchQueue::new("com.alvum.spike.audio", None);

    unsafe {
        stream
            .addStreamOutput_type_sampleHandlerQueue_error(
                output_proto,
                SCStreamOutputType::Audio,
                Some(&queue),
            )
            .map_err(|e| format!("addStreamOutput failed: {:?}", e))?;
    }

    // Start — completion handler just prints the error if any.
    let start_block = RcBlock::new(move |err: *mut NSError| {
        if !err.is_null() {
            eprintln!(
                "start error: {}",
                unsafe { (*err).localizedDescription().to_string() }
            );
        }
    });
    unsafe { stream.startCaptureWithCompletionHandler(Some(&start_block)) };

    eprintln!("started. now switch audio output a few times. 30s run.");

    let start = Instant::now();
    while start.elapsed() < Duration::from_secs(30) {
        std::thread::sleep(Duration::from_secs(1));
        eprintln!(
            "  t={:>2}s  total_samples={}",
            start.elapsed().as_secs(),
            state.samples.load(Ordering::Relaxed)
        );
    }

    // Stop — no need to block on completion for a spike.
    unsafe { stream.stopCaptureWithCompletionHandler(None) };

    let total = state.samples.load(Ordering::Relaxed);
    eprintln!(
        "done. {} total samples across 30s (~48000 * 30 = 1.44M expected with audio playing)",
        total
    );
    if total == 0 {
        return Err("zero samples received — SCK audio is not delivering".into());
    }
    Ok(())
}
