# ScreenCaptureKit Migration (Authoritative)

Capture of **system audio** and **screen** moves from legacy/fragile APIs to Apple's **ScreenCaptureKit (SCK)**. Mic capture stays on `cpal` — it's already device-resilient because macOS exposes the mic as a stable input device.

## Decision

1. **System audio** capture is owned by SCK. We stop binding to "an output device as if it were an input" via `cpal` — that approach is fundamentally broken for AirPods/AirPlay/HDMI, where the output isn't input-capturable at all.
2. **Screen** capture is owned by SCK. We retire `CGWindowListCreateImage` (legacy Quartz) which silently returns zero-pixel images on TCC denial and has been the single biggest source of permission-debugging friction.
3. **Mic** capture is unchanged: `cpal` + the default input device. No regression risk for a working path.
4. **Minimum macOS**: 13.0+ (Ventura). SCK audio requires 13.0; we already target macOS 26 in practice, so this is a non-constraint.

## Why, from first principles

"System audio" is not a device — it's a **process-graph concept** on macOS. The only way to capture it resiliently is to tap it at the system layer, above the device boundary. SCK does exactly this:

- The capture is **decoupled from the output route**. When macOS switches from built-in speakers → AirPods → AirPlay → HDMI, SCK keeps delivering samples. No rebinding, no `device_no_longer_available` errors.
- It works with AirPlay targets. AirPlay never exposes itself as a capturable input; SCK taps before the AirPlay sink, so it just works.
- Permission is **one TCC entry** (Screen Recording) that also covers screen capture — consolidating our permission surface instead of spreading it across microphone-vs-screen rules.
- The legacy `CGWindowListCreateImage` path we're on today for screen gives **no TCC prompt** on denial and returns blank pixels silently. SCK surfaces real errors and forces a real prompt on first use.

Options A (rebind on default-output-changed) and B (require BlackHole install) were considered and rejected:

- **A** doesn't work for the actual scenarios the user cares about (AirPods, AirPlay, HDMI). The new default isn't input-capturable. Rebinding captures silence.
- **B** shifts the problem onto user setup friction, and the BlackHole path still breaks for AirPlay groups. Third-party virtual device installs are not acceptable in a "make this dead simple" product.

C (SCK) is the only option that honors the stated requirement — "resilient to output device, including AirPods, speakers, AirPlay".

## Scope

### In scope

- New module path: `crates/alvum-capture-screen/src/sck.rs` owns SCK-driven screen frame capture.
- New module path: `crates/alvum-capture-audio/src/sck.rs` owns SCK-driven system audio capture. This file is adjacent to (not a replacement for) the existing `capture.rs`, which continues to own cpal-based mic capture.
- Replace `AudioSystemSource::run()` internals to use SCK instead of cpal output-device binding. Public `CaptureSource` trait impl is unchanged — `name()` is still `"audio-system"`, contract with the daemon is untouched.
- Replace `ScreenSource::run()` internals to use SCK. The on-disk output contract (PNG files under `capture/<date>/screen/`, metadata format, `idle_interval_secs` semantics) is **preserved byte-compatible** so the downstream `alvum-connector-screen` needs no changes.
- Delete `screenshot.rs::check_screen_recording_permission` and `capture_frontmost_window` once SCK screen replaces them.
- Delete the `AudioSystemSource` cpal code path (including `devices::get_output_device`) once SCK audio replaces it.
- Add dependency `screencapturekit = "1.5"` (pinned to ≥ 1.5.0, < 2.0). The crate is actively maintained; 1.5.4 landed March 2026.
- Add a one-time macOS version check at source startup: refuse to start the SCK audio source on macOS < 13 with a clear error. (We never hit this in practice; it's a defense-in-depth readability thing.)

### Out of scope

- Mic capture stays on `cpal`. No change.
- Multi-display screen capture. We capture the main display only, matching current behavior.
- Per-app audio tapping (SCK can isolate a process's audio — a future capability, not this migration).
- Merging audio and screen into a single SCStream. We run **two independent SCStreams**, one per `CaptureSource`, to preserve the existing trait-based boundaries. See "Architectural choice" below.
- Changing the extract/connector side. Audio connector still reads `.wav` files; screen connector still reads PNG + metadata. The pipeline doesn't care that SCK produced them.

## Architectural choice: two SCStreams, not one

The current `CaptureSource` trait gives each source an isolated `async fn run()` with its own shutdown signal. We preserve that: `AudioSystemSource` owns its SCStream (audio-only), `ScreenSource` owns its SCStream (video-only). Two processes' worth of state, cleanly separable.

We considered unifying them into a single SCStream with both audio and video output handlers. Rejected because:

- It breaks the `CaptureSource` abstraction — "one source = one lifecycle" becomes "one shared stream with coordinated lifecycle across two sources". More plumbing for no efficiency gain at our scale.
- SCK does not charge extra for two streams vs one with two handlers. Tested in the crate's example code — both patterns work fine.
- Independent streams mean mic stays unaffected when screen is toggled off, audio keeps flowing when screen stream errors, etc. Smaller blast radius.

Risk: macOS may rate-limit simultaneous SCStreams in the same process. The crate's authors advertise this as a supported pattern and we've seen no explicit per-process cap documented. We accept this risk and validate empirically in the first implementation task (smoke test: both streams active for 5 minutes, confirm both produce output).

## Interface contract (preserved byte-compatible)

### Audio

Input: SCK `CMSampleBuffer` at 48 kHz stereo (SCK's native rate — we ask for this).
Output: 16 kHz mono `.wav` segments in `capture/<date>/audio/system/HH-MM-SS.wav`, identical to today's cpal path. The existing `AudioEncoder` is reused as-is.

Between them, a new decode step converts `CMSampleBuffer` → `&[f32]` by reading the audio buffer list, de-interleaving channels, averaging to mono, and decimating 48 kHz → 16 kHz. This is the one genuinely new piece of code; everything else is composition of existing parts.

### Screen

Input: SCK `CMSampleBuffer` holding a `CVPixelBuffer` (BGRA, display-native resolution).
Output: PNG files at `capture/<date>/screen/<timestamp>-<app>-<window>.png` with sidecar metadata, identical to today.

The current `ScreenSource` has a trigger loop (focus changes + 30s idle) driving `capture_frontmost_window()`. SCK streams continuously — we throttle to the existing trigger cadence at the output-handler layer: discard frames between triggers, encode the first frame after each trigger, write PNG + metadata.

## Required permissions & entitlements

- `NSScreenCaptureUsageDescription` in the binary's Info.plist (we don't bundle an app yet — added when we ship the Electron shell; until then, the user grants via the "add binary to Screen Recording" flow we already use).
- TCC Screen Recording entry — one entry covers both SCK screen and SCK audio. This is a **consolidation win** over the current world (Screen Recording for screen, nothing for system audio since cpal doesn't trigger TCC, and the system-audio path has been fragile precisely because TCC isn't in the loop).

## Non-goals / explicit anti-features

- We do not expose `channelCount` or `sampleRate` as user config. The audio output contract is 16 kHz mono — fixed. Surfacing knobs that don't change the downstream behavior would be YAGNI.
- We do not attempt to preserve the very-early byte of audio lost when a stream restarts. First 100ms can be silent; the briefing is not sensitive to this.
- We do not build a shim layer that can flip between cpal and SCK for system audio. YAGNI. SCK is the new truth; cpal system-audio is deleted.

## Success criteria

The migration is complete when:

1. `audio-system` keeps writing 60s `.wav` segments continuously across:
   - Plugging/unplugging wired headphones
   - AirPods connect/disconnect
   - AirPlay to another Mac / Apple TV
   - HDMI-out connect/disconnect
   No daemon restart, no manual re-enable.
2. `screen` captures continuously and the capture.out log shows no `CGWindowListCreateImage returned null` lines.
3. First-run macOS TCC prompt appears automatically on SCK start, instead of the current silent-deny failure.
4. Removing and re-granting Screen Recording permission in System Settings → Privacy & Security resumes capture within 1 daemon restart — no manual `tccutil` dance needed.
5. `cargo test -p alvum-capture-audio` and `cargo test -p alvum-capture-screen` both pass. A new test in each crate asserts the SCK module compiles on macOS 13+ and is wired to the source.

## What this does NOT fix

- The SwiftBar menu-bar crash pattern in `-[NSOperationQueue addOperations:]`. That's a SwiftBar bug independent of our capture stack.
- The startup latency of `launchctl kickstart` on toggle (~1-3s). Orthogonal.
- Electron app shell, which is the real long-term home for status/controls.

The migration is a surgical fix to the capture layer only. The stop-gap menu bar stays the stop-gap menu bar.
