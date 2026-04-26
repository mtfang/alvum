//! ScreenConnector — user-facing plugin bundling screen capture + vision/OCR.

pub mod processor;

use alvum_core::capture::CaptureSource;
use alvum_core::connector::Connector;
use alvum_core::processor::Processor;
use alvum_pipeline::llm::LlmProvider;
use alvum_processor_screen::VisionMode;
use anyhow::Result;
use std::collections::HashMap;
use std::sync::Arc;

use processor::ScreenProcessor;

pub struct ScreenConnector {
    idle_interval_secs: u64,
    vision_mode: VisionMode,
    provider: Option<Arc<dyn LlmProvider>>,
}

impl ScreenConnector {
    pub fn from_config(
        settings: &HashMap<String, toml::Value>,
        provider: Option<Arc<dyn LlmProvider>>,
    ) -> Result<Self> {
        let idle_interval_secs = settings
            .get("idle_interval_secs")
            .and_then(|v| v.as_integer())
            .unwrap_or(30) as u64;
        let vision_str = settings
            .get("vision")
            .and_then(|v| v.as_str())
            .unwrap_or("local");
        let vision_mode = VisionMode::from_str(vision_str).unwrap_or(VisionMode::Local);

        Ok(Self {
            idle_interval_secs,
            vision_mode,
            provider,
        })
    }
}

impl Connector for ScreenConnector {
    fn name(&self) -> &str {
        "screen"
    }

    fn expected_sources(&self) -> Vec<&'static str> {
        // Screenshot capture writes a single `screen` source. The
        // briefing pipeline considers an empty screen modality
        // noteworthy — historically the connector silently emitted
        // zero refs without any warning, which is exactly what this
        // declaration exists to prevent.
        vec!["screen"]
    }

    fn capture_sources(&self) -> Vec<Box<dyn CaptureSource>> {
        let mut settings = HashMap::new();
        settings.insert(
            "idle_interval_secs".into(),
            toml::Value::Integer(self.idle_interval_secs as i64),
        );
        vec![Box::new(
            alvum_capture_screen::source::ScreenSource::from_config(&settings),
        )]
    }

    fn processors(&self) -> Vec<Box<dyn Processor>> {
        vec![Box::new(ScreenProcessor::new(
            self.vision_mode,
            self.provider.clone(),
        ))]
    }

    fn gather_data_refs(
        &self,
        capture_dir: &std::path::Path,
    ) -> Result<Vec<alvum_core::data_ref::DataRef>> {
        use alvum_core::pipeline_events::{emit, Event};
        let captures_path = capture_dir.join("screen").join("captures.jsonl");
        if !captures_path.exists() {
            // Capture daemon either isn't running or hasn't produced
            // its index file yet. Surface it explicitly — the screen
            // modality has historically gone dark without warning.
            emit(Event::Warning {
                source: "connector/screen".into(),
                message: format!(
                    "captures.jsonl does not exist: {}",
                    captures_path.display()
                ),
            });
            return Ok(vec![]);
        }
        let refs: Vec<alvum_core::data_ref::DataRef> = alvum_core::storage::read_jsonl(&captures_path)
            .map_err(|e| anyhow::anyhow!("read screen captures.jsonl at {}: {e}", captures_path.display()))?;
        if refs.is_empty() {
            emit(Event::Warning {
                source: "connector/screen".into(),
                message: format!(
                    "captures.jsonl is empty: {}",
                    captures_path.display()
                ),
            });
        }
        Ok(refs)
    }
}
