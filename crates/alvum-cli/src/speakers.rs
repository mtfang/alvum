use alvum_core::synthesis_profile::SynthesisProfile;
use anyhow::Result;
use chrono::Utc;
use clap::Subcommand;

#[derive(Subcommand)]
pub(crate) enum Action {
    /// List locally known anonymous speakers.
    List {
        /// Emit machine-readable JSON.
        #[arg(long)]
        json: bool,
    },

    /// List playable voice evidence samples.
    Samples {
        /// Emit machine-readable JSON.
        #[arg(long)]
        json: bool,
    },

    /// Assign or update a user-confirmed speaker label.
    Rename {
        speaker_id: String,
        label: String,
        /// Emit machine-readable JSON.
        #[arg(long)]
        json: bool,
    },

    /// Link a voice cluster to a tracked person.
    Link {
        speaker_id: String,
        interest_id: String,
        /// Emit machine-readable JSON.
        #[arg(long)]
        json: bool,
    },

    /// Link one voice evidence sample to a tracked person.
    #[command(name = "link-sample")]
    LinkSample {
        sample_id: String,
        interest_id: String,
        /// Emit machine-readable JSON.
        #[arg(long)]
        json: bool,
    },

    /// Move one voice evidence sample to an existing cluster or `new`.
    #[command(name = "move-sample")]
    MoveSample {
        sample_id: String,
        cluster_id: String,
        /// Emit machine-readable JSON.
        #[arg(long)]
        json: bool,
    },

    /// Mark one voice evidence sample as not useful for identity review.
    #[command(name = "ignore-sample")]
    IgnoreSample {
        sample_id: String,
        /// Emit machine-readable JSON.
        #[arg(long)]
        json: bool,
    },

    /// Remove a tracked person link from one voice evidence sample.
    #[command(name = "unlink-sample")]
    UnlinkSample {
        sample_id: String,
        /// Emit machine-readable JSON.
        #[arg(long)]
        json: bool,
    },

    /// Split one mixed voice evidence sample into two independently editable samples.
    #[command(name = "split-sample")]
    SplitSample {
        sample_id: String,
        /// Split point in seconds, relative to the same media file as the sample offsets.
        #[arg(long)]
        at: f32,
        /// Transcript text for the left child sample.
        #[arg(long = "left-text")]
        left_text: String,
        /// Transcript text for the right child sample.
        #[arg(long = "right-text")]
        right_text: String,
        /// Emit machine-readable JSON.
        #[arg(long)]
        json: bool,
    },

    /// Split selected samples from a cluster into a new cluster.
    Split {
        cluster_id: String,
        /// Voice sample ids to split from the cluster.
        #[arg(long = "sample")]
        samples: Vec<String>,
        /// Emit machine-readable JSON.
        #[arg(long)]
        json: bool,
    },

    /// Recompute derived cluster summaries.
    Recluster {
        /// Emit machine-readable JSON.
        #[arg(long)]
        json: bool,
    },

    /// Remove a tracked person link from a voice cluster.
    Unlink {
        speaker_id: String,
        /// Emit machine-readable JSON.
        #[arg(long)]
        json: bool,
    },

    /// Merge one speaker profile into another.
    Merge {
        source_speaker_id: String,
        target_speaker_id: String,
        /// Emit machine-readable JSON.
        #[arg(long)]
        json: bool,
    },

    /// Forget a speaker profile and its fingerprints.
    Forget {
        speaker_id: String,
        /// Emit machine-readable JSON.
        #[arg(long)]
        json: bool,
    },

    /// Clear every local speaker profile.
    Reset {
        /// Emit machine-readable JSON.
        #[arg(long)]
        json: bool,
    },
}

#[derive(serde::Serialize)]
struct SpeakersReport {
    ok: bool,
    path: String,
    speakers: Vec<alvum_processor_audio::speaker_registry::SpeakerProfileSummary>,
    clusters: Vec<alvum_processor_audio::speaker_registry::SpeakerProfileSummary>,
    samples: Vec<alvum_processor_audio::speaker_registry::VoiceSampleSummary>,
    voice_models: Vec<alvum_processor_audio::speaker_registry::PersonVoiceModelSummary>,
    error: Option<String>,
}

pub(crate) fn run(action: Action) -> Result<()> {
    match action {
        Action::List { json } => {
            let (registry, profile) = load_registry_and_profile_with_migration()?;
            emit_report(&registry, json, None, Some(&profile))
        }
        Action::Samples { json } => {
            let (registry, profile) = load_registry_and_profile_with_migration()?;
            emit_report(&registry, json, None, Some(&profile))
        }
        Action::Rename {
            speaker_id,
            label,
            json,
        } => {
            let mut registry = load_registry()?;
            let mut profile = load_profile()?;
            let days = stale_days_for_voice_model_change(
                &registry,
                registry.days_for_cluster(&speaker_id)?,
            );
            registry.rename(&speaker_id, &label)?;
            registry.migrate_legacy_labels(&mut profile)?;
            registry.save()?;
            profile.save()?;
            mark_stale_days(&days, "voice_identity", None)?;
            emit_report(&registry, json, None, Some(&profile))
        }
        Action::Link {
            speaker_id,
            interest_id,
            json,
        } => {
            let mut registry = load_registry()?;
            let profile = load_profile()?;
            let days = stale_days_for_voice_model_change(
                &registry,
                registry.days_for_cluster(&speaker_id)?,
            );
            registry.link_to_interest(&speaker_id, &interest_id, &profile)?;
            registry.save()?;
            mark_stale_days(&days, "voice_identity", None)?;
            emit_report(&registry, json, None, Some(&profile))
        }
        Action::LinkSample {
            sample_id,
            interest_id,
            json,
        } => {
            let mut registry = load_registry()?;
            let profile = load_profile()?;
            let days = stale_days_for_voice_model_change(
                &registry,
                vec![registry.day_for_sample(&sample_id)?],
            );
            registry.link_sample_to_interest(&sample_id, &interest_id, &profile)?;
            registry.save()?;
            mark_stale_days(&days, "voice_identity", Some(&sample_id))?;
            emit_report(&registry, json, None, Some(&profile))
        }
        Action::MoveSample {
            sample_id,
            cluster_id,
            json,
        } => {
            let (mut registry, profile) = load_registry_and_profile_with_migration()?;
            let days = stale_days_for_voice_model_change(
                &registry,
                vec![registry.day_for_sample(&sample_id)?],
            );
            registry.move_sample_to_cluster(&sample_id, &cluster_id)?;
            registry.save()?;
            mark_stale_days(&days, "diarization_correction", Some(&sample_id))?;
            emit_report(&registry, json, None, Some(&profile))
        }
        Action::IgnoreSample { sample_id, json } => {
            let (mut registry, profile) = load_registry_and_profile_with_migration()?;
            let days = stale_days_for_voice_model_change(
                &registry,
                vec![registry.day_for_sample(&sample_id)?],
            );
            registry.ignore_sample(&sample_id)?;
            registry.save()?;
            mark_stale_days(&days, "diarization_correction", Some(&sample_id))?;
            emit_report(&registry, json, None, Some(&profile))
        }
        Action::UnlinkSample { sample_id, json } => {
            let (mut registry, profile) = load_registry_and_profile_with_migration()?;
            let days = stale_days_for_voice_model_change(
                &registry,
                vec![registry.day_for_sample(&sample_id)?],
            );
            registry.unlink_sample_interest(&sample_id)?;
            registry.save()?;
            mark_stale_days(&days, "voice_identity", Some(&sample_id))?;
            emit_report(&registry, json, None, Some(&profile))
        }
        Action::SplitSample {
            sample_id,
            at,
            left_text,
            right_text,
            json,
        } => {
            let (mut registry, profile) = load_registry_and_profile_with_migration()?;
            let days = stale_days_for_voice_model_change(
                &registry,
                vec![registry.day_for_sample(&sample_id)?],
            );
            registry.split_sample_at(&sample_id, at, &left_text, &right_text)?;
            registry.save()?;
            mark_stale_days(&days, "diarization_correction", Some(&sample_id))?;
            emit_report(&registry, json, None, Some(&profile))
        }
        Action::Split {
            cluster_id,
            samples,
            json,
        } => {
            let (mut registry, profile) = load_registry_and_profile_with_migration()?;
            let mut days = Vec::new();
            for sample_id in &samples {
                days.push(registry.day_for_sample(sample_id)?);
            }
            let days = stale_days_for_voice_model_change(&registry, days);
            registry.split_samples_to_new_cluster(&cluster_id, &samples)?;
            registry.save()?;
            mark_stale_days(
                &days,
                "diarization_correction",
                samples.first().map(String::as_str),
            )?;
            emit_report(&registry, json, None, Some(&profile))
        }
        Action::Recluster { json } => {
            let (registry, profile) = load_registry_and_profile_with_migration()?;
            emit_report(&registry, json, None, Some(&profile))
        }
        Action::Unlink { speaker_id, json } => {
            let mut registry = load_registry()?;
            let profile = load_profile()?;
            let days = stale_days_for_voice_model_change(
                &registry,
                registry.days_for_cluster(&speaker_id)?,
            );
            registry.unlink_interest(&speaker_id)?;
            registry.save()?;
            mark_stale_days(&days, "voice_identity", None)?;
            emit_report(&registry, json, None, Some(&profile))
        }
        Action::Merge {
            source_speaker_id,
            target_speaker_id,
            json,
        } => {
            let (mut registry, profile) = load_registry_and_profile_with_migration()?;
            let mut days = registry.days_for_cluster(&source_speaker_id)?;
            days.extend(registry.days_for_cluster(&target_speaker_id)?);
            let days = stale_days_for_voice_model_change(&registry, days);
            registry.merge(&source_speaker_id, &target_speaker_id)?;
            registry.save()?;
            mark_stale_days(&days, "voice_identity", None)?;
            emit_report(&registry, json, None, Some(&profile))
        }
        Action::Forget { speaker_id, json } => {
            let (mut registry, profile) = load_registry_and_profile_with_migration()?;
            let days = stale_days_for_voice_model_change(
                &registry,
                registry.days_for_cluster(&speaker_id)?,
            );
            registry.forget(&speaker_id)?;
            registry.save()?;
            mark_stale_days(&days, "voice_identity", None)?;
            emit_report(&registry, json, None, Some(&profile))
        }
        Action::Reset { json } => {
            let (mut registry, profile) = load_registry_and_profile_with_migration()?;
            let days = stale_days_for_voice_model_change(&registry, registry.all_sample_days());
            registry.reset();
            registry.save()?;
            mark_stale_days(&days, "voice_identity", None)?;
            emit_report(&registry, json, None, Some(&profile))
        }
    }
}

fn stale_days_for_voice_model_change(
    registry: &alvum_processor_audio::speaker_registry::SpeakerRegistry,
    fallback_days: Vec<String>,
) -> Vec<String> {
    let all_days = registry.all_sample_days();
    if all_days.is_empty() {
        fallback_days
    } else {
        all_days
    }
}

fn mark_stale_days(days: &[String], kind: &str, sample_id: Option<&str>) -> Result<()> {
    let base = dirs::home_dir()
        .unwrap_or_else(|| std::path::PathBuf::from("."))
        .join(".alvum")
        .join("generated")
        .join("briefings");
    let mut unique_days = days.to_vec();
    unique_days.sort();
    unique_days.dedup();
    for date in unique_days {
        if !is_date_stamp(&date) {
            continue;
        }
        let dir = base.join(&date);
        std::fs::create_dir_all(&dir)?;
        let marker = serde_json::json!({
            "date": date,
            "kind": kind,
            "sample_id": sample_id,
            "marked_at": Utc::now().to_rfc3339(),
            "reason": "voice labels changed; resynthesize this day to refresh speaker attribution"
        });
        std::fs::write(
            dir.join("voice.stale.json"),
            serde_json::to_string_pretty(&marker)?,
        )?;
    }
    Ok(())
}

fn is_date_stamp(value: &str) -> bool {
    value.len() == 10
        && value.as_bytes()[4] == b'-'
        && value.as_bytes()[7] == b'-'
        && value
            .bytes()
            .enumerate()
            .all(|(idx, byte)| idx == 4 || idx == 7 || byte.is_ascii_digit())
}

fn load_registry() -> Result<alvum_processor_audio::speaker_registry::SpeakerRegistry> {
    alvum_processor_audio::speaker_registry::SpeakerRegistry::load_or_default(
        &alvum_processor_audio::speaker_registry::SpeakerRegistry::default_path(),
    )
}

fn load_profile() -> Result<SynthesisProfile> {
    SynthesisProfile::load_or_default()
}

fn load_registry_and_profile_with_migration() -> Result<(
    alvum_processor_audio::speaker_registry::SpeakerRegistry,
    SynthesisProfile,
)> {
    let mut registry = load_registry()?;
    let mut profile = load_profile()?;
    if registry.migrate_legacy_labels(&mut profile)? {
        registry.save()?;
        profile.save()?;
    }
    Ok((registry, profile))
}

fn emit_report(
    registry: &alvum_processor_audio::speaker_registry::SpeakerRegistry,
    json: bool,
    error: Option<String>,
    profile: Option<&SynthesisProfile>,
) -> Result<()> {
    let report = SpeakersReport {
        ok: error.is_none(),
        path: alvum_processor_audio::speaker_registry::SpeakerRegistry::default_path()
            .display()
            .to_string(),
        speakers: registry.speakers_with_profile(profile),
        clusters: registry.speakers_with_profile(profile),
        samples: registry.voice_samples_with_profile(profile),
        voice_models: registry.person_voice_model_summaries(profile),
        error,
    };
    if json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else if report.speakers.is_empty() {
        println!("No speakers recorded yet.");
    } else {
        for speaker in report.speakers {
            let label = speaker
                .linked_interest
                .as_ref()
                .map(|interest| interest.name.clone())
                .or(speaker.label)
                .unwrap_or_else(|| "Unlinked".into());
            println!(
                "{}\t{}\t{} fingerprint{}",
                speaker.speaker_id,
                label,
                speaker.fingerprint_count,
                if speaker.fingerprint_count == 1 {
                    ""
                } else {
                    "s"
                }
            );
        }
    }
    Ok(())
}
