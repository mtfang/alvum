//! CaptureSource implementations for audio: mic and system audio.
//! Each source independently manages one audio stream + encoder.

use alvum_core::capture::CaptureSource;
use alvum_core::config::CaptureSourceConfig;
use anyhow::{Context, Result};
use std::path::Path;
use std::sync::{Arc, Mutex};
use tokio::sync::watch;
use tracing::info;

use crate::capture::{self, SAMPLE_RATE};
use crate::devices;
use crate::encoder::AudioEncoder;
use crate::recorder::make_chunked_callback;

/// Captures microphone audio. Reads `device` and `chunk_duration_secs` from config.
pub struct AudioMicSource {
    device_name: Option<String>,
    chunk_duration_secs: u32,
}

impl AudioMicSource {
    pub fn from_config(config: &CaptureSourceConfig) -> Self {
        let device_name = config.settings.get("device")
            .and_then(|v| v.as_str())
            .filter(|s| *s != "default")
            .map(|s| s.to_string());

        let chunk_duration_secs = config.settings.get("chunk_duration_secs")
            .and_then(|v| v.as_integer())
            .unwrap_or(60) as u32;

        Self { device_name, chunk_duration_secs }
    }
}

#[async_trait::async_trait]
impl CaptureSource for AudioMicSource {
    fn name(&self) -> &str {
        "audio-mic"
    }

    async fn run(&self, capture_dir: &Path, mut shutdown: watch::Receiver<bool>) -> Result<()> {
        let samples_per_chunk = SAMPLE_RATE as usize * self.chunk_duration_secs as usize;

        let encoder = Arc::new(Mutex::new(AudioEncoder::new(
            capture_dir.to_path_buf(),
            std::path::PathBuf::from("audio").join("mic"),
            SAMPLE_RATE,
        )?));
        let callback = make_chunked_callback(encoder.clone(), samples_per_chunk, "mic".into());

        let mut current_bound: Option<String> = None;
        let mut current_stream: Option<capture::AudioStream> = None;

        // Repoll cadence for device-change detection. When a call starts
        // and macOS swaps default-input to AirPods-HFP, we pick it up at
        // most this long afterward and rebind the cpal stream.
        const REPOLL_INTERVAL: std::time::Duration = std::time::Duration::from_secs(3);

        loop {
            let hal_devices = crate::coreaudio_hal::list_input_devices()
                .context("enumerate CoreAudio input devices")?;
            let default_id = crate::coreaudio_hal::default_input_device_id()
                .context("query default input device")?;

            let want = crate::mic_selection::decide_swap(
                &hal_devices,
                default_id,
                self.device_name.as_deref(),
                current_bound.as_deref(),
            );

            if let Some(new_name) = want {
                // Drop the old stream first so cpal releases the device
                // handle before we open the new one.
                drop(current_stream.take());
                let new_name = new_name.to_string();
                let device = devices::get_input_device(Some(&new_name))
                    .with_context(|| format!("open cpal device {new_name:?}"))?;
                let stream = capture::start_capture(&device, "mic", callback.clone())?;
                info!(device = %new_name, "audio-mic bound input device");
                current_bound = Some(new_name);
                current_stream = Some(stream);
            }

            tokio::select! {
                _ = shutdown.changed() => {
                    if *shutdown.borrow() {
                        break;
                    }
                }
                _ = tokio::time::sleep(REPOLL_INTERVAL) => {}
            }
        }

        drop(current_stream);
        if let Ok(mut enc) = encoder.lock() {
            let _ = enc.flush_segment();
        }
        info!("audio-mic source stopped");
        Ok(())
    }
}

/// Captures system audio via ScreenCaptureKit. The audio is tapped at the
/// macOS process graph, independent of which output device is active — so
/// it stays alive across AirPods/AirPlay/HDMI switches. No `device` config
/// key is consulted: SCK owns device selection.
pub struct AudioSystemSource {
    chunk_duration_secs: u32,
}

impl AudioSystemSource {
    pub fn from_config(config: &CaptureSourceConfig) -> Self {
        Self::try_from_config(config)
            .expect("audio-system config invalid; fix [capture.audio-system] in ~/.alvum/runtime/config.toml")
    }

    pub fn try_from_config(config: &CaptureSourceConfig) -> Result<Self> {
        let chunk_duration_secs = config.settings.get("chunk_duration_secs")
            .and_then(|v| v.as_integer())
            .unwrap_or(60) as u32;

        let exclude_names = extract_string_list(&config.settings, "exclude_apps");
        let exclude_bundles = extract_string_list(&config.settings, "exclude_bundle_ids");
        let include_names = extract_string_list(&config.settings, "include_apps");
        let include_bundles = extract_string_list(&config.settings, "include_bundle_ids");

        let has_exclude = !exclude_names.is_empty() || !exclude_bundles.is_empty();
        let has_include = !include_names.is_empty() || !include_bundles.is_empty();
        if has_exclude && has_include {
            anyhow::bail!(
                "[capture.audio-system] include_apps/include_bundle_ids and \
                 exclude_apps/exclude_bundle_ids are mutually exclusive (set at most one pair)"
            );
        }

        let filter = if has_include {
            alvum_capture_sck::AppFilter::Include {
                names: include_names,
                bundle_ids: include_bundles,
            }
        } else {
            alvum_capture_sck::AppFilter::Exclude {
                names: exclude_names,
                bundle_ids: exclude_bundles,
            }
        };

        // Push filter config to SCK synchronously. This runs on the
        // pipeline-setup task BEFORE any source .run() is spawned, so
        // whichever source lazily triggers ensure_started first will
        // see this filter in the SCContentFilter it builds.
        alvum_capture_sck::configure(alvum_capture_sck::SharedStreamConfig { filter });

        Ok(Self { chunk_duration_secs })
    }
}

fn extract_string_list(
    settings: &std::collections::HashMap<String, toml::Value>,
    key: &str,
) -> Vec<String> {
    settings
        .get(key)
        .and_then(|v| v.as_array())
        .map(|a| a.iter().filter_map(|v| v.as_str().map(String::from)).collect())
        .unwrap_or_default()
}

#[async_trait::async_trait]
impl CaptureSource for AudioSystemSource {
    fn name(&self) -> &str {
        "audio-system"
    }

    async fn run(&self, capture_dir: &Path, mut shutdown: watch::Receiver<bool>) -> Result<()> {
        let samples_per_chunk = SAMPLE_RATE as usize * self.chunk_duration_secs as usize;

        let encoder = Arc::new(Mutex::new(AudioEncoder::new(
            capture_dir.to_path_buf(),
            std::path::PathBuf::from("audio").join("system"),
            SAMPLE_RATE,
        )?));
        let callback = make_chunked_callback(encoder.clone(), samples_per_chunk, "system".into());

        // System audio flows through the shared SCK stream (owned by
        // alvum_capture_sck). Starting is idempotent — screen may already
        // have brought the stream up. Failure is typically a Screen
        // Recording permission denial; degrade to no-op instead of
        // aborting other sources.
        if let Err(e) = alvum_capture_sck::ensure_started() {
            tracing::warn!(error = %e, "SCK shared stream unavailable, audio-system will not run");
            while !*shutdown.borrow_and_update() {
                if shutdown.changed().await.is_err() {
                    break;
                }
            }
            return Ok(());
        }

        alvum_capture_sck::set_audio_callback(Some(callback));
        info!("audio-system source started (SCK)");

        while !*shutdown.borrow_and_update() {
            if shutdown.changed().await.is_err() {
                break;
            }
        }

        alvum_capture_sck::set_audio_callback(None);
        if let Ok(mut enc) = encoder.lock() {
            let _ = enc.flush_segment();
        }

        info!("audio-system source stopped");
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alvum_capture_sck::AppFilter;
    use std::collections::HashMap;

    fn toml_str_array(items: &[&str]) -> toml::Value {
        toml::Value::Array(items.iter().map(|s| toml::Value::String((*s).into())).collect())
    }

    // Tests share global SCK filter state via configure()/snapshot_config_for_test();
    // serialize them so they don't clobber each other's config.
    fn lock() -> std::sync::MutexGuard<'static, ()> {
        use std::sync::{Mutex, OnceLock};
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
            .lock()
            .unwrap_or_else(|e| e.into_inner())
    }

    #[test]
    fn audio_system_from_config_defaults_to_open_world_exclude() {
        let _guard = lock();
        let cfg = CaptureSourceConfig { enabled: true, settings: HashMap::new() };
        let _ = AudioSystemSource::try_from_config(&cfg).expect("default config");
        let live = alvum_capture_sck::snapshot_config_for_test();
        match live.filter {
            AppFilter::Exclude { names, bundle_ids } => {
                assert!(names.is_empty());
                assert!(bundle_ids.is_empty());
            }
            other => panic!("expected Exclude, got {other:?}"),
        }
    }

    #[test]
    fn audio_system_from_config_exclude_mode() {
        let _guard = lock();
        let mut settings: HashMap<String, toml::Value> = HashMap::new();
        settings.insert("exclude_apps".into(), toml_str_array(&["Music", "Spotify"]));
        settings.insert("exclude_bundle_ids".into(), toml_str_array(&["com.apple.Music"]));
        let cfg = CaptureSourceConfig { enabled: true, settings };
        let _ = AudioSystemSource::try_from_config(&cfg).expect("exclude config");
        let live = alvum_capture_sck::snapshot_config_for_test();
        match live.filter {
            AppFilter::Exclude { names, bundle_ids } => {
                assert_eq!(names, vec!["Music".to_string(), "Spotify".to_string()]);
                assert_eq!(bundle_ids, vec!["com.apple.Music".to_string()]);
            }
            other => panic!("expected Exclude, got {other:?}"),
        }
    }

    #[test]
    fn audio_system_from_config_include_mode() {
        let _guard = lock();
        let mut settings: HashMap<String, toml::Value> = HashMap::new();
        settings.insert("include_apps".into(), toml_str_array(&["Zoom", "Safari"]));
        settings.insert("include_bundle_ids".into(), toml_str_array(&["us.zoom.xos"]));
        let cfg = CaptureSourceConfig { enabled: true, settings };
        let _ = AudioSystemSource::try_from_config(&cfg).expect("include config");
        let live = alvum_capture_sck::snapshot_config_for_test();
        match live.filter {
            AppFilter::Include { names, bundle_ids } => {
                assert_eq!(names, vec!["Zoom".to_string(), "Safari".to_string()]);
                assert_eq!(bundle_ids, vec!["us.zoom.xos".to_string()]);
            }
            other => panic!("expected Include, got {other:?}"),
        }
    }

    #[test]
    fn audio_system_from_config_both_include_and_exclude_is_error() {
        let _guard = lock();
        let mut settings: HashMap<String, toml::Value> = HashMap::new();
        settings.insert("exclude_apps".into(), toml_str_array(&["Music"]));
        settings.insert("include_apps".into(), toml_str_array(&["Zoom"]));
        let cfg = CaptureSourceConfig { enabled: true, settings };
        let err = match AudioSystemSource::try_from_config(&cfg) {
            Err(e) => e.to_string(),
            Ok(_) => panic!("expected mutual-exclusivity error, got Ok"),
        };
        assert!(
            err.contains("mutually exclusive"),
            "error should mention the mutual-exclusivity violation, got: {err}"
        );
    }
}
