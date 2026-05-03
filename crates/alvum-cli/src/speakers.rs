use alvum_core::synthesis_profile::SynthesisProfile;
use anyhow::Result;
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
            registry.rename(&speaker_id, &label)?;
            registry.migrate_legacy_labels(&mut profile)?;
            registry.save()?;
            profile.save()?;
            emit_report(&registry, json, None, Some(&profile))
        }
        Action::Link {
            speaker_id,
            interest_id,
            json,
        } => {
            let mut registry = load_registry()?;
            let profile = load_profile()?;
            registry.link_to_interest(&speaker_id, &interest_id, &profile)?;
            registry.save()?;
            emit_report(&registry, json, None, Some(&profile))
        }
        Action::LinkSample {
            sample_id,
            interest_id,
            json,
        } => {
            let mut registry = load_registry()?;
            let profile = load_profile()?;
            registry.link_sample_to_interest(&sample_id, &interest_id, &profile)?;
            registry.save()?;
            emit_report(&registry, json, None, Some(&profile))
        }
        Action::MoveSample {
            sample_id,
            cluster_id,
            json,
        } => {
            let (mut registry, profile) = load_registry_and_profile_with_migration()?;
            registry.move_sample_to_cluster(&sample_id, &cluster_id)?;
            registry.save()?;
            emit_report(&registry, json, None, Some(&profile))
        }
        Action::IgnoreSample { sample_id, json } => {
            let (mut registry, profile) = load_registry_and_profile_with_migration()?;
            registry.ignore_sample(&sample_id)?;
            registry.save()?;
            emit_report(&registry, json, None, Some(&profile))
        }
        Action::Split {
            cluster_id,
            samples,
            json,
        } => {
            let (mut registry, profile) = load_registry_and_profile_with_migration()?;
            registry.split_samples_to_new_cluster(&cluster_id, &samples)?;
            registry.save()?;
            emit_report(&registry, json, None, Some(&profile))
        }
        Action::Recluster { json } => {
            let (registry, profile) = load_registry_and_profile_with_migration()?;
            emit_report(&registry, json, None, Some(&profile))
        }
        Action::Unlink { speaker_id, json } => {
            let mut registry = load_registry()?;
            let profile = load_profile()?;
            registry.unlink_interest(&speaker_id)?;
            registry.save()?;
            emit_report(&registry, json, None, Some(&profile))
        }
        Action::Merge {
            source_speaker_id,
            target_speaker_id,
            json,
        } => {
            let (mut registry, profile) = load_registry_and_profile_with_migration()?;
            registry.merge(&source_speaker_id, &target_speaker_id)?;
            registry.save()?;
            emit_report(&registry, json, None, Some(&profile))
        }
        Action::Forget { speaker_id, json } => {
            let (mut registry, profile) = load_registry_and_profile_with_migration()?;
            registry.forget(&speaker_id)?;
            registry.save()?;
            emit_report(&registry, json, None, Some(&profile))
        }
        Action::Reset { json } => {
            let (mut registry, profile) = load_registry_and_profile_with_migration()?;
            registry.reset();
            registry.save()?;
            emit_report(&registry, json, None, Some(&profile))
        }
    }
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
