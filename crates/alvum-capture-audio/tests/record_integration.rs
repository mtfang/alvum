/// Integration test: start recording, wait briefly, stop, check directory structure.
/// Requires microphone permission.
#[tokio::test]
#[ignore] // requires mic permission — run with: cargo test --test record_integration -- --ignored
async fn record_creates_capture_directory() {
    let tmp = tempfile::TempDir::new().unwrap();

    let config = alvum_capture_audio::recorder::RecordConfig {
        capture_dir: tmp.path().to_path_buf(),
        mic_device: None,
        system_device: Some("off".into()),
        chunk_duration_secs: 60,
    };

    let recorder = alvum_capture_audio::recorder::Recorder::start(config).unwrap();

    // Record for 2 seconds
    tokio::time::sleep(std::time::Duration::from_secs(2)).await;

    recorder.stop();
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;

    // Verify directory structure was created
    let mic_dir = tmp.path().join("audio").join("mic");
    assert!(mic_dir.is_dir(), "mic capture directory should exist at {:?}", mic_dir);
}
