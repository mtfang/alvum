use assert_cmd::Command;
use predicates::prelude::*;
use std::path::Path;

fn write_manifest(dir: &Path) {
    let manifest = serde_json::json!({
        "schema_version": 1,
        "id": "fixture",
        "name": "Fixture",
        "version": "0.1.0",
        "server": {"start": ["node", "server.js"]},
        "captures": [{
            "id": "capture",
            "display_name": "Fixture capture",
            "schemas": ["fixture.event.v1"]
        }],
        "processors": [{
            "id": "processor",
            "display_name": "Fixture processor",
            "accepts": [{
                "component": "fixture/capture",
                "schema": "fixture.event.v1"
            }]
        }],
        "analyses": [{
            "id": "analysis",
            "display_name": "Fixture analysis",
            "scopes": ["briefing"],
            "output": "artifact"
        }],
        "connectors": [{
            "id": "main",
            "display_name": "Main",
            "routes": [{
                "from": {
                    "component": "fixture/capture",
                    "schema": "fixture.event.v1"
                },
                "to": ["fixture/processor"]
            }],
            "analyses": ["fixture/analysis"]
        }]
    });
    std::fs::write(
        dir.join("alvum.extension.json"),
        serde_json::to_vec_pretty(&manifest).unwrap(),
    )
    .unwrap();
    std::fs::write(
        dir.join("server.js"),
        r#"
const http = require('http');
const fs = require('fs');
const port = Number(process.env.ALVUM_EXTENSION_PORT);
const token = process.env.ALVUM_EXTENSION_TOKEN;
const manifest = JSON.parse(fs.readFileSync('alvum.extension.json', 'utf8'));
http.createServer((req, res) => {
  if (req.url !== '/v1/health' && req.headers.authorization !== `Bearer ${token}`) {
    res.writeHead(401, {'content-type': 'application/json'});
    res.end(JSON.stringify({error: 'unauthorized'}));
    return;
  }
  if (req.url === '/v1/health') {
    res.writeHead(200, {'content-type': 'text/plain'});
    res.end('ok');
    return;
  }
  if (req.url === '/v1/manifest') {
    res.writeHead(200, {'content-type': 'application/json'});
    res.end(JSON.stringify(manifest));
    return;
  }
  res.writeHead(404, {'content-type': 'application/json'});
  res.end(JSON.stringify({error: 'not found'}));
}).listen(port, '127.0.0.1');
"#,
    )
    .unwrap();
}

#[test]
fn profile_show_and_save_use_runtime_profile_file() {
    let tmp = tempfile::tempdir().unwrap();

    let show = Command::cargo_bin("alvum")
        .unwrap()
        .env("HOME", tmp.path())
        .args(["profile", "show", "--json"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let default_profile: serde_json::Value = serde_json::from_slice(&show).unwrap();
    assert_eq!(default_profile["domains"][0]["id"], "Career");
    assert!(default_profile["intentions"].as_array().unwrap().is_empty());

    let custom = serde_json::json!({
        "intentions": [{
            "id": "ship_alignment_engine",
            "kind": "Goal",
            "domain": "Career",
            "description": "Ship the alignment engine",
            "aliases": ["alignment engine"],
            "notes": "",
            "success_criteria": "Signed app with grounded synthesis",
            "cadence": "",
            "target_date": "2026-05-31",
            "priority": 0,
            "enabled": true,
            "confirmed": true,
            "source": "UserDefined",
            "nudge": "Protect focused implementation blocks."
        }],
        "domains": [{
            "id": "Alvum",
            "name": "Alvum",
            "description": "Product and platform work",
            "aliases": ["tray"],
            "priority": 0,
            "enabled": true
        }],
        "interests": [{
            "id": "project_alvum",
            "type": "project",
            "name": "Alvum",
            "aliases": [],
            "notes": "",
            "priority": 0,
            "enabled": true,
            "linked_knowledge_ids": []
        }],
        "writing": {
            "detail_level": "exhaustive",
            "tone": "analytical",
            "outline": "Lead with alignment, then product architecture."
        },
        "advanced_instructions": "Prioritize product architecture.",
        "ignored_suggestions": []
    });

    Command::cargo_bin("alvum")
        .unwrap()
        .env("HOME", tmp.path())
        .args(["profile", "save", "--json", &custom.to_string()])
        .assert()
        .success()
        .stdout(predicate::str::contains("Saved synthesis profile"));

    let profile_path = tmp.path().join(".alvum/runtime/synthesis-profile.toml");
    let saved = std::fs::read_to_string(profile_path).unwrap();
    assert!(saved.contains("advanced_instructions"));
    assert!(saved.contains("ship_alignment_engine"));
    assert!(saved.contains("Alvum"));
}

#[test]
fn profile_suggestions_only_surface_recurring_trackable_items() {
    let tmp = tempfile::tempdir().unwrap();
    let knowledge_dir = tmp.path().join("knowledge");
    std::fs::create_dir_all(&knowledge_dir).unwrap();
    std::fs::write(
        knowledge_dir.join("entities.jsonl"),
        [
            serde_json::json!({
                "id": "project_alvum",
                "name": "Alvum",
                "entity_type": "project",
                "description": "Product work that reappears across days.",
                "relationships": [],
                "first_seen": "2026-04-20",
                "last_seen": "2026-04-25"
            })
            .to_string(),
            serde_json::json!({
                "id": "one_off_topic",
                "name": "One-off topic",
                "entity_type": "topic",
                "description": "Mentioned once.",
                "relationships": [],
                "first_seen": "2026-04-25",
                "last_seen": "2026-04-25"
            })
            .to_string(),
        ]
        .join("\n")
            + "\n",
    )
    .unwrap();
    std::fs::write(
        knowledge_dir.join("patterns.jsonl"),
        [
            serde_json::json!({
                "id": "scope_creep",
                "description": "Repeated scope expansion during product work.",
                "occurrences": 2,
                "first_seen": "2026-04-25",
                "last_seen": "2026-04-25",
                "domains": ["Career"],
                "evidence": ["dec_1", "dec_2"]
            })
            .to_string(),
            serde_json::json!({
                "id": "single_ping",
                "description": "One isolated mention.",
                "occurrences": 1,
                "first_seen": "2026-04-25",
                "last_seen": "2026-04-25",
                "domains": ["Career"],
                "evidence": ["dec_3"]
            })
            .to_string(),
        ]
        .join("\n")
            + "\n",
    )
    .unwrap();
    std::fs::write(knowledge_dir.join("facts.jsonl"), "").unwrap();

    let output = Command::cargo_bin("alvum")
        .unwrap()
        .env("HOME", tmp.path())
        .env("ALVUM_KNOWLEDGE_DIR", &knowledge_dir)
        .args(["profile", "suggestions", "--json"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let json: serde_json::Value = serde_json::from_slice(&output).unwrap();
    let ids: Vec<&str> = json["suggestions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|suggestion| suggestion["id"].as_str().unwrap())
        .collect();

    assert!(ids.contains(&"entity_project_alvum"));
    assert!(ids.contains(&"pattern_scope_creep"));
    assert!(!ids.contains(&"entity_one_off_topic"));
    assert!(!ids.contains(&"pattern_single_ping"));
}

#[test]
fn extensions_install_enable_and_list_use_isolated_home() {
    let tmp = tempfile::tempdir().unwrap();
    let source = tmp.path().join("fixture-source");
    std::fs::create_dir_all(&source).unwrap();
    write_manifest(&source);

    Command::cargo_bin("alvum")
        .unwrap()
        .env("HOME", tmp.path())
        .args(["extensions", "install", source.to_str().unwrap()])
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "Installed extension package: fixture",
        ));

    Command::cargo_bin("alvum")
        .unwrap()
        .env("HOME", tmp.path())
        .args(["extensions", "enable", "fixture"])
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "Enabled extension connector: fixture/main",
        ));

    Command::cargo_bin("alvum")
        .unwrap()
        .env("HOME", tmp.path())
        .args(["extensions", "list"])
        .assert()
        .success()
        .stdout(predicate::str::contains("fixture (enabled)"));

    let config = std::fs::read_to_string(tmp.path().join(".alvum/runtime/config.toml")).unwrap();
    assert!(config.contains("kind = \"external-http\""));
    assert!(config.contains("package = \"fixture\""));

    let output = Command::cargo_bin("alvum")
        .unwrap()
        .env("HOME", tmp.path())
        .args(["extensions", "list", "--json"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let json: serde_json::Value = serde_json::from_slice(&output).unwrap();
    assert_eq!(json["extensions"][0]["id"], "fixture");
    assert_eq!(json["extensions"][0]["enabled"], true);
    assert_eq!(json["extensions"][0]["connectors"][0]["id"], "main");

    let output = Command::cargo_bin("alvum")
        .unwrap()
        .env("HOME", tmp.path())
        .args(["extensions", "doctor", "--json"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let json: serde_json::Value = serde_json::from_slice(&output).unwrap();
    assert_eq!(json["extensions"][0]["id"], "fixture");
    assert_eq!(json["extensions"][0]["ok"], true);
}

#[test]
fn connectors_list_json_projects_core_and_external_connectors_with_routes() {
    let tmp = tempfile::tempdir().unwrap();
    let source = tmp.path().join("fixture-source");
    std::fs::create_dir_all(&source).unwrap();
    write_manifest(&source);

    Command::cargo_bin("alvum")
        .unwrap()
        .env("HOME", tmp.path())
        .args(["extensions", "install", source.to_str().unwrap()])
        .assert()
        .success();

    Command::cargo_bin("alvum")
        .unwrap()
        .env("HOME", tmp.path())
        .args(["extensions", "enable", "fixture"])
        .assert()
        .success();

    let output = Command::cargo_bin("alvum")
        .unwrap()
        .env("HOME", tmp.path())
        .args(["connectors", "list", "--json"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let json: serde_json::Value = serde_json::from_slice(&output).unwrap();
    let connectors = json["connectors"].as_array().unwrap();

    let audio = connectors
        .iter()
        .find(|connector| connector["component_id"] == "alvum.audio/audio")
        .unwrap();
    assert_eq!(audio["kind"], "core");
    assert_eq!(audio["enabled"], true);
    assert_eq!(audio["aggregate_state"], "all_off");
    assert_eq!(audio["source_count"], 2);
    assert_eq!(audio["enabled_source_count"], 0);
    assert_eq!(audio["source_controls"][0]["id"], "audio-mic");
    assert_eq!(audio["source_controls"][0]["label"], "Microphone");
    assert_eq!(
        audio["source_controls"][0]["component"],
        "alvum.audio/audio-mic"
    );
    assert_eq!(audio["source_controls"][0]["enabled"], false);
    assert_eq!(audio["source_controls"][0]["toggleable"], true);
    assert_eq!(audio["source_controls"][1]["id"], "audio-system");
    assert_eq!(audio["source_controls"][1]["label"], "System audio");
    assert_eq!(audio["source_controls"][1]["enabled"], false);
    assert_eq!(
        audio["routes"][0]["from"]["component"],
        "alvum.audio/audio-mic"
    );
    assert_eq!(
        audio["routes"][0]["to"][0]["component"],
        "alvum.audio/whisper"
    );
    assert_eq!(
        audio["processor_controls"][0]["component"],
        "alvum.audio/whisper"
    );
    assert_eq!(
        audio["processor_controls"][0]["label"],
        "Whisper transcription"
    );
    assert_eq!(audio["processor_controls"][0]["settings"][0]["key"], "mode");

    let fixture = connectors
        .iter()
        .find(|connector| connector["component_id"] == "fixture/main")
        .unwrap();
    assert_eq!(fixture["kind"], "external");
    assert_eq!(fixture["enabled"], true);
    assert_eq!(fixture["package_id"], "fixture");
    assert_eq!(fixture["connector_id"], "main");
    assert_eq!(fixture["routes"][0]["from"]["component"], "fixture/capture");
    assert_eq!(
        fixture["routes"][0]["from"]["display_name"],
        "Fixture capture"
    );
    assert_eq!(
        fixture["routes"][0]["to"][0]["component"],
        "fixture/processor"
    );
    assert_eq!(
        fixture["routes"][0]["to"][0]["display_name"],
        "Fixture processor"
    );
    assert_eq!(
        fixture["processor_controls"][0]["component"],
        "fixture/processor"
    );
    assert_eq!(fixture["analyses"][0]["component_id"], "fixture/analysis");
    assert_eq!(fixture["analyses"][0]["output"], "artifact");
    assert!(fixture["issues"].as_array().unwrap().is_empty());
}

#[test]
fn connectors_list_json_reports_processor_settings_separately_from_capture_controls() {
    let tmp = tempfile::tempdir().unwrap();

    Command::cargo_bin("alvum")
        .unwrap()
        .env("HOME", tmp.path())
        .args([
            "config-set",
            "processors.audio.whisper_model",
            "/models/ggml-base.en.bin",
        ])
        .assert()
        .success();
    Command::cargo_bin("alvum")
        .unwrap()
        .env("HOME", tmp.path())
        .args(["config-set", "processors.audio.whisper_language", "en"])
        .assert()
        .success();
    Command::cargo_bin("alvum")
        .unwrap()
        .env("HOME", tmp.path())
        .args(["config-set", "processors.screen.mode", "ocr"])
        .assert()
        .success();

    let output = Command::cargo_bin("alvum")
        .unwrap()
        .env("HOME", tmp.path())
        .args(["connectors", "list", "--json"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let json: serde_json::Value = serde_json::from_slice(&output).unwrap();
    let connectors = json["connectors"].as_array().unwrap();
    let audio = connectors
        .iter()
        .find(|connector| connector["component_id"] == "alvum.audio/audio")
        .unwrap();
    let audio_processor = &audio["processor_controls"][0];
    assert_eq!(audio_processor["component"], "alvum.audio/whisper");
    assert_eq!(audio_processor["settings"][0]["key"], "mode");
    assert_eq!(audio_processor["settings"][1]["key"], "whisper_model");
    assert_eq!(
        audio_processor["settings"][1]["value"],
        "/models/ggml-base.en.bin"
    );
    assert_eq!(
        audio_processor["settings"][1]["value_label"],
        "/models/ggml-base.en.bin"
    );
    assert_eq!(
        audio_processor["settings"][1]["options"][0]["value"],
        "/models/ggml-base.en.bin"
    );
    assert_eq!(audio_processor["settings"][2]["key"], "whisper_language");
    assert_eq!(audio_processor["settings"][2]["value"], "en");
    assert!(
        audio["source_controls"][0].get("settings").is_none(),
        "capture source controls should not carry processor settings"
    );

    let screen = connectors
        .iter()
        .find(|connector| connector["component_id"] == "alvum.screen/screen")
        .unwrap();
    let screen_processor = &screen["processor_controls"][0];
    assert_eq!(screen_processor["component"], "alvum.screen/vision");
    assert_eq!(screen_processor["settings"][0]["key"], "mode");
    assert_eq!(
        screen_processor["settings"][0]["label"],
        "Recognition method"
    );
    assert_eq!(screen_processor["settings"][0]["value"], "ocr");
    assert_eq!(screen_processor["settings"][0]["value_label"], "OCR");
    assert_eq!(
        screen_processor["settings"][0]["options"][0]["value"],
        "ocr"
    );
    assert_eq!(
        screen_processor["settings"][0]["options"][0]["label"],
        "OCR"
    );
}

#[test]
fn connectors_list_json_reports_partial_owned_source_state() {
    let tmp = tempfile::tempdir().unwrap();

    Command::cargo_bin("alvum")
        .unwrap()
        .env("HOME", tmp.path())
        .args(["config-set", "capture.audio-mic.enabled", "true"])
        .assert()
        .success();

    Command::cargo_bin("alvum")
        .unwrap()
        .env("HOME", tmp.path())
        .args(["config-set", "capture.audio-system.enabled", "false"])
        .assert()
        .success();

    let output = Command::cargo_bin("alvum")
        .unwrap()
        .env("HOME", tmp.path())
        .args(["connectors", "list", "--json"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let json: serde_json::Value = serde_json::from_slice(&output).unwrap();
    let audio = json["connectors"]
        .as_array()
        .unwrap()
        .iter()
        .find(|connector| connector["component_id"] == "alvum.audio/audio")
        .unwrap();

    assert_eq!(audio["enabled"], true);
    assert_eq!(audio["aggregate_state"], "partial");
    assert_eq!(audio["source_count"], 2);
    assert_eq!(audio["enabled_source_count"], 1);
    assert_eq!(audio["source_controls"][0]["enabled"], true);
    assert_eq!(audio["source_controls"][1]["enabled"], false);
}

#[test]
fn connectors_enable_disable_updates_external_connector_config() {
    let tmp = tempfile::tempdir().unwrap();
    let source = tmp.path().join("fixture-source");
    std::fs::create_dir_all(&source).unwrap();
    write_manifest(&source);

    Command::cargo_bin("alvum")
        .unwrap()
        .env("HOME", tmp.path())
        .args(["extensions", "install", source.to_str().unwrap()])
        .assert()
        .success();

    Command::cargo_bin("alvum")
        .unwrap()
        .env("HOME", tmp.path())
        .args(["connectors", "enable", "fixture/main"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Enabled connector: fixture/main"));

    let output = Command::cargo_bin("alvum")
        .unwrap()
        .env("HOME", tmp.path())
        .args(["connectors", "list", "--json"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let json: serde_json::Value = serde_json::from_slice(&output).unwrap();
    let fixture = json["connectors"]
        .as_array()
        .unwrap()
        .iter()
        .find(|connector| connector["component_id"] == "fixture/main")
        .unwrap();
    assert_eq!(fixture["enabled"], true);

    Command::cargo_bin("alvum")
        .unwrap()
        .env("HOME", tmp.path())
        .args(["connectors", "disable", "fixture/main"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Disabled connector: fixture/main"));

    let output = Command::cargo_bin("alvum")
        .unwrap()
        .env("HOME", tmp.path())
        .args(["connectors", "list", "--json"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let json: serde_json::Value = serde_json::from_slice(&output).unwrap();
    let fixture = json["connectors"]
        .as_array()
        .unwrap()
        .iter()
        .find(|connector| connector["component_id"] == "fixture/main")
        .unwrap();
    assert_eq!(fixture["enabled"], false);
}

#[test]
fn connectors_disable_core_connector_disables_owned_capture_sources() {
    let tmp = tempfile::tempdir().unwrap();

    Command::cargo_bin("alvum")
        .unwrap()
        .env("HOME", tmp.path())
        .args(["connectors", "disable", "alvum.audio/audio"])
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "Disabled connector: alvum.audio/audio",
        ));

    let config = std::fs::read_to_string(tmp.path().join(".alvum/runtime/config.toml")).unwrap();
    let doc: toml::Value = config.parse().unwrap();
    assert_eq!(doc["connectors"]["audio"]["enabled"].as_bool(), Some(false));
    assert_eq!(
        doc["capture"]["audio-mic"]["enabled"].as_bool(),
        Some(false)
    );
    assert_eq!(
        doc["capture"]["audio-system"]["enabled"].as_bool(),
        Some(false)
    );

    Command::cargo_bin("alvum")
        .unwrap()
        .env("HOME", tmp.path())
        .args(["connectors", "enable", "alvum.audio/audio"])
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "Enabled connector: alvum.audio/audio",
        ));

    let config = std::fs::read_to_string(tmp.path().join(".alvum/runtime/config.toml")).unwrap();
    let doc: toml::Value = config.parse().unwrap();
    assert_eq!(doc["connectors"]["audio"]["enabled"].as_bool(), Some(true));
    assert_eq!(doc["capture"]["audio-mic"]["enabled"].as_bool(), Some(true));
    assert_eq!(
        doc["capture"]["audio-system"]["enabled"].as_bool(),
        Some(true)
    );
}

#[test]
fn connectors_disable_screen_connector_disables_screen_capture_source() {
    let tmp = tempfile::tempdir().unwrap();

    Command::cargo_bin("alvum")
        .unwrap()
        .env("HOME", tmp.path())
        .args(["connectors", "disable", "alvum.screen/screen"])
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "Disabled connector: alvum.screen/screen",
        ));

    let config = std::fs::read_to_string(tmp.path().join(".alvum/runtime/config.toml")).unwrap();
    let doc: toml::Value = config.parse().unwrap();
    assert_eq!(
        doc["connectors"]["screen"]["enabled"].as_bool(),
        Some(false)
    );
    assert_eq!(doc["capture"]["screen"]["enabled"].as_bool(), Some(false));
}

#[test]
fn extensions_scaffold_writes_a_starter_package() {
    let tmp = tempfile::tempdir().unwrap();
    let out = tmp.path().join("sample-extension");

    Command::cargo_bin("alvum")
        .unwrap()
        .env("HOME", tmp.path())
        .args([
            "extensions",
            "scaffold",
            out.to_str().unwrap(),
            "--id",
            "sample",
            "--name",
            "Sample Extension",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("Scaffolded extension package"));

    let manifest: serde_json::Value =
        serde_json::from_slice(&std::fs::read(out.join("alvum.extension.json")).unwrap()).unwrap();
    assert_eq!(manifest["id"], "sample");
    assert_eq!(
        manifest["connectors"][0]["routes"][0]["from"]["component"],
        "sample/capture"
    );
    assert!(out.join("package.json").exists());
    assert!(out.join("src/server.mjs").exists());
}

#[test]
fn extensions_list_json_includes_read_only_core_packages() {
    let tmp = tempfile::tempdir().unwrap();

    let output = Command::cargo_bin("alvum")
        .unwrap()
        .env("HOME", tmp.path())
        .args(["extensions", "list", "--json"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let json: serde_json::Value = serde_json::from_slice(&output).unwrap();

    assert_eq!(json["extensions"].as_array().unwrap().len(), 0);
    assert_eq!(json["core"][0]["id"], "alvum.audio");
    assert_eq!(json["core"][0]["kind"], "core");
    assert_eq!(json["core"][0]["read_only"], true);
    assert_eq!(
        json["core"][0]["captures"][0]["component_id"],
        "alvum.audio/audio-mic"
    );
    assert_eq!(json["core"][1]["id"], "alvum.screen");
    assert_eq!(json["core"][2]["id"], "alvum.session");
}

#[test]
fn doctor_json_reports_global_checks() {
    let tmp = tempfile::tempdir().unwrap();

    let output = Command::cargo_bin("alvum")
        .unwrap()
        .env("HOME", tmp.path())
        .args(["doctor", "--json"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let json: serde_json::Value = serde_json::from_slice(&output).unwrap();
    let checks = json["checks"].as_array().unwrap();

    assert_eq!(json["ok"], true);
    assert!(checks.iter().any(|check| check["id"] == "config"));
    assert!(checks.iter().any(|check| check["id"] == "connectors"));
    assert!(checks.iter().any(|check| check["id"] == "providers"));
    assert!(checks.iter().any(|check| check["id"] == "extensions"));
}

#[test]
fn providers_list_respects_enabled_config_for_auto_resolution() {
    let tmp = tempfile::tempdir().unwrap();

    Command::cargo_bin("alvum")
        .unwrap()
        .env("HOME", tmp.path())
        .env("ALVUM_DISABLE_KEYCHAIN", "1")
        .args(["providers", "disable", "claude-cli"])
        .assert()
        .success();

    let config = std::fs::read_to_string(tmp.path().join(".alvum/runtime/config.toml")).unwrap();
    let doc: toml::Value = config.parse().unwrap();
    assert_eq!(
        doc["providers"]["claude-cli"]["enabled"].as_bool(),
        Some(false)
    );

    let output = Command::cargo_bin("alvum")
        .unwrap()
        .env("HOME", tmp.path())
        .env("ALVUM_DISABLE_KEYCHAIN", "1")
        .args(["providers", "list", "--json"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let json: serde_json::Value = serde_json::from_slice(&output).unwrap();
    let claude = json["providers"]
        .as_array()
        .unwrap()
        .iter()
        .find(|provider| provider["name"] == "claude-cli")
        .unwrap();
    assert_eq!(claude["enabled"], false);
    assert_ne!(json["auto_resolved"], "claude-cli");
}

#[test]
fn providers_list_includes_management_metadata() {
    let tmp = tempfile::tempdir().unwrap();

    let output = Command::cargo_bin("alvum")
        .unwrap()
        .env("HOME", tmp.path())
        .env("ALVUM_DISABLE_KEYCHAIN", "1")
        .args(["providers", "list"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let json: serde_json::Value = serde_json::from_slice(&output).unwrap();
    let claude = json["providers"]
        .as_array()
        .unwrap()
        .iter()
        .find(|provider| provider["name"] == "claude-cli")
        .unwrap();
    assert_eq!(claude["setup_kind"], "instructions");
    assert!(claude["setup_command"].is_null());
    assert!(
        !claude["setup_hint"]
            .as_str()
            .unwrap()
            .contains("claude login")
    );
    assert_eq!(claude["selected_models"]["text"], "CLI default");

    let codex = json["providers"]
        .as_array()
        .unwrap()
        .iter()
        .find(|provider| provider["name"] == "codex-cli")
        .unwrap();
    assert_eq!(codex["display_name"], "Codex CLI");
    assert_eq!(codex["setup_kind"], "terminal");
    assert_eq!(codex["setup_command"], "codex login");
    assert!(codex["setup_hint"].as_str().unwrap().contains("Terminal"));
    assert!(
        codex["config_fields"]
            .as_array()
            .unwrap()
            .iter()
            .any(|field| field["key"] == "text_model"
                && field["options"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|option| option["value"] == ""))
    );
    assert_eq!(codex["capabilities"]["text"]["supported"], true);
    assert_eq!(codex["capabilities"]["image"]["adapter_supported"], false);
    assert_eq!(codex["selected_models"]["text"], "CLI default");

    let anthropic = json["providers"]
        .as_array()
        .unwrap()
        .iter()
        .find(|provider| provider["name"] == "anthropic-api")
        .unwrap();
    assert_eq!(anthropic["setup_kind"], "inline");
    assert_eq!(
        anthropic["setup_url"],
        "https://console.anthropic.com/settings/keys"
    );
    assert!(
        anthropic["config_fields"]
            .as_array()
            .unwrap()
            .iter()
            .any(|field| field["key"] == "api_key" && field["secret"] == true)
    );

    let ollama = json["providers"]
        .as_array()
        .unwrap()
        .iter()
        .find(|provider| provider["name"] == "ollama")
        .unwrap();
    assert_eq!(ollama["setup_kind"], "inline");
    assert_eq!(ollama["setup_command"], "ollama serve");
    assert!(
        ollama["setup_hint"]
            .as_str()
            .unwrap()
            .contains("already running")
    );
}

#[test]
fn providers_models_unknown_provider_returns_json() {
    let tmp = tempfile::tempdir().unwrap();

    let output = Command::cargo_bin("alvum")
        .unwrap()
        .env("HOME", tmp.path())
        .env("ALVUM_DISABLE_KEYCHAIN", "1")
        .args(["providers", "models", "--provider", "no-such-provider"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let json: serde_json::Value = serde_json::from_slice(&output).unwrap();
    assert_eq!(json["provider"], "no-such-provider");
    assert_eq!(json["ok"], false);
    assert_eq!(json["source"], "none");
}

#[test]
fn providers_models_ollama_falls_back_when_live_query_fails() {
    let tmp = tempfile::tempdir().unwrap();

    Command::cargo_bin("alvum")
        .unwrap()
        .env("HOME", tmp.path())
        .env("ALVUM_DISABLE_KEYCHAIN", "1")
        .env("PATH", "")
        .write_stdin(r#"{"settings":{"base_url":"http://127.0.0.1:9"}}"#)
        .args(["providers", "configure", "ollama"])
        .assert()
        .success();

    let output = Command::cargo_bin("alvum")
        .unwrap()
        .env("HOME", tmp.path())
        .env("ALVUM_DISABLE_KEYCHAIN", "1")
        .env("ALVUM_OLLAMA_LIBRARY_BASE_URL", "http://127.0.0.1:9")
        .env("PATH", "")
        .args(["providers", "models", "--provider", "ollama"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let json: serde_json::Value = serde_json::from_slice(&output).unwrap();
    assert_eq!(json["provider"], "ollama");
    assert_eq!(json["source"], "fallback");
    assert!(json["options"].as_array().unwrap().is_empty());
    assert!(
        json["options_by_modality"]["text"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    assert!(
        json["options_by_modality"]["image"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    assert!(json["installable_options"].as_array().unwrap().is_empty());
    assert!(json["installable_error"].as_str().unwrap().len() > 0);
}

#[test]
fn providers_models_ollama_can_fall_back_to_cli_list() {
    use std::os::unix::fs::PermissionsExt;

    let tmp = tempfile::tempdir().unwrap();
    let bin_dir = tmp.path().join("bin");
    std::fs::create_dir_all(&bin_dir).unwrap();
    let ollama = bin_dir.join("ollama");
    std::fs::write(
        &ollama,
        "#!/bin/sh\nif [ \"$1\" = \"ls\" ]; then\n  printf 'NAME               ID              SIZE     MODIFIED\\n'\n  printf 'deepseek-r1:70b    0c1615a8ca32    42 GB    15 months ago\\n'\n  printf 'deepseek-r1:32b    38056bbcbb2d    19 GB    15 months ago\\n'\n  exit 0\nfi\nexit 1\n",
    )
    .unwrap();
    let mut perms = std::fs::metadata(&ollama).unwrap().permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(&ollama, perms).unwrap();

    Command::cargo_bin("alvum")
        .unwrap()
        .env("HOME", tmp.path())
        .env("ALVUM_DISABLE_KEYCHAIN", "1")
        .env("PATH", &bin_dir)
        .write_stdin(r#"{"settings":{"base_url":"http://127.0.0.1:9"}}"#)
        .args(["providers", "configure", "ollama"])
        .assert()
        .success();

    let output = Command::cargo_bin("alvum")
        .unwrap()
        .env("HOME", tmp.path())
        .env("ALVUM_DISABLE_KEYCHAIN", "1")
        .env("ALVUM_OLLAMA_LIBRARY_BASE_URL", "http://127.0.0.1:9")
        .env("PATH", &bin_dir)
        .args(["providers", "models", "--provider", "ollama"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let json: serde_json::Value = serde_json::from_slice(&output).unwrap();
    assert_eq!(json["provider"], "ollama");
    assert_eq!(json["ok"], true);
    assert_eq!(json["source"], "ollama-cli");
    assert!(
        json["options_by_modality"]["text"]
            .as_array()
            .unwrap()
            .iter()
            .any(|option| option["value"] == "deepseek-r1:70b")
    );
    assert!(
        json["options_by_modality"]["image"]
            .as_array()
            .unwrap()
            .is_empty()
    );
}

#[test]
fn providers_install_model_rejects_unsupported_provider_without_download() {
    let tmp = tempfile::tempdir().unwrap();

    let output = Command::cargo_bin("alvum")
        .unwrap()
        .env("HOME", tmp.path())
        .env("ALVUM_DISABLE_KEYCHAIN", "1")
        .args([
            "providers",
            "install-model",
            "--provider",
            "codex-cli",
            "--model",
            "gemma4:e2b",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let json: serde_json::Value = serde_json::from_slice(&output).unwrap();
    assert_eq!(json["ok"], false);
    assert_eq!(json["provider"], "codex-cli");
    assert_eq!(json["status"], "unsupported_provider");
}

#[test]
fn providers_install_model_rejects_invalid_ollama_model_refs() {
    let tmp = tempfile::tempdir().unwrap();

    let output = Command::cargo_bin("alvum")
        .unwrap()
        .env("HOME", tmp.path())
        .env("ALVUM_DISABLE_KEYCHAIN", "1")
        .args([
            "providers",
            "install-model",
            "--provider",
            "ollama",
            "--model",
            "bad model",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let json: serde_json::Value = serde_json::from_slice(&output).unwrap();
    assert_eq!(json["ok"], false);
    assert_eq!(json["provider"], "ollama");
    assert_eq!(json["status"], "invalid_model");
}

#[test]
fn providers_test_unknown_provider_returns_json() {
    let tmp = tempfile::tempdir().unwrap();

    let output = Command::cargo_bin("alvum")
        .unwrap()
        .env("HOME", tmp.path())
        .env("ALVUM_DISABLE_KEYCHAIN", "1")
        .args(["providers", "test", "--provider", "no-such-provider"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let json: serde_json::Value = serde_json::from_slice(&output).unwrap();
    assert_eq!(json["provider"], "no-such-provider");
    assert_eq!(json["status"], "unknown_provider");
    assert_eq!(json["ok"], false);
}

#[test]
fn providers_bootstrap_enables_only_live_passing_providers() {
    let tmp = tempfile::tempdir().unwrap();

    let output = Command::cargo_bin("alvum")
        .unwrap()
        .env("HOME", tmp.path())
        .env("ALVUM_DISABLE_KEYCHAIN", "1")
        .env("PATH", "")
        .args(["providers", "bootstrap"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let json: serde_json::Value = serde_json::from_slice(&output).unwrap();
    assert_eq!(json["ok"], true);
    assert_eq!(json["skipped"], false);
    assert!(json["enabled"].as_array().unwrap().is_empty());
    assert!(
        json["providers"]
            .as_array()
            .unwrap()
            .iter()
            .all(|provider| provider["ok"] == false)
    );

    let config = std::fs::read_to_string(tmp.path().join(".alvum/runtime/config.toml")).unwrap();
    let doc: toml::Value = config.parse().unwrap();
    for provider in [
        "claude-cli",
        "codex-cli",
        "anthropic-api",
        "bedrock",
        "ollama",
    ] {
        assert_eq!(doc["providers"][provider]["enabled"].as_bool(), Some(false));
    }
    assert!(
        tmp.path()
            .join(".alvum/runtime/provider-bootstrap.json")
            .exists()
    );
}

#[test]
fn providers_configure_saves_provider_settings_without_secret_values() {
    let tmp = tempfile::tempdir().unwrap();

    let output = Command::cargo_bin("alvum")
        .unwrap()
        .env("HOME", tmp.path())
        .env("ALVUM_DISABLE_KEYCHAIN", "1")
        .write_stdin(r#"{"enabled":true,"settings":{"base_url":"http://localhost:11435","model":"llama3.1"}}"#)
        .args(["providers", "configure", "ollama"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let json: serde_json::Value = serde_json::from_slice(&output).unwrap();
    assert_eq!(json["ok"], true);
    assert_eq!(json["provider"], "ollama");
    assert_eq!(json["enabled"], true);

    let config = std::fs::read_to_string(tmp.path().join(".alvum/runtime/config.toml")).unwrap();
    let doc: toml::Value = config.parse().unwrap();
    assert_eq!(
        doc["providers"]["ollama"]["base_url"].as_str(),
        Some("http://localhost:11435")
    );
    assert_eq!(
        doc["providers"]["ollama"]["model"].as_str(),
        Some("llama3.1")
    );
    assert!(config.contains("llama3.1"));
    assert!(!config.contains("api_key"));
}

#[test]
fn providers_disable_active_provider_resets_active_to_auto() {
    let tmp = tempfile::tempdir().unwrap();

    let output = Command::cargo_bin("alvum")
        .unwrap()
        .env("HOME", tmp.path())
        .env("ALVUM_DISABLE_KEYCHAIN", "1")
        .args(["providers", "set-active", "codex-cli"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let json: serde_json::Value = serde_json::from_slice(&output).unwrap();
    assert_eq!(json["ok"], true);
    assert_eq!(json["configured"], "codex-cli");
    Command::cargo_bin("alvum")
        .unwrap()
        .env("HOME", tmp.path())
        .env("ALVUM_DISABLE_KEYCHAIN", "1")
        .args(["providers", "disable", "codex-cli"])
        .assert()
        .success();

    let config = std::fs::read_to_string(tmp.path().join(".alvum/runtime/config.toml")).unwrap();
    let doc: toml::Value = config.parse().unwrap();
    assert_eq!(doc["pipeline"]["provider"].as_str(), Some("auto"));
    assert_eq!(
        doc["providers"]["codex-cli"]["enabled"].as_bool(),
        Some(false)
    );
}

#[test]
fn doctor_json_reports_config_parse_errors_without_failing_command() {
    let tmp = tempfile::tempdir().unwrap();
    let config_dir = tmp.path().join(".alvum/runtime");
    std::fs::create_dir_all(&config_dir).unwrap();
    std::fs::write(config_dir.join("config.toml"), "not = [valid").unwrap();

    let output = Command::cargo_bin("alvum")
        .unwrap()
        .env("HOME", tmp.path())
        .args(["doctor", "--json"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let json: serde_json::Value = serde_json::from_slice(&output).unwrap();
    let checks = json["checks"].as_array().unwrap();
    let config = checks.iter().find(|check| check["id"] == "config").unwrap();

    assert_eq!(json["ok"], false);
    assert_eq!(config["level"], "error");
    assert!(
        config["message"]
            .as_str()
            .unwrap()
            .contains("failed to parse config")
    );
}
