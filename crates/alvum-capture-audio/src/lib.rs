//! Audio capture daemon: records microphone and system audio in fixed-length chunks.
//!
//! Captures audio from configurable input/output devices, encodes as Opus, and
//! writes fixed-length chunk files to the capture directory. No VAD — every
//! sample is recorded. VAD and speech detection live in the processor layer.

pub mod capture;
pub mod coreaudio_hal;
pub mod devices;
pub mod encoder;
pub mod mic_selection;
pub mod recorder;
pub mod source;
