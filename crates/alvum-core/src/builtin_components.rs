//! Virtual extension manifests for Alvum's built-in components.
//!
//! These are not installed packages and they are not launched as external
//! HTTP services. They give the extension runtime a stable catalog for core
//! captures/processors/connectors so external packages can route against
//! built-in data without taking ownership of the built-in lifecycle.

use crate::extension::{
    CaptureComponent, ConnectorComponent, ExtensionManifest, ExtensionServer, ProcessorComponent,
    RouteDescriptor, RouteSelector, SourceDescriptor,
};

pub fn manifests() -> Vec<ExtensionManifest> {
    vec![audio_manifest(), screen_manifest(), session_manifest()]
}

pub fn manifest(package_id: &str) -> Option<ExtensionManifest> {
    manifests()
        .into_iter()
        .find(|manifest| manifest.id == package_id)
}

pub fn capture_component(component_id: &str) -> Option<CaptureComponent> {
    let (package_id, local_id) = component_id.split_once('/')?;
    manifest(package_id)?
        .captures
        .into_iter()
        .find(|capture| capture.id == local_id)
}

pub fn processor_component(component_id: &str) -> Option<ProcessorComponent> {
    let (package_id, local_id) = component_id.split_once('/')?;
    manifest(package_id)?
        .processors
        .into_iter()
        .find(|processor| processor.id == local_id)
}

fn builtin_server() -> ExtensionServer {
    ExtensionServer {
        start: vec!["builtin".into()],
        health_path: "/v1/health".into(),
        startup_timeout_ms: 0,
    }
}

fn source(id: &str, display_name: &str, expected: bool) -> SourceDescriptor {
    SourceDescriptor {
        id: id.into(),
        display_name: display_name.into(),
        expected,
    }
}

fn selector(component: &str, schema: Option<&str>, mime: Option<&str>) -> RouteSelector {
    RouteSelector {
        component: component.into(),
        source: None,
        mime: mime.map(str::to_string),
        schema: schema.map(str::to_string),
    }
}

fn route(from: RouteSelector, to: &str) -> RouteDescriptor {
    RouteDescriptor {
        from,
        to: vec![to.into()],
    }
}

fn audio_manifest() -> ExtensionManifest {
    ExtensionManifest {
        schema_version: 1,
        id: "alvum.audio".into(),
        name: "Alvum Audio".into(),
        version: env!("CARGO_PKG_VERSION").into(),
        description: "Built-in microphone and system-audio capture with transcription.".into(),
        server: builtin_server(),
        captures: vec![
            CaptureComponent {
                id: "audio-mic".into(),
                display_name: "Microphone audio".into(),
                description: "Built-in microphone capture source.".into(),
                sources: vec![source("audio-mic", "Microphone", true)],
                schemas: vec!["alvum.audio.opus.v1".into(), "alvum.audio.wav.v1".into()],
            },
            CaptureComponent {
                id: "audio-system".into(),
                display_name: "System audio".into(),
                description: "Built-in system-audio capture source.".into(),
                sources: vec![source("audio-system", "System audio", true)],
                schemas: vec!["alvum.audio.opus.v1".into(), "alvum.audio.wav.v1".into()],
            },
        ],
        processors: vec![ProcessorComponent {
            id: "whisper".into(),
            display_name: "Whisper transcription".into(),
            description: "Built-in audio transcription processor.".into(),
            accepts: vec![
                selector("alvum.audio/audio-mic", Some("alvum.audio.opus.v1"), None),
                selector("alvum.audio/audio-mic", Some("alvum.audio.wav.v1"), None),
                selector(
                    "alvum.audio/audio-system",
                    Some("alvum.audio.opus.v1"),
                    None,
                ),
                selector("alvum.audio/audio-system", Some("alvum.audio.wav.v1"), None),
            ],
        }],
        analyses: Vec::new(),
        connectors: vec![ConnectorComponent {
            id: "audio".into(),
            display_name: "Audio".into(),
            description: "Built-in user-facing audio connector.".into(),
            routes: vec![
                route(
                    selector("alvum.audio/audio-mic", None, None),
                    "alvum.audio/whisper",
                ),
                route(
                    selector("alvum.audio/audio-system", None, None),
                    "alvum.audio/whisper",
                ),
            ],
            analyses: Vec::new(),
        }],
        permissions: Vec::new(),
    }
}

fn screen_manifest() -> ExtensionManifest {
    ExtensionManifest {
        schema_version: 1,
        id: "alvum.screen".into(),
        name: "Alvum Screen".into(),
        version: env!("CARGO_PKG_VERSION").into(),
        description: "Built-in screen snapshot capture and visual processor.".into(),
        server: builtin_server(),
        captures: vec![CaptureComponent {
            id: "snapshot".into(),
            display_name: "Screen snapshot".into(),
            description: "Built-in periodic screen snapshot capture source.".into(),
            sources: vec![source("screen", "Screen", true)],
            schemas: vec!["alvum.screen.image.v1".into()],
        }],
        processors: vec![ProcessorComponent {
            id: "vision".into(),
            display_name: "Vision/OCR".into(),
            description: "Built-in screen image processor.".into(),
            accepts: vec![selector(
                "alvum.screen/snapshot",
                Some("alvum.screen.image.v1"),
                None,
            )],
        }],
        analyses: Vec::new(),
        connectors: vec![ConnectorComponent {
            id: "screen".into(),
            display_name: "Screen".into(),
            description: "Built-in user-facing screen connector.".into(),
            routes: vec![route(
                selector("alvum.screen/snapshot", None, None),
                "alvum.screen/vision",
            )],
            analyses: Vec::new(),
        }],
        permissions: Vec::new(),
    }
}

fn session_manifest() -> ExtensionManifest {
    ExtensionManifest {
        schema_version: 1,
        id: "alvum.session".into(),
        name: "Alvum Sessions".into(),
        version: env!("CARGO_PKG_VERSION").into(),
        description: "Built-in Claude Code and Codex session importers.".into(),
        server: builtin_server(),
        captures: vec![
            CaptureComponent {
                id: "claude-code".into(),
                display_name: "Claude Code sessions".into(),
                description: "Built-in Claude Code JSONL session importer.".into(),
                sources: vec![source("claude-code", "Claude Code", true)],
                schemas: vec!["alvum.session.jsonl.v1".into()],
            },
            CaptureComponent {
                id: "codex".into(),
                display_name: "Codex sessions".into(),
                description: "Built-in Codex JSONL session importer.".into(),
                sources: vec![source("codex", "Codex", true)],
                schemas: vec!["alvum.session.jsonl.v1".into()],
            },
        ],
        processors: vec![
            ProcessorComponent {
                id: "claude-code-parser".into(),
                display_name: "Claude Code parser".into(),
                description: "Built-in Claude Code session processor.".into(),
                accepts: vec![selector(
                    "alvum.session/claude-code",
                    Some("alvum.session.jsonl.v1"),
                    None,
                )],
            },
            ProcessorComponent {
                id: "codex-parser".into(),
                display_name: "Codex parser".into(),
                description: "Built-in Codex session processor.".into(),
                accepts: vec![selector(
                    "alvum.session/codex",
                    Some("alvum.session.jsonl.v1"),
                    None,
                )],
            },
        ],
        analyses: Vec::new(),
        connectors: vec![
            ConnectorComponent {
                id: "claude-code".into(),
                display_name: "Claude Code".into(),
                description: "Built-in Claude Code connector.".into(),
                routes: vec![route(
                    selector("alvum.session/claude-code", None, None),
                    "alvum.session/claude-code-parser",
                )],
                analyses: Vec::new(),
            },
            ConnectorComponent {
                id: "codex".into(),
                display_name: "Codex".into(),
                description: "Built-in Codex connector.".into(),
                routes: vec![route(
                    selector("alvum.session/codex", None, None),
                    "alvum.session/codex-parser",
                )],
                analyses: Vec::new(),
            },
        ],
        permissions: Vec::new(),
    }
}
