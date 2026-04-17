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
}
