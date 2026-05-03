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
