use anyhow::{Context, Result, bail};
use clap::Subcommand;
use std::collections::BTreeSet;

#[derive(Subcommand)]
pub(crate) enum Action {
    /// Show the effective synthesis profile.
    Show {
        /// Emit machine-readable JSON instead of TOML.
        #[arg(long)]
        json: bool,
    },

    /// Set a simple profile value.
    Set {
        /// Dotted key path, e.g. advanced_instructions, writing.detail_level, writing.tone, or writing.outline.
        key: String,
        /// Value to assign.
        value: String,
    },

    /// Replace the profile from a JSON payload.
    Save {
        /// JSON-encoded SynthesisProfile.
        #[arg(long)]
        json: String,
    },

    /// List detected knowledge suggestions that can be tracked.
    Suggestions {
        /// Emit machine-readable JSON.
        #[arg(long)]
        json: bool,
    },

    /// Promote a detected suggestion into tracked profile interests.
    Promote { id: String },

    /// Ignore a detected suggestion.
    Ignore { id: String },
}

pub(crate) fn run(action: Action) -> Result<()> {
    use alvum_core::synthesis_profile::{SynthesisProfile, generated_knowledge_dir, profile_path};

    match action {
        Action::Show { json } => {
            let profile = SynthesisProfile::load_or_default()?;
            if json {
                println!("{}", serde_json::to_string_pretty(&profile)?);
            } else {
                println!("{}", toml::to_string_pretty(&profile)?);
            }
            Ok(())
        }
        Action::Set { key, value } => {
            let mut profile = SynthesisProfile::load_or_default()?;
            set_profile_value(&mut profile, &key, &value)?;
            profile.save()?;
            println!("Saved synthesis profile: {}", profile_path().display());
            Ok(())
        }
        Action::Save { json } => {
            let profile: SynthesisProfile =
                serde_json::from_str(&json).context("failed to parse profile JSON")?;
            profile.save()?;
            println!("Saved synthesis profile: {}", profile_path().display());
            Ok(())
        }
        Action::Suggestions { json } => {
            let profile = SynthesisProfile::load_or_default()?;
            let suggestions = profile_suggestions(&profile)?;
            if json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&serde_json::json!({
                        "knowledge_dir": generated_knowledge_dir(),
                        "suggestions": suggestions,
                    }))?
                );
            } else if suggestions.is_empty() {
                println!("No untracked profile suggestions.");
            } else {
                for suggestion in suggestions {
                    println!(
                        "{} [{}] {} — {}",
                        suggestion.id,
                        suggestion.suggestion_type,
                        suggestion.name,
                        suggestion.description
                    );
                }
            }
            Ok(())
        }
        Action::Promote { id } => {
            let mut profile = SynthesisProfile::load_or_default()?;
            let suggestions = profile_suggestions(&profile)?;
            let suggestion = suggestions
                .into_iter()
                .find(|suggestion| suggestion.id == id)
                .with_context(|| format!("unknown profile suggestion: {id}"))?;
            profile.promote_suggestion(&suggestion);
            profile.save()?;
            println!("Tracked {}", suggestion.name);
            Ok(())
        }
        Action::Ignore { id } => {
            let mut profile = SynthesisProfile::load_or_default()?;
            profile.ignore_suggestion(&id);
            profile.save()?;
            println!("Ignored {id}");
            Ok(())
        }
    }
}

fn set_profile_value(
    profile: &mut alvum_core::synthesis_profile::SynthesisProfile,
    key: &str,
    value: &str,
) -> Result<()> {
    match key {
        "advanced_instructions" => {
            profile.advanced_instructions = value.to_string();
            Ok(())
        }
        "writing.detail_level" => {
            profile.writing.detail_level = value.to_string();
            Ok(())
        }
        "writing.tone" => {
            profile.writing.tone = value.to_string();
            Ok(())
        }
        "writing.outline" => {
            profile.writing.outline = value.to_string();
            Ok(())
        }
        _ if key.starts_with("intentions.") => set_intention_profile_value(profile, key, value),
        _ if key.starts_with("domains.") => set_domain_profile_value(profile, key, value),
        _ if key.starts_with("interests.") => set_interest_profile_value(profile, key, value),
        _ => bail!("unsupported profile key: {key}"),
    }
}

fn set_intention_profile_value(
    profile: &mut alvum_core::synthesis_profile::SynthesisProfile,
    key: &str,
    value: &str,
) -> Result<()> {
    let parts: Vec<&str> = key.split('.').collect();
    if parts.len() != 3 {
        bail!("intention profile keys must be intentions.<id>.<field>");
    }
    let Some(intention) = profile
        .intentions
        .iter_mut()
        .find(|intention| intention.id == parts[1])
    else {
        bail!("unknown synthesis intention: {}", parts[1]);
    };
    match parts[2] {
        "enabled" => intention.enabled = parse_bool(value)?,
        "confirmed" => intention.confirmed = parse_bool(value)?,
        "kind" => intention.kind = value.to_string(),
        "domain" => intention.domain = value.to_string(),
        "description" => intention.description = value.to_string(),
        "notes" => intention.notes = value.to_string(),
        "success_criteria" => intention.success_criteria = value.to_string(),
        "cadence" => intention.cadence = value.to_string(),
        "target_date" => {
            intention.target_date = (!value.trim().is_empty()).then(|| value.to_string())
        }
        "aliases" => intention.aliases = parse_csv(value),
        "priority" => intention.priority = value.parse().context("priority must be an integer")?,
        "source" => intention.source = value.to_string(),
        "nudge" => intention.nudge = value.to_string(),
        _ => bail!("unsupported intention field: {}", parts[2]),
    }
    Ok(())
}

fn set_domain_profile_value(
    profile: &mut alvum_core::synthesis_profile::SynthesisProfile,
    key: &str,
    value: &str,
) -> Result<()> {
    let parts: Vec<&str> = key.split('.').collect();
    if parts.len() != 3 {
        bail!("domain profile keys must be domains.<id>.<field>");
    }
    let Some(domain) = profile
        .domains
        .iter_mut()
        .find(|domain| domain.id == parts[1])
    else {
        bail!("unknown synthesis domain: {}", parts[1]);
    };
    match parts[2] {
        "enabled" => domain.enabled = parse_bool(value)?,
        "name" => domain.name = value.to_string(),
        "description" => domain.description = value.to_string(),
        "aliases" => domain.aliases = parse_csv(value),
        "priority" => domain.priority = value.parse().context("priority must be an integer")?,
        _ => bail!("unsupported domain field: {}", parts[2]),
    }
    Ok(())
}

fn set_interest_profile_value(
    profile: &mut alvum_core::synthesis_profile::SynthesisProfile,
    key: &str,
    value: &str,
) -> Result<()> {
    let parts: Vec<&str> = key.split('.').collect();
    if parts.len() != 3 {
        bail!("interest profile keys must be interests.<id>.<field>");
    }
    let Some(interest) = profile
        .interests
        .iter_mut()
        .find(|interest| interest.id == parts[1])
    else {
        bail!("unknown synthesis interest: {}", parts[1]);
    };
    match parts[2] {
        "enabled" => interest.enabled = parse_bool(value)?,
        "name" => interest.name = value.to_string(),
        "type" => interest.interest_type = value.to_string(),
        "notes" => interest.notes = value.to_string(),
        "aliases" => interest.aliases = parse_csv(value),
        "priority" => interest.priority = value.parse().context("priority must be an integer")?,
        _ => bail!("unsupported interest field: {}", parts[2]),
    }
    Ok(())
}

fn parse_bool(value: &str) -> Result<bool> {
    match value {
        "true" => Ok(true),
        "false" => Ok(false),
        _ => bail!("expected true or false"),
    }
}

fn parse_csv(value: &str) -> Vec<String> {
    value
        .split(',')
        .map(str::trim)
        .filter(|item| !item.is_empty())
        .map(str::to_string)
        .collect()
}

fn profile_suggestions(
    profile: &alvum_core::synthesis_profile::SynthesisProfile,
) -> Result<Vec<alvum_core::synthesis_profile::SynthesisProfileSuggestion>> {
    use alvum_core::synthesis_profile::{SynthesisProfileSuggestion, generated_knowledge_dir};

    let corpus = alvum_knowledge::store::load(&generated_knowledge_dir()).unwrap_or_default();
    let ignored: BTreeSet<&str> = profile
        .ignored_suggestions
        .iter()
        .map(String::as_str)
        .collect();
    let tracked_knowledge: BTreeSet<&str> = profile
        .interests
        .iter()
        .flat_map(|interest| interest.linked_knowledge_ids.iter().map(String::as_str))
        .collect();

    let mut suggestions = Vec::new();
    for entity in corpus.entities {
        if entity.id.is_empty() || tracked_knowledge.contains(entity.id.as_str()) {
            continue;
        }
        if !entity_is_recurring(&entity) {
            continue;
        }
        let id = profile_suggestion_id("entity", &entity.id);
        if ignored.contains(id.as_str()) {
            continue;
        }
        suggestions.push(SynthesisProfileSuggestion {
            id,
            suggestion_type: normalize_interest_type(&entity.entity_type),
            name: entity.name,
            description: entity.description,
            source: "knowledge.entity".into(),
            knowledge_id: entity.id,
        });
    }
    for pattern in corpus.patterns {
        if pattern.id.is_empty() || tracked_knowledge.contains(pattern.id.as_str()) {
            continue;
        }
        if !pattern_is_recurring(&pattern) {
            continue;
        }
        let id = profile_suggestion_id("pattern", &pattern.id);
        if ignored.contains(id.as_str()) {
            continue;
        }
        suggestions.push(SynthesisProfileSuggestion {
            id,
            suggestion_type: "topic".into(),
            name: pattern.id.replace('_', " "),
            description: pattern.description,
            source: "knowledge.pattern".into(),
            knowledge_id: pattern.id,
        });
    }
    suggestions.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(suggestions)
}

fn entity_is_recurring(entity: &alvum_knowledge::types::Entity) -> bool {
    entity.first_seen != entity.last_seen
}

fn pattern_is_recurring(pattern: &alvum_knowledge::types::Pattern) -> bool {
    pattern.occurrences > 1 || pattern.first_seen != pattern.last_seen
}

fn profile_suggestion_id(kind: &str, knowledge_id: &str) -> String {
    let slug = knowledge_id
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() {
                ch.to_ascii_lowercase()
            } else {
                '_'
            }
        })
        .collect::<String>()
        .trim_matches('_')
        .to_string();
    format!("{kind}_{slug}")
}

fn normalize_interest_type(entity_type: &str) -> String {
    match entity_type {
        "person" | "place" | "project" | "organization" | "tool" | "topic" => {
            entity_type.to_string()
        }
        _ => "topic".into(),
    }
}
