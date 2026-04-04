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
    pub actor: Actor,
    pub causes: Vec<CausalLink>,
    pub tags: Vec<String>,
    pub expected_outcome: Option<String>,
}

/// Who made this decision. Not every decision that affects you is yours.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Actor {
    pub name: String,
    pub kind: ActorKind,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum ActorKind {
    #[serde(rename = "self")]
    Self_,          // the user
    Person,         // named individual (manager, partner, colleague)
    Agent,          // AI assistant, algorithm, automated system
    Organization,   // company, institution
    Environment,    // market conditions, circumstances
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
            actor: Actor { name: "user".into(), kind: ActorKind::Self_ },
            causes: vec![],
            tags: vec!["pipeline".into(), "cost".into()],
            expected_outcome: None,
        };
        let json = serde_json::to_string(&dec).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed["id"], "dec_001");
        assert_eq!(parsed["alternatives"].as_array().unwrap().len(), 2);
        assert_eq!(parsed["actor"]["kind"], "self");
    }

    #[test]
    fn serialize_external_actor() {
        let dec = Decision {
            id: "dec_010".into(),
            timestamp: "2026-04-03T17:51:00Z".into(),
            summary: "Proposed stripping differentiators for simplicity".into(),
            reasoning: Some("Applying 5-step process aggressively".into()),
            alternatives: vec![],
            domain: "Architecture".into(),
            source: "claude-code".into(),
            actor: Actor { name: "claude".into(), kind: ActorKind::Agent },
            causes: vec![],
            tags: vec!["simplification".into()],
            expected_outcome: None,
        };
        let json = serde_json::to_string(&dec).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed["actor"]["name"], "claude");
        assert_eq!(parsed["actor"]["kind"], "agent");
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
            actor: Actor { name: "user".into(), kind: ActorKind::Self_ },
            causes: vec![CausalLink {
                from_id: "dec_001".into(),
                mechanism: "direct".into(),
                strength: CausalStrength::Primary,
            }],
            tags: vec!["capture".into(), "camera".into()],
            expected_outcome: None,
        };
        let json = serde_json::to_string(&dec).unwrap();
        let deserialized: Decision = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized, dec);
    }
}
