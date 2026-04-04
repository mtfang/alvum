use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Decision {
    pub id: String,
    pub timestamp: String,
    pub summary: String,
    pub reasoning: Option<String>,
    pub alternatives: Vec<String>,
    pub domain: String,
    pub source: String,
    pub causes: Vec<CausalLink>,
    pub tags: Vec<String>,
    pub open: bool,
    pub expected_outcome: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CausalLink {
    pub from_id: String,
    pub mechanism: String,
    pub strength: CausalStrength,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum CausalStrength {
    Primary,
    Contributing,
    Background,
}

/// The complete output of one pipeline run.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExtractionResult {
    pub session_id: String,
    pub extracted_at: String,
    pub decisions: Vec<Decision>,
    pub briefing: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serialize_decision_to_jsonl() {
        let dec = Decision {
            id: "dec_001".into(),
            timestamp: "2026-04-02T04:35:00Z".into(),
            summary: "Process data overnight, not real-time".into(),
            reasoning: Some("Overnight batch gives full-day context, reduces cost".into()),
            alternatives: vec!["Real-time streaming".into(), "Hybrid approach".into()],
            domain: "Architecture".into(),
            source: "claude-code".into(),
            causes: vec![],
            tags: vec!["pipeline".into(), "cost".into()],
            open: false,
            expected_outcome: None,
        };
        let json = serde_json::to_string(&dec).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed["id"], "dec_001");
        assert_eq!(parsed["alternatives"].as_array().unwrap().len(), 2);
    }

    #[test]
    fn serialize_causal_link() {
        let link = CausalLink {
            from_id: "dec_003".into(),
            mechanism: "User pushback on oversimplification".into(),
            strength: CausalStrength::Primary,
        };
        let json = serde_json::to_string(&link).unwrap();
        assert!(json.contains("primary"));
    }

    #[test]
    fn roundtrip_decision() {
        let dec = Decision {
            id: "dec_002".into(),
            timestamp: "2026-04-03T17:54:00Z".into(),
            summary: "Restore camera for physical-world alignment".into(),
            reasoning: Some("Camera captures physical actions vs intentions".into()),
            alternatives: vec![],
            domain: "Product".into(),
            source: "claude-code".into(),
            causes: vec![CausalLink {
                from_id: "dec_001".into(),
                mechanism: "direct".into(),
                strength: CausalStrength::Primary,
            }],
            tags: vec!["capture".into(), "camera".into()],
            open: false,
            expected_outcome: None,
        };
        let json = serde_json::to_string(&dec).unwrap();
        let deserialized: Decision = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized, dec);
    }
}
