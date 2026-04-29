//! User-managed synthesis profile.
//!
//! The profile is authored by the user and injected into synthesis prompts as
//! context. It is intentionally separate from generated knowledge: knowledge is
//! model-owned and accumulates from observations, while this profile records
//! what the user explicitly wants the synthesizer to emphasize or mute.

use anyhow::{Context, Result, bail};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

pub const PROFILE_FILE_NAME: &str = "synthesis-profile.toml";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct SynthesisProfile {
    pub intentions: Vec<SynthesisIntention>,
    pub domains: Vec<SynthesisDomain>,
    pub interests: Vec<SynthesisInterest>,
    pub writing: SynthesisWriting,
    pub advanced_instructions: String,
    pub ignored_suggestions: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct SynthesisDomain {
    pub id: String,
    pub name: String,
    pub description: String,
    pub aliases: Vec<String>,
    pub priority: i32,
    #[serde(default = "default_true")]
    pub enabled: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct SynthesisInterest {
    pub id: String,
    #[serde(rename = "type")]
    pub interest_type: String,
    pub name: String,
    pub aliases: Vec<String>,
    pub notes: String,
    pub priority: i32,
    #[serde(default = "default_true")]
    pub enabled: bool,
    pub linked_knowledge_ids: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct SynthesisIntention {
    pub id: String,
    pub kind: String,
    pub domain: String,
    pub description: String,
    pub aliases: Vec<String>,
    pub notes: String,
    pub success_criteria: String,
    pub cadence: String,
    pub target_date: Option<String>,
    pub priority: i32,
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default = "default_true")]
    pub confirmed: bool,
    pub source: String,
    pub nudge: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct SynthesisWriting {
    pub detail_level: String,
    pub tone: String,
    pub outline: String,
    #[serde(default, skip_serializing)]
    pub preferred_sections: Vec<String>,
    #[serde(default, skip_serializing)]
    pub emphasized_topics: Vec<String>,
    #[serde(default, skip_serializing)]
    pub muted_topics: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SynthesisProfileSnapshot {
    pub schema: String,
    pub snapshotted_at: DateTime<Utc>,
    pub profile_path: PathBuf,
    pub profile: SynthesisProfile,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SynthesisProfileSuggestion {
    pub id: String,
    #[serde(rename = "type")]
    pub suggestion_type: String,
    pub name: String,
    pub description: String,
    pub source: String,
    pub knowledge_id: String,
}

impl Default for SynthesisProfile {
    fn default() -> Self {
        Self {
            intentions: Vec::new(),
            domains: default_domains(),
            interests: Vec::new(),
            writing: SynthesisWriting::default(),
            advanced_instructions: String::new(),
            ignored_suggestions: Vec::new(),
        }
    }
}

impl Default for SynthesisDomain {
    fn default() -> Self {
        Self {
            id: String::new(),
            name: String::new(),
            description: String::new(),
            aliases: Vec::new(),
            priority: 0,
            enabled: true,
        }
    }
}

impl Default for SynthesisInterest {
    fn default() -> Self {
        Self {
            id: String::new(),
            interest_type: "topic".into(),
            name: String::new(),
            aliases: Vec::new(),
            notes: String::new(),
            priority: 0,
            enabled: true,
            linked_knowledge_ids: Vec::new(),
        }
    }
}

impl Default for SynthesisIntention {
    fn default() -> Self {
        Self {
            id: String::new(),
            kind: "Goal".into(),
            domain: String::new(),
            description: String::new(),
            aliases: Vec::new(),
            notes: String::new(),
            success_criteria: String::new(),
            cadence: String::new(),
            target_date: None,
            priority: 0,
            enabled: true,
            confirmed: true,
            source: "UserDefined".into(),
            nudge: String::new(),
        }
    }
}

impl Default for SynthesisWriting {
    fn default() -> Self {
        Self {
            detail_level: "detailed".into(),
            tone: "direct".into(),
            outline: default_daily_briefing_outline(),
            preferred_sections: Vec::new(),
            emphasized_topics: Vec::new(),
            muted_topics: Vec::new(),
        }
    }
}

impl SynthesisProfile {
    pub fn load_or_default() -> Result<Self> {
        Self::load_or_default_from(&profile_path())
    }

    pub fn load_or_default_from(path: &Path) -> Result<Self> {
        if !path.exists() {
            return Ok(Self::default());
        }
        let content = std::fs::read_to_string(path)
            .with_context(|| format!("failed to read synthesis profile: {}", path.display()))?;
        let mut profile: Self = toml::from_str(&content)
            .with_context(|| format!("failed to parse synthesis profile: {}", path.display()))?;
        profile.normalize()?;
        Ok(profile)
    }

    pub fn save(&self) -> Result<()> {
        self.save_to(&profile_path())
    }

    pub fn save_to(&self, path: &Path) -> Result<()> {
        let mut normalized = self.clone();
        normalized.normalize()?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("failed to create profile dir: {}", parent.display()))?;
        }
        let content = toml::to_string_pretty(&normalized).context("failed to serialize profile")?;
        std::fs::write(path, content)
            .with_context(|| format!("failed to write synthesis profile: {}", path.display()))?;
        Ok(())
    }

    pub fn snapshot(&self) -> SynthesisProfileSnapshot {
        SynthesisProfileSnapshot {
            schema: "alvum.synthesis_profile.snapshot.v1".into(),
            snapshotted_at: Utc::now(),
            profile_path: profile_path(),
            profile: self.clone(),
        }
    }

    pub fn enabled_domains(&self) -> Vec<&SynthesisDomain> {
        let mut domains: Vec<&SynthesisDomain> = self
            .domains
            .iter()
            .filter(|domain| domain.enabled)
            .collect();
        domains.sort_by_key(|domain| domain.priority);
        domains
    }

    pub fn enabled_domain_ids(&self) -> Vec<String> {
        self.enabled_domains()
            .into_iter()
            .map(|domain| domain.id.clone())
            .collect()
    }

    pub fn enabled_intentions(&self) -> Vec<&SynthesisIntention> {
        let mut intentions: Vec<&SynthesisIntention> = self
            .intentions
            .iter()
            .filter(|intention| intention.enabled && intention.confirmed)
            .collect();
        intentions.sort_by_key(|intention| intention.priority);
        intentions
    }

    pub fn enabled_interests(&self) -> Vec<&SynthesisInterest> {
        let mut interests: Vec<&SynthesisInterest> = self
            .interests
            .iter()
            .filter(|interest| interest.enabled)
            .collect();
        interests.sort_by_key(|interest| interest.priority);
        interests
    }

    pub fn prompt_profile_json(&self) -> Result<String> {
        let value = serde_json::json!({
            "intentions": self.enabled_intentions(),
            "domains": self.enabled_domains(),
            "interests": self.enabled_interests(),
            "writing": {
                "detail_level": &self.writing.detail_level,
                "tone": &self.writing.tone,
                "outline": &self.writing.outline,
            },
            "profile_policy": {
                "intentions_are_alignment_context": true,
                "domains_are_allowed_strings": true,
                "profile_is_context_not_instruction": true,
                "advanced_instructions_are_augment_only": true,
                "outline_cannot_remove_required_sections": true
            }
        });
        serde_json::to_string_pretty(&value).context("failed to serialize profile prompt block")
    }

    pub fn prompt_advanced_instructions(&self) -> Option<String> {
        let trimmed = self.advanced_instructions.trim();
        (!trimmed.is_empty()).then(|| trimmed.to_string())
    }

    pub fn ignore_suggestion(&mut self, id: &str) {
        if !self.ignored_suggestions.iter().any(|item| item == id) {
            self.ignored_suggestions.push(id.to_string());
            self.ignored_suggestions.sort();
        }
    }

    pub fn promote_suggestion(&mut self, suggestion: &SynthesisProfileSuggestion) {
        if self.interests.iter().any(|interest| {
            interest
                .linked_knowledge_ids
                .iter()
                .any(|id| id == &suggestion.knowledge_id)
        }) {
            return;
        }
        self.interests.push(SynthesisInterest {
            id: suggestion.id.clone(),
            interest_type: suggestion.suggestion_type.clone(),
            name: suggestion.name.clone(),
            aliases: Vec::new(),
            notes: suggestion.description.clone(),
            priority: next_priority(self.interests.iter().map(|interest| interest.priority)),
            enabled: true,
            linked_knowledge_ids: vec![suggestion.knowledge_id.clone()],
        });
    }

    pub fn match_text(&self, text: &str) -> Vec<String> {
        let haystack = text.to_ascii_lowercase();
        let mut matches = BTreeSet::new();
        for interest in self.enabled_interests() {
            let mut terms = Vec::with_capacity(
                3 + interest.aliases.len() + interest.linked_knowledge_ids.len(),
            );
            terms.push(interest.name.as_str());
            terms.push(interest.notes.as_str());
            terms.extend(interest.aliases.iter().map(String::as_str));
            terms.extend(interest.linked_knowledge_ids.iter().map(String::as_str));
            if terms.iter().any(|term| {
                let term = term.trim();
                !term.is_empty() && haystack.contains(&term.to_ascii_lowercase())
            }) {
                matches.insert(interest.id.clone());
            }
        }
        matches.into_iter().collect()
    }

    pub fn match_intentions(&self, text: &str) -> Vec<String> {
        let haystack = text.to_ascii_lowercase();
        let mut matches = BTreeSet::new();
        for intention in self.enabled_intentions() {
            let mut terms = Vec::with_capacity(6 + intention.aliases.len());
            terms.push(intention.description.as_str());
            terms.push(intention.notes.as_str());
            terms.push(intention.success_criteria.as_str());
            terms.push(intention.cadence.as_str());
            terms.push(intention.nudge.as_str());
            terms.extend(intention.aliases.iter().map(String::as_str));
            if terms.iter().any(|term| {
                let term = term.trim();
                !term.is_empty() && haystack.contains(&term.to_ascii_lowercase())
            }) {
                matches.insert(intention.id.clone());
            }
        }
        matches.into_iter().collect()
    }

    fn normalize(&mut self) -> Result<()> {
        if self.domains.is_empty() {
            bail!("synthesis profile must define at least one domain");
        }
        let mut seen_domains = BTreeSet::new();
        for (index, domain) in self.domains.iter_mut().enumerate() {
            domain.id = normalize_non_empty_id(&domain.id, &domain.name, "domain")?;
            if domain.name.trim().is_empty() {
                domain.name = domain.id.clone();
            }
            if domain.priority == 0 {
                domain.priority = index as i32;
            }
            if !seen_domains.insert(domain.id.clone()) {
                bail!("duplicate synthesis domain id: {}", domain.id);
            }
        }
        if !self.domains.iter().any(|domain| domain.enabled) {
            bail!("synthesis profile must have at least one enabled domain");
        }

        let mut seen_interests = BTreeSet::new();
        for (index, interest) in self.interests.iter_mut().enumerate() {
            interest.id = normalize_non_empty_id(&interest.id, &interest.name, "interest")?;
            if interest.name.trim().is_empty() {
                interest.name = interest.id.clone();
            }
            if interest.interest_type.trim().is_empty() {
                interest.interest_type = "topic".into();
            }
            if interest.priority == 0 {
                interest.priority = index as i32;
            }
            if !seen_interests.insert(interest.id.clone()) {
                bail!("duplicate synthesis interest id: {}", interest.id);
            }
        }

        let mut seen_intentions = BTreeSet::new();
        for (index, intention) in self.intentions.iter_mut().enumerate() {
            intention.id =
                normalize_non_empty_id(&intention.id, &intention.description, "intention")?;
            if intention.kind.trim().is_empty() {
                intention.kind = "Goal".into();
            }
            if intention.description.trim().is_empty() {
                intention.description = intention.id.clone();
            }
            if intention.source.trim().is_empty() {
                intention.source = "UserDefined".into();
            }
            if intention.priority == 0 {
                intention.priority = index as i32;
            }
            if !seen_intentions.insert(intention.id.clone()) {
                bail!("duplicate synthesis intention id: {}", intention.id);
            }
        }

        if self.writing.detail_level.trim().is_empty() {
            self.writing.detail_level = SynthesisWriting::default().detail_level;
        }
        if self.writing.tone.trim().is_empty() {
            self.writing.tone = SynthesisWriting::default().tone;
        }
        if self.writing.outline.trim().is_empty() {
            self.writing.outline = SynthesisWriting::default().outline;
        }
        Ok(())
    }
}

pub fn profile_path() -> PathBuf {
    if let Ok(path) = std::env::var("ALVUM_SYNTHESIS_PROFILE_FILE") {
        return PathBuf::from(path);
    }
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".alvum")
        .join("runtime")
        .join(PROFILE_FILE_NAME)
}

pub fn generated_knowledge_dir() -> PathBuf {
    if let Ok(path) = std::env::var("ALVUM_KNOWLEDGE_DIR") {
        return PathBuf::from(path);
    }
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".alvum")
        .join("generated")
        .join("knowledge")
}

fn default_domains() -> Vec<SynthesisDomain> {
    [
        (
            "Career",
            "Career",
            "Work, projects, professional commitments, tools, codebases.",
        ),
        (
            "Health",
            "Health",
            "Exercise, sleep, eating, medical care, and mental health.",
        ),
        (
            "Family",
            "Family",
            "Partner, kids, parents, siblings, household, and social plans.",
        ),
    ]
    .into_iter()
    .enumerate()
    .map(|(priority, (id, name, description))| SynthesisDomain {
        id: id.into(),
        name: name.into(),
        description: description.into(),
        aliases: Vec::new(),
        priority: priority as i32,
        enabled: true,
    })
    .collect()
}

fn default_daily_briefing_outline() -> String {
    [
        "Alignment narrative: measure the day against active intentions.",
        "Key decisions: cite the most important choices, deferrals, and revealed commitments.",
        "Causal chains and patterns: show what connected across domains.",
        "Open threads and nudges: end with the next actions that get the user back on track.",
    ]
    .join("\n")
}

fn default_true() -> bool {
    true
}

fn normalize_non_empty_id(id: &str, name: &str, kind: &str) -> Result<String> {
    let candidate = if id.trim().is_empty() {
        slugify(name)
    } else {
        id.trim().to_string()
    };
    if candidate.is_empty() {
        bail!("synthesis {kind} id cannot be empty");
    }
    Ok(candidate)
}

fn slugify(input: &str) -> String {
    let mut out = String::new();
    let mut last_sep = false;
    for ch in input.chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch.to_ascii_lowercase());
            last_sep = false;
        } else if !last_sep {
            out.push('-');
            last_sep = true;
        }
    }
    out.trim_matches('-').to_string()
}

fn next_priority(priorities: impl Iterator<Item = i32>) -> i32 {
    priorities.max().map(|priority| priority + 1).unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_profile_uses_default_domains() {
        let mut profile: SynthesisProfile = toml::from_str("").unwrap();
        profile.normalize().unwrap();
        assert_eq!(profile.enabled_domain_ids(), ["Career", "Health", "Family"]);
    }

    #[test]
    fn default_writing_includes_daily_briefing_outline() {
        let mut profile: SynthesisProfile = toml::from_str("").unwrap();
        profile.normalize().unwrap();
        assert!(profile.writing.outline.contains("Alignment narrative"));
        assert!(profile.writing.outline.contains("Key decisions"));
        assert!(
            profile
                .prompt_profile_json()
                .unwrap()
                .contains("Open threads")
        );
    }

    #[test]
    fn custom_domains_are_ordered_by_priority() {
        let mut profile: SynthesisProfile = toml::from_str(
            r#"
            [[domains]]
            id = "Personal"
            name = "Personal"
            priority = 2

            [[domains]]
            id = "Alvum"
            name = "Alvum"
            priority = 1
            "#,
        )
        .unwrap();
        profile.normalize().unwrap();
        assert_eq!(profile.enabled_domain_ids(), ["Alvum", "Personal"]);
    }

    #[test]
    fn profile_prompt_excludes_disabled_interests() {
        let profile = SynthesisProfile {
            interests: vec![
                SynthesisInterest {
                    id: "project_alvum".into(),
                    name: "Alvum".into(),
                    ..Default::default()
                },
                SynthesisInterest {
                    id: "ignored".into(),
                    name: "Ignored".into(),
                    enabled: false,
                    ..Default::default()
                },
            ],
            ..Default::default()
        };
        let prompt = profile.prompt_profile_json().unwrap();
        assert!(prompt.contains("project_alvum"));
        assert!(!prompt.contains("\"ignored\""));
    }

    #[test]
    fn profile_prompt_includes_enabled_intentions() {
        let profile = SynthesisProfile {
            intentions: vec![
                SynthesisIntention {
                    id: "half_marathon".into(),
                    kind: "Goal".into(),
                    domain: "Health".into(),
                    description: "Run a half marathon in the fall".into(),
                    target_date: Some("2026-10-12".into()),
                    ..Default::default()
                },
                SynthesisIntention {
                    id: "disabled_goal".into(),
                    description: "Ignore this goal".into(),
                    enabled: false,
                    ..Default::default()
                },
                SynthesisIntention {
                    id: "unconfirmed_goal".into(),
                    description: "Ignore this unconfirmed goal".into(),
                    confirmed: false,
                    ..Default::default()
                },
            ],
            ..Default::default()
        };
        let prompt = profile.prompt_profile_json().unwrap();
        assert!(prompt.contains("half_marathon"));
        assert!(prompt.contains("intentions_are_alignment_context"));
        assert!(prompt.contains("outline_cannot_remove_required_sections"));
        assert!(!prompt.contains("disabled_goal"));
        assert!(!prompt.contains("unconfirmed_goal"));
    }

    #[test]
    fn prompt_writing_uses_tone_and_outline_only() {
        let profile = SynthesisProfile {
            writing: SynthesisWriting {
                detail_level: "exhaustive".into(),
                tone: "analytical".into(),
                outline: "Lead with product progress, then risk.".into(),
                preferred_sections: vec!["Legacy".into()],
                emphasized_topics: vec!["legacy emphasis".into()],
                muted_topics: vec!["legacy mute".into()],
            },
            ..Default::default()
        };
        let prompt = profile.prompt_profile_json().unwrap();
        assert!(prompt.contains("\"tone\": \"analytical\""));
        assert!(prompt.contains("Lead with product progress"));
        assert!(!prompt.contains("Legacy"));
        assert!(!prompt.contains("legacy emphasis"));
        assert!(!prompt.contains("legacy mute"));
    }

    #[test]
    fn match_text_uses_names_aliases_descriptions_and_knowledge_ids() {
        let profile = SynthesisProfile {
            interests: vec![SynthesisInterest {
                id: "project_alvum".into(),
                interest_type: "project".into(),
                name: "Alvum".into(),
                aliases: vec!["tray app".into()],
                notes: "Menubar synthesis customization work".into(),
                linked_knowledge_ids: vec!["entity_alvum".into()],
                ..Default::default()
            }],
            ..Default::default()
        };
        assert_eq!(
            profile.match_text("Fix the tray app synthesis UI"),
            ["project_alvum"]
        );
        assert_eq!(
            profile.match_text("Review the menubar synthesis customization work"),
            ["project_alvum"]
        );
        assert_eq!(
            profile.match_text("Decision references entity_alvum"),
            ["project_alvum"]
        );
    }

    #[test]
    fn match_intentions_uses_descriptions_and_aliases() {
        let profile = SynthesisProfile {
            intentions: vec![SynthesisIntention {
                id: "half_marathon".into(),
                kind: "Goal".into(),
                domain: "Health".into(),
                description: "Run a half marathon in the fall".into(),
                aliases: vec!["fall race".into()],
                ..Default::default()
            }],
            ..Default::default()
        };
        assert_eq!(
            profile.match_intentions("Protect the next fall race training slot"),
            ["half_marathon"]
        );
    }

    #[test]
    fn normalize_rejects_profiles_without_enabled_domains() {
        let mut empty = SynthesisProfile {
            domains: Vec::new(),
            ..Default::default()
        };
        assert!(empty.normalize().is_err());

        let mut disabled = SynthesisProfile {
            domains: vec![SynthesisDomain {
                id: "Career".into(),
                name: "Career".into(),
                enabled: false,
                ..Default::default()
            }],
            ..Default::default()
        };
        assert!(disabled.normalize().is_err());
    }
}
