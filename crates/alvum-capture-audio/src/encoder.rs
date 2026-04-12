use anyhow::{Context, Result};
use std::path::{Path, PathBuf};
use tracing::info;

/// Encodes f32 audio samples to Opus and writes segmented files.
pub struct AudioEncoder {
    output_dir: PathBuf,
    sample_rate: u32,
    segment_buffer: Vec<f32>,
}

impl AudioEncoder {
    pub fn new(output_dir: PathBuf, sample_rate: u32) -> Result<Self> {
        std::fs::create_dir_all(&output_dir)?;
        Ok(Self {
            output_dir,
            sample_rate,
            segment_buffer: Vec::new(),
        })
    }

    /// Accumulate samples into the current segment.
    pub fn push_samples(&mut self, samples: &[f32]) {
        self.segment_buffer.extend_from_slice(samples);
    }

    /// Flush the current segment to an Opus file. Returns the file path if written.
    pub fn flush_segment(&mut self) -> Result<Option<PathBuf>> {
        if self.segment_buffer.is_empty() {
            return Ok(None);
        }

        let timestamp = chrono::Utc::now().format("%H-%M-%S");
        let path = self.output_dir.join(format!("{timestamp}.opus"));

        encode_opus_file(&self.segment_buffer, self.sample_rate, &path)?;

        let duration_secs = self.segment_buffer.len() as f32 / self.sample_rate as f32;
        info!(
            path = %path.display(),
            duration_secs = format!("{:.1}", duration_secs),
            "wrote audio segment"
        );

        self.segment_buffer.clear();
        Ok(Some(path))
    }

    /// Discard the current segment without writing.
    pub fn discard_segment(&mut self) {
        self.segment_buffer.clear();
    }

    /// Number of samples in the current buffer.
    pub fn buffered_samples(&self) -> usize {
        self.segment_buffer.len()
    }
}

/// Encode f32 PCM samples to an Opus file.
/// Uses a simple container: 2-byte frame length prefix + encoded frame data.
fn encode_opus_file(samples: &[f32], sample_rate: u32, path: &Path) -> Result<()> {
    let mut encoder = opus::Encoder::new(
        sample_rate,
        opus::Channels::Mono,
        opus::Application::Voip,
    ).context("failed to create Opus encoder")?;

    let frame_size = sample_rate as usize / 50; // 20ms frames
    let mut data = Vec::new();

    for frame in samples.chunks(frame_size) {
        if frame.len() < frame_size {
            break;
        }
        let mut output = vec![0u8; 4000];
        let len = encoder.encode_float(frame, &mut output)
            .context("Opus encode failed")?;
        data.extend_from_slice(&(len as u16).to_le_bytes());
        data.extend_from_slice(&output[..len]);
    }

    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, &data)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn encoder_writes_opus_file() {
        let tmp = TempDir::new().unwrap();
        let mut encoder = AudioEncoder::new(tmp.path().to_path_buf(), 16000).unwrap();

        // 1 second of 440Hz tone
        let samples: Vec<f32> = (0..16000)
            .map(|i| (2.0 * std::f32::consts::PI * 440.0 * i as f32 / 16000.0).sin() * 0.5)
            .collect();

        encoder.push_samples(&samples);
        let path = encoder.flush_segment().unwrap();

        assert!(path.is_some());
        let path = path.unwrap();
        assert!(path.exists());
        assert!(std::fs::metadata(&path).unwrap().len() > 0);
    }

    #[test]
    fn flush_empty_returns_none() {
        let tmp = TempDir::new().unwrap();
        let mut encoder = AudioEncoder::new(tmp.path().to_path_buf(), 16000).unwrap();
        let path = encoder.flush_segment().unwrap();
        assert!(path.is_none());
    }

    #[test]
    fn discard_clears_buffer() {
        let tmp = TempDir::new().unwrap();
        let mut encoder = AudioEncoder::new(tmp.path().to_path_buf(), 16000).unwrap();
        encoder.push_samples(&[0.0; 1000]);
        assert_eq!(encoder.buffered_samples(), 1000);
        encoder.discard_segment();
        assert_eq!(encoder.buffered_samples(), 0);
    }
}
