use anyhow::Result;
use std::path::{Path, PathBuf};
use tracing::info;

/// Writes f32 audio samples as WAV files.
/// WAV is universally playable and Whisper reads it natively — no custom decoder needed.
///
/// Path layout is computed at each flush, not fixed at construction, so a
/// long-running capture process rolls into the new day's directory once
/// local midnight passes instead of dumping tomorrow's audio into today's
/// folder forever.
pub struct AudioEncoder {
    /// Capture root (no date component). Typically `~/.alvum/capture`.
    root: PathBuf,
    /// Source-specific subpath under the daily dir, e.g. `audio/mic`.
    subpath: PathBuf,
    sample_rate: u32,
    segment_buffer: Vec<f32>,
}

impl AudioEncoder {
    pub fn new(root: PathBuf, subpath: PathBuf, sample_rate: u32) -> Result<Self> {
        Ok(Self {
            root,
            subpath,
            sample_rate,
            segment_buffer: Vec::new(),
        })
    }

    /// Accumulate samples into the current segment.
    pub fn push_samples(&mut self, samples: &[f32]) {
        self.segment_buffer.extend_from_slice(samples);
    }

    /// Flush the current segment to a WAV file. Returns the file path if written.
    pub fn flush_segment(&mut self) -> Result<Option<PathBuf>> {
        if self.segment_buffer.is_empty() {
            return Ok(None);
        }

        let now = chrono::Local::now();
        let date = now.format("%Y-%m-%d").to_string();
        let dir = self.root.join(&date).join(&self.subpath);
        std::fs::create_dir_all(&dir)?;

        let timestamp = now.format("%H-%M-%S");
        let path = dir.join(format!("{timestamp}.wav"));

        write_wav(&self.segment_buffer, self.sample_rate, &path)?;

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

/// Write f32 PCM samples as a standard 16-bit mono WAV file.
fn write_wav(samples: &[f32], sample_rate: u32, path: &Path) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    // Convert f32 [-1.0, 1.0] to i16
    let pcm16: Vec<i16> = samples.iter()
        .map(|&s| (s.clamp(-1.0, 1.0) * 32767.0) as i16)
        .collect();

    let data_len = (pcm16.len() * 2) as u32;
    let file_len = 36 + data_len;

    let mut buf = Vec::with_capacity(44 + data_len as usize);

    // RIFF header
    buf.extend_from_slice(b"RIFF");
    buf.extend_from_slice(&file_len.to_le_bytes());
    buf.extend_from_slice(b"WAVE");

    // fmt chunk
    buf.extend_from_slice(b"fmt ");
    buf.extend_from_slice(&16u32.to_le_bytes());        // chunk size
    buf.extend_from_slice(&1u16.to_le_bytes());          // PCM format
    buf.extend_from_slice(&1u16.to_le_bytes());          // mono
    buf.extend_from_slice(&sample_rate.to_le_bytes());   // sample rate
    buf.extend_from_slice(&(sample_rate * 2).to_le_bytes()); // byte rate (16-bit mono)
    buf.extend_from_slice(&2u16.to_le_bytes());          // block align
    buf.extend_from_slice(&16u16.to_le_bytes());         // bits per sample

    // data chunk
    buf.extend_from_slice(b"data");
    buf.extend_from_slice(&data_len.to_le_bytes());
    for sample in &pcm16 {
        buf.extend_from_slice(&sample.to_le_bytes());
    }

    std::fs::write(path, &buf)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn encoder_writes_wav_file() {
        let tmp = TempDir::new().unwrap();
        let mut encoder = AudioEncoder::new(tmp.path().to_path_buf(), PathBuf::new(), 16000).unwrap();

        let samples: Vec<f32> = (0..16000)
            .map(|i| (2.0 * std::f32::consts::PI * 440.0 * i as f32 / 16000.0).sin() * 0.5)
            .collect();

        encoder.push_samples(&samples);
        let path = encoder.flush_segment().unwrap();

        assert!(path.is_some());
        let path = path.unwrap();
        assert!(path.exists());
        assert!(path.extension().unwrap() == "wav");

        // Check WAV header
        let data = std::fs::read(&path).unwrap();
        assert_eq!(&data[0..4], b"RIFF");
        assert_eq!(&data[8..12], b"WAVE");
    }

    #[test]
    fn flush_empty_returns_none() {
        let tmp = TempDir::new().unwrap();
        let mut encoder = AudioEncoder::new(tmp.path().to_path_buf(), PathBuf::new(), 16000).unwrap();
        let path = encoder.flush_segment().unwrap();
        assert!(path.is_none());
    }

    #[test]
    fn flush_writes_into_current_date_subdir() {
        // The encoder should compute the date dir at flush time, not at
        // construction — so a long-running process rolls into the next
        // day's directory as soon as it crosses local midnight.
        let tmp = TempDir::new().unwrap();
        let mut encoder = AudioEncoder::new(
            tmp.path().to_path_buf(),
            PathBuf::from("audio/mic"),
            16000,
        )
        .unwrap();
        encoder.push_samples(&[0.1_f32; 16]);
        let path = encoder.flush_segment().unwrap().unwrap();
        let today = chrono::Local::now().format("%Y-%m-%d").to_string();
        let expected_dir = tmp.path().join(&today).join("audio").join("mic");
        assert!(
            path.starts_with(&expected_dir),
            "expected {} to start with {}",
            path.display(),
            expected_dir.display()
        );
        assert!(expected_dir.is_dir(), "date dir should be created on flush");
    }

    #[test]
    fn discard_clears_buffer() {
        let tmp = TempDir::new().unwrap();
        let mut encoder = AudioEncoder::new(tmp.path().to_path_buf(), PathBuf::new(), 16000).unwrap();
        encoder.push_samples(&[0.0; 1000]);
        assert_eq!(encoder.buffered_samples(), 1000);
        encoder.discard_segment();
        assert_eq!(encoder.buffered_samples(), 0);
    }
}
