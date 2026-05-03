use alvum_core::data_ref::DataRef;
use alvum_core::synthesis_profile::{SynthesisInterest, SynthesisProfile};
use alvum_processor_audio::fingerprint::AudioFingerprint;
use alvum_processor_audio::openai::OpenAiDiarizedTranscript;
use alvum_processor_audio::pyannote::{PyannoteDiarization, align_segments_to_diarization};
use alvum_processor_audio::speaker_registry::{SpeakerRegistry, SpeakerSample};
use alvum_processor_audio::transcriber::Segment;
use alvum_processor_audio::voice::{AudioIntelligenceArtifact, FingerprintRef, SpeakerTurn};

fn sample_ref() -> DataRef {
    DataRef {
        ts: "2026-04-30T08:09:03Z".parse().unwrap(),
        source: "audio-mic".into(),
        producer: "alvum.audio/audio-mic".into(),
        schema: "alvum.audio.wav.v1".into(),
        path: "/tmp/alvum-audio.wav".into(),
        mime: "audio/wav".into(),
        metadata: None,
    }
}

#[test]
fn audio_fingerprint_is_stable_and_named() {
    let samples = [0.0_f32, 0.25, -0.25, 0.5, -0.5, 0.0, 0.125, -0.125];

    let first = AudioFingerprint::from_samples(&samples, 16_000);
    let second = AudioFingerprint::from_samples(&samples, 16_000);

    assert_eq!(first.model, "alvum.acoustic-v1");
    assert_eq!(first.vector, second.vector);
    assert!(!first.digest.is_empty());
}

#[test]
fn speaker_registry_confirms_labels_without_changing_ids() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("speakers.json");
    let fingerprint = AudioFingerprint::from_samples(&[0.0_f32, 0.3, -0.2, 0.1], 16_000);
    let mut registry = SpeakerRegistry::load_or_default(&path).unwrap();

    let speaker_id = registry.resolve_or_create(&fingerprint);
    registry.confirm_label(&speaker_id, "Michael").unwrap();
    registry.save().unwrap();

    let reloaded = SpeakerRegistry::load_or_default(&path).unwrap();
    let matched = reloaded.resolve_existing(&fingerprint).unwrap();
    assert_eq!(matched.speaker_id, speaker_id);
    assert_eq!(matched.label.as_deref(), Some("Michael"));
}

#[test]
fn speaker_registry_lists_merges_renames_and_forgets_speakers() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("speakers.json");
    let first = AudioFingerprint::from_samples(&[0.0_f32, 0.2, -0.1, 0.15], 16_000);
    let second = AudioFingerprint::from_samples(&[0.0_f32, -0.4, 0.4, -0.2], 16_000);
    let mut registry = SpeakerRegistry::load_or_default(&path).unwrap();

    let keep_id = registry.resolve_or_create(&first);
    let merge_id = registry.resolve_or_create(&second);
    registry.rename(&keep_id, "Michael").unwrap();
    registry
        .record_sample(
            &keep_id,
            SpeakerSample {
                text: "Status update.".into(),
                source: "audio-mic".into(),
                ts: "2026-04-30T08:09:03Z".into(),
                start_secs: 0.0,
                end_secs: 1.0,
                media_path: None,
                mime: None,
            },
        )
        .unwrap();
    registry.merge(&merge_id, &keep_id).unwrap();

    let speakers = registry.speakers();
    assert_eq!(speakers.len(), 1);
    assert_eq!(speakers[0].speaker_id, keep_id);
    assert_eq!(speakers[0].label.as_deref(), Some("Michael"));
    assert_eq!(speakers[0].fingerprint_count, 2);
    assert_eq!(speakers[0].samples[0].text, "Status update.");

    registry.forget(&keep_id).unwrap();
    assert!(registry.speakers().is_empty());
    let recreated = registry.resolve_or_create(&first);
    assert!(!recreated.is_empty());
    registry.reset();
    assert!(registry.speakers().is_empty());
}

#[test]
fn speaker_registry_does_not_match_incompatible_fingerprints() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("speakers.json");
    let mut registry = SpeakerRegistry::load_or_default(&path).unwrap();
    let baseline = AudioFingerprint::from_vector("model-a", 16_000, vec![1.0, 0.0, 0.0]);
    let same_digest_different_model =
        AudioFingerprint::from_vector("model-b", 16_000, vec![1.0, 0.0, 0.0]);
    let truncated_same_model = AudioFingerprint::from_vector("model-a", 16_000, vec![1.0, 0.0]);
    let different_rate = AudioFingerprint::from_vector("model-a", 8_000, vec![1.0, 0.0, 0.0]);

    registry.resolve_or_create(&baseline);

    assert!(
        registry
            .resolve_existing(&same_digest_different_model)
            .is_none()
    );
    assert!(registry.resolve_existing(&truncated_same_model).is_none());
    assert!(registry.resolve_existing(&different_rate).is_none());
}

#[test]
fn speaker_registry_links_only_to_tracked_people() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("speakers.json");
    let fingerprint = AudioFingerprint::from_samples(&[0.0_f32, 0.3, -0.2, 0.1], 16_000);
    let mut registry = SpeakerRegistry::load_or_default(&path).unwrap();
    let mut profile = SynthesisProfile::default();
    profile.interests = vec![
        SynthesisInterest {
            id: "person_michael".into(),
            interest_type: "person".into(),
            name: "Michael".into(),
            ..SynthesisInterest::default()
        },
        SynthesisInterest {
            id: "project_alvum".into(),
            interest_type: "project".into(),
            name: "Alvum".into(),
            ..SynthesisInterest::default()
        },
    ];

    let speaker_id = registry.resolve_or_create(&fingerprint);
    registry
        .link_to_interest(&speaker_id, "person_michael", &profile)
        .unwrap();
    assert_eq!(
        registry.speakers_with_profile(Some(&profile))[0]
            .linked_interest
            .as_ref()
            .unwrap()
            .name,
        "Michael"
    );

    let err = registry
        .link_to_interest(&speaker_id, "project_alvum", &profile)
        .unwrap_err()
        .to_string();
    assert!(err.contains("person"));
}

#[test]
fn speaker_registry_suggests_people_from_supported_voice_fingerprint_distribution() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("speakers.json");
    let linked_fingerprint =
        AudioFingerprint::from_vector("pyannote.embedding", 16_000, vec![1.0, 0.0]);
    let new_fingerprint =
        AudioFingerprint::from_vector("pyannote.embedding", 16_000, vec![0.86, 0.14]);
    let mut registry = SpeakerRegistry::load_or_default(&path).unwrap();
    let mut profile = SynthesisProfile::default();
    profile.interests = vec![SynthesisInterest {
        id: "person_michael".into(),
        interest_type: "person".into(),
        name: "Michael".into(),
        ..SynthesisInterest::default()
    }];

    let linked_cluster = registry.resolve_or_create(&linked_fingerprint);
    registry
        .record_sample_with_fingerprint(
            &linked_cluster,
            Some(linked_fingerprint),
            SpeakerSample {
                text: "Known speaker sample.".into(),
                source: "audio-mic".into(),
                ts: "2026-05-02T09:00:00Z".into(),
                start_secs: 0.0,
                end_secs: 1.0,
                media_path: None,
                mime: None,
            },
            "pyannote",
        )
        .unwrap();
    let seed_sample_id = registry
        .voice_samples_with_profile(None)
        .into_iter()
        .find(|sample| sample.text == "Known speaker sample.")
        .unwrap()
        .sample_id;
    registry
        .link_sample_to_interest(&seed_sample_id, "person_michael", &profile)
        .unwrap();
    for (index, vector) in [vec![0.99, 0.01], vec![0.985, 0.015], vec![0.995, -0.005]]
        .into_iter()
        .enumerate()
    {
        let fingerprint = AudioFingerprint::from_vector("pyannote.embedding", 16_000, vector);
        let cluster = registry.resolve_or_create(&fingerprint);
        registry
            .record_sample_with_fingerprint(
                &cluster,
                Some(fingerprint),
                SpeakerSample {
                    text: format!("Additional confirmed sample {index}."),
                    source: "audio-mic".into(),
                    ts: format!("2026-05-02T09:0{}:00Z", index + 2),
                    start_secs: 0.0,
                    end_secs: 1.0,
                    media_path: None,
                    mime: None,
                },
                "pyannote",
            )
            .unwrap();
        let text = format!("Additional confirmed sample {index}.");
        let sample_id = registry
            .voice_samples_with_profile(None)
            .into_iter()
            .find(|sample| sample.text == text)
            .unwrap()
            .sample_id;
        registry
            .link_sample_to_interest(&sample_id, "person_michael", &profile)
            .unwrap();
    }

    let new_cluster = registry.resolve_or_create(&new_fingerprint);
    assert_ne!(new_cluster, linked_cluster);
    registry
        .record_sample_with_fingerprint(
            &new_cluster,
            Some(new_fingerprint),
            SpeakerSample {
                text: "Fresh speaker sample.".into(),
                source: "audio-mic".into(),
                ts: "2026-05-02T09:01:00Z".into(),
                start_secs: 0.0,
                end_secs: 1.0,
                media_path: None,
                mime: None,
            },
            "pyannote",
        )
        .unwrap();

    let sample = registry
        .voice_samples_with_profile(Some(&profile))
        .into_iter()
        .find(|sample| sample.cluster_id == new_cluster)
        .unwrap();
    assert_eq!(sample.person_candidates[0].id, "person_michael");
    assert!(sample.person_candidates[0].score >= 0.70);
    assert!(sample.person_candidates[0].reason.contains("fingerprint"));

    let cluster = registry
        .speakers_with_profile(Some(&profile))
        .into_iter()
        .find(|speaker| speaker.speaker_id == new_cluster)
        .unwrap();
    assert_eq!(cluster.person_candidates[0].id, "person_michael");
    assert!(cluster.person_candidates[0].score >= 0.70);
    assert!(cluster.person_candidates[0].reason.contains("fingerprint"));
}

#[test]
fn speaker_registry_does_not_suggest_people_from_one_voice_fingerprint() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("speakers.json");
    let linked_fingerprint =
        AudioFingerprint::from_vector("pyannote.embedding", 16_000, vec![1.0, 0.0]);
    let query_fingerprint =
        AudioFingerprint::from_vector("pyannote.embedding", 16_000, vec![0.9, 0.1]);
    let mut registry = SpeakerRegistry::load_or_default(&path).unwrap();
    let mut profile = SynthesisProfile::default();
    profile.interests = vec![SynthesisInterest {
        id: "person_michael".into(),
        interest_type: "person".into(),
        name: "Michael".into(),
        ..SynthesisInterest::default()
    }];

    let linked_cluster = registry.resolve_or_create(&linked_fingerprint);
    registry
        .record_sample_with_fingerprint(
            &linked_cluster,
            Some(linked_fingerprint),
            SpeakerSample {
                text: "Only one confirmed sample.".into(),
                source: "audio-mic".into(),
                ts: "2026-05-02T09:00:00Z".into(),
                start_secs: 0.0,
                end_secs: 1.0,
                media_path: None,
                mime: None,
            },
            "pyannote",
        )
        .unwrap();
    registry
        .link_to_interest(&linked_cluster, "person_michael", &profile)
        .unwrap();

    let query_cluster = registry.resolve_or_create(&query_fingerprint);
    registry
        .record_sample_with_fingerprint(
            &query_cluster,
            Some(query_fingerprint),
            SpeakerSample {
                text: "Unassigned query sample.".into(),
                source: "audio-mic".into(),
                ts: "2026-05-02T09:01:00Z".into(),
                start_secs: 0.0,
                end_secs: 1.0,
                media_path: None,
                mime: None,
            },
            "pyannote",
        )
        .unwrap();

    let sample = registry
        .voice_samples_with_profile(Some(&profile))
        .into_iter()
        .find(|sample| sample.cluster_id == query_cluster)
        .unwrap();
    assert!(sample.person_candidates.is_empty());
}

#[test]
fn speaker_registry_trains_person_prediction_only_from_verified_samples() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("speakers.json");
    let mut registry = SpeakerRegistry::load_or_default(&path).unwrap();
    let profile = voice_test_profile();
    let linked_fingerprint =
        AudioFingerprint::from_vector("pyannote.embedding", 16_000, vec![1.0, 0.0]);

    let linked_cluster = registry.resolve_or_create(&linked_fingerprint);
    registry
        .record_sample_with_fingerprint(
            &linked_cluster,
            Some(linked_fingerprint),
            voice_test_sample(
                "Cluster-linked but not verified.",
                "audio-mic",
                "2026-05-02T09:00:00Z",
            ),
            "pyannote",
        )
        .unwrap();
    registry
        .link_to_interest(&linked_cluster, "person_michael", &profile)
        .unwrap();
    for (index, vector) in [
        vec![0.99, 0.01],
        vec![0.985, 0.015],
        vec![0.995, -0.005],
        vec![0.98, 0.02],
    ]
    .into_iter()
    .enumerate()
    {
        let fingerprint = AudioFingerprint::from_vector("pyannote.embedding", 16_000, vector);
        registry
            .record_sample_with_fingerprint(
                &linked_cluster,
                Some(fingerprint),
                voice_test_sample(
                    &format!("Cluster inherited sample {index}."),
                    "audio-mic",
                    &format!("2026-05-02T09:0{}:00Z", index + 1),
                ),
                "pyannote",
            )
            .unwrap();
    }

    let _query_cluster = record_unverified_vector_sample(
        &mut registry,
        vec![0.86, 0.14],
        "audio-system",
        "Unverified query.",
        "2026-05-02T09:10:00Z",
    );
    let query = registry
        .voice_samples_with_profile(Some(&profile))
        .into_iter()
        .find(|sample| sample.text == "Unverified query.")
        .unwrap();

    assert!(
        query
            .person_candidates
            .iter()
            .all(|candidate| candidate.id != "person_michael"),
        "cluster links are assignments, not verified predictor training data"
    );
}

#[test]
fn speaker_registry_exposes_high_confidence_cross_source_ablation_for_verified_samples() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("speakers.json");
    let mut registry = SpeakerRegistry::load_or_default(&path).unwrap();
    let profile = voice_test_profile();

    record_verified_voice_set(
        &mut registry,
        &profile,
        "person_michael",
        &[
            (vec![1.0, 0.0], "audio-mic"),
            (vec![0.999, 0.020], "audio-system"),
            (vec![0.998, -0.015], "audio-mic"),
            (vec![0.997, 0.025], "audio-system"),
            (vec![0.996, -0.020], "audio-mic"),
            (vec![0.995, 0.030], "audio-system"),
        ],
    );
    record_verified_voice_set(
        &mut registry,
        &profile,
        "person_christine",
        &[
            (vec![0.0, 1.0], "audio-mic"),
            (vec![0.020, 0.999], "audio-system"),
            (vec![-0.015, 0.998], "audio-mic"),
            (vec![0.025, 0.997], "audio-system"),
            (vec![-0.020, 0.996], "audio-mic"),
            (vec![0.030, 0.995], "audio-system"),
        ],
    );

    let query_fingerprint =
        AudioFingerprint::from_vector("pyannote.embedding", 16_000, vec![0.992, 0.018]);
    let _query_cluster = record_unverified_fingerprint_sample(
        &mut registry,
        query_fingerprint.clone(),
        "audio-system",
        "High confidence query.",
        "2026-05-02T10:00:00Z",
    );
    let query = registry
        .voice_samples_with_profile(Some(&profile))
        .into_iter()
        .find(|sample| sample.text == "High confidence query.")
        .unwrap();
    let candidate = query
        .person_candidates
        .iter()
        .find(|candidate| candidate.id == "person_michael")
        .expect("verified cross-source model should suggest Michael");
    let candidate_json = serde_json::to_value(candidate).unwrap();

    assert_eq!(candidate_json["voice_model_confidence"], "high");
    assert_eq!(candidate_json["verified_sample_count"], 6);
    assert_eq!(candidate_json["source_count"], 2);
    assert_eq!(candidate_json["auto_predict"], true);
    assert!(
        candidate_json["holdout_accuracy"].as_f64().unwrap() >= 0.95,
        "holdout accuracy should come from leave-one-out verification"
    );
    assert!(
        candidate_json["holdout_margin"].as_f64().unwrap() >= 0.08,
        "margin should prove the verified set is separated from competing people"
    );
    let source_stats = candidate_json["source_stats"]
        .as_array()
        .expect("candidate should expose per-source embedding stats");
    assert_eq!(source_stats.len(), 2);
    assert!(source_stats.iter().all(|stat| {
        stat["support_count"].as_u64().unwrap() >= 3
            && stat["confidence_radius"].as_f64().is_some()
            && stat["mean_similarity"].as_f64().is_some()
    }));
    assert_eq!(
        registry
            .predict_label_for_fingerprint(&query_fingerprint, Some(&profile))
            .as_deref(),
        Some("Michael")
    );
}

#[test]
fn speaker_registry_keeps_single_source_models_below_auto_prediction() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("speakers.json");
    let mut registry = SpeakerRegistry::load_or_default(&path).unwrap();
    let profile = voice_test_profile();

    record_verified_voice_set(
        &mut registry,
        &profile,
        "person_michael",
        &[
            (vec![1.0, 0.0], "audio-mic"),
            (vec![0.999, 0.020], "audio-mic"),
            (vec![0.998, -0.015], "audio-mic"),
            (vec![0.997, 0.025], "audio-mic"),
            (vec![0.996, -0.020], "audio-mic"),
            (vec![0.995, 0.030], "audio-mic"),
        ],
    );

    let query_fingerprint =
        AudioFingerprint::from_vector("pyannote.embedding", 16_000, vec![0.992, 0.018]);
    let _query_cluster = record_unverified_fingerprint_sample(
        &mut registry,
        query_fingerprint.clone(),
        "audio-mic",
        "Single source query.",
        "2026-05-02T10:00:00Z",
    );
    let query = registry
        .voice_samples_with_profile(Some(&profile))
        .into_iter()
        .find(|sample| sample.text == "Single source query.")
        .unwrap();
    let candidate = query
        .person_candidates
        .iter()
        .find(|candidate| candidate.id == "person_michael")
        .expect("single-source verified model should still surface as a suggestion");
    let candidate_json = serde_json::to_value(candidate).unwrap();

    assert_eq!(candidate_json["voice_model_confidence"], "medium");
    assert_eq!(candidate_json["source_count"], 1);
    assert_eq!(candidate_json["auto_predict"], false);
    assert!(
        registry
            .predict_label_for_fingerprint(&query_fingerprint, Some(&profile))
            .is_none(),
        "general prediction requires high cross-source confidence"
    );
}

#[test]
fn speaker_registry_exposes_tracked_person_voice_model_confidence() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("speakers.json");
    let mut registry = SpeakerRegistry::load_or_default(&path).unwrap();
    let profile = voice_test_profile();

    record_verified_voice_set(
        &mut registry,
        &profile,
        "person_michael",
        &[
            (vec![1.0, 0.0], "audio-mic"),
            (vec![0.999, 0.020], "audio-system"),
            (vec![0.998, -0.015], "audio-mic"),
            (vec![0.997, 0.025], "audio-system"),
            (vec![0.996, -0.020], "audio-mic"),
            (vec![0.995, 0.030], "audio-system"),
        ],
    );
    record_verified_voice_set(
        &mut registry,
        &profile,
        "person_christine",
        &[
            (vec![0.0, 1.0], "audio-mic"),
            (vec![0.020, 0.999], "audio-system"),
            (vec![-0.015, 0.998], "audio-mic"),
            (vec![0.025, 0.997], "audio-system"),
            (vec![-0.020, 0.996], "audio-mic"),
            (vec![0.030, 0.995], "audio-system"),
        ],
    );

    let models = registry.person_voice_model_summaries(Some(&profile));
    let michael = models
        .iter()
        .find(|model| model.linked_interest.id == "person_michael")
        .expect("tracked person should expose a voice model summary");

    assert_eq!(michael.confidence, "high");
    assert_eq!(michael.verified_sample_count, 6);
    assert_eq!(michael.source_count, 2);
    assert!(michael.holdout_accuracy >= 0.95);
    assert!(michael.holdout_margin >= 0.08);
    assert!(michael.auto_predict_ready);
}

#[test]
fn speaker_registry_applies_cluster_links_to_new_samples_live() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("speakers.json");
    let fingerprint = AudioFingerprint::from_vector("pyannote.embedding", 16_000, vec![1.0, 0.0]);
    let mut registry = SpeakerRegistry::load_or_default(&path).unwrap();
    let mut profile = SynthesisProfile::default();
    profile.interests = vec![SynthesisInterest {
        id: "person_michael".into(),
        interest_type: "person".into(),
        name: "Michael".into(),
        ..SynthesisInterest::default()
    }];

    let cluster = registry.resolve_or_create(&fingerprint);
    registry
        .link_to_interest(&cluster, "person_michael", &profile)
        .unwrap();
    registry
        .record_sample_with_fingerprint(
            &cluster,
            Some(fingerprint),
            SpeakerSample {
                text: "Live speaker sample.".into(),
                source: "audio-mic".into(),
                ts: "2026-05-02T09:02:00Z".into(),
                start_secs: 0.0,
                end_secs: 1.0,
                media_path: None,
                mime: None,
            },
            "pyannote",
        )
        .unwrap();

    let sample = registry.voice_samples_with_profile(Some(&profile))[0].clone();
    assert_eq!(sample.linked_interest_id.as_deref(), Some("person_michael"));
    assert_eq!(sample.linked_interest.as_ref().unwrap().name, "Michael");
}

#[test]
fn speaker_registry_tightens_person_voice_distribution_as_confirmed_samples_accumulate() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("speakers.json");
    let query_fingerprint =
        AudioFingerprint::from_vector("pyannote.embedding", 16_000, vec![0.9, 0.1]);
    let mut registry = SpeakerRegistry::load_or_default(&path).unwrap();
    let mut profile = SynthesisProfile::default();
    profile.interests = vec![SynthesisInterest {
        id: "person_michael".into(),
        interest_type: "person".into(),
        name: "Michael".into(),
        ..SynthesisInterest::default()
    }];

    let seed_cluster = registry.resolve_or_create(&AudioFingerprint::from_vector(
        "pyannote.embedding",
        16_000,
        vec![1.0, 0.0],
    ));
    registry
        .record_sample_with_fingerprint(
            &seed_cluster,
            Some(AudioFingerprint::from_vector(
                "pyannote.embedding",
                16_000,
                vec![1.0, 0.0],
            )),
            SpeakerSample {
                text: "Seed confirmed sample.".into(),
                source: "audio-mic".into(),
                ts: "2026-05-02T09:00:00Z".into(),
                start_secs: 0.0,
                end_secs: 1.0,
                media_path: None,
                mime: None,
            },
            "pyannote",
        )
        .unwrap();
    let seed_sample_id = registry
        .voice_samples_with_profile(None)
        .into_iter()
        .find(|sample| sample.text == "Seed confirmed sample.")
        .unwrap()
        .sample_id;
    registry
        .link_sample_to_interest(&seed_sample_id, "person_michael", &profile)
        .unwrap();

    let query_cluster = registry.resolve_or_create(&query_fingerprint);
    assert_ne!(query_cluster, seed_cluster);
    registry
        .record_sample_with_fingerprint(
            &query_cluster,
            Some(query_fingerprint.clone()),
            SpeakerSample {
                text: "Unassigned query sample.".into(),
                source: "audio-mic".into(),
                ts: "2026-05-02T09:01:00Z".into(),
                start_secs: 0.0,
                end_secs: 1.0,
                media_path: None,
                mime: None,
            },
            "pyannote",
        )
        .unwrap();

    let initial_candidates = registry
        .voice_samples_with_profile(Some(&profile))
        .into_iter()
        .find(|sample| sample.cluster_id == query_cluster)
        .unwrap()
        .person_candidates;
    assert!(initial_candidates.is_empty());

    for (index, vector) in [vec![0.99, 0.01], vec![0.985, 0.015], vec![0.995, -0.005]]
        .into_iter()
        .enumerate()
    {
        let fingerprint = AudioFingerprint::from_vector("pyannote.embedding", 16_000, vector);
        let cluster = registry.resolve_or_create(&fingerprint);
        registry
            .record_sample_with_fingerprint(
                &cluster,
                Some(fingerprint),
                SpeakerSample {
                    text: format!("Additional confirmed sample {index}."),
                    source: "audio-mic".into(),
                    ts: format!("2026-05-02T09:0{}:00Z", index + 2),
                    start_secs: 0.0,
                    end_secs: 1.0,
                    media_path: None,
                    mime: None,
                },
                "pyannote",
            )
            .unwrap();
        let text = format!("Additional confirmed sample {index}.");
        let sample_id = registry
            .voice_samples_with_profile(None)
            .into_iter()
            .find(|sample| sample.text == text)
            .unwrap()
            .sample_id;
        registry
            .link_sample_to_interest(&sample_id, "person_michael", &profile)
            .unwrap();
    }

    let refined_candidate = registry
        .voice_samples_with_profile(Some(&profile))
        .into_iter()
        .find(|sample| sample.cluster_id == query_cluster)
        .unwrap()
        .person_candidates
        .into_iter()
        .find(|candidate| candidate.id == "person_michael")
        .unwrap();
    let refined_radius = candidate_radius(&refined_candidate);
    let refined_support = candidate_support_count(&refined_candidate);

    assert!(refined_candidate.score >= 0.70);
    assert!(refined_radius < 0.20);
    assert_eq!(refined_support, 4);
    assert!(refined_candidate.reason.contains("distribution"));
}

fn candidate_radius(
    candidate: &alvum_processor_audio::speaker_registry::TrackedInterestCandidate,
) -> f64 {
    serde_json::to_value(candidate).unwrap()["confidence_radius"]
        .as_f64()
        .expect("candidate should expose confidence_radius")
}

fn candidate_support_count(
    candidate: &alvum_processor_audio::speaker_registry::TrackedInterestCandidate,
) -> u64 {
    serde_json::to_value(candidate).unwrap()["support_count"]
        .as_u64()
        .expect("candidate should expose support_count")
}

fn voice_test_profile() -> SynthesisProfile {
    let mut profile = SynthesisProfile::default();
    profile.interests = vec![
        SynthesisInterest {
            id: "person_michael".into(),
            interest_type: "person".into(),
            name: "Michael".into(),
            ..SynthesisInterest::default()
        },
        SynthesisInterest {
            id: "person_christine".into(),
            interest_type: "person".into(),
            name: "Christine".into(),
            ..SynthesisInterest::default()
        },
    ];
    profile
}

fn voice_test_sample(text: &str, source: &str, ts: &str) -> SpeakerSample {
    SpeakerSample {
        text: text.into(),
        source: source.into(),
        ts: ts.into(),
        start_secs: 0.0,
        end_secs: 1.0,
        media_path: None,
        mime: None,
    }
}

fn record_unverified_vector_sample(
    registry: &mut SpeakerRegistry,
    vector: Vec<f32>,
    source: &str,
    text: &str,
    ts: &str,
) -> String {
    let fingerprint = AudioFingerprint::from_vector("pyannote.embedding", 16_000, vector);
    record_unverified_fingerprint_sample(registry, fingerprint, source, text, ts)
}

fn record_unverified_fingerprint_sample(
    registry: &mut SpeakerRegistry,
    fingerprint: AudioFingerprint,
    source: &str,
    text: &str,
    ts: &str,
) -> String {
    let cluster = registry.resolve_or_create(&fingerprint);
    registry
        .record_sample_with_fingerprint(
            &cluster,
            Some(fingerprint),
            voice_test_sample(text, source, ts),
            "pyannote",
        )
        .unwrap();
    cluster
}

fn record_verified_voice_set(
    registry: &mut SpeakerRegistry,
    profile: &SynthesisProfile,
    interest_id: &str,
    vectors: &[(Vec<f32>, &str)],
) {
    for (index, (vector, source)) in vectors.iter().enumerate() {
        let fingerprint =
            AudioFingerprint::from_vector("pyannote.embedding", 16_000, vector.clone());
        let cluster = registry.resolve_or_create(&fingerprint);
        let text = format!("{interest_id} verified sample {index}.");
        registry
            .record_sample_with_fingerprint(
                &cluster,
                Some(fingerprint),
                voice_test_sample(&text, source, &format!("2026-05-02T09:{index:02}:00Z")),
                "pyannote",
            )
            .unwrap();
        let sample_id = registry
            .voice_samples_with_profile(None)
            .into_iter()
            .find(|sample| sample.text == text)
            .unwrap()
            .sample_id;
        registry
            .link_sample_to_interest(&sample_id, interest_id, profile)
            .unwrap();
    }
}

#[test]
fn speaker_registry_migrates_legacy_labels_to_person_interests() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("speakers.json");
    let fingerprint = AudioFingerprint::from_samples(&[0.0_f32, 0.3, -0.2, 0.1], 16_000);
    let mut registry = SpeakerRegistry::load_or_default(&path).unwrap();
    let speaker_id = registry.resolve_or_create(&fingerprint);
    registry.rename(&speaker_id, "Michael").unwrap();
    registry
        .record_sample(
            &speaker_id,
            SpeakerSample {
                text: "We should review the release checklist.".into(),
                source: "audio-mic".into(),
                ts: "2026-04-30T08:09:03Z".into(),
                start_secs: 0.0,
                end_secs: 1.0,
                media_path: Some(
                    "/Users/michael/.alvum/capture/2026-04-30/audio/mic/08-09-03.wav".into(),
                ),
                mime: Some("audio/wav".into()),
            },
        )
        .unwrap();
    let mut profile = SynthesisProfile::default();

    let changed = registry.migrate_legacy_labels(&mut profile).unwrap();

    assert!(changed);
    assert_eq!(profile.interests.len(), 1);
    assert_eq!(profile.interests[0].interest_type, "person");
    assert_eq!(profile.interests[0].name, "Michael");
    let speakers = registry.speakers_with_profile(Some(&profile));
    assert_eq!(
        speakers[0].linked_interest.as_ref().unwrap().id,
        profile.interests[0].id
    );
    assert_eq!(
        speakers[0].samples[0].media_path.as_deref(),
        Some("/Users/michael/.alvum/capture/2026-04-30/audio/mic/08-09-03.wav")
    );
    let samples = registry.voice_samples_with_profile(Some(&profile));
    assert_eq!(
        samples[0].linked_interest_id.as_deref(),
        Some(profile.interests[0].id.as_str())
    );
    assert_eq!(samples[0].linked_interest.as_ref().unwrap().name, "Michael");
}

#[test]
fn speaker_registry_migrates_to_sample_first_schema_and_supports_sample_actions() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("speakers.json");
    let fingerprint = AudioFingerprint::from_samples(&[0.0_f32, 0.3, -0.2, 0.1], 16_000);
    std::fs::write(
        &path,
        serde_json::to_vec_pretty(&serde_json::json!({
            "schema_version": 1,
            "speakers": [{
                "speaker_id": "spk_local_legacy",
                "label": null,
                "fingerprints": [fingerprint],
                "samples": [{
                    "text": "A good story does not need to be loud.",
                    "source": "audio-mic",
                    "ts": "2026-05-02T03:46:16Z",
                    "start_secs": 19.0,
                    "end_secs": 22.4,
                    "media_path": "/Users/michael/.alvum/capture/2026-05-01/audio/mic/20-46-16.wav",
                    "mime": "audio/wav"
                }]
            }],
            "future_sync": {"enabled": false}
        }))
        .unwrap(),
    )
    .unwrap();
    let mut registry = SpeakerRegistry::load_or_default(&path).unwrap();
    let mut profile = SynthesisProfile::default();
    profile.interests = vec![SynthesisInterest {
        id: "person_michael".into(),
        interest_type: "person".into(),
        name: "Michael".into(),
        ..SynthesisInterest::default()
    }];

    let samples = registry.voice_samples_with_profile(Some(&profile));
    assert_eq!(samples.len(), 1);
    assert_eq!(samples[0].cluster_id, "spk_local_legacy");
    assert!(samples[0].sample_id.starts_with("vsm_"));
    assert_eq!(
        samples[0].media_path.as_deref(),
        Some("/Users/michael/.alvum/capture/2026-05-01/audio/mic/20-46-16.wav")
    );
    assert!(samples[0].linked_interest_id.is_none());

    registry
        .link_sample_to_interest(&samples[0].sample_id, "person_michael", &profile)
        .unwrap();
    let linked = registry.voice_samples_with_profile(Some(&profile));
    assert_eq!(
        linked[0].linked_interest_id.as_deref(),
        Some("person_michael")
    );
    assert_eq!(linked[0].linked_interest.as_ref().unwrap().name, "Michael");
    assert!(
        registry.speakers_with_profile(Some(&profile))[0]
            .linked_interest_id
            .is_none()
    );

    registry
        .unlink_sample_interest(&linked[0].sample_id)
        .unwrap();
    let unlinked = registry.voice_samples_with_profile(Some(&profile));
    assert!(unlinked[0].linked_interest_id.is_none());
    assert_eq!(unlinked[0].assignment_source, "user_unassigned_sample");

    registry
        .link_to_interest("spk_local_legacy", "person_michael", &profile)
        .unwrap();
    let still_unlinked = registry.voice_samples_with_profile(Some(&profile));
    assert!(still_unlinked[0].linked_interest_id.is_none());
    assert_eq!(
        still_unlinked[0].assignment_source,
        "user_unassigned_sample"
    );

    registry
        .link_sample_to_interest(&linked[0].sample_id, "person_michael", &profile)
        .unwrap();
    let new_cluster = registry
        .move_sample_to_cluster(&linked[0].sample_id, "new")
        .unwrap();
    assert_ne!(new_cluster, "spk_local_legacy");
    let moved = registry.voice_samples_with_profile(Some(&profile));
    assert_eq!(moved[0].cluster_id, new_cluster);
    assert_eq!(
        moved[0].linked_interest_id.as_deref(),
        Some("person_michael")
    );
    assert!(
        registry
            .speakers_with_profile(Some(&profile))
            .iter()
            .any(|cluster| cluster.speaker_id == new_cluster)
    );
}

#[test]
fn speaker_registry_splits_samples_without_losing_playback_metadata() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("speakers.json");
    let first = AudioFingerprint::from_samples(&[0.0_f32, 0.2, -0.1, 0.15], 16_000);
    let second = AudioFingerprint::from_samples(&[0.0_f32, -0.4, 0.4, -0.2], 16_000);
    let mut registry = SpeakerRegistry::load_or_default(&path).unwrap();
    let cluster_id = registry.resolve_or_create(&first);
    registry
        .record_sample_with_fingerprint(
            &cluster_id,
            Some(first.clone()),
            SpeakerSample {
                text: "First speaker.".into(),
                source: "audio-mic".into(),
                ts: "2026-05-02T03:46:16Z".into(),
                start_secs: 1.0,
                end_secs: 2.0,
                media_path: Some("/capture/first.wav".into()),
                mime: Some("audio/wav".into()),
            },
            "pyannote",
        )
        .unwrap();
    registry
        .record_sample_with_fingerprint(
            &cluster_id,
            Some(second),
            SpeakerSample {
                text: "Second speaker.".into(),
                source: "audio-mic".into(),
                ts: "2026-05-02T03:47:16Z".into(),
                start_secs: 3.0,
                end_secs: 4.0,
                media_path: Some("/capture/second.wav".into()),
                mime: Some("audio/wav".into()),
            },
            "pyannote",
        )
        .unwrap();
    let sample_id = registry
        .voice_samples_with_profile(None)
        .iter()
        .find(|sample| sample.text == "Second speaker.")
        .unwrap()
        .sample_id
        .clone();

    let split_cluster = registry
        .split_samples_to_new_cluster(&cluster_id, &[sample_id])
        .unwrap();
    let samples = registry.voice_samples_with_profile(None);
    let moved = samples
        .iter()
        .find(|sample| sample.text == "Second speaker.")
        .unwrap();

    assert_eq!(moved.cluster_id, split_cluster);
    assert_eq!(moved.media_path.as_deref(), Some("/capture/second.wav"));
    assert_eq!(moved.start_secs, 3.0);
    assert_eq!(registry.speakers().len(), 2);
    let speakers = registry.speakers();
    let source = speakers
        .iter()
        .find(|speaker| speaker.speaker_id == cluster_id)
        .unwrap();
    let target = speakers
        .iter()
        .find(|speaker| speaker.speaker_id == split_cluster)
        .unwrap();
    assert_eq!(source.fingerprint_count, 1);
    assert_eq!(target.fingerprint_count, 1);
}

#[test]
fn speaker_registry_rebuilds_fingerprints_after_sample_moves_and_ignores() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("speakers.json");
    let first = AudioFingerprint::from_samples(&[0.0_f32, 0.2, -0.1, 0.15], 16_000);
    let second = AudioFingerprint::from_samples(&[0.0_f32, -0.4, 0.4, -0.2], 16_000);
    let mut registry = SpeakerRegistry::load_or_default(&path).unwrap();
    let cluster_id = registry.resolve_or_create(&first);
    registry
        .record_sample_with_fingerprint(
            &cluster_id,
            Some(first),
            SpeakerSample {
                text: "Original speaker.".into(),
                source: "audio-mic".into(),
                ts: "2026-05-02T03:46:16Z".into(),
                start_secs: 1.0,
                end_secs: 2.0,
                media_path: Some("/capture/first.wav".into()),
                mime: Some("audio/wav".into()),
            },
            "pyannote",
        )
        .unwrap();
    registry
        .record_sample_with_fingerprint(
            &cluster_id,
            Some(second),
            SpeakerSample {
                text: "Different speaker.".into(),
                source: "audio-mic".into(),
                ts: "2026-05-02T03:47:16Z".into(),
                start_secs: 3.0,
                end_secs: 4.0,
                media_path: Some("/capture/second.wav".into()),
                mime: Some("audio/wav".into()),
            },
            "pyannote",
        )
        .unwrap();
    let sample_id = registry
        .voice_samples_with_profile(None)
        .iter()
        .find(|sample| sample.text == "Different speaker.")
        .unwrap()
        .sample_id
        .clone();

    let moved_cluster = registry.move_sample_to_cluster(&sample_id, "new").unwrap();
    let speakers = registry.speakers();
    assert_eq!(
        speakers
            .iter()
            .find(|speaker| speaker.speaker_id == cluster_id)
            .unwrap()
            .fingerprint_count,
        1
    );
    assert_eq!(
        speakers
            .iter()
            .find(|speaker| speaker.speaker_id == moved_cluster)
            .unwrap()
            .fingerprint_count,
        1
    );

    registry.ignore_sample(&sample_id).unwrap();
    let speakers = registry.speakers();
    assert_eq!(
        speakers
            .iter()
            .find(|speaker| speaker.speaker_id == moved_cluster)
            .unwrap()
            .fingerprint_count,
        0
    );
}

#[test]
fn speaker_registry_splits_one_mixed_sample_into_independent_children() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("speakers.json");
    let fingerprint = AudioFingerprint::from_samples(&[0.0_f32, 0.2, -0.1, 0.15], 16_000);
    let mut profile = SynthesisProfile::default();
    profile.interests = vec![SynthesisInterest {
        id: "person_michael".into(),
        interest_type: "person".into(),
        name: "Michael".into(),
        ..SynthesisInterest::default()
    }];
    let mut registry = SpeakerRegistry::load_or_default(&path).unwrap();
    let cluster_id = registry.resolve_or_create(&fingerprint);
    registry
        .record_sample_with_fingerprint(
            &cluster_id,
            Some(fingerprint),
            SpeakerSample {
                text: "Michael starts. Lana answers.".into(),
                source: "audio-mic".into(),
                ts: "2026-05-02T03:46:16Z".into(),
                start_secs: 1.0,
                end_secs: 9.0,
                media_path: Some("/capture/mixed.wav".into()),
                mime: Some("audio/wav".into()),
            },
            "pyannote",
        )
        .unwrap();
    let sample_id = registry.voice_samples_with_profile(None)[0]
        .sample_id
        .clone();

    assert!(
        registry
            .split_sample_at(&sample_id, 1.0, "Michael starts.", "Lana answers.")
            .unwrap_err()
            .to_string()
            .contains("inside the sample")
    );
    assert!(
        registry
            .split_sample_at(&sample_id, 5.0, "", "Lana answers.")
            .unwrap_err()
            .to_string()
            .contains("required")
    );

    let split = registry
        .split_sample_at(&sample_id, 5.0, "Michael starts.", "Lana answers.")
        .unwrap();
    assert_eq!(split.len(), 2);
    let samples = registry.voice_samples_with_profile(Some(&profile));
    assert!(samples.iter().all(|sample| sample.sample_id != sample_id));
    let left = samples
        .iter()
        .find(|sample| sample.sample_id == split[0])
        .unwrap();
    let right = samples
        .iter()
        .find(|sample| sample.sample_id == split[1])
        .unwrap();
    assert_eq!(left.media_path.as_deref(), Some("/capture/mixed.wav"));
    assert_eq!(right.media_path.as_deref(), Some("/capture/mixed.wav"));
    assert_eq!(left.mime.as_deref(), Some("audio/wav"));
    assert_eq!(left.start_secs, 1.0);
    assert_eq!(left.end_secs, 5.0);
    assert_eq!(right.start_secs, 5.0);
    assert_eq!(right.end_secs, 9.0);
    assert_eq!(left.assignment_source, "user_split_turn");
    assert_eq!(right.assignment_source, "user_split_turn");

    registry
        .link_sample_to_interest(&split[0], "person_michael", &profile)
        .unwrap();
    let new_cluster = registry.move_sample_to_cluster(&split[1], "new").unwrap();
    registry.ignore_sample(&split[1]).unwrap();
    let samples = registry.voice_samples_with_profile(Some(&profile));
    let left = samples
        .iter()
        .find(|sample| sample.sample_id == split[0])
        .unwrap();
    let right = samples
        .iter()
        .find(|sample| sample.sample_id == split[1])
        .unwrap();
    assert_eq!(left.linked_interest_id.as_deref(), Some("person_michael"));
    assert_eq!(right.cluster_id, new_cluster);
    assert!(
        right
            .quality_flags
            .iter()
            .any(|flag| flag == "ignored_by_user")
    );
}

#[test]
fn speaker_registry_ignores_samples_and_declusters_inherited_links() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("speakers.json");
    let first = AudioFingerprint::from_samples(&[0.0_f32, 0.2, -0.1, 0.15], 16_000);
    let second = AudioFingerprint::from_samples(&[0.0_f32, -0.4, 0.4, -0.2], 16_000);
    let mut profile = SynthesisProfile::default();
    profile.interests = vec![SynthesisInterest {
        id: "person_michael".into(),
        interest_type: "person".into(),
        name: "Michael".into(),
        ..SynthesisInterest::default()
    }];
    let mut registry = SpeakerRegistry::load_or_default(&path).unwrap();
    let cluster_id = registry.resolve_or_create(&first);
    registry
        .record_sample_with_fingerprint(
            &cluster_id,
            Some(first),
            SpeakerSample {
                text: "Same speaker.".into(),
                source: "audio-mic".into(),
                ts: "2026-05-02T03:46:16Z".into(),
                start_secs: 1.0,
                end_secs: 2.0,
                media_path: Some("/capture/first.wav".into()),
                mime: Some("audio/wav".into()),
            },
            "pyannote",
        )
        .unwrap();
    registry
        .record_sample_with_fingerprint(
            &cluster_id,
            Some(second),
            SpeakerSample {
                text: "Different speaker.".into(),
                source: "audio-mic".into(),
                ts: "2026-05-02T03:47:16Z".into(),
                start_secs: 3.0,
                end_secs: 4.0,
                media_path: Some("/capture/second.wav".into()),
                mime: Some("audio/wav".into()),
            },
            "pyannote",
        )
        .unwrap();

    registry
        .link_to_interest(&cluster_id, "person_michael", &profile)
        .unwrap();
    let samples = registry.voice_samples_with_profile(Some(&profile));
    assert!(
        samples
            .iter()
            .all(|sample| { sample.linked_interest_id.as_deref() == Some("person_michael") })
    );
    let different_sample = samples
        .iter()
        .find(|sample| sample.text == "Different speaker.")
        .unwrap()
        .sample_id
        .clone();

    let split_cluster = registry
        .split_samples_to_new_cluster(&cluster_id, std::slice::from_ref(&different_sample))
        .unwrap();
    let samples = registry.voice_samples_with_profile(Some(&profile));
    let split_sample = samples
        .iter()
        .find(|sample| sample.sample_id == different_sample)
        .unwrap();
    assert_eq!(split_sample.cluster_id, split_cluster);
    assert!(split_sample.linked_interest_id.is_none());
    assert_eq!(split_sample.assignment_source, "user_split_sample");

    registry.ignore_sample(&different_sample).unwrap();
    let ignored = registry
        .voice_samples_with_profile(Some(&profile))
        .into_iter()
        .find(|sample| sample.sample_id == different_sample)
        .unwrap();
    assert!(
        ignored
            .quality_flags
            .iter()
            .any(|flag| flag == "ignored_by_user")
    );
    assert_eq!(ignored.assignment_source, "user_ignored_sample");
    assert!(ignored.linked_interest_id.is_none());
}

#[test]
fn speaker_registry_merge_reconciles_sample_links_to_target_cluster_identity() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("speakers.json");
    let first = AudioFingerprint::from_samples(&[0.0_f32, 0.2, -0.1, 0.15], 16_000);
    let second = AudioFingerprint::from_samples(&[0.0_f32, -0.4, 0.4, -0.2], 16_000);
    let mut profile = SynthesisProfile::default();
    profile.interests = vec![SynthesisInterest {
        id: "person_michael".into(),
        interest_type: "person".into(),
        name: "Michael".into(),
        ..SynthesisInterest::default()
    }];
    let mut registry = SpeakerRegistry::load_or_default(&path).unwrap();
    let target_id = registry.resolve_or_create(&first);
    let source_id = registry.resolve_or_create(&second);
    registry
        .record_sample_with_fingerprint(
            &target_id,
            Some(first),
            SpeakerSample {
                text: "Michael gives the update.".into(),
                source: "audio-mic".into(),
                ts: "2026-05-02T03:46:16Z".into(),
                start_secs: 1.0,
                end_secs: 2.0,
                media_path: Some("/capture/target.wav".into()),
                mime: Some("audio/wav".into()),
            },
            "pyannote",
        )
        .unwrap();
    registry
        .record_sample_with_fingerprint(
            &source_id,
            Some(second),
            SpeakerSample {
                text: "This should merge into Michael.".into(),
                source: "audio-mic".into(),
                ts: "2026-05-02T03:47:16Z".into(),
                start_secs: 3.0,
                end_secs: 4.0,
                media_path: Some("/capture/source.wav".into()),
                mime: Some("audio/wav".into()),
            },
            "pyannote",
        )
        .unwrap();
    registry
        .link_to_interest(&target_id, "person_michael", &profile)
        .unwrap();

    registry.merge(&source_id, &target_id).unwrap();

    let samples = registry.voice_samples_with_profile(Some(&profile));
    let merged = samples
        .iter()
        .find(|sample| sample.text == "This should merge into Michael.")
        .unwrap();
    assert_eq!(merged.cluster_id, target_id);
    assert_eq!(merged.linked_interest_id.as_deref(), Some("person_michael"));
    assert_eq!(merged.linked_interest.as_ref().unwrap().name, "Michael");
}

#[test]
fn openai_diarized_json_maps_to_speaker_turns() {
    let response = serde_json::json!({
        "text": "Good morning. Let's review the launch.",
        "segments": [
            {
                "start": 0.0,
                "end": 1.2,
                "text": "Good morning.",
                "speaker": "speaker_0"
            },
            {
                "start": 1.2,
                "end": 3.4,
                "text": "Let's review the launch.",
                "speaker": "speaker_1"
            }
        ]
    });

    let transcript = OpenAiDiarizedTranscript::from_value(response).unwrap();

    assert_eq!(transcript.text, "Good morning. Let's review the launch.");
    assert_eq!(transcript.turns.len(), 2);
    assert_eq!(
        transcript.turns[0].provider_speaker.as_deref(),
        Some("speaker_0")
    );
    assert_eq!(
        transcript.turns[1].provider_speaker.as_deref(),
        Some("speaker_1")
    );
}

#[test]
fn openai_diarized_json_maps_provider_speakers_through_registry() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("speakers.json");
    let mut registry = SpeakerRegistry::load_or_default(&path).unwrap();
    let response = serde_json::json!({
        "text": "Good morning. Let's review the launch.",
        "segments": [
            {"start": 0.0, "end": 0.5, "text": "Good morning.", "speaker": "speaker_0"},
            {"start": 0.5, "end": 1.0, "text": "Let's review the launch.", "speaker": "speaker_1"}
        ]
    });
    let samples: Vec<f32> = (0..16_000)
        .map(|i| {
            if i < 8_000 {
                0.08
            } else if i % 2 == 0 {
                0.65
            } else {
                -0.65
            }
        })
        .collect();

    let turns = OpenAiDiarizedTranscript::from_value(response)
        .unwrap()
        .into_speaker_turns_with_registry(
            &samples,
            16_000,
            Some(&mut registry),
            None,
            "audio-mic",
            "2026-04-30T08:09:03Z",
            Some("/Users/michael/.alvum/capture/2026-04-30/audio/mic/08-09-03.wav"),
            Some("audio/wav"),
        );

    assert_eq!(turns.len(), 2);
    assert_ne!(turns[0].speaker_id, turns[1].speaker_id);
    assert_eq!(turns[0].provider_speaker.as_deref(), Some("speaker_0"));
    assert_eq!(turns[1].provider_speaker.as_deref(), Some("speaker_1"));
    assert!(turns[0].fingerprint_ref.is_some());
    assert!(turns[1].fingerprint_ref.is_some());
    assert_eq!(registry.speakers().len(), 2);
    assert_eq!(registry.speakers()[0].samples[0].text, "Good morning.");
    assert_eq!(
        registry.voice_samples_with_profile(None)[0]
            .media_path
            .as_deref(),
        Some("/Users/michael/.alvum/capture/2026-04-30/audio/mic/08-09-03.wav")
    );
}

#[test]
fn openai_diarized_turns_use_confirmed_sample_identity_when_reprocessed() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("speakers.json");
    let mut registry = SpeakerRegistry::load_or_default(&path).unwrap();
    let mut profile = SynthesisProfile::default();
    profile.interests = vec![SynthesisInterest {
        id: "person_michael".into(),
        interest_type: "person".into(),
        name: "Michael".into(),
        ..SynthesisInterest::default()
    }];
    let response = serde_json::json!({
        "text": "Good morning.",
        "segments": [
            {"start": 0.0, "end": 0.5, "text": "Good morning.", "speaker": "speaker_0"}
        ]
    });
    let samples = vec![0.08; 16_000];

    OpenAiDiarizedTranscript::from_value(response.clone())
        .unwrap()
        .into_speaker_turns_with_registry(
            &samples,
            16_000,
            Some(&mut registry),
            Some(&profile),
            "audio-mic",
            "2026-04-30T08:09:03Z",
            Some("/Users/michael/.alvum/capture/2026-04-30/audio/mic/08-09-03.wav"),
            Some("audio/wav"),
        );
    let sample_id = registry.voice_samples_with_profile(Some(&profile))[0]
        .sample_id
        .clone();
    registry
        .link_sample_to_interest(&sample_id, "person_michael", &profile)
        .unwrap();

    let turns = OpenAiDiarizedTranscript::from_value(response)
        .unwrap()
        .into_speaker_turns_with_registry(
            &samples,
            16_000,
            Some(&mut registry),
            Some(&profile),
            "audio-mic",
            "2026-04-30T08:09:03Z",
            Some("/Users/michael/.alvum/capture/2026-04-30/audio/mic/08-09-03.wav"),
            Some("audio/wav"),
        );

    assert_eq!(turns[0].speaker_label.as_deref(), Some("Michael"));
}

#[test]
fn pyannote_diarization_aligns_segments_to_speaker_turns() {
    let diarization = PyannoteDiarization::from_value(serde_json::json!({
        "turns": [
            {"start": 0.0, "end": 1.5, "speaker": "SPEAKER_00", "confidence": 0.91, "embedding": [0.1, 0.2, 0.3]},
            {"start": 1.5, "end": 3.0, "speaker": "SPEAKER_01", "confidence": 0.88, "embedding": [0.3, 0.2, 0.1]}
        ]
    }))
    .unwrap();
    let segments = vec![
        Segment {
            start_secs: 0.2,
            end_secs: 1.0,
            text: "First speaker.".into(),
        },
        Segment {
            start_secs: 1.8,
            end_secs: 2.4,
            text: "Second speaker.".into(),
        },
    ];

    let aligned = align_segments_to_diarization(&segments, &diarization);

    assert_eq!(aligned.len(), 2);
    assert_eq!(aligned[0].provider_speaker.as_deref(), Some("SPEAKER_00"));
    assert_eq!(aligned[1].provider_speaker.as_deref(), Some("SPEAKER_01"));
    assert_eq!(aligned[0].confidence, Some(0.91));
    assert_eq!(aligned[0].fingerprint.model, "pyannote.embedding");
}

#[test]
fn voice_artifact_keeps_text_and_structured_layers() {
    let turns = vec![SpeakerTurn {
        start_secs: 0.0,
        end_secs: 2.0,
        text: "Status update from the meeting.".into(),
        speaker_id: "spk_local_1234".into(),
        speaker_label: Some("Michael".into()),
        provider_speaker: None,
        confidence: Some(0.82),
        fingerprint_ref: Some(FingerprintRef {
            model: "alvum.acoustic-v1".into(),
            digest: "abc123".into(),
        }),
    }];

    let artifact = AudioIntelligenceArtifact::new(
        sample_ref(),
        "Status update from the meeting.".into(),
        turns,
        "local_whisper",
        "alvum.acoustic-v1",
    )
    .into_artifact();

    assert_eq!(
        artifact.text(),
        Some("Michael: Status update from the meeting.")
    );
    assert!(artifact.layer("structured.audio.v2").is_some());
    assert!(artifact.layer("structured.diarization").is_some());
    assert!(artifact.layer("structured.speakers").is_some());
    assert_eq!(
        artifact.layer("structured.audio.v2").unwrap()["turns"][0]["speaker_label"],
        "Michael"
    );
    assert_eq!(
        artifact.layer("structured.diarization").unwrap()["source"],
        "local_whisper"
    );
    assert_eq!(
        artifact.layer("structured.speakers").unwrap()["speakers"][0]["fingerprint_refs"][0]["digest"],
        "abc123"
    );
}
