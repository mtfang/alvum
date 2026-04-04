use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum ObservationKind {
    Dialogue { speaker: String },
    Action { detail: String },
    Visual { description: String },
    Note { context: String },
}

/// Universal intermediate format. Every connector produces these.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Observation {
    pub ts: DateTime<Utc>,
    pub source: String,
    pub kind: ObservationKind,
    pub content: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serialize_dialogue_observation() {
        let obs = Observation {
            ts: "2026-04-02T04:31:55Z".parse().unwrap(),
            source: "claude-code".into(),
            kind: ObservationKind::Dialogue {
                speaker: "user".into(),
            },
            content: "imagine we have endless context about a person's life".into(),
        };
        let json = serde_json::to_string(&obs).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed["source"], "claude-code");
        assert_eq!(parsed["kind"]["dialogue"]["speaker"], "user");
    }

    #[test]
    fn roundtrip_observation() {
        let obs = Observation {
            ts: "2026-04-02T04:31:55Z".parse().unwrap(),
            source: "claude-code".into(),
            kind: ObservationKind::Dialogue {
                speaker: "assistant".into(),
            },
            content: "This is a fascinating problem.".into(),
        };
        let json = serde_json::to_string(&obs).unwrap();
        let deserialized: Observation = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized, obs);
    }
}
