//! Deterministic evidence artifacts for the L5 briefing.
//!
//! The L2/L3/L4 tree stages intentionally compress. These dossiers keep
//! the compression inspectable by carrying source refs and representative
//! excerpts forward into the final briefing prompt.

use alvum_core::decision::{Decision, Edge};
use alvum_core::observation::{MediaRef, Observation};
use alvum_core::synthesis_profile::{SynthesisProfile, SynthesisProfileSnapshot};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet, HashMap};

use super::cluster::Cluster;
use super::domain::DomainNode;
use super::thread::{Thread, ThreadingResult};

const MAX_THREAD_EXCERPTS: usize = 8;
const MAX_CLUSTER_EXCERPTS: usize = 12;
const MAX_DECISION_EXCERPTS: usize = 8;
const MAX_EXCERPT_CHARS: usize = 420;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ObservationExcerpt {
    pub ref_id: String,
    pub ts: DateTime<Utc>,
    pub local_ts: String,
    pub source: String,
    pub kind: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub speaker: Option<String>,
    pub excerpt: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub media_ref: Option<MediaRef>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ThreadDossier {
    pub id: String,
    pub label: String,
    pub start: DateTime<Utc>,
    pub end: DateTime<Utc>,
    pub local_start: String,
    pub local_end: String,
    pub thread_type: String,
    pub relevance: f32,
    pub sources: Vec<String>,
    pub primary_actor: Option<String>,
    pub relevance_signals: Vec<String>,
    pub interest_refs: Vec<String>,
    pub intention_refs: Vec<String>,
    pub observation_refs: Vec<String>,
    pub evidence: Vec<ObservationExcerpt>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClusterDossier {
    pub id: String,
    pub label: String,
    pub theme: String,
    pub narrative: String,
    pub start: DateTime<Utc>,
    pub end: DateTime<Utc>,
    pub local_start: String,
    pub local_end: String,
    pub thread_ids: Vec<String>,
    pub primary_actors: Vec<String>,
    pub domains: Vec<String>,
    pub knowledge_refs: Vec<String>,
    pub interest_refs: Vec<String>,
    pub intention_refs: Vec<String>,
    pub observation_refs: Vec<String>,
    pub evidence: Vec<ObservationExcerpt>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DomainDossier {
    pub id: String,
    pub summary: String,
    pub cluster_ids: Vec<String>,
    pub key_actors: Vec<String>,
    pub decision_ids: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DecisionDossier {
    pub decision: Decision,
    pub interest_refs: Vec<String>,
    pub intention_refs: Vec<String>,
    pub supporting_cluster_ids: Vec<String>,
    pub incoming_edges: Vec<Edge>,
    pub outgoing_edges: Vec<Edge>,
    pub resolved_evidence: Vec<ObservationExcerpt>,
    pub unresolved_evidence: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BriefingSourcePack {
    pub schema: String,
    pub date: String,
    pub time_context: crate::local_time::LocalTimeContext,
    pub observation_count: usize,
    pub source_counts: BTreeMap<String, usize>,
    pub synthesis_profile: SynthesisProfileSnapshot,
    pub threads: Vec<ThreadDossier>,
    pub clusters: Vec<ClusterDossier>,
    pub domains: Vec<DomainDossier>,
    pub decisions: Vec<DecisionDossier>,
    pub edges: Vec<Edge>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BriefingArtifacts {
    pub thread_dossiers: Vec<ThreadDossier>,
    pub cluster_dossiers: Vec<ClusterDossier>,
    pub domain_dossiers: Vec<DomainDossier>,
    pub decision_dossiers: Vec<DecisionDossier>,
    pub source_pack: BriefingSourcePack,
}

pub fn build_briefing_artifacts(
    date: &str,
    observations: &[Observation],
    threading: &ThreadingResult,
    clusters: &[Cluster],
    domains: &[DomainNode],
    decisions: &[Decision],
    edges: &[Edge],
    profile_snapshot: &SynthesisProfileSnapshot,
) -> BriefingArtifacts {
    let profile = &profile_snapshot.profile;
    let lookup = ObservationLookup::new(observations);
    let thread_dossiers: Vec<ThreadDossier> = threading
        .threads
        .iter()
        .map(|thread| build_thread_dossier(thread, &lookup, profile))
        .collect();

    let thread_by_id: HashMap<&str, &ThreadDossier> = thread_dossiers
        .iter()
        .map(|thread| (thread.id.as_str(), thread))
        .collect();
    let cluster_dossiers: Vec<ClusterDossier> = clusters
        .iter()
        .map(|cluster| build_cluster_dossier(cluster, &thread_by_id, profile))
        .collect();

    let domain_dossiers: Vec<DomainDossier> = domains.iter().map(build_domain_dossier).collect();
    let decision_dossiers: Vec<DecisionDossier> = decisions
        .iter()
        .map(|decision| build_decision_dossier(decision, domains, edges, &lookup, profile))
        .collect();

    let source_pack = BriefingSourcePack {
        schema: "alvum.briefing_source_pack.v1".into(),
        date: date.to_string(),
        time_context: crate::local_time::LocalTimeContext::now(),
        observation_count: observations.len(),
        source_counts: source_counts(observations),
        synthesis_profile: profile_snapshot.clone(),
        threads: thread_dossiers.clone(),
        clusters: cluster_dossiers.clone(),
        domains: domain_dossiers.clone(),
        decisions: decision_dossiers.clone(),
        edges: edges.to_vec(),
    };

    BriefingArtifacts {
        thread_dossiers,
        cluster_dossiers,
        domain_dossiers,
        decision_dossiers,
        source_pack,
    }
}

fn build_thread_dossier(
    thread: &Thread,
    lookup: &ObservationLookup,
    profile: &SynthesisProfile,
) -> ThreadDossier {
    let observation_refs = refs_for(&thread.observations, lookup);
    let evidence = excerpts_for(&thread.observations, lookup, MAX_THREAD_EXCERPTS);
    let primary_actor = thread
        .metadata
        .as_ref()
        .and_then(|m| m.get("primary_actor"))
        .and_then(|v| v.as_str())
        .map(str::to_string);

    ThreadDossier {
        id: thread.id.clone(),
        label: thread.label.clone(),
        start: thread.start,
        end: thread.end,
        local_start: crate::local_time::format_rfc3339(thread.start),
        local_end: crate::local_time::format_rfc3339(thread.end),
        thread_type: thread.thread_type.clone(),
        relevance: thread.relevance,
        sources: thread.sources.clone(),
        primary_actor,
        relevance_signals: thread.relevance_signals.clone(),
        interest_refs: profile.match_text(&thread_interest_text(thread)),
        intention_refs: profile.match_intentions(&thread_interest_text(thread)),
        observation_refs,
        evidence,
    }
}

fn build_cluster_dossier(
    cluster: &Cluster,
    thread_by_id: &HashMap<&str, &ThreadDossier>,
    profile: &SynthesisProfile,
) -> ClusterDossier {
    let mut observation_refs = BTreeSet::new();
    let mut evidence = Vec::new();
    let mut seen_evidence = BTreeSet::new();

    for thread_id in &cluster.thread_ids {
        let Some(thread) = thread_by_id.get(thread_id.as_str()) else {
            continue;
        };
        observation_refs.extend(thread.observation_refs.iter().cloned());
        for excerpt in &thread.evidence {
            if evidence.len() >= MAX_CLUSTER_EXCERPTS {
                break;
            }
            if seen_evidence.insert(excerpt.ref_id.clone()) {
                evidence.push(excerpt.clone());
            }
        }
    }

    ClusterDossier {
        id: cluster.id.clone(),
        label: cluster.label.clone(),
        theme: cluster.theme.clone(),
        narrative: cluster.narrative.clone(),
        start: cluster.start,
        end: cluster.end,
        local_start: crate::local_time::format_rfc3339(cluster.start),
        local_end: crate::local_time::format_rfc3339(cluster.end),
        thread_ids: cluster.thread_ids.clone(),
        primary_actors: cluster.primary_actors.clone(),
        domains: cluster.domains.clone(),
        knowledge_refs: cluster.knowledge_refs.clone(),
        interest_refs: merge_interest_refs(
            profile.match_text(&cluster_interest_text(cluster)),
            cluster
                .thread_ids
                .iter()
                .filter_map(|thread_id| thread_by_id.get(thread_id.as_str()))
                .flat_map(|thread| thread.interest_refs.clone()),
        ),
        intention_refs: merge_interest_refs(
            profile.match_intentions(&cluster_interest_text(cluster)),
            cluster
                .thread_ids
                .iter()
                .filter_map(|thread_id| thread_by_id.get(thread_id.as_str()))
                .flat_map(|thread| thread.intention_refs.clone()),
        ),
        observation_refs: observation_refs.into_iter().collect(),
        evidence,
    }
}

fn build_domain_dossier(domain: &DomainNode) -> DomainDossier {
    DomainDossier {
        id: domain.id.clone(),
        summary: domain.summary.clone(),
        cluster_ids: domain.cluster_ids.clone(),
        key_actors: domain.key_actors.clone(),
        decision_ids: domain
            .decisions
            .iter()
            .map(|decision| decision.id.clone())
            .collect(),
    }
}

fn build_decision_dossier(
    decision: &Decision,
    domains: &[DomainNode],
    edges: &[Edge],
    lookup: &ObservationLookup,
    profile: &SynthesisProfile,
) -> DecisionDossier {
    let supporting_cluster_ids = domains
        .iter()
        .find(|domain| domain.decisions.iter().any(|d| d.id == decision.id))
        .map(|domain| domain.cluster_ids.clone())
        .unwrap_or_default();

    let incoming_edges: Vec<Edge> = edges
        .iter()
        .filter(|edge| edge.to_id == decision.id)
        .cloned()
        .collect();
    let outgoing_edges: Vec<Edge> = edges
        .iter()
        .filter(|edge| edge.from_id == decision.id)
        .cloned()
        .collect();

    let resolved_evidence = lookup.resolve_decision(decision);
    let unresolved_evidence = if resolved_evidence.is_empty() {
        decision.evidence.clone()
    } else {
        decision
            .evidence
            .iter()
            .filter(|quote| {
                !resolved_evidence.iter().any(|excerpt| {
                    excerpt.excerpt.contains(quote.as_str()) || quote.contains(&excerpt.excerpt)
                })
            })
            .cloned()
            .collect()
    };

    DecisionDossier {
        decision: decision.clone(),
        interest_refs: merge_interest_refs(
            decision.interest_refs.clone(),
            profile.match_text(&decision_interest_text(decision)),
        ),
        intention_refs: merge_interest_refs(
            decision.intention_refs.clone(),
            profile.match_intentions(&decision_interest_text(decision)),
        ),
        supporting_cluster_ids,
        incoming_edges,
        outgoing_edges,
        resolved_evidence,
        unresolved_evidence,
    }
}

fn thread_interest_text(thread: &Thread) -> String {
    format!(
        "{} {} {}",
        thread.id,
        thread.label,
        thread
            .relevance_signals
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>()
            .join(" ")
    )
}

fn cluster_interest_text(cluster: &Cluster) -> String {
    format!(
        "{} {} {} {} {}",
        cluster.id,
        cluster.label,
        cluster.theme,
        cluster.narrative,
        cluster.domains.join(" ")
    )
}

fn decision_interest_text(decision: &Decision) -> String {
    format!(
        "{} {} {} {} {}",
        decision.id,
        decision.summary,
        decision.reasoning.clone().unwrap_or_default(),
        decision.evidence.join(" "),
        decision.knowledge_refs.join(" ")
    )
}

fn merge_interest_refs(
    first: impl IntoIterator<Item = String>,
    second: impl IntoIterator<Item = String>,
) -> Vec<String> {
    let mut set = BTreeSet::new();
    set.extend(first);
    set.extend(second);
    set.into_iter().collect()
}

fn refs_for(observations: &[Observation], lookup: &ObservationLookup) -> Vec<String> {
    let mut refs = Vec::new();
    let mut seen = BTreeSet::new();
    for obs in observations {
        let ref_id = lookup.ref_for(obs);
        if seen.insert(ref_id.clone()) {
            refs.push(ref_id);
        }
    }
    refs
}

fn excerpts_for(
    observations: &[Observation],
    lookup: &ObservationLookup,
    max: usize,
) -> Vec<ObservationExcerpt> {
    let mut out = Vec::new();
    let mut seen = BTreeSet::new();
    for obs in observations {
        if out.len() >= max {
            break;
        }
        if obs.content.trim().is_empty() {
            continue;
        }
        let ref_id = lookup.ref_for(obs);
        if seen.insert(ref_id.clone()) {
            out.push(excerpt_for(obs, ref_id));
        }
    }
    out
}

fn excerpt_for(obs: &Observation, ref_id: String) -> ObservationExcerpt {
    ObservationExcerpt {
        ref_id,
        ts: obs.ts,
        local_ts: crate::local_time::format_rfc3339(obs.ts),
        source: obs.source.clone(),
        kind: obs.kind.clone(),
        speaker: obs.speaker().map(str::to_string),
        excerpt: truncate(&obs.content, MAX_EXCERPT_CHARS),
        media_ref: obs.media_ref.clone(),
    }
}

fn source_counts(observations: &[Observation]) -> BTreeMap<String, usize> {
    let mut counts = BTreeMap::new();
    for obs in observations {
        *counts.entry(obs.source.clone()).or_insert(0) += 1;
    }
    counts
}

fn truncate(s: &str, max_chars: usize) -> String {
    if s.chars().count() <= max_chars {
        return s.to_string();
    }
    let mut out: String = s.chars().take(max_chars).collect();
    out.push_str("...");
    out
}

struct ObservationLookup {
    by_key: HashMap<String, String>,
    observations: Vec<(String, Observation)>,
}

impl ObservationLookup {
    fn new(observations: &[Observation]) -> Self {
        let mut by_key = HashMap::new();
        let mut indexed = Vec::new();
        for (idx, obs) in observations.iter().enumerate() {
            let ref_id = format!("obs_{:06}", idx + 1);
            by_key.entry(obs_key(obs)).or_insert_with(|| ref_id.clone());
            indexed.push((ref_id, obs.clone()));
        }
        Self {
            by_key,
            observations: indexed,
        }
    }

    fn ref_for(&self, obs: &Observation) -> String {
        self.by_key.get(&obs_key(obs)).cloned().unwrap_or_else(|| {
            format!(
                "{} {}/{}",
                crate::local_time::format_rfc3339(obs.ts),
                obs.source,
                obs.kind
            )
        })
    }

    fn resolve_decision(&self, decision: &Decision) -> Vec<ObservationExcerpt> {
        let mut out = Vec::new();
        let mut seen = BTreeSet::new();

        for anchor in &decision.anchor_observations {
            for (ref_id, obs) in self.observations_matching_anchor(anchor) {
                if out.len() >= MAX_DECISION_EXCERPTS {
                    return out;
                }
                if seen.insert(ref_id.clone()) {
                    out.push(excerpt_for(obs, ref_id.clone()));
                }
            }
        }

        for quote in &decision.evidence {
            for (ref_id, obs) in self.observations_matching_quote(quote) {
                if out.len() >= MAX_DECISION_EXCERPTS {
                    return out;
                }
                if seen.insert(ref_id.clone()) {
                    out.push(excerpt_for(obs, ref_id.clone()));
                }
            }
        }

        out
    }

    fn observations_matching_anchor<'a>(
        &'a self,
        anchor: &'a str,
    ) -> impl Iterator<Item = (&'a String, &'a Observation)> {
        self.observations.iter().filter_map(move |(ref_id, obs)| {
            let local_minute_ref = format!(
                "[{}] {}/{}",
                crate::local_time::format_hm(obs.ts),
                obs.source,
                obs.kind
            );
            let local_second_ref = format!(
                "[{}] {}/{}",
                crate::local_time::format_hms(obs.ts),
                obs.source,
                obs.kind
            );
            let utc_minute_ref =
                format!("[{}] {}/{}", obs.ts.format("%H:%M"), obs.source, obs.kind);
            let utc_second_ref = format!(
                "[{}] {}/{}",
                obs.ts.format("%H:%M:%S"),
                obs.source,
                obs.kind
            );
            (anchor.contains(&local_minute_ref)
                || anchor.contains(&local_second_ref)
                || anchor.contains(&utc_minute_ref)
                || anchor.contains(&utc_second_ref))
            .then_some((ref_id, obs))
        })
    }

    fn observations_matching_quote<'a>(
        &'a self,
        quote: &'a str,
    ) -> impl Iterator<Item = (&'a String, &'a Observation)> {
        let quote = quote.trim();
        self.observations.iter().filter_map(move |(ref_id, obs)| {
            (!quote.is_empty() && (obs.content.contains(quote) || quote.contains(&obs.content)))
                .then_some((ref_id, obs))
        })
    }
}

fn obs_key(obs: &Observation) -> String {
    format!(
        "{}\u{1f}{}\u{1f}{}\u{1f}{}",
        obs.ts.to_rfc3339(),
        obs.source,
        obs.kind,
        obs.content
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use alvum_core::decision::{
        Actor, ActorAttribution, ActorKind, DecisionSource, DecisionStatus, EdgeStrength,
    };

    fn self_attr() -> ActorAttribution {
        ActorAttribution {
            actor: Actor {
                name: "self".into(),
                kind: ActorKind::Self_,
            },
            confidence: 0.9,
        }
    }

    fn obs(ts: &str, source: &str, content: &str) -> Observation {
        Observation::dialogue(ts.parse().unwrap(), source, "user", content)
    }

    fn decision(id: &str, evidence: Vec<String>, anchors: Vec<String>) -> Decision {
        Decision {
            id: id.into(),
            date: "2026-04-22".into(),
            time: "09:00".into(),
            summary: format!("{id} summary"),
            domain: "Career".into(),
            source: DecisionSource::Spoken,
            magnitude: 0.7,
            reasoning: Some("because the user said it".into()),
            alternatives: vec!["defer".into()],
            participants: vec!["self".into()],
            proposed_by: self_attr(),
            status: DecisionStatus::ActedOn,
            resolved_by: Some(self_attr()),
            open: false,
            check_by: None,
            cross_domain: Vec::new(),
            evidence,
            multi_source_evidence: false,
            confidence_overall: 0.8,
            anchor_observations: anchors,
            knowledge_refs: Vec::new(),
            interest_refs: Vec::new(),
            intention_refs: Vec::new(),
            causes: Vec::new(),
            effects: Vec::new(),
        }
    }

    #[test]
    fn decision_dossier_resolves_anchor_observations() {
        let observations = vec![
            obs(
                "2026-04-22T09:00:15Z",
                "codex",
                "Let's preserve detailed lower-level artifacts.",
            ),
            obs("2026-04-22T09:05:00Z", "screen", "Opened tree/day.rs"),
        ];
        let decision = decision(
            "dec_001",
            vec!["Let's preserve detailed lower-level artifacts.".into()],
            vec!["[09:00] codex/dialogue".into()],
        );
        let domain = DomainNode {
            id: "Career".into(),
            summary: "Career work happened.".into(),
            cluster_ids: vec!["cluster_artifacts".into()],
            key_actors: vec!["self".into()],
            decisions: vec![decision.clone()],
        };

        let dossier = build_decision_dossier(
            &decision,
            &[domain],
            &[],
            &ObservationLookup::new(&observations),
            &SynthesisProfile::default(),
        );

        assert_eq!(dossier.supporting_cluster_ids, vec!["cluster_artifacts"]);
        assert_eq!(dossier.resolved_evidence.len(), 1);
        assert_eq!(dossier.resolved_evidence[0].ref_id, "obs_000001");
        assert!(dossier.unresolved_evidence.is_empty());
    }

    #[test]
    fn source_pack_carries_lower_level_artifacts() {
        let observations = vec![obs(
            "2026-04-22T09:00:15Z",
            "codex",
            "Let's preserve detailed lower-level artifacts.",
        )];
        let thread = Thread {
            id: "thread_001".into(),
            label: "Artifact planning".into(),
            start: "2026-04-22T09:00:15Z".parse().unwrap(),
            end: "2026-04-22T09:00:30Z".parse().unwrap(),
            sources: vec!["codex".into()],
            observations: observations.clone(),
            relevance: 0.9,
            relevance_signals: vec!["explicit design decision".into()],
            thread_type: "solo_work".into(),
            metadata: Some(serde_json::json!({"primary_actor": "self"})),
        };
        let threading = ThreadingResult {
            start: thread.start,
            end: thread.end,
            time_blocks: Vec::new(),
            threads: vec![thread],
            observation_count: observations.len(),
            source_count: 1,
        };
        let cluster = Cluster {
            id: "cluster_artifacts".into(),
            label: "Briefing artifacts".into(),
            theme: "Carry evidence upward".into(),
            thread_ids: vec!["thread_001".into()],
            narrative: "The user chose to preserve lower-level artifacts.".into(),
            start: "2026-04-22T09:00:15Z".parse().unwrap(),
            end: "2026-04-22T09:00:30Z".parse().unwrap(),
            primary_actors: vec!["self".into()],
            domains: vec!["software".into()],
            knowledge_refs: Vec::new(),
        };
        let decision = decision(
            "dec_001",
            vec!["Let's preserve detailed lower-level artifacts.".into()],
            vec!["[09:00] codex/dialogue".into()],
        );
        let domain = DomainNode {
            id: "Career".into(),
            summary: "Career work happened.".into(),
            cluster_ids: vec!["cluster_artifacts".into()],
            key_actors: vec!["self".into()],
            decisions: vec![decision.clone()],
        };
        let edge = Edge {
            from_id: "dec_001".into(),
            to_id: "dec_001".into(),
            relation: "direct".into(),
            mechanism: "self-loop fixture".into(),
            strength: EdgeStrength::Primary,
            rationale: None,
        };

        let artifacts = build_briefing_artifacts(
            "2026-04-22",
            &observations,
            &threading,
            &[cluster],
            &[domain],
            &[decision],
            &[edge],
            &SynthesisProfile::default().snapshot(),
        );

        assert_eq!(
            artifacts.source_pack.schema,
            "alvum.briefing_source_pack.v1"
        );
        assert_eq!(
            artifacts
                .source_pack
                .synthesis_profile
                .profile
                .enabled_domain_ids()[0],
            "Career"
        );
        assert_eq!(artifacts.source_pack.threads.len(), 1);
        assert_eq!(artifacts.source_pack.clusters.len(), 1);
        assert_eq!(artifacts.source_pack.decisions.len(), 1);
        assert_eq!(
            artifacts.source_pack.threads[0].local_start,
            crate::local_time::format_rfc3339(threading.threads[0].start)
        );
        assert_eq!(
            artifacts.source_pack.clusters[0].local_start,
            crate::local_time::format_rfc3339(artifacts.cluster_dossiers[0].start)
        );
        assert_eq!(
            artifacts.source_pack.decisions[0].resolved_evidence[0].local_ts,
            crate::local_time::format_rfc3339(observations[0].ts)
        );
        assert!(!artifacts.source_pack.time_context.utc_offset.is_empty());
        assert_eq!(
            artifacts.source_pack.source_counts.get("codex").copied(),
            Some(1)
        );
        assert_eq!(
            artifacts.source_pack.decisions[0].resolved_evidence[0].ref_id,
            "obs_000001"
        );
    }
}
