use anyhow::{Context, Result};
use cpal::traits::{DeviceTrait, StreamTrait};
use cpal::{Device, Stream, StreamConfig};
use std::sync::{Arc, Mutex};
use tracing::{error, info};

pub const SAMPLE_RATE: u32 = 16000;

pub struct AudioStream {
    _stream: Stream,
    pub device_name: String,
    pub label: String,
}

pub type SampleCallback = Arc<Mutex<dyn FnMut(&[f32]) + Send>>;

/// Start capturing audio from a device. Calls `callback` with f32 sample chunks at
/// `SAMPLE_RATE` (16kHz). If the device does not natively support 16kHz, the capture
/// runs at the device's default rate and samples are decimated down to 16kHz before
/// being forwarded to the callback.
/// `label` is for logging (e.g., "mic", "system").
pub fn start_capture(
    device: &Device,
    label: &str,
    callback: SampleCallback,
) -> Result<AudioStream> {
    let device_name = device.description()
        .map(|d| d.name().to_string())
        .unwrap_or_else(|_| "Unknown".into());
    info!(device = %device_name, label, "starting audio capture");

    // Determine capture sample rate: prefer 16kHz if the device supports it, otherwise
    // use the device's default rate and downsample in the callback.
    let supported = device
        .supported_input_configs()
        .context("failed to query supported input configs")?;

    let preferred_rate = SAMPLE_RATE;
    let capture_rate = supported
        .filter(|c| c.channels() == 1)
        .find(|c| c.min_sample_rate() <= preferred_rate && preferred_rate <= c.max_sample_rate())
        .map(|_| preferred_rate)
        .unwrap_or_else(|| {
            device
                .default_input_config()
                .map(|c| c.sample_rate())
                .unwrap_or(48000)
        });

    let config = StreamConfig {
        channels: 1,
        sample_rate: capture_rate,
        buffer_size: cpal::BufferSize::Default,
    };

    info!(device = %device_name, label, capture_rate, "audio capture config");

    // Build a downsampler state if capture_rate != SAMPLE_RATE.
    // Simple integer or near-integer decimation using a linear accumulator.
    let label_err = label.to_string();
    let stream = if capture_rate == SAMPLE_RATE {
        device.build_input_stream(
            &config,
            move |data: &[f32], _: &cpal::InputCallbackInfo| {
                if let Ok(mut cb) = callback.lock() {
                    cb(data);
                }
            },
            move |err| {
                error!(label = %label_err, error = %err, "audio stream error");
            },
            None,
        )
    } else {
        // Resample via linear interpolation decimation.
        // ratio = capture_rate / SAMPLE_RATE (e.g. 3.0 for 48kHz → 16kHz).
        let ratio = capture_rate as f64 / SAMPLE_RATE as f64;
        // Phase accumulator tracks position in the input stream (in input samples).
        let phase: Arc<Mutex<f64>> = Arc::new(Mutex::new(0.0));
        device.build_input_stream(
            &config,
            move |data: &[f32], _: &cpal::InputCallbackInfo| {
                let mut out = Vec::with_capacity((data.len() as f64 / ratio) as usize + 1);
                let mut ph = phase.lock().unwrap();
                let mut i = *ph;
                while i < data.len() as f64 {
                    let idx = i as usize;
                    let frac = i - idx as f64;
                    let s0 = data[idx];
                    let s1 = if idx + 1 < data.len() { data[idx + 1] } else { s0 };
                    out.push(s0 + (s1 - s0) * frac as f32);
                    i += ratio;
                }
                // Carry over the fractional position into the next callback.
                *ph = i - data.len() as f64;
                drop(ph);
                if !out.is_empty() {
                    if let Ok(mut cb) = callback.lock() {
                        cb(&out);
                    }
                }
            },
            move |err| {
                error!(label = %label_err, error = %err, "audio stream error");
            },
            None,
        )
    }
    .with_context(|| format!("failed to build audio stream for {label}"))?;

    stream.play().with_context(|| format!("failed to start audio stream for {label}"))?;
    info!(device = %device_name, label, "audio capture started");

    Ok(AudioStream {
        _stream: stream,
        device_name,
        label: label.to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::Duration;

    #[test]
    fn capture_receives_samples() {
        let device = crate::devices::get_input_device(None).unwrap();
        let received = Arc::new(AtomicBool::new(false));
        let received_clone = received.clone();

        let callback: SampleCallback = Arc::new(Mutex::new(move |data: &[f32]| {
            if !data.is_empty() {
                received_clone.store(true, Ordering::SeqCst);
            }
        }));

        let _stream = start_capture(&device, "test", callback).unwrap();
        std::thread::sleep(Duration::from_millis(500));
        assert!(received.load(Ordering::SeqCst), "expected to receive audio samples");
    }
}
