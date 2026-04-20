//! Decode a ScreenCaptureKit audio `CMSampleBuffer` into 16 kHz mono f32
//! samples — the format the rest of the audio pipeline expects.
//!
//! SCK delivers 48 kHz stereo interleaved f32 (as configured). We:
//!   1. Extract the interleaved buffer via CMSampleBuffer::data_buffer().
//!   2. Average L+R to mono.
//!   3. Decimate 3:1 using linear interpolation, mirroring the resampling
//!      approach already used in `capture.rs` so output is acoustically
//!      comparable with the cpal mic path.

use anyhow::{bail, Context, Result};
use objc2_core_media::CMSampleBuffer;
use std::ptr;

pub const SCK_INPUT_RATE: u32 = 48_000;
pub const TARGET_RATE: u32 = 16_000;

/// Decimation ratio as f64 for precise phase accumulation.
const RATIO: f64 = SCK_INPUT_RATE as f64 / TARGET_RATE as f64;

/// Decode one SCK audio sample buffer to 16 kHz mono f32 samples.
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

/// Read the CMBlockBuffer's contiguous bytes as interleaved L,R,L,R,... f32.
///
/// # Safety
/// CMSampleBuffer from SCK with `setCapturesAudio(true)` + `setChannelCount(2)`
/// delivers 32-bit float interleaved PCM; format is stable across the stream.
/// We validate size against `num_samples` before reinterpreting.
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
        bail!("CMBlockBufferGetDataPointer returned status {}", status);
    }
    if ptr_out.is_null() || total_len == 0 {
        bail!("CMBlockBuffer returned null pointer or zero length");
    }

    // Expected shape: n_samples frames × 2 channels × 4 bytes/sample.
    let expected = n_samples * 2 * 4;
    if total_len < expected {
        bail!(
            "short CMBlockBuffer: expected ≥{} bytes, got {} (n_samples={})",
            expected, total_len, n_samples
        );
    }

    // Reinterpret the raw bytes as f32. Alignment: CMBlockBuffer returns
    // 16-byte aligned buffers when the kCMSampleBufferFlag_AudioBufferList_
    // Assure16ByteAlignment flag is used by the source; SCK honors this for
    // its audio path. We copy out rather than returning a borrowed slice
    // because the caller (stream_didOutputSampleBuffer) is on a dispatch
    // queue and the buffer may be released after the callback returns.
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stereo_to_mono_averages_channels() {
        let interleaved = [0.2, 0.8, -0.4, 0.4];
        let mono = stereo_to_mono(&interleaved);
        assert_eq!(mono, vec![0.5_f32, 0.0_f32]);
    }

    #[test]
    fn resample_48k_to_16k_drops_two_of_three() {
        // One second of 48 kHz → should produce ~16 000 output samples.
        let input: Vec<f32> = (0..48_000).map(|i| i as f32 * 0.001).collect();
        let mut phase = 0.0_f64;
        let out = resample_linear(&input, &mut phase);
        assert!(
            (15_999..=16_001).contains(&out.len()),
            "expected ~16000 samples, got {}",
            out.len()
        );
    }

    #[test]
    fn resample_phase_carries_across_buffers() {
        // One call with the full input vs two calls splitting it in half —
        // the total output length must be within 1 sample either way.
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

    #[test]
    fn resample_empty_input_yields_empty() {
        let mut phase = 0.0_f64;
        assert!(resample_linear(&[], &mut phase).is_empty());
    }
}
