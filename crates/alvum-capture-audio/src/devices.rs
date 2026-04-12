use anyhow::{Context, Result};
use cpal::traits::{DeviceTrait, HostTrait};

#[derive(Debug, Clone)]
pub struct AudioDevice {
    pub name: String,
    pub is_input: bool,
    pub is_output: bool,
}

pub fn list_devices() -> Result<Vec<AudioDevice>> {
    let host = cpal::default_host();
    let mut devices = Vec::new();
    for device in host.devices().context("failed to enumerate audio devices")? {
        let name = device.name().unwrap_or_else(|_| "Unknown".into());
        let is_input = device.supported_input_configs()
            .map(|mut c| c.next().is_some())
            .unwrap_or(false);
        let is_output = device.supported_output_configs()
            .map(|mut c| c.next().is_some())
            .unwrap_or(false);
        if is_input || is_output {
            devices.push(AudioDevice { name, is_input, is_output });
        }
    }
    Ok(devices)
}

pub fn get_input_device(name: Option<&str>) -> Result<cpal::Device> {
    let host = cpal::default_host();
    match name {
        Some(target) => {
            host.devices()
                .context("failed to enumerate devices")?
                .find(|d| d.name().ok().as_deref() == Some(target))
                .with_context(|| format!("input device not found: {target}"))
        }
        None => {
            host.default_input_device()
                .context("no default input device available")
        }
    }
}

pub fn get_output_device(name: Option<&str>) -> Result<cpal::Device> {
    let host = cpal::default_host();
    match name {
        Some(target) => {
            host.devices()
                .context("failed to enumerate devices")?
                .find(|d| d.name().ok().as_deref() == Some(target))
                .with_context(|| format!("output device not found: {target}"))
        }
        None => {
            host.default_output_device()
                .context("no default output device available")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn list_devices_returns_at_least_one() {
        let devices = list_devices().unwrap();
        assert!(!devices.is_empty(), "expected at least one audio device");
    }

    #[test]
    fn default_input_device_exists() {
        let device = get_input_device(None).unwrap();
        assert!(!device.name().unwrap().is_empty());
    }

    #[test]
    fn nonexistent_device_errors() {
        let result = get_input_device(Some("NONEXISTENT_DEVICE_12345"));
        assert!(result.is_err());
    }
}
