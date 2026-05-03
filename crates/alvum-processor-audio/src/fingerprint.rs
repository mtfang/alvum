use std::hash::{Hash, Hasher};

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct AudioFingerprint {
    pub model: String,
    pub sample_rate_hz: u32,
    pub vector: Vec<f32>,
    pub digest: String,
}

impl AudioFingerprint {
    pub fn from_samples(samples: &[f32], sample_rate_hz: u32) -> Self {
        let vector = acoustic_vector(samples);
        Self {
            model: "alvum.acoustic-v1".into(),
            sample_rate_hz,
            digest: digest_vector(&vector),
            vector,
        }
    }

    pub fn from_vector(model: impl Into<String>, sample_rate_hz: u32, vector: Vec<f32>) -> Self {
        Self {
            model: model.into(),
            sample_rate_hz,
            digest: digest_vector(&vector),
            vector,
        }
    }
}

fn acoustic_vector(samples: &[f32]) -> Vec<f32> {
    if samples.is_empty() {
        return vec![0.0; 12];
    }

    let mut sum = 0.0_f32;
    let mut abs_sum = 0.0_f32;
    let mut sq_sum = 0.0_f32;
    let mut peak = 0.0_f32;
    let mut crossings = 0usize;
    let mut prev = samples[0];
    for &sample in samples {
        sum += sample;
        abs_sum += sample.abs();
        sq_sum += sample * sample;
        peak = peak.max(sample.abs());
        if (prev < 0.0 && sample >= 0.0) || (prev >= 0.0 && sample < 0.0) {
            crossings += 1;
        }
        prev = sample;
    }

    let len = samples.len() as f32;
    let mut vector = vec![
        sum / len,
        abs_sum / len,
        (sq_sum / len).sqrt(),
        peak,
        crossings as f32 / len,
    ];

    let bucket_count = 7usize;
    for bucket in 0..bucket_count {
        let start = bucket * samples.len() / bucket_count;
        let end = ((bucket + 1) * samples.len() / bucket_count).max(start + 1);
        let slice = &samples[start..end.min(samples.len())];
        let energy = slice.iter().map(|sample| sample * sample).sum::<f32>() / slice.len() as f32;
        vector.push(energy.sqrt());
    }
    vector
}

fn digest_vector(vector: &[f32]) -> String {
    let mut hasher = Fnv1a64::default();
    for value in vector {
        let quantized = (value * 10_000.0).round() as i32;
        quantized.hash(&mut hasher);
    }
    format!("{:016x}", hasher.finish())
}

#[derive(Default)]
struct Fnv1a64(u64);

impl Hasher for Fnv1a64 {
    fn write(&mut self, bytes: &[u8]) {
        if self.0 == 0 {
            self.0 = 0xcbf29ce484222325;
        }
        for byte in bytes {
            self.0 ^= u64::from(*byte);
            self.0 = self.0.wrapping_mul(0x100000001b3);
        }
    }

    fn finish(&self) -> u64 {
        self.0
    }
}
