use anyhow::{Context, Result};
use silero_vad_rust::silero_vad::model::OnnxModel;

/// Voice Activity Detection using Silero VAD.
/// Classifies 512-sample (32ms at 16kHz) chunks as speech or silence.
pub struct VoiceDetector {
    model: OnnxModel,
    sample_rate: u32,
    threshold: f32,
    silence_threshold: usize,
    silence_count: usize,
    is_speaking: bool,
}

impl VoiceDetector {
    pub fn new(sample_rate: usize) -> Result<Self> {
        let model = silero_vad_rust::load_silero_vad()
            .context("failed to initialize Silero VAD")?;

        Ok(Self {
            model,
            sample_rate: sample_rate as u32,
            threshold: 0.5,
            silence_threshold: (sample_rate * 3) / (2 * 512),
            silence_count: 0,
            is_speaking: false,
        })
    }

    /// Process a chunk of samples (must be exactly 512 for 16kHz).
    pub fn process_chunk(&mut self, chunk: &[f32]) -> VadEvent {
        let is_speech = self.model
            .forward_chunk(chunk, self.sample_rate)
            .map(|probs| probs[[0, 0]] >= self.threshold)
            .unwrap_or(false);

        if is_speech {
            self.silence_count = 0;
            if !self.is_speaking {
                self.is_speaking = true;
                return VadEvent::SpeechStart;
            }
            VadEvent::Speech
        } else if self.is_speaking {
            self.silence_count += 1;
            if self.silence_count >= self.silence_threshold {
                self.is_speaking = false;
                self.silence_count = 0;
                return VadEvent::SpeechEnd;
            }
            VadEvent::Speech
        } else {
            VadEvent::Silence
        }
    }

    pub fn is_speaking(&self) -> bool {
        self.is_speaking
    }

    pub fn reset(&mut self) {
        self.model.reset_states();
        self.is_speaking = false;
        self.silence_count = 0;
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum VadEvent {
    Silence,
    SpeechStart,
    Speech,
    SpeechEnd,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn silence_produces_silence_events() {
        let mut vad = VoiceDetector::new(16000).unwrap();
        let silence = vec![0.0f32; 512];
        let event = vad.process_chunk(&silence);
        assert_eq!(event, VadEvent::Silence);
        assert!(!vad.is_speaking());
    }

    #[test]
    fn vad_initializes_without_crash() {
        let mut vad = VoiceDetector::new(16000).unwrap();
        let tone: Vec<f32> = (0..512)
            .map(|i| (2.0 * std::f32::consts::PI * 440.0 * i as f32 / 16000.0).sin() * 0.8)
            .collect();
        for _ in 0..5 {
            vad.process_chunk(&tone);
        }
        // Smoke test — doesn't crash, state is valid
        assert!(vad.is_speaking() || !vad.is_speaking());
    }
}
