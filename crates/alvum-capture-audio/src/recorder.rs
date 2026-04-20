//! Helpers shared across capture sources. Originally hosted a combined
//! `Recorder` that drove both mic + system audio via cpal; that role was
//! superseded by the per-source `CaptureSource` trait in source.rs. The
//! chunked-callback factory remains here as a shared utility.

use crate::capture::SampleCallback;
use crate::encoder::AudioEncoder;
use std::sync::{Arc, Mutex};

/// Create a callback that writes audio in fixed-length chunks.
/// No VAD — every sample is recorded. Chunks are flushed every
/// `samples_per_chunk` samples.
pub(crate) fn make_chunked_callback(
    encoder: Arc<Mutex<AudioEncoder>>,
    samples_per_chunk: usize,
    label: String,
) -> SampleCallback {
    let sample_count = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let chunk_count = Arc::new(std::sync::atomic::AtomicU64::new(0));

    Arc::new(Mutex::new(move |samples: &[f32]| {
        let mut enc = encoder.lock().unwrap();
        enc.push_samples(samples);

        let count = sample_count
            .fetch_add(samples.len(), std::sync::atomic::Ordering::Relaxed)
            + samples.len();

        // Log audio level every ~5 seconds
        let chunks = chunk_count.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        if chunks % 155 == 0 && !samples.is_empty() {
            let rms: f32 =
                (samples.iter().map(|s| s * s).sum::<f32>() / samples.len() as f32).sqrt();
            tracing::debug!(label = %label, rms = format!("{:.4}", rms), "audio level");
        }

        // Flush chunk when we've accumulated enough samples
        if count >= samples_per_chunk {
            if let Err(e) = enc.flush_segment() {
                tracing::error!(label = %label, error = %e, "failed to flush audio chunk");
            }
            sample_count.store(0, std::sync::atomic::Ordering::Relaxed);
        }
    }))
}
