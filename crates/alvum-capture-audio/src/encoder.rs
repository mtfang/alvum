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
/// Silence-gate parameters. Applied per 20-ms window: a window is kept
/// iff its RMS reaches the threshold OR it sits within `hold_secs` of a
/// passing window (the hold-time halo). Hold-time prevents speech from
/// sounding chopped — a single loud burst preserves `±hold_secs` of its
/// neighbours, so unvoiced consonants and natural inter-word pauses
/// stay intact while still trimming long dead air.
///
/// One config struct per source — there is no separate per-segment gate,
/// the windowed pass subsumes it.
#[derive(Debug, Clone, Copy)]
pub struct SilenceGate {
    pub threshold_dbfs: f32,
    pub hold_secs: f32,
}

pub struct AudioEncoder {
    /// Capture root (no date component). Typically `~/.alvum/capture`.
    root: PathBuf,
    /// Source-specific subpath under the daily dir, e.g. `audio/mic`.
    subpath: PathBuf,
    sample_rate: u32,
    /// `None` disables the filter; otherwise the threshold gates each
    /// 20-ms window — see [`SilenceGate`].
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
    /// written, or `Ok(None)` when the segment was empty or every window
    /// fell below the silence threshold.
    pub fn flush_segment(&mut self) -> Result<Option<PathBuf>> {
        if self.segment_buffer.is_empty() {
            return Ok(None);
        }

        // Window-level silence gate: split the buffer into 20-ms windows,
        // drop any window whose RMS sits below the configured threshold,
        // concatenate the rest. This both excises between-word pauses and
        // skips writing the file at all when the entire segment is below
        // the floor — there is no separate "segment-level" gate.
        if let Some(gate) = self.silence_gate {
            let original_len = self.segment_buffer.len();
            self.segment_buffer = trim_subthreshold_windows(
                &self.segment_buffer,
                self.sample_rate,
                gate,
                WINDOW_TRIM_SECS,
            );
            if self.segment_buffer.is_empty() {
                info!(
                    threshold_dbfs = format!("{:.1}", gate.threshold_dbfs),
                    original_secs = format!("{:.1}", original_len as f32 / self.sample_rate as f32),
                    subpath = %self.subpath.display(),
                    "dropping silent audio segment (every window below threshold)"
                );
                return Ok(None);
            }
            let trimmed_len = self.segment_buffer.len();
            if trimmed_len < original_len {
                info!(
                    original_secs = format!("{:.1}", original_len as f32 / self.sample_rate as f32),
                    kept_secs = format!("{:.1}", trimmed_len as f32 / self.sample_rate as f32),
                    subpath = %self.subpath.display(),
                    "trimmed sub-threshold windows from segment"
                );
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

/// Window length used by the silence gate. 20 ms is the standard
/// short-time-analysis frame for speech: small enough to remove dead air
/// between words, large enough that a single window's RMS is a stable
/// measurement (320 samples @ 16 kHz, 960 @ 48 kHz).
const WINDOW_TRIM_SECS: f32 = 0.020;

/// Excise contiguous 20-ms windows whose RMS sits below the threshold,
/// preserving a ±`hold_secs` halo around any passing window. Returns the
/// kept-only buffer; ordering is preserved.
///
/// Speech at desk distance has a lot of dynamic range — voiced vowels
/// sit at -25 dBFS, unvoiced consonants and tiny inter-word gaps dip
/// to -50 dBFS+. A bare per-window threshold chops between syllables
/// and produces a "fast-forwarded" listen. The hold-time halo solves
/// this: any window that passes the threshold guarantees `±hold_secs`
/// of its neighbours stay in the output, so natural speech rhythm is
/// preserved and only stretches longer than ~`2 × hold_secs` of true
/// silence get trimmed.
///
/// Three-pass:
/// 1. RMS each 20-ms window → bool mask `passes[i]`.
/// 2. Dilate `passes` by `hold_window_count` → `keep[i]`.
/// 3. Emit windows where `keep[i]` is true.
///
/// RMS (rather than peak) is the right per-window metric: transient
/// single-sample peaks (mouse clicks, video chapter pops) shouldn't
/// keep otherwise-silent windows alive, and sustained quiet speech
/// reliably exceeds the gate at typical mic distances.
///
/// Boundaries between kept blocks are NOT crossfaded; the discarded
/// windows are sub-threshold AND outside the hold halo, so their
/// boundary samples are near-zero, which keeps splice clicks below
/// audibility in practice.
fn trim_subthreshold_windows(
    samples: &[f32],
    sample_rate: u32,
    gate: SilenceGate,
    window_secs: f32,
) -> Vec<f32> {
    let win = ((window_secs * sample_rate as f32) as usize).max(1);
    if samples.len() < win {
        return samples.to_vec();
    }

    // Convert the dB threshold to linear once.
    let rms_lin = 10f32.powf(gate.threshold_dbfs / 20.0);
    let rms_lin_sq = (rms_lin as f64) * (rms_lin as f64);

    // Pass 1: per-window RMS test → passes[i].
    let n_windows = samples.len().div_ceil(win);
    let mut passes = vec![false; n_windows];
    for (idx, w) in passes.iter_mut().enumerate() {
        let start = idx * win;
        let end = (start + win).min(samples.len());
        let window = &samples[start..end];
        let mut sum_sq = 0.0_f64;
        for &s in window {
            sum_sq += (s as f64) * (s as f64);
        }
        let mean_sq = sum_sq / window.len() as f64;
        *w = mean_sq >= rms_lin_sq;
    }

    // Pass 2: dilate passes by ±hold_windows → keep[i]. A window stays
    // iff itself or any neighbour within the halo passes the threshold.
    let hold_windows = ((gate.hold_secs * sample_rate as f32) / win as f32).round() as usize;
    let mut keep = vec![false; n_windows];
    for i in 0..n_windows {
        if !passes[i] {
            continue;
        }
        let lo = i.saturating_sub(hold_windows);
        let hi = (i + hold_windows + 1).min(n_windows);
        for k in keep.iter_mut().take(hi).skip(lo) {
            *k = true;
        }
    }

    // Pass 3: emit kept windows.
    let mut out = Vec::with_capacity(samples.len());
    for (i, &k) in keep.iter().enumerate() {
        if !k {
            continue;
        }
        let start = i * win;
        let end = (start + win).min(samples.len());
        out.extend_from_slice(&samples[start..end]);
    }
    out
}

/// Write f32 PCM samples as a standard 16-bit mono WAV file.
fn write_wav(samples: &[f32], sample_rate: u32, path: &Path) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    // Convert f32 [-1.0, 1.0] to i16
    let pcm16: Vec<i16> = samples
        .iter()
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
    buf.extend_from_slice(&16u32.to_le_bytes()); // chunk size
    buf.extend_from_slice(&1u16.to_le_bytes()); // PCM format
    buf.extend_from_slice(&1u16.to_le_bytes()); // mono
    buf.extend_from_slice(&sample_rate.to_le_bytes()); // sample rate
    buf.extend_from_slice(&(sample_rate * 2).to_le_bytes()); // byte rate (16-bit mono)
    buf.extend_from_slice(&2u16.to_le_bytes()); // block align
    buf.extend_from_slice(&16u16.to_le_bytes()); // bits per sample

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
        let mut encoder =
            AudioEncoder::new(tmp.path().to_path_buf(), PathBuf::new(), 16000, None).unwrap();

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
        let mut encoder =
            AudioEncoder::new(tmp.path().to_path_buf(), PathBuf::new(), 16000, None).unwrap();
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
        // Tests use hold_secs=0.0 so they isolate the threshold logic
        // from the dilation pass. A separate test covers hold-time.
        SilenceGate {
            threshold_dbfs: -45.0,
            hold_secs: 0.0,
        }
    }

    #[test]
    fn silence_gate_drops_when_below_threshold() {
        let tmp = TempDir::new().unwrap();
        let mut encoder = AudioEncoder::new(
            tmp.path().to_path_buf(),
            PathBuf::from("audio/mic"),
            16000,
            Some(default_gate()),
        )
        .unwrap();
        // 1e-4 flat → RMS ≈ -80 dB everywhere; every window fails the
        // -45 dB threshold so the segment is dropped entirely.
        encoder.push_samples(&[1e-4_f32; 16000]);
        assert!(encoder.flush_segment().unwrap().is_none());
        // Buffer must be cleared even on a drop.
        assert_eq!(encoder.buffered_samples(), 0);
    }

    #[test]
    fn silence_gate_keeps_segment_above_threshold() {
        let tmp = TempDir::new().unwrap();
        let mut encoder = AudioEncoder::new(
            tmp.path().to_path_buf(),
            PathBuf::from("audio/mic"),
            16000,
            Some(default_gate()),
        )
        .unwrap();
        // 0.05 flat → RMS ≈ -26 dB per window, comfortably above -45.
        encoder.push_samples(&[0.05_f32; 16000]);
        assert!(encoder.flush_segment().unwrap().is_some());
    }

    #[test]
    fn silence_gate_trims_only_subthreshold_windows() {
        // A buffer that's half loud, half quiet should round-trip into a
        // file containing only the loud half. 16000 samples @ 16 kHz = 1 s,
        // split evenly: first 0.5 s loud (0.05 ≈ -26 dB RMS), second 0.5 s
        // quiet (1e-4 ≈ -80 dB RMS). Threshold -45 dB drops the quiet half.
        let tmp = TempDir::new().unwrap();
        let mut encoder = AudioEncoder::new(
            tmp.path().to_path_buf(),
            PathBuf::from("audio/mic"),
            16000,
            Some(default_gate()),
        )
        .unwrap();
        let mut samples = vec![0.05_f32; 8000];
        samples.extend(std::iter::repeat(1e-4_f32).take(8000));
        encoder.push_samples(&samples);
        let path = encoder
            .flush_segment()
            .unwrap()
            .expect("loud half should survive");

        // Trimmed WAV duration ≈ kept window count × 20 ms. With 8000
        // samples (0.5 s) of loud audio the kept output is exactly 0.5 s
        // — the silent half is 25 windows of 320 samples each (8000 total),
        // all dropped. Allow ±1 window of tolerance for boundary alignment.
        let bytes = std::fs::read(&path).unwrap();
        let data_chunk_size = u32::from_le_bytes(bytes[40..44].try_into().unwrap());
        let kept_samples = (data_chunk_size as usize) / 2; // 16-bit mono
        assert!(
            (kept_samples as i32 - 8000).abs() <= 320,
            "expected ~8000 samples kept, got {kept_samples}"
        );
    }

    #[test]
    fn silence_gate_drops_lone_transient_in_quiet_window() {
        // A single sample at 0.5 surrounded by 1e-4 flat: RMS over a
        // 20 ms / 320-sample window with one 0.5 spike = sqrt(0.5²/320)
        // ≈ 0.028 = -31 dB. That's above the -45 floor, so the WINDOW
        // containing the spike is kept; surrounding silent windows are
        // still dropped. Verifies the gate is RMS-based, not peak-based.
        let tmp = TempDir::new().unwrap();
        let mut encoder = AudioEncoder::new(
            tmp.path().to_path_buf(),
            PathBuf::from("audio/mic"),
            16000,
            Some(default_gate()),
        )
        .unwrap();
        let mut samples = vec![1e-4_f32; 16000];
        samples[100] = 0.5;
        encoder.push_samples(&samples);
        let path = encoder
            .flush_segment()
            .unwrap()
            .expect("spike window should survive");

        // Exactly one 320-sample window kept out of fifty.
        let bytes = std::fs::read(&path).unwrap();
        let data_chunk_size = u32::from_le_bytes(bytes[40..44].try_into().unwrap());
        let kept_samples = (data_chunk_size as usize) / 2;
        assert_eq!(kept_samples, 320, "expected one 20 ms window kept");
    }

    #[test]
    fn hold_time_preserves_neighbouring_silent_windows() {
        // 1 s loud + 2 s quiet + 1 s loud at 16 kHz. With hold_secs=1.0,
        // the 1-s gap between the loud sections sits entirely inside
        // both halos (1 s before the trailing loud + 1 s after the
        // leading loud). All 4 s of audio survive.
        //
        // With hold_secs=0.0 the same buffer would lose the entire
        // silent middle (verified by silence_gate_trims_only_subthreshold
        // above using a different layout).
        let tmp = TempDir::new().unwrap();
        let gate = SilenceGate {
            threshold_dbfs: -45.0,
            hold_secs: 1.0,
        };
        let mut encoder = AudioEncoder::new(
            tmp.path().to_path_buf(),
            PathBuf::from("audio/mic"),
            16000,
            Some(gate),
        )
        .unwrap();

        let mut samples = vec![0.05_f32; 16000]; // loud 1 s
        samples.extend(std::iter::repeat(1e-4_f32).take(32000)); // quiet 2 s
        samples.extend(std::iter::repeat(0.05_f32).take(16000)); // loud 1 s
        encoder.push_samples(&samples);
        let path = encoder
            .flush_segment()
            .unwrap()
            .expect("loud sections survive");

        let bytes = std::fs::read(&path).unwrap();
        let data_chunk_size = u32::from_le_bytes(bytes[40..44].try_into().unwrap());
        let kept_samples = (data_chunk_size as usize) / 2;

        // Whole 4 s should be preserved (within ±1 window slack for
        // boundary alignment of the dilation kernel).
        let expected = 64000;
        assert!(
            (kept_samples as i32 - expected as i32).abs() <= 320,
            "expected ~{expected} samples kept (4 s held by halo), got {kept_samples}"
        );
    }

    #[test]
    fn hold_time_still_trims_long_dead_air() {
        // Same loud-quiet-loud shape, but 6 s of quiet in the middle and
        // hold_secs=1.0. The ±1 s halo on each loud chunk reaches 1 s
        // into the silence; the middle 4 s never gets covered and is
        // trimmed. Final length: 1(loud) + 1(halo) + 1(halo) + 1(loud) = 4 s.
        let tmp = TempDir::new().unwrap();
        let gate = SilenceGate {
            threshold_dbfs: -45.0,
            hold_secs: 1.0,
        };
        let mut encoder = AudioEncoder::new(
            tmp.path().to_path_buf(),
            PathBuf::from("audio/mic"),
            16000,
            Some(gate),
        )
        .unwrap();

        let mut samples = vec![0.05_f32; 16000]; // loud 1 s
        samples.extend(std::iter::repeat(1e-4_f32).take(96000)); // quiet 6 s
        samples.extend(std::iter::repeat(0.05_f32).take(16000)); // loud 1 s
        encoder.push_samples(&samples);
        let path = encoder
            .flush_segment()
            .unwrap()
            .expect("loud + halos survive");

        let bytes = std::fs::read(&path).unwrap();
        let data_chunk_size = u32::from_le_bytes(bytes[40..44].try_into().unwrap());
        let kept_samples = (data_chunk_size as usize) / 2;

        let expected = 4 * 16000;
        assert!(
            (kept_samples as i32 - expected as i32).abs() <= 320,
            "expected ~{expected} samples kept (loud + ±1 s halos), got {kept_samples}"
        );
    }

    #[test]
    fn discard_clears_buffer() {
        let tmp = TempDir::new().unwrap();
        let mut encoder =
            AudioEncoder::new(tmp.path().to_path_buf(), PathBuf::new(), 16000, None).unwrap();
        encoder.push_samples(&[0.0; 1000]);
        assert_eq!(encoder.buffered_samples(), 1000);
        encoder.discard_segment();
        assert_eq!(encoder.buffered_samples(), 0);
    }
}
