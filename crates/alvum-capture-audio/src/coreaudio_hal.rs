//! Thin CoreAudio HAL wrappers for device enumeration and default-input
//! tracking. Exists because cpal doesn't expose transport type or device
//! IDs — and we need both to skip silent A2DP Bluetooth mic endpoints
//! and follow the OS default-input when a call starts.

use anyhow::{anyhow, Result};
use std::ffi::c_void;

const K_AUDIO_OBJECT_SYSTEM_OBJECT: u32 = 1;
const K_AUDIO_HARDWARE_PROPERTY_DEVICES: u32 = fourcc(b"dev#");
const K_AUDIO_HARDWARE_PROPERTY_DEFAULT_INPUT_DEVICE: u32 = fourcc(b"dIn ");
const K_AUDIO_DEVICE_PROPERTY_TRANSPORT_TYPE: u32 = fourcc(b"tran");
const K_AUDIO_OBJECT_PROPERTY_NAME: u32 = fourcc(b"lnam");
const K_AUDIO_DEVICE_PROPERTY_STREAMS: u32 = fourcc(b"stm#");
const K_AUDIO_OBJECT_PROPERTY_SCOPE_GLOBAL: u32 = fourcc(b"glob");
const K_AUDIO_OBJECT_PROPERTY_SCOPE_INPUT: u32 = fourcc(b"inpt");
const K_AUDIO_OBJECT_PROPERTY_ELEMENT_MAIN: u32 = 0;

pub const K_AUDIO_DEVICE_TRANSPORT_TYPE_BLUETOOTH: u32 = fourcc(b"blue");
pub const K_AUDIO_DEVICE_TRANSPORT_TYPE_BLUETOOTH_LE: u32 = fourcc(b"blea");

#[repr(C)]
struct AudioObjectPropertyAddress {
    m_selector: u32,
    m_scope: u32,
    m_element: u32,
}

#[link(name = "CoreAudio", kind = "framework")]
unsafe extern "C" {
    fn AudioObjectGetPropertyDataSize(
        in_object_id: u32,
        in_address: *const AudioObjectPropertyAddress,
        in_qualifier_data_size: u32,
        in_qualifier_data: *const c_void,
        out_data_size: *mut u32,
    ) -> i32;

    fn AudioObjectGetPropertyData(
        in_object_id: u32,
        in_address: *const AudioObjectPropertyAddress,
        in_qualifier_data_size: u32,
        in_qualifier_data: *const c_void,
        io_data_size: *mut u32,
        out_data: *mut c_void,
    ) -> i32;
}

const fn fourcc(code: &[u8; 4]) -> u32 {
    ((code[0] as u32) << 24) | ((code[1] as u32) << 16) | ((code[2] as u32) << 8) | (code[3] as u32)
}

#[derive(Debug, Clone, PartialEq)]
pub struct DeviceInfo {
    pub id: u32,
    pub name: String,
    pub transport_type: u32,
    pub has_input_stream: bool,
}

impl DeviceInfo {
    pub fn is_bluetooth(&self) -> bool {
        matches!(
            self.transport_type,
            K_AUDIO_DEVICE_TRANSPORT_TYPE_BLUETOOTH | K_AUDIO_DEVICE_TRANSPORT_TYPE_BLUETOOTH_LE
        )
    }
}

/// List all audio devices that have at least one input stream. The returned
/// devices include name and transport type so higher-level code can skip
/// Bluetooth A2DP endpoints.
pub fn list_input_devices() -> Result<Vec<DeviceInfo>> {
    let ids = all_device_ids()?;
    let mut out = Vec::with_capacity(ids.len());
    for id in ids {
        let has_input = device_has_input_stream(id).unwrap_or(false);
        if !has_input {
            continue;
        }
        let name = device_name(id).unwrap_or_else(|_| format!("Unknown({id})"));
        let transport = device_transport_type(id).unwrap_or(0);
        out.push(DeviceInfo {
            id,
            name,
            transport_type: transport,
            has_input_stream: true,
        });
    }
    Ok(out)
}

/// Current default input device ID (what cpal would default to).
pub fn default_input_device_id() -> Result<u32> {
    let addr = AudioObjectPropertyAddress {
        m_selector: K_AUDIO_HARDWARE_PROPERTY_DEFAULT_INPUT_DEVICE,
        m_scope: K_AUDIO_OBJECT_PROPERTY_SCOPE_GLOBAL,
        m_element: K_AUDIO_OBJECT_PROPERTY_ELEMENT_MAIN,
    };
    let mut id: u32 = 0;
    let mut size = std::mem::size_of::<u32>() as u32;
    let status = unsafe {
        AudioObjectGetPropertyData(
            K_AUDIO_OBJECT_SYSTEM_OBJECT,
            &addr,
            0,
            std::ptr::null(),
            &mut size,
            &mut id as *mut _ as *mut c_void,
        )
    };
    if status != 0 {
        return Err(anyhow!("AudioObjectGetPropertyData(default input) → {status}"));
    }
    Ok(id)
}

fn all_device_ids() -> Result<Vec<u32>> {
    let addr = AudioObjectPropertyAddress {
        m_selector: K_AUDIO_HARDWARE_PROPERTY_DEVICES,
        m_scope: K_AUDIO_OBJECT_PROPERTY_SCOPE_GLOBAL,
        m_element: K_AUDIO_OBJECT_PROPERTY_ELEMENT_MAIN,
    };
    let mut size: u32 = 0;
    let status = unsafe {
        AudioObjectGetPropertyDataSize(
            K_AUDIO_OBJECT_SYSTEM_OBJECT,
            &addr,
            0,
            std::ptr::null(),
            &mut size,
        )
    };
    if status != 0 {
        return Err(anyhow!("AudioObjectGetPropertyDataSize(devices) → {status}"));
    }
    let count = (size as usize) / std::mem::size_of::<u32>();
    let mut ids = vec![0u32; count];
    let mut io_size = size;
    let status = unsafe {
        AudioObjectGetPropertyData(
            K_AUDIO_OBJECT_SYSTEM_OBJECT,
            &addr,
            0,
            std::ptr::null(),
            &mut io_size,
            ids.as_mut_ptr() as *mut c_void,
        )
    };
    if status != 0 {
        return Err(anyhow!("AudioObjectGetPropertyData(devices) → {status}"));
    }
    Ok(ids)
}

fn device_has_input_stream(device_id: u32) -> Result<bool> {
    let addr = AudioObjectPropertyAddress {
        m_selector: K_AUDIO_DEVICE_PROPERTY_STREAMS,
        m_scope: K_AUDIO_OBJECT_PROPERTY_SCOPE_INPUT,
        m_element: K_AUDIO_OBJECT_PROPERTY_ELEMENT_MAIN,
    };
    let mut size: u32 = 0;
    let status = unsafe {
        AudioObjectGetPropertyDataSize(device_id, &addr, 0, std::ptr::null(), &mut size)
    };
    if status != 0 {
        return Err(anyhow!("streams size → {status}"));
    }
    Ok(size > 0)
}

fn device_transport_type(device_id: u32) -> Result<u32> {
    let addr = AudioObjectPropertyAddress {
        m_selector: K_AUDIO_DEVICE_PROPERTY_TRANSPORT_TYPE,
        m_scope: K_AUDIO_OBJECT_PROPERTY_SCOPE_GLOBAL,
        m_element: K_AUDIO_OBJECT_PROPERTY_ELEMENT_MAIN,
    };
    let mut val: u32 = 0;
    let mut size = std::mem::size_of::<u32>() as u32;
    let status = unsafe {
        AudioObjectGetPropertyData(
            device_id,
            &addr,
            0,
            std::ptr::null(),
            &mut size,
            &mut val as *mut _ as *mut c_void,
        )
    };
    if status != 0 {
        return Err(anyhow!("transport type → {status}"));
    }
    Ok(val)
}

fn device_name(device_id: u32) -> Result<String> {
    use objc2_core_foundation::{CFRetained, CFString};

    let addr = AudioObjectPropertyAddress {
        m_selector: K_AUDIO_OBJECT_PROPERTY_NAME,
        m_scope: K_AUDIO_OBJECT_PROPERTY_SCOPE_GLOBAL,
        m_element: K_AUDIO_OBJECT_PROPERTY_ELEMENT_MAIN,
    };
    let mut cf_ptr: *mut CFString = std::ptr::null_mut();
    let mut size = std::mem::size_of::<*mut CFString>() as u32;
    let status = unsafe {
        AudioObjectGetPropertyData(
            device_id,
            &addr,
            0,
            std::ptr::null(),
            &mut size,
            &mut cf_ptr as *mut _ as *mut c_void,
        )
    };
    if status != 0 || cf_ptr.is_null() {
        return Err(anyhow!("device name → {status}"));
    }
    let retained = unsafe { CFRetained::from_raw(std::ptr::NonNull::new_unchecked(cf_ptr)) };
    Ok(retained.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn list_input_devices_returns_at_least_one() {
        let devices = list_input_devices().expect("enumerate inputs");
        assert!(
            !devices.is_empty(),
            "expected at least one input device on this host"
        );
        for d in &devices {
            assert!(d.has_input_stream);
            assert!(!d.name.is_empty(), "device id {} has empty name", d.id);
        }
    }

    #[test]
    fn default_input_device_id_returns_nonzero() {
        let id = default_input_device_id().expect("default input id");
        assert!(id > 0, "default input id should be nonzero");
    }
}
