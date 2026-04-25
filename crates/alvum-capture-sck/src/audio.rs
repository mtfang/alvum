//! Audio side of the shared SCK stream: format-aware decode of the
//! incoming `CMSampleBuffer` into a 16 kHz mono `f32` slice, then fan-out
//! to the optional subscriber callback.

use anyhow::{Context, Result};
use objc2_core_audio_types::{kAudioFormatFlagIsNonInterleaved, AudioStreamBasicDescription};
use objc2_core_media::{CMAudioFormatDescriptionGetStreamBasicDescription, CMSampleBuffer};
use std::ptr;
use tracing::{info, warn};

use crate::stream::SharedState;

pub(crate) fn handle_audio(sample: &CMSampleBuffer, state: &SharedState) {
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
