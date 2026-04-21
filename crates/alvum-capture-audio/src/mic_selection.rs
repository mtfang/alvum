//! Pure mic-selection policy. Separated from the CoreAudio FFI so we can
//! test every combination of connected devices and config overrides
//! without a real audio host.

use crate::coreaudio_hal::DeviceInfo;

/// Choose which input device the mic capture should bind to.
///
/// Rules:
/// 1. If `override_name` is Some, return the device whose name matches
///    exactly. No fallback — user override is authoritative.
/// 2. Else prefer a non-Bluetooth input (built-in, USB, Thunderbolt, etc.).
///    Among non-BT candidates, the one matching the OS default input wins,
///    else the first in the list.
/// 3. If only Bluetooth inputs exist, fall back to the OS default, else
///    the first device. A call in progress makes AirPods-HFP the default
///    and it delivers real audio — so "only BT" is still useful then.
pub fn choose_mic_device<'a>(
    devices: &'a [DeviceInfo],
    default_input_id: u32,
    override_name: Option<&str>,
) -> Option<&'a DeviceInfo> {
    if let Some(name) = override_name {
        return devices.iter().find(|d| d.name == name);
    }
    let non_bt: Vec<&DeviceInfo> = devices.iter().filter(|d| !d.is_bluetooth()).collect();
    if !non_bt.is_empty() {
        return non_bt
            .iter()
            .find(|d| d.id == default_input_id)
            .copied()
            .or_else(|| non_bt.first().copied());
    }
    devices.iter().find(|d| d.id == default_input_id).or_else(|| devices.first())
}

/// Given the currently-bound device name and a fresh snapshot of devices
/// + default-input, decide whether to swap and to what.
///
/// Returns `Some(new_name)` when a swap should happen, `None` when the
/// current binding is still best. Pure — no side effects — so swap logic
/// is fully unit-testable.
pub fn decide_swap<'a>(
    devices: &'a [DeviceInfo],
    default_input_id: u32,
    override_name: Option<&str>,
    currently_bound: Option<&str>,
) -> Option<&'a str> {
    let best = choose_mic_device(devices, default_input_id, override_name)?;
    match currently_bound {
        Some(cur) if cur == best.name => None,
        _ => Some(best.name.as_str()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::coreaudio_hal::{DeviceInfo, K_AUDIO_DEVICE_TRANSPORT_TYPE_BLUETOOTH};

    const TRANSPORT_BUILT_IN: u32 = 0x62696c74; // 'bilt'
    const TRANSPORT_USB: u32 = 0x75736220; // 'usb '

    fn device(id: u32, name: &str, transport: u32) -> DeviceInfo {
        DeviceInfo {
            id,
            name: name.into(),
            transport_type: transport,
            has_input_stream: true,
        }
    }

    #[test]
    fn override_name_exact_match_wins() {
        let devs = vec![
            device(1, "MacBook Pro Microphone", TRANSPORT_BUILT_IN),
            device(2, "Michael's AirPods Pro", K_AUDIO_DEVICE_TRANSPORT_TYPE_BLUETOOTH),
        ];
        let chosen = choose_mic_device(&devs, 1, Some("Michael's AirPods Pro")).unwrap();
        assert_eq!(chosen.id, 2);
    }

    #[test]
    fn override_name_no_match_returns_none() {
        let devs = vec![device(1, "MacBook Pro Microphone", TRANSPORT_BUILT_IN)];
        let chosen = choose_mic_device(&devs, 1, Some("Nonexistent"));
        assert!(chosen.is_none());
    }

    #[test]
    fn prefers_built_in_when_airpods_is_default() {
        let devs = vec![
            device(1, "MacBook Pro Microphone", TRANSPORT_BUILT_IN),
            device(2, "AirPods Pro", K_AUDIO_DEVICE_TRANSPORT_TYPE_BLUETOOTH),
        ];
        // OS default is AirPods (id=2) — we should still pick built-in.
        let chosen = choose_mic_device(&devs, 2, None).unwrap();
        assert_eq!(chosen.id, 1, "built-in must win over BT A2DP default");
    }

    #[test]
    fn picks_os_default_among_non_bt_options() {
        let devs = vec![
            device(1, "Built-in", TRANSPORT_BUILT_IN),
            device(2, "USB Yeti", TRANSPORT_USB),
        ];
        let chosen = choose_mic_device(&devs, 2, None).unwrap();
        assert_eq!(chosen.id, 2);
    }

    #[test]
    fn falls_back_to_default_when_only_bt() {
        let devs = vec![
            device(2, "AirPods Pro", K_AUDIO_DEVICE_TRANSPORT_TYPE_BLUETOOTH),
            device(3, "AirPods Max", K_AUDIO_DEVICE_TRANSPORT_TYPE_BLUETOOTH),
        ];
        let chosen = choose_mic_device(&devs, 3, None).unwrap();
        assert_eq!(chosen.id, 3);
    }

    #[test]
    fn empty_returns_none() {
        let chosen = choose_mic_device(&[], 0, None);
        assert!(chosen.is_none());
    }

    #[test]
    fn swap_none_when_current_still_best() {
        let devs = vec![device(1, "Built-in", TRANSPORT_BUILT_IN)];
        let r = decide_swap(&devs, 1, None, Some("Built-in"));
        assert!(r.is_none());
    }

    #[test]
    fn swap_to_airpods_hfp_when_call_makes_it_real() {
        let devs = vec![device(2, "AirPods Pro", K_AUDIO_DEVICE_TRANSPORT_TYPE_BLUETOOTH)];
        let r = decide_swap(&devs, 2, None, Some("Built-in"));
        assert_eq!(r, Some("AirPods Pro"));
    }

    #[test]
    fn swap_back_to_built_in_after_call() {
        let devs = vec![
            device(1, "Built-in", TRANSPORT_BUILT_IN),
            device(2, "AirPods Pro", K_AUDIO_DEVICE_TRANSPORT_TYPE_BLUETOOTH),
        ];
        // OS default back to built-in; we're still bound to AirPods.
        let r = decide_swap(&devs, 1, None, Some("AirPods Pro"));
        assert_eq!(r, Some("Built-in"));
    }

    #[test]
    fn swap_from_unbound_to_first_pick() {
        let devs = vec![device(1, "Built-in", TRANSPORT_BUILT_IN)];
        let r = decide_swap(&devs, 1, None, None);
        assert_eq!(r, Some("Built-in"));
    }

    #[test]
    fn swap_none_when_no_devices_and_nothing_bound() {
        let r = decide_swap(&[], 0, None, None);
        assert!(r.is_none());
    }
}
