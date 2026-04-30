//! ScreenProcessor — implements Processor trait, dispatches to vision model or OCR.

use alvum_core::data_ref::DataRef;
use alvum_core::observation::Observation;
use alvum_core::processor::Processor;
use alvum_pipeline::llm::LlmProvider;
use alvum_processor_screen::VisionMode;
use anyhow::Result;
use async_trait::async_trait;
use std::path::Path;
use std::sync::Arc;

pub struct ScreenProcessor {
    mode: VisionMode,
    provider: Option<Arc<dyn LlmProvider>>,
}

impl ScreenProcessor {
    pub fn new(mode: VisionMode, provider: Option<Arc<dyn LlmProvider>>) -> Self {
        Self { mode, provider }
    }
}

#[async_trait]
impl Processor for ScreenProcessor {
    fn name(&self) -> &str {
        match self.mode {
            VisionMode::Provider => "vision-provider",
            VisionMode::Ocr => "ocr",
            VisionMode::Off => "screen-off",
        }
    }

    fn handles(&self) -> Vec<String> {
        vec!["screen".into()]
    }

    async fn process(&self, data_refs: &[DataRef], capture_dir: &Path) -> Result<Vec<Observation>> {
        match self.mode {
            VisionMode::Provider => {
                let provider = self
                    .provider
                    .as_ref()
                    .ok_or_else(|| anyhow::anyhow!("vision mode requires LlmProvider"))?;
                alvum_processor_screen::describe::process_screen_data_refs(
                    provider.as_ref(),
                    data_refs,
                    capture_dir,
                )
                .await
            }
            VisionMode::Ocr => {
                alvum_processor_screen::ocr::process_screen_data_refs_ocr(data_refs, capture_dir)
            }
            VisionMode::Off => Ok(vec![]),
        }
    }
}
