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
/// Silence-gate thresholds. A segment is kept when *either* RMS ≥ `rms_dbfs`
/// OR peak ≥ `peak_dbfs` — i.e. it's dropped only if BOTH metrics fall
/// below their floors. RMS catches sustained quiet speech (averaged out by
/// a peak-only filter); peak catches transients (claps, door slams) that
/// would be smoothed away by a pure RMS filter.
#[derive(Debug, Clone, Copy)]
pub struct SilenceGate {
    pub rms_dbfs: f32,
    pub peak_dbfs: f32,
}

pub struct AudioEncoder {
    /// Capture root (no date component). Typically `~/.alvum/capture`.
    root: PathBuf,
    /// Source-specific subpath under the daily dir, e.g. `audio/mic`.
    subpath: PathBuf,
    sample_rate: u32,
    /// `None` disables the filter; otherwise both thresholds apply in
    /// OR fashion — see [`SilenceGate`].
    silence_gate: Option<SilenceGate>,
    segment_buffer: Vec<f32>,
}

impl AudioEncoder {
    pub fn new(
        root: PathBuf,
        subpath: PathBuf,
        sample_rate: u32,
        silence_gate: Option<SilenceGate>,
    ) -> Result<Self> {
        Ok(Self {
            root,
            subpath,
            sample_rate,
            silence_gate,
            segment_buffer: Vec::new(),
        })
    }

    /// Accumulate samples into the current segment.
    pub fn push_samples(&mut self, samples: &[f32]) {
        self.segment_buffer.extend_from_slice(samples);
    }

    /// Flush the current segment to a WAV file. Returns the file path if
    /// written, or `Ok(None)` when the segment was empty or rejected by
    /// the silence filter.
    pub fn flush_segment(&mut self) -> Result<Option<PathBuf>> {
        if self.segment_buffer.is_empty() {
            return Ok(None);
        }

        // Silence gate: one pass over the buffer computes both RMS (same
        // metric as ffmpeg's volumedetect.mean_volume) and peak (max
        // absolute sample). Drop only when BOTH are below their floors —
        // RMS catches sustained quiet (speech, fan hum); peak catches
        // short transients (clap, keystroke) that get averaged away.
        if let Some(gate) = self.silence_gate {
            let (sum_sq, peak) = self.segment_buffer.iter().fold(
                (0.0_f64, 0.0_f32),
                |(sum, pk), &s| (sum + (s as f64) * (s as f64), pk.max(s.abs())),
            );
            let rms = (sum_sq / self.segment_buffer.len() as f64).sqrt() as f32;
            let rms_dbfs = if rms > 0.0 { 20.0 * rms.log10() } else { f32::NEG_INFINITY };
            let peak_dbfs = if peak > 0.0 { 20.0 * peak.log10() } else { f32::NEG_INFINITY };
            if rms_dbfs < gate.rms_dbfs && peak_dbfs < gate.peak_dbfs {
                info!(
                    rms_dbfs = format!("{:.1}", rms_dbfs),
                    peak_dbfs = format!("{:.1}", peak_dbfs),
                    rms_threshold = format!("{:.1}", gate.rms_dbfs),
                    peak_threshold = format!("{:.1}", gate.peak_dbfs),
                    subpath = %self.subpath.display(),
                    "dropping silent audio segment"
                );
                self.segment_buffer.clear();
                return Ok(None);
            }
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
        let mut encoder = AudioEncoder::new(tmp.path().to_path_buf(), PathBuf::new(), 16000, None).unwrap();

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
        let mut encoder = AudioEncoder::new(tmp.path().to_path_buf(), PathBuf::new(), 16000, None).unwrap();
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
            None,
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

    fn default_gate() -> SilenceGate {
        SilenceGate { rms_dbfs: -45.0, peak_dbfs: -15.0 }
    }

    #[test]
    fn silence_gate_drops_when_both_below() {
        let tmp = TempDir::new().unwrap();
        let mut encoder = AudioEncoder::new(
            tmp.path().to_path_buf(),
            PathBuf::from("audio/mic"),
            16000,
            Some(default_gate()),
        )
        .unwrap();
        // 1e-4 flat → RMS = peak = -80 dB: both well below the floors.
        encoder.push_samples(&[1e-4_f32; 16000]);
        assert!(encoder.flush_segment().unwrap().is_none());
        // Buffer must be cleared even on a drop.
        assert_eq!(encoder.buffered_samples(), 0);
    }

    #[test]
    fn silence_gate_keeps_segment_with_loud_peak() {
        let tmp = TempDir::new().unwrap();
        let mut encoder = AudioEncoder::new(
            tmp.path().to_path_buf(),
            PathBuf::from("audio/mic"),
            16000,
            Some(default_gate()),
        )
        .unwrap();
        // Mostly very quiet (RMS ~ -80) but a single sample hits 0.5 (-6 dB
        // peak). Must pass because peak ≥ -10.
        let mut samples = vec![1e-4_f32; 16000];
        samples[100] = 0.5;
        encoder.push_samples(&samples);
        assert!(encoder.flush_segment().unwrap().is_some());
    }

    #[test]
    fn silence_gate_keeps_segment_with_loud_rms() {
        let tmp = TempDir::new().unwrap();
        let mut encoder = AudioEncoder::new(
            tmp.path().to_path_buf(),
            PathBuf::from("audio/mic"),
            16000,
            Some(default_gate()),
        )
        .unwrap();
        // 0.05 flat → RMS -26 dB, peak -26 dB. Peak fails (-26 < -15) but
        // RMS passes (-26 > -45) so segment survives.
        encoder.push_samples(&[0.05_f32; 16000]);
        assert!(encoder.flush_segment().unwrap().is_some());
    }

    #[test]
    fn discard_clears_buffer() {
        let tmp = TempDir::new().unwrap();
        let mut encoder = AudioEncoder::new(tmp.path().to_path_buf(), PathBuf::new(), 16000, None).unwrap();
        encoder.push_samples(&[0.0; 1000]);
        assert_eq!(encoder.buffered_samples(), 1000);
        encoder.discard_segment();
        assert_eq!(encoder.buffered_samples(), 0);
    }
}
