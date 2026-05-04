use crate::fingerprint::AudioFingerprint;

pub(super) fn fingerprint_score(left: &AudioFingerprint, right: &AudioFingerprint) -> f32 {
    if left.model != right.model
        || left.sample_rate_hz != right.sample_rate_hz
        || left.vector.len() != right.vector.len()
    {
        return 0.0;
    }
    if left.digest == right.digest {
        return 1.0;
    }
    if left.vector.is_empty() {
        return 0.0;
    }
    let mut dot = 0.0_f32;
    let mut left_norm = 0.0_f32;
    let mut right_norm = 0.0_f32;
    for index in 0..left.vector.len() {
        dot += left.vector[index] * right.vector[index];
        left_norm += left.vector[index] * left.vector[index];
        right_norm += right.vector[index] * right.vector[index];
    }
    if left_norm == 0.0 || right_norm == 0.0 {
        return 0.0;
    }
    dot / (left_norm.sqrt() * right_norm.sqrt())
}
