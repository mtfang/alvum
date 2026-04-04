use std::path::PathBuf;

pub struct AlvumConfig {
    pub data_dir: PathBuf,
    pub anthropic_api_key: String,
    pub model: String,
}

impl AlvumConfig {
    pub fn new(data_dir: PathBuf) -> Self {
        let api_key = std::env::var("ANTHROPIC_API_KEY")
            .unwrap_or_default();
        Self {
            data_dir,
            anthropic_api_key: api_key,
            model: "claude-sonnet-4-6".into(),
        }
    }

    pub fn decisions_path(&self) -> PathBuf {
        self.data_dir.join("decisions.jsonl")
    }

    pub fn briefing_path(&self) -> PathBuf {
        self.data_dir.join("briefing.md")
    }

    pub fn extraction_path(&self) -> PathBuf {
        self.data_dir.join("extraction.json")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn paths_relative_to_data_dir() {
        let tmp = TempDir::new().unwrap();
        let config = AlvumConfig::new(tmp.path().to_path_buf());
        assert!(config.decisions_path().starts_with(tmp.path()));
        assert!(config.decisions_path().ends_with("decisions.jsonl"));
        assert!(config.briefing_path().ends_with("briefing.md"));
    }
}
