use alvum_connector_audio::AudioConnector;
use alvum_core::connector::Connector;
use std::collections::HashMap;

fn connector_with_mode(mode: &str) -> AudioConnector {
    let mut settings = HashMap::new();
    settings.insert("mode".into(), toml::Value::String(mode.into()));
    settings.insert(
        "whisper_model".into(),
        toml::Value::String("/tmp/whisper.bin".into()),
    );
    settings.insert("provider".into(), toml::Value::String("openai-api".into()));
    AudioConnector::from_config(&settings).unwrap()
}

#[test]
fn provider_mode_routes_audio_to_provider_processor() {
    let connector = connector_with_mode("provider");

    let processors = connector.processors();

    assert_eq!(processors.len(), 1);
    assert_eq!(processors[0].name(), "audio");
}

#[test]
fn off_mode_routes_no_audio_processors() {
    let connector = connector_with_mode("off");

    assert!(connector.processors().is_empty());
}
