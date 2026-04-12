//! Audio capture daemon: records microphone and system audio with VAD segmentation.
//!
//! Captures audio from configurable input/output devices, runs Silero VAD to detect
//! speech, encodes speech segments as Opus, and writes them to the capture directory.

pub mod devices;
pub mod capture;
pub mod vad;
pub mod encoder;
pub mod recorder;
