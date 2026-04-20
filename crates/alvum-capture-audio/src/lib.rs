//! Audio capture daemon: records microphone and system audio in fixed-length chunks.
//!
//! Captures audio from configurable input/output devices, encodes as Opus, and
//! writes fixed-length chunk files to the capture directory. No VAD — every
//! sample is recorded. VAD and speech detection live in the processor layer.

pub mod devices;
pub mod capture;
pub mod encoder;
pub mod recorder;
pub mod source;

#[cfg(target_os = "macos")]
pub mod sck_decode;
#[cfg(target_os = "macos")]
pub mod sck;
