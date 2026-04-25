//! Audio transcription processor: reads audio files and produces text transcripts
//! using whisper.cpp via whisper-rs.
//!
//! This is a processor in the three-layer architecture:
//! - Input: DataRef pointing to an audio file
//! - Output: Artifact with "text" layer (transcript) and "structured" layer (segments)

pub mod decoder;
pub mod transcriber;
