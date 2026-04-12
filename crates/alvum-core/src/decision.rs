use serde::{Deserialize, Deserializer, Serialize};

/// Deserialize a Vec that tolerates null (treats null as empty vec).
/// LLMs sometimes return null instead of [] for empty arrays.
fn deserialize_null_as_empty_vec<'de, D, T>(deserializer: D) -> Result<Vec<T>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    let opt: Option<Vec<T>> = Option::deserialize(deserializer)?;
    Ok(opt.unwrap_or_default())
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Decision {
    pub id: String,
    pub timestamp: String,
    pub summary: String,
    pub reasoning: Option<String>,
    #[serde(default, deserialize_with = "deserialize_null_as_empty_vec")]
    pub alternatives: Vec<String>,
    pub domain: String,
    pub source: String,
    pub proposed_by: ActorAttribution,
    pub status: DecisionStatus,
    pub resolved_by: Option<ActorAttribution>,
    #[serde(default, deserialize_with = "deserialize_null_as_empty_vec")]
    pub causes: Vec<CausalLink>,
    #[serde(default, deserialize_with = "deserialize_null_as_empty_vec")]
    pub tags: Vec<String>,
    pub expected_outcome: Option<String>,
}

/// An actor with a confidence score for the attribution.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ActorAttribution {
    pub actor: Actor,
    pub confidence: f32,  // 0.0 to 1.0
}

/// An entity that can propose or act on decisions.
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
    Person,         // named individual
    Agent,          // AI assistant, algorithm, automated system
    Organization,   // company, institution
    Environment,    // market conditions, circumstances
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum DecisionStatus {
    ActedOn,    // someone did the thing
    Accepted,   // agreed to but not yet done
    Rejected,   // explicitly turned down
    Pending,    // still under consideration
    Ignored,    // proposed but got no response
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

    fn self_attr(confidence: f32) -> ActorAttribution {
        ActorAttribution {
            actor: Actor { name: "user".into(), kind: ActorKind::Self_ },
            confidence,
        }
    }

    fn agent_attr(name: &str, confidence: f32) -> ActorAttribution {
        ActorAttribution {
            actor: Actor { name: name.into(), kind: ActorKind::Agent },
            confidence,
        }
    }

    #[test]
    fn serialize_self_proposed_acted_on() {
        let dec = Decision {
            id: "dec_001".into(),
            timestamp: "2026-04-02T04:35:00Z".into(),
            summary: "Process data overnight, not real-time".into(),
            reasoning: Some("Overnight batch gives full-day context, reduces cost".into()),
            alternatives: vec!["Real-time streaming".into(), "Hybrid approach".into()],
            domain: "Architecture".into(),
            source: "claude-code".into(),
            proposed_by: self_attr(0.95),
            status: DecisionStatus::ActedOn,
            resolved_by: Some(self_attr(0.95)),
            causes: vec![],
            tags: vec!["pipeline".into(), "cost".into()],
            expected_outcome: None,
        };
        let json = serde_json::to_string(&dec).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed["proposed_by"]["actor"]["kind"], "self");
        assert_eq!(parsed["status"], "acted_on");
        assert_eq!(parsed["resolved_by"]["actor"]["kind"], "self");
    }

    #[test]
    fn serialize_agent_proposed_user_acted() {
        let dec = Decision {
            id: "dec_008".into(),
            timestamp: "2026-04-02T05:00:00Z".into(),
            summary: "Use Omi pendant for audio capture".into(),
            reasoning: Some("Open source, raw audio accessible".into()),
            alternatives: vec!["Limitless".into(), "Build custom".into()],
            domain: "Technology".into(),
            source: "claude-code".into(),
            proposed_by: agent_attr("claude", 0.9),
            status: DecisionStatus::ActedOn,
            resolved_by: Some(self_attr(0.7)),
            causes: vec![],
            tags: vec!["wearable".into(), "audio".into()],
            expected_outcome: None,
        };
        let json = serde_json::to_string(&dec).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed["proposed_by"]["actor"]["name"], "claude");
        assert_eq!(parsed["proposed_by"]["confidence"], 0.9);
        assert_eq!(parsed["status"], "acted_on");
        assert_eq!(parsed["resolved_by"]["confidence"], 0.7);
    }

    #[test]
    fn serialize_rejected_decision() {
        let dec = Decision {
            id: "dec_012".into(),
            timestamp: "2026-04-03T17:51:00Z".into(),
            summary: "Strip all differentiators for simplicity".into(),
            reasoning: Some("5-step process applied aggressively".into()),
            alternatives: vec![],
            domain: "Architecture".into(),
            source: "claude-code".into(),
            proposed_by: agent_attr("claude", 0.95),
            status: DecisionStatus::Rejected,
            resolved_by: Some(self_attr(0.95)),
            causes: vec![],
            tags: vec!["simplification".into()],
            expected_outcome: None,
        };
        let json = serde_json::to_string(&dec).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed["status"], "rejected");
        assert_eq!(parsed["proposed_by"]["actor"]["kind"], "agent");
        assert_eq!(parsed["resolved_by"]["actor"]["kind"], "self");
    }

    #[test]
    fn serialize_pending_decision() {
        let dec = Decision {
            id: "dec_031".into(),
            timestamp: "2026-04-03T18:52:00Z".into(),
            summary: "Dedicated hardware box as product north star".into(),
            reasoning: None,
            alternatives: vec![],
            domain: "Product".into(),
            source: "claude-code".into(),
            proposed_by: agent_attr("claude", 0.8),
            status: DecisionStatus::Pending,
            resolved_by: None,
            causes: vec![],
            tags: vec!["hardware".into()],
            expected_outcome: None,
        };
        let json = serde_json::to_string(&dec).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed["status"], "pending");
        assert!(parsed["resolved_by"].is_null());
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
    fn roundtrip_decision_with_actors() {
        let dec = Decision {
            id: "dec_002".into(),
            timestamp: "2026-04-03T17:54:00Z".into(),
            summary: "Restore camera for physical-world alignment".into(),
            reasoning: Some("Camera captures physical actions vs intentions".into()),
            alternatives: vec![],
            domain: "Product".into(),
            source: "claude-code".into(),
            proposed_by: self_attr(0.85),
            status: DecisionStatus::ActedOn,
            resolved_by: Some(agent_attr("claude", 0.8)),
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
