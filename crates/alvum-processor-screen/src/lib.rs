//! Screen processor: sends screenshots to a vision model and produces Observations.
//!
//! Reads DataRefs (PNG screenshots from alvum-capture-screen), calls the LLM's
//! vision API, and produces text Observations with actor attribution hints.
//! Supports configurable modes: provider-backed image recognition, OCR, or off.

pub mod describe;
pub mod ocr;

/// Screen processing mode, selected by `[processors.screen].mode`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum VisionMode {
    /// Provider-backed image recognition.
    Provider,
    /// macOS Vision framework OCR only (free, text-only fallback).
    Ocr,
    /// Skip processing. Save screenshots but produce no Observations.
    Off,
}

impl VisionMode {
    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "provider" | "local" | "api" => Some(Self::Provider),
            "ocr" => Some(Self::Ocr),
            "off" => Some(Self::Off),
            _ => None,
        }
    }
}
