//! Shared ScreenCaptureKit stream for system audio + screen capture.
//!
//! One SCStream per process. Two concurrent SCStreams interfere on macOS
//! (delegate callbacks get starved) — we hit this the moment both audio and
//! screen were enabled at once. The fix is a single stream with both audio
//! and video outputs, fan-out to two sets of subscribers:
//!
//!   - `set_audio_callback` — system audio, decoded to 16 kHz mono f32.
//!   - `pop_latest_frame`   — most recent screen frame with metadata.
//!
//! The stream starts on the first call to [`ensure_started`] and stays up
//! for the process lifetime. Dropping subscribers is fine — their samples
//! simply stop flowing; the underlying stream keeps running (the cost of
//! silence + one PNG encode every ~500 ms is negligible).

#![cfg(target_os = "macos")]

mod audio;
mod display_watcher;
mod filter;
mod helpers;
mod screen;
mod stream;

pub use filter::{AppFilter, SharedStreamConfig, configure, snapshot_config_for_test};
pub use stream::{
    Frame, SampleCallback, ensure_started, pop_latest_frame, restart, set_audio_callback,
    sync_active_display,
};
