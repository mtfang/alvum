//! Decode alvum's custom Opus container format back to f32 PCM samples.

use anyhow::{Context, Result};
use std::path::Path;

/// Decode an Opus file (alvum's 2-byte-length-prefix container) to f32 PCM at 16kHz mono.
pub fn decode_opus_file(path: &Path) -> Result<Vec<f32>> {
    let data = std::fs::read(path)
        .with_context(|| format!("failed to read audio file: {}", path.display()))?;

    let mut decoder = opus::Decoder::new(16000, opus::Channels::Mono)
        .context("failed to create Opus decoder")?;

    let frame_size = 16000 / 50; // 20ms frames at 16kHz = 320 samples
    let mut samples = Vec::new();
    let mut offset = 0;

    while offset + 2 <= data.len() {
        let frame_len = u16::from_le_bytes([data[offset], data[offset + 1]]) as usize;
        offset += 2;

        if offset + frame_len > data.len() {
            tracing::warn!(offset, frame_len, file_len = data.len(), "truncated opus frame, skipping");
            break;
        }

        let frame_data = &data[offset..offset + frame_len];
        offset += frame_len;

        let mut output = vec![0.0f32; frame_size];
        let decoded = decoder.decode_float(frame_data, &mut output, false)
            .with_context(|| "Opus decode failed")?;
        samples.extend_from_slice(&output[..decoded]);
    }

    Ok(samples)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn decode_roundtrip_with_encoder() {
        // Create a test opus file using the encoder from alvum-capture-audio
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("test.opus");

        // Generate 1 second of 440Hz tone
        let original: Vec<f32> = (0..16000)
            .map(|i| (2.0 * std::f32::consts::PI * 440.0 * i as f32 / 16000.0).sin() * 0.5)
            .collect();

        // Encode using the same format as alvum-capture-audio
        encode_test_opus(&original, &path);

        // Decode
        let decoded = decode_opus_file(&path).unwrap();

        // Opus is lossy, so we can't compare exactly. Check length and that it's not silent.
        assert!(decoded.len() >= 15000, "decoded should have ~16000 samples, got {}", decoded.len());
        let max_amplitude = decoded.iter().map(|s| s.abs()).fold(0.0f32, f32::max);
        assert!(max_amplitude > 0.1, "decoded audio should not be silent");
    }

    /// Encode using the same format as alvum-capture-audio's encoder.
    fn encode_test_opus(samples: &[f32], path: &std::path::Path) {
        let mut encoder = opus::Encoder::new(16000, opus::Channels::Mono, opus::Application::Voip).unwrap();
        let frame_size = 16000 / 50; // 320 samples per 20ms frame
        let mut data = Vec::new();

        for frame in samples.chunks(frame_size) {
            if frame.len() < frame_size { break; }
            let mut output = vec![0u8; 4000];
            let len = encoder.encode_float(frame, &mut output).unwrap();
            data.extend_from_slice(&(len as u16).to_le_bytes());
            data.extend_from_slice(&output[..len]);
        }

        std::fs::write(path, &data).unwrap();
    }
}
