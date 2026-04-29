//! Decision domain types — atoms emitted by the L4 (domain) layer of the
//! recursive distillation tree.
//!
//! The schema is the website prototype's `DECISIONS` shape (see
//! `~/git/alvum-website/lib/prototype/fixtures.ts`) augmented with the
//! "aim-higher" engine fields documented in the implementation plan
//! (`multi_source_evidence`, `confidence_overall`, `anchor_observations`,
//! `knowledge_refs`).
//!
//! The richer `Edge` metadata for cross-decision relationships lives in
//! the L4-edges artifact, not on the Decision itself. `causes` and
//! `effects` on Decision are flat arrays of decision IDs derived from
//! L4-edges at serialization time — that's what the website's decision
//! graph viz consumes.

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

fn default_confidence() -> f32 {
    0.5
}

/// How a decision came into the engine's view. The distinction is
/// load-bearing: the briefing layer pairs `Spoken` intents with
/// `Revealed` behaviors to surface alignment gaps.
///
/// - `Spoken`    — verbalized in audio or chat; an explicit choice with a quote.
/// - `Revealed`  — inferred from observed behavior; the action IS the choice.
/// - `Explained` — post-hoc rationalization for an earlier `Revealed` action.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub enum DecisionSource {
    Spoken,
    Revealed,
    Explained,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Decision {
    /// Globally-unique within a single pipeline run, in chronological order
    /// (dec_001, dec_002, …).
    pub id: String,
    /// Local date `YYYY-MM-DD`.
    pub date: String,
    /// Local time `HH:MM`.
    pub time: String,

    /// 1–2 sentence summary, actionable and specific.
    pub summary: String,
    /// User-configured synthesis domain. Defaults start with Career, Health,
    /// and Family, but custom profiles can replace them with any canonical
    /// strings.
    pub domain: String,
    /// Spoken / Revealed / Explained. See `DecisionSource`.
    pub source: DecisionSource,

    /// Estimated importance / cascade potential, 0.0–1.0.
    /// Anchors: 0.1 trivial, 0.5 notable, 0.9 day-defining.
    pub magnitude: f32,

    /// Optional 1-line rationale (only when stated).
    pub reasoning: Option<String>,
    /// 0–3 alternatives the user considered.
    #[serde(default, deserialize_with = "deserialize_null_as_empty_vec")]
    pub alternatives: Vec<String>,
    /// Actor IDs that participated in the decision (people, agents, orgs).
    #[serde(default, deserialize_with = "deserialize_null_as_empty_vec")]
    pub participants: Vec<String>,

    pub proposed_by: ActorAttribution,
    pub status: DecisionStatus,
    pub resolved_by: Option<ActorAttribution>,

    /// True when the decision has unresolved follow-ups.
    #[serde(default)]
    pub open: bool,
    /// `YYYY-MM-DD` when this decision needs to be revisited (only if `open`).
    #[serde(default)]
    pub check_by: Option<String>,

    /// Other domain strings this decision touches.
    #[serde(default, deserialize_with = "deserialize_null_as_empty_vec")]
    pub cross_domain: Vec<String>,

    /// 1–3 short verbatim quotes grounding this decision.
    #[serde(default, deserialize_with = "deserialize_null_as_empty_vec")]
    pub evidence: Vec<String>,

    // === Aim-higher engine fields ===
    /// True iff `evidence` quotes span ≥ 2 distinct connector sources.
    #[serde(default)]
    pub multi_source_evidence: bool,
    /// Calibrated overall confidence the decision is real and correctly
    /// attributed. See plan for calibration anchors. Default 0.5 means
    /// "neutral" — used when the LLM omits the field.
    #[serde(default = "default_confidence")]
    pub confidence_overall: f32,
    /// Up to 5 observation refs anchoring the decision. The L5 briefing
    /// layer uses these for citation-by-quote.
    #[serde(default, deserialize_with = "deserialize_null_as_empty_vec")]
    pub anchor_observations: Vec<String>,
    /// Knowledge corpus IDs (entities / patterns / facts) referenced by
    /// this decision. The pipeline validates these against the supplied
    /// corpus and drops unresolved ids — same pattern as Phase 3.5.
    #[serde(default, deserialize_with = "deserialize_null_as_empty_vec")]
    pub knowledge_refs: Vec<String>,
    /// User-managed synthesis profile interest ids associated with this
    /// decision. The model may emit them from the supplied profile block;
    /// deterministic dossier construction can also match tracked interest
    /// names and aliases.
    #[serde(default, deserialize_with = "deserialize_null_as_empty_vec")]
    pub interest_refs: Vec<String>,
    /// User-managed intention ids associated with this decision. These
    /// are the top-level goals, habits, commitments, missions, or
    /// ambitions this decision either served or drifted from.
    #[serde(default, deserialize_with = "deserialize_null_as_empty_vec")]
    pub intention_refs: Vec<String>,

    // === Graph projection (populated at serialization time, not by the LLM) ===
    /// IDs of decisions that caused this one. Derived from L4-edges; the
    /// LLM emits structured `Edge` records, this field is populated by
    /// the pipeline before writing `decisions.jsonl` so the website's
    /// decisions UI (which reads `decision.causes`) works directly.
    #[serde(default, deserialize_with = "deserialize_null_as_empty_vec")]
    pub causes: Vec<String>,
    /// IDs of decisions caused BY this one. Same provenance as `causes`.
    #[serde(default, deserialize_with = "deserialize_null_as_empty_vec")]
    pub effects: Vec<String>,
}

/// An actor with a confidence score for the attribution.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ActorAttribution {
    pub actor: Actor,
    pub confidence: f32, // 0.0–1.0
}

/// An entity that can propose or act on decisions.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Actor {
    pub name: String,
    pub kind: ActorKind,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ActorKind {
    #[serde(rename = "self")]
    Self_, // the user
    Person,
    Agent,        // AI assistant / automated system
    Organization, // company, institution
    Environment,  // market conditions, circumstances
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DecisionStatus {
    ActedOn,
    Accepted,
    Rejected,
    Pending,
    Ignored,
}

/// A directed relationship between two nodes at a single tree level.
/// Used uniformly for L2 (thread↔thread), L3 (cluster↔cluster), and L4
/// (decision↔decision) cross-correlation outputs. `relation` vocabulary
/// varies per level — see the level-specific edge prompts in the plan.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Edge {
    pub from_id: String,
    pub to_id: String,
    /// Free-form vocabulary, validated per level. Examples:
    ///   L2: "caused", "continued", "thematic", "interrupted", "supports"
    ///   L3: "fed_into", "thematic", "blocked_by", "context_for", "compete_for_attention"
    ///   L4: "direct", "resource_competition", "emotional_influence",
    ///       "precedent", "accumulation", "constraint",
    ///       "alignment_break", "alignment_honor"
    pub relation: String,
    /// 1-line grounding describing how the link was inferred.
    pub mechanism: String,
    pub strength: EdgeStrength,
    /// Optional verbatim rationale citing evidence quotes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rationale: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum EdgeStrength {
    Primary,
    Contributing,
    Background,
}

/// Backwards-compat alias used by code paths still in transition. New
/// code should reach for `Edge` directly. Removed in a follow-up commit
/// once all call sites migrate.
#[deprecated(note = "Use `Edge` for tree-level cross-correlation outputs")]
pub type CausalLink = Edge;

#[allow(deprecated)]
pub use self::EdgeStrength as CausalStrength;

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
            actor: Actor {
                name: "user".into(),
                kind: ActorKind::Self_,
            },
            confidence,
        }
    }

    fn agent_attr(name: &str, confidence: f32) -> ActorAttribution {
        ActorAttribution {
            actor: Actor {
                name: name.into(),
                kind: ActorKind::Agent,
            },
            confidence,
        }
    }

    fn make_decision(id: &str, domain: &str, source: DecisionSource) -> Decision {
        Decision {
            id: id.into(),
            date: "2026-04-22".into(),
            time: "10:30".into(),
            summary: "Test decision".into(),
            domain: domain.into(),
            source,
            magnitude: 0.5,
            reasoning: None,
            alternatives: vec![],
            participants: vec![],
            proposed_by: self_attr(0.9),
            status: DecisionStatus::ActedOn,
            resolved_by: Some(self_attr(0.9)),
            open: false,
            check_by: None,
            cross_domain: vec![],
            evidence: vec![],
            multi_source_evidence: false,
            confidence_overall: 0.5,
            anchor_observations: vec![],
            knowledge_refs: vec![],
            interest_refs: vec![],
            intention_refs: vec![],
            causes: vec![],
            effects: vec![],
        }
    }

    #[test]
    fn decision_source_serializes_pascal_case() {
        let json = serde_json::to_string(&DecisionSource::Spoken).unwrap();
        assert_eq!(json, "\"Spoken\"");
        let json = serde_json::to_string(&DecisionSource::Revealed).unwrap();
        assert_eq!(json, "\"Revealed\"");
        let json = serde_json::to_string(&DecisionSource::Explained).unwrap();
        assert_eq!(json, "\"Explained\"");
    }

    #[test]
    fn decision_roundtrip_preserves_all_fields() {
        let dec = Decision {
            id: "dec_017".into(),
            date: "2026-04-22".into(),
            time: "14:23".into(),
            summary: "Deferred migration launch by two weeks".into(),
            domain: "Career".into(),
            source: DecisionSource::Spoken,
            magnitude: 0.82,
            reasoning: Some("Need a dry run on staging before risking production data.".into()),
            alternatives: vec!["Ship on the original date".into()],
            participants: vec!["russ_hanneman".into()],
            proposed_by: self_attr(0.85),
            status: DecisionStatus::ActedOn,
            resolved_by: Some(agent_attr("claude", 0.7)),
            open: true,
            check_by: Some("2026-05-02".into()),
            cross_domain: vec!["Health".into()],
            evidence: vec![
                "\"Let's do a dry run before we cut over.\"".into(),
                "Linear MIG-142: In Progress → Backlog".into(),
            ],
            multi_source_evidence: true,
            confidence_overall: 0.88,
            anchor_observations: vec!["[10:45] audio-mic".into(), "[10:46] screen".into()],
            knowledge_refs: vec!["entity_russ_hanneman".into()],
            interest_refs: vec!["project_alvum".into()],
            intention_refs: vec!["ship_alignment_engine".into()],
            causes: vec!["dec_004".into()],
            effects: vec!["dec_023".into()],
        };
        let json = serde_json::to_string(&dec).unwrap();
        let parsed: Decision = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, dec);
    }

    #[test]
    fn decision_default_fields_tolerate_omissions() {
        // The LLM may omit aim-higher fields on first attempts. Defaults
        // must let the parse succeed without forcing the LLM to emit
        // every field; the validator pass surfaces missing fields as
        // events.
        let json = r#"{
            "id": "dec_001",
            "date": "2026-04-22",
            "time": "09:00",
            "summary": "minimal decision",
            "domain": "Health",
            "source": "Revealed",
            "magnitude": 0.3,
            "reasoning": null,
            "proposed_by": {"actor":{"name":"user","kind":"self"},"confidence":0.5},
            "status": "acted_on",
            "resolved_by": null
        }"#;
        let parsed: Decision = serde_json::from_str(json).unwrap();
        assert_eq!(parsed.id, "dec_001");
        assert!(parsed.alternatives.is_empty());
        assert_eq!(parsed.confidence_overall, 0.5); // default
        assert!(!parsed.multi_source_evidence);
        assert!(!parsed.open);
        assert!(parsed.interest_refs.is_empty());
        assert!(parsed.intention_refs.is_empty());
        assert!(parsed.causes.is_empty());
        assert!(parsed.effects.is_empty());
    }

    #[test]
    fn cross_domain_round_trips_as_pascal_case_array() {
        let dec = make_decision("dec_001", "Career", DecisionSource::Spoken);
        let mut dec = dec;
        dec.cross_domain = vec!["Health".into(), "Family".into()];
        let json = serde_json::to_string(&dec).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed["cross_domain"][0], "Health");
        assert_eq!(parsed["cross_domain"][1], "Family");
    }

    #[test]
    fn edge_serializes_with_strength_snake_case() {
        let edge = Edge {
            from_id: "dec_001".into(),
            to_id: "dec_002".into(),
            relation: "alignment_break".into(),
            mechanism: "Spoken intent contradicted by Revealed action".into(),
            strength: EdgeStrength::Primary,
            rationale: Some("Audio: 'I'll prioritize this' followed by 15 min logged".into()),
        };
        let json = serde_json::to_string(&edge).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed["strength"], "primary");
        assert_eq!(parsed["relation"], "alignment_break");
    }

    #[test]
    fn edge_omits_rationale_when_none() {
        let edge = Edge {
            from_id: "a".into(),
            to_id: "b".into(),
            relation: "thematic".into(),
            mechanism: "shared theme".into(),
            strength: EdgeStrength::Contributing,
            rationale: None,
        };
        let json = serde_json::to_string(&edge).unwrap();
        // `skip_serializing_if = "Option::is_none"` keeps the wire shape
        // tight for downstream consumers.
        assert!(!json.contains("rationale"));
    }

    #[test]
    fn actor_kind_self_renames_correctly() {
        let actor = Actor {
            name: "user".into(),
            kind: ActorKind::Self_,
        };
        let json = serde_json::to_string(&actor).unwrap();
        assert!(json.contains("\"kind\":\"self\""));
    }
}
