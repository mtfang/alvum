//! Decode audio files to f32 PCM samples at 16kHz mono.

use anyhow::{Context, Result};
use std::path::Path;

/// Decode a WAV file to f32 PCM at 16kHz mono.
pub fn decode_wav_file(path: &Path) -> Result<Vec<f32>> {
    let data = std::fs::read(path)
        .with_context(|| format!("failed to read audio file: {}", path.display()))?;

    // Parse WAV header
    if data.len() < 44 || &data[0..4] != b"RIFF" || &data[8..12] != b"WAVE" {
        anyhow::bail!("not a valid WAV file: {}", path.display());
    }

    let channels = u16::from_le_bytes([data[22], data[23]]) as usize;
    let sample_rate = u32::from_le_bytes([data[24], data[25], data[26], data[27]]);
    let bits_per_sample = u16::from_le_bytes([data[34], data[35]]);

    // Find data chunk
    let mut offset = 12;
    while offset + 8 <= data.len() {
        let chunk_id = &data[offset..offset + 4];
        let chunk_size = u32::from_le_bytes([
            data[offset + 4], data[offset + 5], data[offset + 6], data[offset + 7],
        ]) as usize;

        if chunk_id == b"data" {
            let audio_data = &data[offset + 8..offset + 8 + chunk_size.min(data.len() - offset - 8)];
            let samples = decode_pcm_to_f32(audio_data, bits_per_sample, channels)?;

            if sample_rate == 16000 {
                return Ok(samples);
            } else {
                return Ok(resample(&samples, sample_rate, 16000));
            }
        }

        offset += 8 + chunk_size;
    }

    anyhow::bail!("no data chunk found in WAV file: {}", path.display())
}

fn decode_pcm_to_f32(data: &[u8], bits_per_sample: u16, channels: usize) -> Result<Vec<f32>> {
    match bits_per_sample {
        16 => {
            let samples: Vec<f32> = data.chunks_exact(2)
                .map(|c| i16::from_le_bytes([c[0], c[1]]) as f32 / 32768.0)
                .collect();
            if channels > 1 {
                Ok(samples.chunks(channels).map(|ch| ch.iter().sum::<f32>() / channels as f32).collect())
            } else {
                Ok(samples)
            }
        }
        32 => {
            let samples: Vec<f32> = data.chunks_exact(4)
                .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
                .collect();
            if channels > 1 {
                Ok(samples.chunks(channels).map(|ch| ch.iter().sum::<f32>() / channels as f32).collect())
            } else {
                Ok(samples)
            }
        }
        other => anyhow::bail!("unsupported bits per sample: {other}"),
    }
}

fn resample(samples: &[f32], from_rate: u32, to_rate: u32) -> Vec<f32> {
    let ratio = from_rate as f64 / to_rate as f64;
    let output_len = (samples.len() as f64 / ratio) as usize;
    (0..output_len)
        .map(|i| {
            let src = i as f64 * ratio;
            let idx = src as usize;
            let frac = src - idx as f64;
            let s0 = samples.get(idx).copied().unwrap_or(0.0);
            let s1 = samples.get(idx + 1).copied().unwrap_or(s0);
            s0 + (s1 - s0) * frac as f32
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn decode_wav_roundtrip() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("test.wav");

        let original: Vec<f32> = (0..16000)
            .map(|i| (2.0 * std::f32::consts::PI * 440.0 * i as f32 / 16000.0).sin() * 0.5)
            .collect();

        write_test_wav(&original, 16000, &path);

        let decoded = decode_wav_file(&path).unwrap();
        assert_eq!(decoded.len(), original.len());

        let max_error: f32 = original.iter().zip(decoded.iter())
            .map(|(a, b)| (a - b).abs())
            .fold(0.0f32, f32::max);
        assert!(max_error < 0.001, "max error {max_error} too large");
    }

    fn write_test_wav(samples: &[f32], sample_rate: u32, path: &Path) {
        let pcm16: Vec<i16> = samples.iter()
            .map(|&s| (s.clamp(-1.0, 1.0) * 32767.0) as i16)
            .collect();
        let data_len = (pcm16.len() * 2) as u32;
        let file_len = 36 + data_len;
        let mut buf = Vec::new();
        buf.extend_from_slice(b"RIFF");
        buf.extend_from_slice(&file_len.to_le_bytes());
        buf.extend_from_slice(b"WAVE");
        buf.extend_from_slice(b"fmt ");
        buf.extend_from_slice(&16u32.to_le_bytes());
        buf.extend_from_slice(&1u16.to_le_bytes());
        buf.extend_from_slice(&1u16.to_le_bytes());
        buf.extend_from_slice(&sample_rate.to_le_bytes());
        buf.extend_from_slice(&(sample_rate * 2).to_le_bytes());
        buf.extend_from_slice(&2u16.to_le_bytes());
        buf.extend_from_slice(&16u16.to_le_bytes());
        buf.extend_from_slice(b"data");
        buf.extend_from_slice(&data_len.to_le_bytes());
        for s in &pcm16 { buf.extend_from_slice(&s.to_le_bytes()); }
        std::fs::write(path, &buf).unwrap();
    }
}
