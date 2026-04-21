# Audio Capture Fixes Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Fix two production audio-capture bugs observed in 2026-04-20 captures, and add per-app filtering for system-audio:
1. Microphone captures 100% digital zeros (−91 dB floor, every chunk) when AirPods are the default input device — because AirPods in A2DP listen-only mode expose a silent mic endpoint.
2. System audio sounds audibly distorted on music because our in-app 48 k → 16 k linear-interpolation resampler has no anti-alias low-pass filter; frequency content above 8 kHz folds into the audible band.
3. System-audio capture is all-or-nothing; there is no way to exclude specific apps (e.g., Apple Music, a password manager) from the recording.

**Architecture:**
- **Part 1 (system audio distortion)** — stop resampling in-app. Configure `SCStreamConfiguration` to emit 16 kHz natively. Apple's internal resampler has proper anti-aliasing. Delete `resample_linear`, its phase state, and its tests. Net code reduction.
- **Part 2 (mic)** — add a CoreAudio-backed device picker. At startup and on `kAudioHardwarePropertyDefaultInputDevice` change, enumerate inputs, skip Bluetooth transport-type devices (unless user overrode via config), restart the cpal stream on the chosen device. This gives two behaviors:
  - AirPods connected for music listening (A2DP) → daemon stays on built-in mic, no silent captures.
  - A call starts → macOS flips default input to AirPods-HFP (a real mic) → daemon follows and captures call speech.
- **Part 3 (per-app filtering)** — introduce `SharedStreamConfig { filter: AppFilter }` in the SCK crate, where `AppFilter` is a two-variant enum: `Exclude { names, bundle_ids }` (default, empty = open world) and `Include { names, bundle_ids }` (whitelist — only those apps are captured). `AudioSystemSource::from_config` reads `exclude_apps` / `exclude_bundle_ids` or `include_apps` / `include_bundle_ids` from `[capture.audio-system]` TOML and calls `alvum_capture_sck::configure()` synchronously at pipeline-setup time, *before* any source task is spawned — so whichever source's `ensure_started` call happens to win the lazy-init race sees the configured filter. Include and exclude are mutually exclusive; supplying both is a config error. `SharedStream::start` branches on the variant to choose `SCContentFilter::initWithDisplay_excludingApplications_exceptingWindows` / `initWithDisplay_includingApplications_exceptingWindows` / the existing wide-open `initWithDisplay_excludingWindows`. **Important coupling:** SCK uses a single content filter for both audio and video, so excluded/non-included apps are also excluded/non-included from screen capture. We document this in the config template so users are not surprised when e.g. the Apple Music window stops appearing in screenshots.

**Tech Stack:**
- Rust 2024 edition, macOS only
- Existing: `cpal` 0.17 for mic audio streaming, `objc2-screen-capture-kit` 0.3 for SCK, `tokio` for async tasks
- New direct deps on existing transitive libs: `objc2-core-audio` 0.3, `objc2-core-foundation` 0.3 (already in the tree via cpal)
- No new third-party dependencies.

---

## Part 1: SCK native 16 kHz

### Task 1: Configure SCK for 16 kHz output, delete in-app resampler

**Files:**
- Modify: `crates/alvum-capture-sck/src/lib.rs`

**Why this works:** `SCStreamConfiguration.setSampleRate(_:)` accepts 8000 / 16000 / 24000 / 48000. Apple's system-level downsampler applies a correctly-designed anti-alias filter before decimation, so the CMSampleBuffers we receive at 16 kHz have no aliasing. Our naive linear interpolator decimating 3:1 with no LPF is strictly worse and can be deleted.

- [ ] **Step 1: Delete the resampler unit tests first (they'll guide what we remove)**

In `crates/alvum-capture-sck/src/lib.rs`, locate the `#[cfg(test)] mod tests` block (lines ~673–707). Delete the two tests referring to `resample_linear`:

```rust
// DELETE: resample_48k_to_16k_drops_two_of_three
// DELETE: resample_phase_carries_across_buffers
```

Keep `stereo_to_mono_averages_channels`.

- [ ] **Step 2: Run tests to confirm only the one test remains compiling before edits**

Run: `cargo test -p alvum-capture-sck`
Expected: passes with 1 test (stereo_to_mono_averages_channels) — the two deleted tests should no longer appear.

- [ ] **Step 3: Add a failing test asserting decode_audio returns input-sample-rate samples unchanged**

Add to the tests block in `crates/alvum-capture-sck/src/lib.rs`:

```rust
#[test]
fn stereo_to_mono_passthrough_does_not_mutate_length() {
    // With SCK delivering 16 kHz stereo directly, decode_audio should
    // produce half the sample count (stereo → mono), no resampling.
    let stereo: Vec<f32> = (0..3200).map(|i| (i as f32) * 0.001).collect();
    let mono = stereo_to_mono(&stereo);
    assert_eq!(mono.len(), 1600, "stereo→mono halves sample count");
}
```

- [ ] **Step 4: Run the test to confirm it passes (stereo_to_mono is unchanged)**

Run: `cargo test -p alvum-capture-sck`
Expected: PASS (2 tests total now).

- [ ] **Step 5: Swap the SCK output sample rate to 16 kHz**

In `crates/alvum-capture-sck/src/lib.rs`, at the constants block (~lines 142–146), replace:

```rust
const SCK_AUDIO_INPUT_RATE: u32 = 48_000;
const SCK_AUDIO_TARGET_RATE: u32 = 16_000;
const SCK_AUDIO_CHANNEL_COUNT: isize = 2;
const SCK_AUDIO_RESAMPLE_RATIO: f64 =
    SCK_AUDIO_INPUT_RATE as f64 / SCK_AUDIO_TARGET_RATE as f64;
```

with:

```rust
const SCK_AUDIO_SAMPLE_RATE: u32 = 16_000;
const SCK_AUDIO_CHANNEL_COUNT: isize = 2;
```

In `SharedStream::start`, update the config block (~line 243):

```rust
config.setSampleRate(SCK_AUDIO_SAMPLE_RATE as isize);
config.setChannelCount(SCK_AUDIO_CHANNEL_COUNT);
```

- [ ] **Step 6: Remove audio_phase from SharedState and simplify decode_audio**

Remove the `audio_phase` field from `SharedState` (~line 154):

```rust
struct SharedState {
    audio_callback: Mutex<Option<SampleCallback>>,
    latest_png: Mutex<Option<Vec<u8>>>,
    current_display_id: Mutex<u32>,
}
```

Update the `SharedState { ... }` construction (~line 266):

```rust
let state = Arc::new(SharedState {
    audio_callback: Mutex::new(None),
    latest_png: Mutex::new(None),
    current_display_id: Mutex::new(initial_display_id),
});
```

Replace `handle_audio` (~line 309) with:

```rust
fn handle_audio(sample: &CMSampleBuffer, state: &SharedState) {
    let cb_arc = {
        let guard = state.audio_callback.lock().unwrap();
        match &*guard {
            Some(cb) => cb.clone(),
            None => return,
        }
    };

    let samples = match decode_audio(sample) {
        Ok(s) if !s.is_empty() => s,
        Ok(_) => return,
        Err(e) => {
            warn!(error = %e, "SCK audio decode failed");
            return;
        }
    };

    if let Ok(mut cb) = cb_arc.lock() {
        cb(&samples);
    }
}
```

Replace `decode_audio` (~line 338) with:

```rust
fn decode_audio(sample: &CMSampleBuffer) -> Result<Vec<f32>> {
    let interleaved = extract_f32_stereo(sample)
        .context("failed to extract f32 stereo from CMSampleBuffer")?;
    if interleaved.is_empty() {
        return Ok(Vec::new());
    }
    Ok(stereo_to_mono(&interleaved))
}
```

Delete `resample_linear` entirely (~lines 392–408).

- [ ] **Step 7: Run cargo build and tests**

Run: `cargo build -p alvum-capture-sck`
Expected: compiles clean (no references to SCK_AUDIO_INPUT_RATE, SCK_AUDIO_TARGET_RATE, SCK_AUDIO_RESAMPLE_RATIO, audio_phase, resample_linear).

Run: `cargo test -p alvum-capture-sck`
Expected: 2 tests pass.

Run: `cargo clippy -p alvum-capture-sck -- -D warnings`
Expected: clean.

- [ ] **Step 8: Manual smoke verification**

Rebuild daemon:
```bash
cd /Users/michael/git/alvum
cargo build --release -p alvum-cli
cp target/release/alvum ~/.alvum/runtime/bin/alvum
launchctl kickstart -k gui/$(id -u)/com.alvum.capture
```

Then play music on Apple Music for ~90 seconds, wait for a chunk to land, and verify mean/peak volume looks like music not aliasing garbage:

```bash
ls -t ~/.alvum/capture/$(date +%Y-%m-%d)/audio/system/*.wav | head -1 | xargs -I{} ffmpeg -hide_banner -nostats -i {} -af volumedetect -f null /dev/null 2>&1 | grep -E "(mean|max)_volume"
```

Expected: real levels (mean between −30 and −10 dB for music), no `-91.0 dB` (silence) for the music chunk.

Optional: open the newest system chunk in QuickTime and listen — should be clean music, not distorted.

- [ ] **Step 9: Commit**

```bash
git add crates/alvum-capture-sck/src/lib.rs
git commit -m "$(cat <<'EOF'
fix(capture-sck): configure 16 kHz native output, remove in-app resampler

SCStreamConfiguration now emits 16 kHz stereo directly; Apple's
resampler has proper anti-alias filtering. Deletes the linear-
interpolation decimator that caused audible aliasing on music.
EOF
)"
```

---

## Part 2: Mic device selection + call-aware follow

### Task 2: CoreAudio FFI helpers for device transport type + default-input

**Files:**
- Create: `crates/alvum-capture-audio/src/coreaudio_hal.rs`
- Modify: `crates/alvum-capture-audio/src/lib.rs`
- Modify: `crates/alvum-capture-audio/Cargo.toml`

**Why:** cpal exposes device names but not `AudioDeviceID`s or transport type. We query CoreAudio directly for (a) the list of input device IDs, (b) each device's name + transport type (Bluetooth vs. built-in vs. USB), and (c) the current default-input ID. Name-based matching links back to a `cpal::Device` for the actual stream.

- [ ] **Step 1: Add direct deps**

Edit `crates/alvum-capture-audio/Cargo.toml`, append to `[dependencies]`:

```toml
objc2-core-audio = "0.3"
objc2-core-foundation = "0.3"
```

- [ ] **Step 2: Run cargo check to confirm deps resolve**

Run: `cargo check -p alvum-capture-audio`
Expected: builds (deps already in tree via cpal; direct dep adds no new downloads).

- [ ] **Step 3: Create the HAL module with a failing test**

Create `crates/alvum-capture-audio/src/coreaudio_hal.rs` with this skeleton and failing test:

```rust
//! Thin CoreAudio HAL wrappers for device enumeration and default-input
//! tracking. Exists because cpal doesn't expose transport type or device
//! IDs — and we need both to skip silent A2DP Bluetooth mic endpoints
//! and follow the OS default-input when a call starts.

use anyhow::{anyhow, Result};
use std::ffi::c_void;

// CoreAudio FourCC constants. Values taken from <CoreAudio/AudioHardware.h>.
const K_AUDIO_OBJECT_SYSTEM_OBJECT: u32 = 1;
const K_AUDIO_HARDWARE_PROPERTY_DEVICES: u32 = fourcc(b"dev#");
const K_AUDIO_HARDWARE_PROPERTY_DEFAULT_INPUT_DEVICE: u32 = fourcc(b"dIn ");
const K_AUDIO_DEVICE_PROPERTY_TRANSPORT_TYPE: u32 = fourcc(b"tran");
const K_AUDIO_OBJECT_PROPERTY_NAME: u32 = fourcc(b"lnam");
const K_AUDIO_DEVICE_PROPERTY_STREAMS: u32 = fourcc(b"stm#");
const K_AUDIO_OBJECT_PROPERTY_SCOPE_GLOBAL: u32 = fourcc(b"glob");
const K_AUDIO_OBJECT_PROPERTY_SCOPE_INPUT: u32 = fourcc(b"inpt");
const K_AUDIO_OBJECT_PROPERTY_ELEMENT_MAIN: u32 = 0;

// Transport-type values (<CoreAudio/AudioHardwareBase.h>).
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
            K_AUDIO_DEVICE_TRANSPORT_TYPE_BLUETOOTH
                | K_AUDIO_DEVICE_TRANSPORT_TYPE_BLUETOOTH_LE
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
        out.push(DeviceInfo { id, name, transport_type: transport, has_input_stream: true });
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
        assert!(!devices.is_empty(), "expected at least one input device on this host");
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
```

Append to `crates/alvum-capture-audio/src/lib.rs`:

```rust
pub mod coreaudio_hal;
```

- [ ] **Step 4: Run tests**

Run: `cargo test -p alvum-capture-audio coreaudio_hal`
Expected: both tests PASS on the dev host (assumes a Mac with audio hardware).

- [ ] **Step 5: Commit**

```bash
git add crates/alvum-capture-audio/src/coreaudio_hal.rs \
        crates/alvum-capture-audio/src/lib.rs \
        crates/alvum-capture-audio/Cargo.toml
git commit -m "feat(capture-audio): coreaudio_hal for device enum + transport type"
```

---

### Task 3: Pure choose_mic_device function with unit tests

**Files:**
- Create: `crates/alvum-capture-audio/src/mic_selection.rs`
- Modify: `crates/alvum-capture-audio/src/lib.rs`

**Why:** Device-picking logic is the correctness-critical part; we isolate it as a pure function over `DeviceInfo` so we can unit-test every scenario (AirPods-only, built-in + AirPods, explicit override, etc.) without real hardware.

- [ ] **Step 1: Write failing tests for choose_mic_device**

Create `crates/alvum-capture-audio/src/mic_selection.rs`:

```rust
//! Pure mic-selection policy. Separated from the CoreAudio FFI so we can
//! test every combination of connected devices and config overrides
//! without a real audio host.

use crate::coreaudio_hal::DeviceInfo;

/// Choose which input device the mic capture should bind to.
///
/// Rules:
/// 1. If `override_name` is Some, return the device whose name matches exactly.
///    No fallback — user override is authoritative.
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::coreaudio_hal::{
        DeviceInfo, K_AUDIO_DEVICE_TRANSPORT_TYPE_BLUETOOTH,
    };

    const TRANSPORT_BUILT_IN: u32 = 0x62696c74; // 'bilt'
    const TRANSPORT_USB: u32 = 0x75736220; // 'usb '

    fn device(id: u32, name: &str, transport: u32) -> DeviceInfo {
        DeviceInfo { id, name: name.into(), transport_type: transport, has_input_stream: true }
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
        // OS default is USB (id=2) — pick USB.
        let chosen = choose_mic_device(&devs, 2, None).unwrap();
        assert_eq!(chosen.id, 2);
    }

    #[test]
    fn falls_back_to_default_when_only_bt() {
        let devs = vec![
            device(2, "AirPods Pro", K_AUDIO_DEVICE_TRANSPORT_TYPE_BLUETOOTH),
            device(3, "AirPods Max", K_AUDIO_DEVICE_TRANSPORT_TYPE_BLUETOOTH),
        ];
        // During a call, AirPods-HFP becomes default and is real — honor it.
        let chosen = choose_mic_device(&devs, 3, None).unwrap();
        assert_eq!(chosen.id, 3);
    }

    #[test]
    fn empty_returns_none() {
        let chosen = choose_mic_device(&[], 0, None);
        assert!(chosen.is_none());
    }
}
```

Append to `crates/alvum-capture-audio/src/lib.rs`:

```rust
pub mod mic_selection;
```

- [ ] **Step 2: Run tests to verify they all pass**

Run: `cargo test -p alvum-capture-audio mic_selection`
Expected: 5 tests PASS.

- [ ] **Step 3: Commit**

```bash
git add crates/alvum-capture-audio/src/mic_selection.rs crates/alvum-capture-audio/src/lib.rs
git commit -m "feat(capture-audio): pure mic-selection policy, skips BT unless overridden"
```

---

### Task 4: Wire choose_mic_device into AudioMicSource startup

**Files:**
- Modify: `crates/alvum-capture-audio/src/source.rs`
- Modify: `crates/alvum-capture-audio/src/devices.rs`

**Why:** Today `AudioMicSource::run` calls `devices::get_input_device(None)` which just asks cpal for the default input — which on 2026-04-20 was AirPods A2DP and delivered zeros. Replace that single call with: enumerate via CoreAudio HAL, run `choose_mic_device`, then use the chosen name to find the cpal device.

- [ ] **Step 1: Add a get_input_device_by_name helper that matches by description**

The existing `devices::get_input_device(name: Option<&str>)` takes `None` → system default. We need a mode that takes an exact name from the HAL and finds the matching cpal device. Inspect the existing code (`crates/alvum-capture-audio/src/devices.rs`): `get_input_device` already handles `Some(name)` with exact-match on `device.description().name()`. That's what we want — no new function needed. We just pass the chosen name.

- [ ] **Step 2: Write failing integration test for AudioMicSource (smoke-level)**

Append to `crates/alvum-capture-audio/src/source.rs` tests module (create `#[cfg(test)] mod tests` at end of file if absent):

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use alvum_core::config::CaptureSourceConfig;
    use tempfile::TempDir;

    #[tokio::test]
    async fn mic_source_starts_and_shuts_down_cleanly() {
        // Smoke: we can construct, start, and shut down a mic source
        // against whatever the test host's CoreAudio HAL reports.
        // Fails cleanly if no input devices exist (headless CI).
        let cfg = CaptureSourceConfig {
            enabled: true,
            settings: Default::default(),
        };
        let source = AudioMicSource::from_config(&cfg);
        let tmp = TempDir::new().unwrap();
        let (tx, rx) = tokio::sync::watch::channel(false);

        let handle = tokio::spawn(async move {
            source.run(tmp.path(), rx).await
        });

        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        tx.send(true).unwrap();
        let result = handle.await.unwrap();
        assert!(result.is_ok(), "mic source shutdown cleanly: {result:?}");
    }
}
```

Run: `cargo test -p alvum-capture-audio mic_source_starts_and_shuts_down_cleanly`
Expected: likely PASS already (tests the existing behavior). If it fails because default device is a silent BT device, that motivates the next step.

- [ ] **Step 3: Refactor AudioMicSource::run to use HAL-backed selection**

Replace the device-picking portion of `AudioMicSource::run` in `crates/alvum-capture-audio/src/source.rs`. Locate the block (~lines 44–53) starting with `let mic_dir = ...` through `let stream = capture::start_capture(...)?;`. Replace:

```rust
let device = devices::get_input_device(self.device_name.as_deref())
    .context("failed to get mic device")?;
```

with:

```rust
let hal_devices = crate::coreaudio_hal::list_input_devices()
    .context("failed to enumerate CoreAudio input devices")?;
let default_id = crate::coreaudio_hal::default_input_device_id()
    .context("failed to query default input device")?;
let chosen = crate::mic_selection::choose_mic_device(
    &hal_devices,
    default_id,
    self.device_name.as_deref(),
)
.with_context(|| match self.device_name.as_deref() {
    Some(n) => format!("no input device named {n:?}"),
    None => "no input devices available".to_string(),
})?;

info!(
    device = %chosen.name,
    is_bluetooth = chosen.is_bluetooth(),
    "audio-mic selected input device"
);

let device = devices::get_input_device(Some(&chosen.name))
    .with_context(|| format!("failed to open cpal device {:?}", chosen.name))?;
```

Note: `choose_mic_device` returns `Option<&DeviceInfo>` — `.with_context` on `Option` needs `.ok_or_else(|| anyhow::anyhow!(...))`. Adjust:

```rust
let chosen = crate::mic_selection::choose_mic_device(
    &hal_devices,
    default_id,
    self.device_name.as_deref(),
)
.ok_or_else(|| match self.device_name.as_deref() {
    Some(n) => anyhow::anyhow!("no input device named {:?}", n),
    None => anyhow::anyhow!("no input devices available"),
})?;
```

- [ ] **Step 4: Run tests**

Run: `cargo test -p alvum-capture-audio`
Expected: all tests pass including the mic_source smoke.

Run: `cargo build --release -p alvum-cli`
Expected: binary rebuilds.

- [ ] **Step 5: Manual verification: capture with AirPods connected**

1. Connect AirPods (in music-listening A2DP mode — just pair, don't start a call).
2. Restart capture daemon: `launchctl kickstart -k gui/$(id -u)/com.alvum.capture`
3. Confirm the log line says the chosen device is built-in:
   ```bash
   grep "audio-mic selected input device" ~/.alvum/runtime/logs/capture.out | tail -1
   ```
   Expected: `device="MacBook Pro Microphone" is_bluetooth=false`.
4. After ~2 min, probe a fresh chunk's RMS:
   ```bash
   ls -t ~/.alvum/capture/$(date +%Y-%m-%d)/audio/mic/*.wav | head -1 | \
     xargs -I{} ffmpeg -hide_banner -nostats -i {} -af volumedetect -f null /dev/null 2>&1 | \
     grep mean_volume
   ```
   Expected: not `-91.0 dB` — typical office ambient floor is around `-50` to `-65 dB`.

- [ ] **Step 6: Commit**

```bash
git add crates/alvum-capture-audio/src/source.rs
git commit -m "$(cat <<'EOF'
fix(capture-audio): skip Bluetooth mics at startup

Replaces cpal-default input selection with CoreAudio HAL enumeration
+ transport-type-aware picker. AirPods in A2DP (music listening) no
longer silently win the mic slot.
EOF
)"
```

---

### Task 5: Follow default-input changes (call in/out)

**Files:**
- Modify: `crates/alvum-capture-audio/src/source.rs`

**Why:** With Task 4, AirPods-A2DP is ignored at startup so music listening keeps our built-in mic recording. But when a call starts, macOS auto-swaps default input to AirPods-HFP (a real mic) — we should follow, so the call audio is captured on the actual microphone the user is speaking into. Likewise when the call ends, follow back to built-in.

Use a simple poll loop (every 3 s) inside the source task that re-evaluates `choose_mic_device` against the current default-input id. If the choice changes, tear down the current cpal stream and reopen on the new device. Polling at 3 s is fine because device events are low-frequency and the worst-case of 3 s lost speech at call start is acceptable for MVP.

- [ ] **Step 1: Write failing test for the polling/swap decision**

Add to `crates/alvum-capture-audio/src/mic_selection.rs`:

```rust
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
mod swap_tests {
    use super::*;
    use crate::coreaudio_hal::{DeviceInfo, K_AUDIO_DEVICE_TRANSPORT_TYPE_BLUETOOTH};

    const TRANSPORT_BUILT_IN: u32 = 0x62696c74;

    fn device(id: u32, name: &str, transport: u32) -> DeviceInfo {
        DeviceInfo { id, name: name.into(), transport_type: transport, has_input_stream: true }
    }

    #[test]
    fn no_swap_when_current_still_best() {
        let devs = vec![device(1, "Built-in", TRANSPORT_BUILT_IN)];
        let r = decide_swap(&devs, 1, None, Some("Built-in"));
        assert!(r.is_none());
    }

    #[test]
    fn swap_when_call_makes_airpods_real() {
        // Only BT available, OS default moved to AirPods-HFP (id=2).
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
    fn no_swap_when_nothing_bound_yet_and_no_devices() {
        let r = decide_swap(&[], 0, None, None);
        assert!(r.is_none());
    }

    #[test]
    fn swap_from_unbound_to_first_pick() {
        let devs = vec![device(1, "Built-in", TRANSPORT_BUILT_IN)];
        let r = decide_swap(&devs, 1, None, None);
        assert_eq!(r, Some("Built-in"));
    }
}
```

- [ ] **Step 2: Run tests**

Run: `cargo test -p alvum-capture-audio mic_selection::swap_tests`
Expected: 5 tests PASS.

- [ ] **Step 3: Restructure AudioMicSource::run to loop with periodic re-evaluation**

Replace the body of `AudioMicSource::run` in `crates/alvum-capture-audio/src/source.rs` (starting at `async fn run...`) with:

```rust
async fn run(&self, capture_dir: &Path, mut shutdown: watch::Receiver<bool>) -> Result<()> {
    let mic_dir = capture_dir.join("audio").join("mic");
    let samples_per_chunk = SAMPLE_RATE as usize * self.chunk_duration_secs as usize;

    let encoder = Arc::new(Mutex::new(AudioEncoder::new(mic_dir, SAMPLE_RATE)?));
    let callback = make_chunked_callback(encoder.clone(), samples_per_chunk, "mic".into());

    let mut current_bound: Option<String> = None;
    let mut current_stream: Option<capture::AudioStream> = None;

    loop {
        // Decide whether to (re)bind the mic stream.
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
            // Drop the old stream first so cpal releases the device handle.
            current_stream = None;
            let device = devices::get_input_device(Some(new_name))
                .with_context(|| format!("open cpal device {new_name:?}"))?;
            let stream = capture::start_capture(&device, "mic", callback.clone())?;
            info!(device = %new_name, "audio-mic bound input device");
            current_bound = Some(new_name.to_string());
            current_stream = Some(stream);
        }

        // Wait either for shutdown or a 3s repoll tick.
        tokio::select! {
            _ = shutdown.changed() => {
                if *shutdown.borrow() { break; }
            }
            _ = tokio::time::sleep(std::time::Duration::from_secs(3)) => {}
        }
    }

    drop(current_stream);
    if let Ok(mut enc) = encoder.lock() {
        let _ = enc.flush_segment();
    }
    info!("audio-mic source stopped");
    Ok(())
}
```

- [ ] **Step 4: Build and run tests**

Run: `cargo build -p alvum-capture-audio`
Expected: compiles.

Run: `cargo test -p alvum-capture-audio`
Expected: all tests pass, including `mic_source_starts_and_shuts_down_cleanly`.

Run: `cargo clippy -p alvum-capture-audio -- -D warnings`
Expected: clean.

- [ ] **Step 5: Manual verification: simulate call in/out**

1. Rebuild daemon and restart:
   ```bash
   cargo build --release -p alvum-cli
   cp target/release/alvum ~/.alvum/runtime/bin/alvum
   launchctl kickstart -k gui/$(id -u)/com.alvum.capture
   ```
2. With AirPods connected (A2DP), verify `audio-mic bound input device device="MacBook Pro Microphone"` in the log.
3. Start a FaceTime call to yourself (another Mac / phone), wait ~10 seconds. Log should now show:
   `audio-mic bound input device device="Michael's AirPods Pro 3"`.
4. End the call, wait ~10 seconds. Log should show it swap back to built-in.
5. Probe a chunk from during the call:
   ```bash
   ls -t ~/.alvum/capture/$(date +%Y-%m-%d)/audio/mic/*.wav | head -3 | \
     xargs -I{} ffmpeg -hide_banner -nostats -i {} -af volumedetect -f null /dev/null 2>&1 | \
     grep -E "(mean|max)_volume"
   ```
   Expected: non-silent levels during the call window.

- [ ] **Step 6: Commit**

```bash
git add crates/alvum-capture-audio/src/mic_selection.rs crates/alvum-capture-audio/src/source.rs
git commit -m "$(cat <<'EOF'
feat(capture-audio): follow default-input changes so calls capture on the right mic

Adds a 3 s poll loop in AudioMicSource that re-runs the selection
policy against the current default-input device. When a call starts
and macOS swaps default input to AirPods-HFP, the daemon tears down
the built-in cpal stream and binds the call mic. Symmetric on call
end. decide_swap() is pure and unit-tested.
EOF
)"
```

---

## Part 3: Per-app system-audio filtering

### Task 6: SharedStreamConfig with AppFilter (exclude / include), app-matcher in SCK

**Files:**
- Modify: `crates/alvum-capture-sck/src/lib.rs`

**Why:** We need both a blacklist (exclude) and whitelist (include) mode. SCK provides symmetric init methods — `initWithDisplay_excludingApplications_exceptingWindows` and `initWithDisplay_includingApplications_exceptingWindows` — so the shape of the work is: a single pure matcher that resolves name/bundle-id rules against `SCShareableContent.applications()`, plus a small enum telling `build_filter` which SCK init to call.

- [ ] **Step 1: Write failing tests for the pure matcher**

Append to the `#[cfg(test)] mod tests` block in `crates/alvum-capture-sck/src/lib.rs`:

```rust
#[test]
fn match_apps_empty_rules_returns_empty() {
    let apps = vec![("Music".into(), "com.apple.Music".into())];
    let idx = match_apps_by_rules(&[], &[], &apps);
    assert!(idx.is_empty());
}

#[test]
fn match_apps_by_name_case_insensitive() {
    let apps = vec![
        ("Music".into(), "com.apple.Music".into()),
        ("Safari".into(), "com.apple.Safari".into()),
    ];
    let idx = match_apps_by_rules(&["music".to_string()], &[], &apps);
    assert_eq!(idx, vec![0]);
}

#[test]
fn match_apps_by_bundle_id() {
    let apps = vec![
        ("Music".into(), "com.apple.Music".into()),
        ("Spotify".into(), "com.spotify.client".into()),
    ];
    let idx = match_apps_by_rules(&[], &["com.spotify.client".to_string()], &apps);
    assert_eq!(idx, vec![1]);
}

#[test]
fn match_apps_by_name_and_bundle_unions() {
    let apps = vec![
        ("Music".into(), "com.apple.Music".into()),
        ("Spotify".into(), "com.spotify.client".into()),
        ("Safari".into(), "com.apple.Safari".into()),
    ];
    let idx = match_apps_by_rules(
        &["music".to_string()],
        &["com.spotify.client".to_string()],
        &apps,
    );
    assert_eq!(idx, vec![0, 1]);
}

#[test]
fn match_apps_no_match_returns_empty() {
    let apps = vec![("Safari".into(), "com.apple.Safari".into())];
    let idx = match_apps_by_rules(
        &["music".to_string()],
        &["com.apple.Music".to_string()],
        &apps,
    );
    assert!(idx.is_empty());
}

#[test]
fn match_apps_deduplicates_when_name_and_bundle_both_hit_same_index() {
    let apps = vec![("Music".into(), "com.apple.Music".into())];
    let idx = match_apps_by_rules(
        &["music".to_string()],
        &["com.apple.Music".to_string()],
        &apps,
    );
    assert_eq!(idx, vec![0], "one app should yield one index even if both rules match");
}
```

- [ ] **Step 2: Run tests — they should fail to compile**

Run: `cargo test -p alvum-capture-sck match_apps`
Expected: compile error — `match_apps_by_rules` not defined.

- [ ] **Step 3: Implement match_apps_by_rules**

Add to `crates/alvum-capture-sck/src/lib.rs` (anywhere in the crate body, before the tests block):

```rust
/// Pure rule-matching helper used by both include and exclude filter modes.
/// Given name/bundle rule lists and a snapshot of (app_name, bundle_id)
/// tuples, return the indices of matching apps. Name match is
/// case-insensitive; bundle match is exact. Names and bundle IDs are
/// OR'd — an app matching either list is a hit. Each matching app
/// appears exactly once in the result (no duplicate indices).
fn match_apps_by_rules(
    names: &[String],
    bundle_ids: &[String],
    apps: &[(String, String)],
) -> Vec<usize> {
    let names_lower: Vec<String> = names.iter().map(|n| n.to_lowercase()).collect();
    let mut hits: Vec<usize> = Vec::new();
    for (i, (name, bundle)) in apps.iter().enumerate() {
        let name_hit = names_lower.iter().any(|n| n == &name.to_lowercase());
        let bundle_hit = bundle_ids.iter().any(|b| b == bundle);
        if name_hit || bundle_hit {
            hits.push(i);
        }
    }
    hits
}
```

- [ ] **Step 4: Run tests — expect PASS**

Run: `cargo test -p alvum-capture-sck match_apps`
Expected: 6 tests PASS.

- [ ] **Step 5: Add AppFilter + SharedStreamConfig + configure() API**

In `crates/alvum-capture-sck/src/lib.rs`, near the top of the internals section (around the `SHARED` OnceLock declaration), add:

```rust
/// Which apps the SCK content filter should let through.
///
/// - `Exclude { ... }` (default) — capture everything except matching apps.
///   Empty lists = open world (capture all).
/// - `Include { ... }` — whitelist mode: capture ONLY matching apps. Empty
///   lists with Include is a degenerate "capture nothing" configuration;
///   `build_filter` logs a warning and falls back to open-world so the
///   daemon doesn't silently record nothing.
#[derive(Debug, Clone)]
pub enum AppFilter {
    Exclude { names: Vec<String>, bundle_ids: Vec<String> },
    Include { names: Vec<String>, bundle_ids: Vec<String> },
}

impl Default for AppFilter {
    fn default() -> Self {
        AppFilter::Exclude { names: Vec::new(), bundle_ids: Vec::new() }
    }
}

/// Pre-start configuration for the shared SCK stream. Set via
/// [`configure`] before [`ensure_started`] is first called.
#[derive(Debug, Clone, Default)]
pub struct SharedStreamConfig {
    pub filter: AppFilter,
}

static FILTER_CONFIG: OnceLock<Mutex<SharedStreamConfig>> = OnceLock::new();

/// Provide the filter config that [`ensure_started`] will use on first
/// start. Safe to call multiple times before start; last-writer-wins.
pub fn configure(cfg: SharedStreamConfig) {
    let slot = FILTER_CONFIG.get_or_init(|| Mutex::new(SharedStreamConfig::default()));
    *slot.lock().unwrap() = cfg;
}

fn current_config() -> SharedStreamConfig {
    FILTER_CONFIG
        .get()
        .map(|m| m.lock().unwrap().clone())
        .unwrap_or_default()
}
```

- [ ] **Step 6: Teach SharedStream::start to honor AppFilter**

In `SharedStream::start` (the filter construction block, ~lines 230–238 of the file before edits), replace the hard-coded `SCContentFilter::initWithDisplay_excludingWindows(...)` construction with a call to a new `build_filter` helper:

```rust
let cfg = current_config();
let filter = build_filter(&content, &initial_display, &cfg)
    .context("failed to build SCContentFilter")?;
```

Add the `build_filter` helper elsewhere in the file (e.g., in the "internals" section just below `current_config`):

```rust
fn build_filter(
    content: &SCShareableContent,
    display: &SCDisplay,
    cfg: &SharedStreamConfig,
) -> Result<Retained<SCContentFilter>> {
    let empty_windows: Retained<NSArray<SCWindow>> = NSArray::new();

    // Early-exit open-world path: default Exclude with no rules → the
    // existing wide-open filter, no app enumeration needed.
    if let AppFilter::Exclude { names, bundle_ids } = &cfg.filter {
        if names.is_empty() && bundle_ids.is_empty() {
            return Ok(unsafe {
                SCContentFilter::initWithDisplay_excludingWindows(
                    SCContentFilter::alloc(),
                    display,
                    &empty_windows,
                )
            });
        }
    }

    let apps = unsafe { content.applications() };
    let mut tuples: Vec<(String, String)> = Vec::with_capacity(apps.count());
    let mut app_vec: Vec<Retained<SCRunningApplication>> = Vec::with_capacity(apps.count());
    for i in 0..apps.count() {
        let app = apps.objectAtIndex(i);
        let name = unsafe { app.applicationName() }.to_string();
        let bundle = unsafe { app.bundleIdentifier() }.to_string();
        tuples.push((name, bundle));
        app_vec.push(app);
    }

    let (names, bundle_ids, is_include) = match &cfg.filter {
        AppFilter::Exclude { names, bundle_ids } => (names, bundle_ids, false),
        AppFilter::Include { names, bundle_ids } => {
            if names.is_empty() && bundle_ids.is_empty() {
                warn!("AppFilter::Include with empty rules = capture-nothing; falling back to open world");
                return Ok(unsafe {
                    SCContentFilter::initWithDisplay_excludingWindows(
                        SCContentFilter::alloc(),
                        display,
                        &empty_windows,
                    )
                });
            }
            (names, bundle_ids, true)
        }
    };

    let indices = match_apps_by_rules(names, bundle_ids, &tuples);
    if indices.is_empty() {
        warn!(
            names = ?names,
            bundles = ?bundle_ids,
            mode = if is_include { "include" } else { "exclude" },
            "no running apps matched SCK filter rules; falling back to open world"
        );
        return Ok(unsafe {
            SCContentFilter::initWithDisplay_excludingWindows(
                SCContentFilter::alloc(),
                display,
                &empty_windows,
            )
        });
    }

    let matched_refs: Vec<&SCRunningApplication> =
        indices.iter().map(|&i| app_vec[i].as_ref()).collect();
    let matched_array: Retained<NSArray<SCRunningApplication>> =
        NSArray::from_slice(&matched_refs);

    let matched_names: Vec<&String> = indices.iter().map(|&i| &tuples[i].0).collect();
    if is_include {
        info!(included = ?matched_names, "SCK filter including only");
        Ok(unsafe {
            SCContentFilter::initWithDisplay_includingApplications_exceptingWindows(
                SCContentFilter::alloc(),
                display,
                &matched_array,
                &empty_windows,
            )
        })
    } else {
        info!(excluded = ?matched_names, "SCK filter excluding apps");
        Ok(unsafe {
            SCContentFilter::initWithDisplay_excludingApplications_exceptingWindows(
                SCContentFilter::alloc(),
                display,
                &matched_array,
                &empty_windows,
            )
        })
    }
}
```

Add `SCRunningApplication` to the top-of-file use statement:

```rust
use objc2_screen_capture_kit::{
    SCContentFilter, SCDisplay, SCRunningApplication, SCShareableContent, SCStream,
    SCStreamConfiguration, SCStreamOutput, SCStreamOutputType, SCWindow,
};
```

- [ ] **Step 7: Preserve filter when sync_active_display swaps filter**

`sync_active_display` currently rebuilds the filter from scratch using `initWithDisplay_excludingWindows`. Replace that construction with a `build_filter` call so display swaps preserve the active AppFilter:

```rust
let cfg = current_config();
let new_filter = build_filter(&content, &target_display, &cfg)
    .context("rebuild SCContentFilter on display swap")?;
```

Delete the now-unused inline `empty` / `SCContentFilter::initWithDisplay_excludingWindows(...)` from `sync_active_display`.

- [ ] **Step 8: Build, tests, clippy**

Run: `cargo build -p alvum-capture-sck`
Expected: compiles.

Run: `cargo test -p alvum-capture-sck`
Expected: all tests pass (stereo_to_mono_*, match_apps_*).

Run: `cargo clippy -p alvum-capture-sck -- -D warnings`
Expected: clean.

- [ ] **Step 9: Commit**

```bash
git add crates/alvum-capture-sck/src/lib.rs
git commit -m "$(cat <<'EOF'
feat(capture-sck): AppFilter (exclude/include) + per-app content filter

Adds configure() + SharedStreamConfig + AppFilter enum with two
variants:
  - Exclude { names, bundle_ids } (default) — blacklist
  - Include { names, bundle_ids }          — whitelist

build_filter() picks the matching SCContentFilter initializer
(excludingApplications vs includingApplications). Empty Exclude
and empty Include both fall back to the existing open-world
filter; the Include-empty case logs a warning first so the
daemon doesn't silently record nothing.

sync_active_display rebuilds via build_filter so the active
AppFilter is preserved across display swaps.

Coupling note: SCK uses one content filter for both audio and
video, so the AppFilter affects screen capture too.
EOF
)"
```

---

### Task 7: Wire exclude_apps / include_apps through AudioSystemSource

**Files:**
- Modify: `crates/alvum-capture-audio/src/source.rs`
- Modify: `crates/alvum-capture-sck/src/lib.rs`

**Why:** User-facing TOML under `[capture.audio-system]` supports two mutually exclusive shapes:

```toml
# Blacklist (default):
exclude_apps        = ["Music"]
exclude_bundle_ids  = ["com.apple.Music"]

# OR whitelist (only these are captured):
include_apps        = ["Zoom", "Safari"]
include_bundle_ids  = ["us.zoom.xos"]
```

Supplying both include and exclude keys is a config error — we fail loudly rather than silently prefer one. `AudioSystemSource::from_config` runs synchronously on the main task at pipeline-setup time, *before* any source is spawned via `tokio::spawn`, which is why it's the right place to validate + call `alvum_capture_sck::configure`.

- [ ] **Step 1: Add a test-visible snapshot helper in SCK**

In `crates/alvum-capture-sck/src/lib.rs`, below `current_config`, add:

```rust
#[doc(hidden)]
pub fn snapshot_config_for_test() -> SharedStreamConfig {
    current_config()
}
```

- [ ] **Step 2: Write failing tests for from_config (exclude, include, both-is-error)**

Add to `crates/alvum-capture-audio/src/source.rs` tests module (create one if absent):

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use alvum_core::config::CaptureSourceConfig;
    use alvum_capture_sck::AppFilter;
    use std::collections::HashMap;

    fn toml_str_array(items: &[&str]) -> toml::Value {
        toml::Value::Array(items.iter().map(|s| toml::Value::String((*s).into())).collect())
    }

    #[test]
    fn audio_system_from_config_defaults_to_open_world_exclude() {
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
        let mut settings: HashMap<String, toml::Value> = HashMap::new();
        settings.insert("exclude_apps".into(), toml_str_array(&["Music"]));
        settings.insert("include_apps".into(), toml_str_array(&["Zoom"]));
        let cfg = CaptureSourceConfig { enabled: true, settings };
        let err = AudioSystemSource::try_from_config(&cfg).unwrap_err().to_string();
        assert!(
            err.contains("mutually exclusive") || err.contains("both include"),
            "error should mention the mutual-exclusivity violation, got: {err}"
        );
    }
}
```

- [ ] **Step 3: Run the tests — expect failure**

Run: `cargo test -p alvum-capture-audio audio_system_from_config`
Expected: compile error — `try_from_config` and the new tests don't resolve yet.

- [ ] **Step 4: Implement**

Update `AudioSystemSource` in `crates/alvum-capture-audio/src/source.rs`. Keep the existing infallible `from_config` for signature compatibility with the pipeline (it calls `try_from_config().expect(...)` on failure):

```rust
pub struct AudioSystemSource {
    chunk_duration_secs: u32,
}

impl AudioSystemSource {
    pub fn from_config(config: &CaptureSourceConfig) -> Self {
        Self::try_from_config(config)
            .expect("audio-system config invalid; fix [capture.audio-system] in ~/.alvum/runtime/config.toml")
    }

    pub fn try_from_config(config: &CaptureSourceConfig) -> anyhow::Result<Self> {
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
```

- [ ] **Step 5: Run tests — expect PASS**

Run: `cargo test -p alvum-capture-audio audio_system_from_config`
Expected: 4 tests PASS.

Run: `cargo test -p alvum-capture-audio`
Expected: all tests pass.

- [ ] **Step 6: Commit**

```bash
git add crates/alvum-capture-audio/src/source.rs crates/alvum-capture-sck/src/lib.rs
git commit -m "$(cat <<'EOF'
feat(capture-audio): exclude_apps + include_apps for system-audio filter

[capture.audio-system] now accepts two mutually exclusive shapes:

  # Blacklist (default):
  exclude_apps        = ["Music"]
  exclude_bundle_ids  = ["com.apple.Music"]

  # Whitelist (only these):
  include_apps        = ["Zoom", "Safari"]
  include_bundle_ids  = ["us.zoom.xos"]

AudioSystemSource::try_from_config validates mutual exclusivity
and hands an AppFilter to alvum_capture_sck::configure at pipeline
setup time. SCK's single-filter design means the chosen apps also
drive what ends up in screen capture — documented in the config
template.
EOF
)"
```

---

### Task 8: Document filter modes in install.sh + manual validation

**Files:**
- Modify: `scripts/install.sh`

- [ ] **Step 1: Add both filter shapes to the config template**

In `scripts/install.sh`, locate the `[capture.audio-system]` section of the generated config template (near where `whisper_model` was added by the 2026-04-20 briefing-fixes work). Append commented examples after the existing keys:

```toml
# Per-app filter for system audio. Two modes, mutually exclusive:
#
#   1. Blacklist (default) — capture everything EXCEPT listed apps.
#        exclude_apps       = ["Music", "Spotify"]
#        exclude_bundle_ids = ["com.apple.Music"]
#
#   2. Whitelist — capture ONLY listed apps.
#        include_apps       = ["Zoom", "Safari"]
#        include_bundle_ids = ["us.zoom.xos"]
#
# Rules: applicationName match is case-insensitive, bundleIdentifier
# match is exact. Setting both include_* and exclude_* is a config error.
#
# Note: SCK uses a single content filter for BOTH audio and screen
# capture. Whichever apps the filter keeps out of the audio mix are
# also kept out of screenshots — keep that in mind when choosing rules.
```

- [ ] **Step 2: Rebuild daemon and reinstall**

```bash
cargo build --release -p alvum-cli
cp target/release/alvum ~/.alvum/runtime/bin/alvum
```

- [ ] **Step 3: Manual validation — blacklist mode**

Edit `~/.alvum/runtime/config.toml` `[capture.audio-system]` section:

```toml
exclude_apps = ["Music"]
```

Restart:

```bash
launchctl kickstart -k gui/$(id -u)/com.alvum.capture
```

1. Confirm the log line shows the filter is active:
   ```bash
   grep "SCK filter excluding apps" ~/.alvum/runtime/logs/capture.out | tail -1
   ```
   Expected: `excluded=["Music"] "SCK filter excluding apps"`.

2. Play a song from Apple Music for ~90 seconds. Probe the newest system-audio chunk:
   ```bash
   ls -t ~/.alvum/capture/$(date +%Y-%m-%d)/audio/system/*.wav | head -1 | \
     xargs -I{} ffmpeg -hide_banner -nostats -i {} -af volumedetect -f null /dev/null 2>&1 | \
     grep mean_volume
   ```
   Expected: `-91.0 dB` (silence — Music is excluded from the audio mix).

3. Play audio from Safari (YouTube / any web player) for ~90 seconds. Newest chunk should now have signal.
   Expected: non-silent mean/max volumes.

4. Focus the Apple Music app to fire a screen trigger, wait for the next PNG to land:
   ```bash
   ls -t ~/.alvum/capture/$(date +%Y-%m-%d)/screen/images/*.png | head -1
   ```
   Open it — Apple Music's window should NOT appear (the single-filter coupling at work).

- [ ] **Step 4: Manual validation — whitelist mode**

Replace the `exclude_*` keys in `~/.alvum/runtime/config.toml` with:

```toml
include_apps = ["Safari"]
```

Restart:

```bash
launchctl kickstart -k gui/$(id -u)/com.alvum.capture
```

1. Confirm the log shows include mode:
   ```bash
   grep "SCK filter including only" ~/.alvum/runtime/logs/capture.out | tail -1
   ```
   Expected: `included=["Safari"] "SCK filter including only"`.

2. Play audio from Apple Music. Newest chunk should be silent (Music is not in whitelist).
3. Play audio from Safari. Newest chunk should have signal.
4. A screenshot trigger on a non-Safari app should contain only Safari (or desktop if Safari isn't visible).

- [ ] **Step 5: Manual validation — mutual-exclusivity guardrail**

Temporarily add both keys:

```toml
exclude_apps = ["Music"]
include_apps = ["Safari"]
```

Restart and check:

```bash
tail -20 ~/.alvum/runtime/logs/capture.err
```

Expected: a clear error mentioning `mutually exclusive`, and the daemon refuses to start. Remove one of the two keys and confirm the daemon comes back up cleanly.

- [ ] **Step 6: Commit install.sh change**

```bash
git add scripts/install.sh
git commit -m "docs(capture): document exclude_apps / include_apps in config template"
```

---

## Self-Review Notes

- **Spec coverage:** mic-silence (Tasks 2–5), system-audio-distortion (Task 1), per-app filtering with both blacklist and whitelist modes (Tasks 6–8) all have implementation + manual verification steps. Mutual exclusivity of include/exclude is enforced at config-parse time and validated manually in Task 8 Step 5.
- **No placeholders:** every code block is literal; no "TODO" or "implement later".
- **Type consistency:** `DeviceInfo`, `choose_mic_device`, `decide_swap`, `SharedStreamConfig`, `AppFilter`, `match_apps_by_rules`, and `build_filter` signatures agree across tasks where they are referenced. `AudioSystemSource::from_config` stays infallible (existing pipeline contract) by delegating to a new fallible `try_from_config` that panics with a user-actionable message on config error.
- **Risks:**
  - Raw CoreAudio FFI path in Task 2. If the HAL tests pass on the dev Mac, downstream layers run on real data and the rest is Rust-level logic.
  - `configure()` is a process-global slot. `AudioSystemSource::try_from_config` must be called before any source's `.run()` reaches `ensure_started`. Current pipeline code builds sources synchronously in the main task then `tokio::spawn`s them, so the ordering holds; Task 7 documents this invariant in-code.
  - SCK single-filter coupling: the AppFilter drives screen capture too (included apps → only their windows appear; excluded apps → their windows don't). Documented in install.sh template (Task 8); no way to decouple without two SCStreams, which we explicitly rejected earlier for audio-starvation reasons.
  - `AppFilter::Include { empty, empty }` is a degenerate "capture nothing" configuration. `build_filter` logs a warning and falls back to the open-world filter rather than silently recording nothing; Task 6 Step 6 enforces this.
