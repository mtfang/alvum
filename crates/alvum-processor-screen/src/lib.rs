//! Screen processor: sends screenshots to a vision model and produces Observations.
//!
//! Reads DataRefs (PNG screenshots from alvum-capture-screen), calls the LLM's
//! vision API, and produces text Observations with actor attribution hints.
//! Supports configurable vision modes: local (Ollama), api (Anthropic), ocr (macOS Vision), off.

pub mod describe;
pub mod ocr;

/// Vision processing mode, selected by `--vision` CLI flag.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum VisionMode {
    /// Ollama vision model (free, local). Default.
    Local,
    /// Anthropic API vision (paid, highest quality).
    Api,
    /// macOS Vision framework OCR only (free, text-only fallback).
    Ocr,
    /// Skip processing. Save screenshots but produce no Observations.
    Off,
}

impl VisionMode {
    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "local" => Some(Self::Local),
            "api" => Some(Self::Api),
            "ocr" => Some(Self::Ocr),
            "off" => Some(Self::Off),
            _ => None,
        }
    }
}
