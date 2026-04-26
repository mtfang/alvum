//! Vision model screenshot description with actor attribution hints.

use alvum_core::data_ref::DataRef;
use alvum_core::llm::complete_with_image_observed;
use alvum_core::observation::{MediaRef, Observation};
use alvum_pipeline::llm::LlmProvider;
use anyhow::{Context, Result};
use std::path::Path;
use tracing::{debug, info, warn};

const VISION_SYSTEM_PROMPT: &str = r#"You are describing a screenshot for a life-logging system. Your output will be used to understand what the user was doing at this moment.

Describe what is on this screen in 1-3 sentences. Focus on:
- What application is shown and what the user appears to be doing
- Any visible content that indicates work activity (documents, code, messages, forms)
- Any notable state (errors, notifications, loading states)

Also identify any ACTORS visible on screen. Look for:
- Active speaker indicators in video calls (highlighted participant name)
- AI tool output (Claude, Copilot, ChatGPT responses visible)
- Bot messages in chat apps (deploy-bot, CI notifications, automated messages)
- System notifications or alerts (not caused by a human)
- Other people's names visible in chat, email, or meeting participant lists

Do NOT describe UI chrome (toolbars, menubars, scroll bars).
Be specific about content visible on screen.

Output as JSON:
{
  "description": "1-3 sentence description of what's on screen",
  "actors": [
    {"name": "actor_identifier", "kind": "person|agent|self|organization|environment", "confidence": 0.0-1.0, "signal": "what you saw"}
  ]
}

The "actors" array can be empty if no specific actors are identifiable beyond the user.
Output ONLY the JSON object. No markdown, no explanation."#;

/// Process a batch of screen DataRefs into Observations using a vision model.
///
/// Each DataRef must point to a PNG screenshot file. The `capture_dir` is used
/// to resolve relative paths in DataRef.path.
pub async fn process_screen_data_refs(
    provider: &dyn LlmProvider,
    data_refs: &[DataRef],
    capture_dir: &Path,
) -> Result<Vec<Observation>> {
    info!(screenshots = data_refs.len(), "processing screen captures");

    let mut observations = Vec::new();

    for data_ref in data_refs {
        match describe_screenshot(provider, data_ref, capture_dir).await {
            Ok(obs) => observations.push(obs),
            Err(e) => {
                warn!(path = %data_ref.path, error = %e, "failed to process screenshot");
            }
        }
        // Advance the shared per-file counter regardless of result so
        // the bar reflects user-visible inputs consumed.
        alvum_core::progress::tick_stage(alvum_core::progress::STAGE_PROCESS);
    }

    info!(observations = observations.len(), "screen processing complete");
    Ok(observations)
}

/// Describe a single screenshot and produce an Observation.
async fn describe_screenshot(
    provider: &dyn LlmProvider,
    data_ref: &DataRef,
    capture_dir: &Path,
) -> Result<Observation> {
    // Resolve the image path (DataRef.path is relative to capture_dir)
    let image_path = if Path::new(&data_ref.path).is_absolute() {
        std::path::PathBuf::from(&data_ref.path)
    } else {
        capture_dir.join(&data_ref.path)
    };

    if !image_path.exists() {
        anyhow::bail!("screenshot file not found: {}", image_path.display());
    }

    debug!(path = %image_path.display(), "describing screenshot");

    let user_message = "Describe this screenshot.";
    // Each screenshot is an independent LLM round-trip; tag the
    // call_site with the file basename so the event stream and tray
    // popover surface "vision/screen-1234.png" rather than a generic
    // "vision" lump that hides per-image latency.
    let call_site = format!(
        "vision/{}",
        image_path
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("unknown")
    );
    let response = complete_with_image_observed(
        provider,
        VISION_SYSTEM_PROMPT,
        user_message,
        &image_path,
        &call_site,
    )
    .await
    .with_context(|| format!("vision call failed for {}", image_path.display()))?;

    // Parse the structured response
    let json_str = alvum_pipeline::util::strip_markdown_fences(&response);
    let parsed: VisionResponse = serde_json::from_str(json_str).unwrap_or_else(|e| {
        warn!(error = %e, raw_len = response.len(),
            "vision response not JSON; using raw text as description");
        VisionResponse {
            description: response.clone(),
            actors: vec![],
        }
    });

    // Build actor_hints from capture metadata + vision model actors
    let mut actor_hints: Vec<serde_json::Value> = Vec::new();

    // Carry forward capture-time hints (Layer 1)
    if let Some(meta) = &data_ref.metadata {
        if let Some(hints) = meta.get("actor_hints") {
            if let Some(arr) = hints.as_array() {
                actor_hints.extend(arr.iter().cloned());
            }
        }
    }

    // Add vision-detected actors (Layer 2)
    for actor in &parsed.actors {
        actor_hints.push(serde_json::json!({
            "actor": actor.name,
            "kind": actor.kind,
            "confidence": actor.confidence,
            "signal": actor.signal,
        }));
    }

    // Build metadata from capture metadata + enrichment
    let mut metadata = data_ref.metadata.clone().unwrap_or(serde_json::json!({}));
    if let Some(obj) = metadata.as_object_mut() {
        obj.insert("actor_hints".into(), serde_json::json!(actor_hints));
    }

    Ok(Observation {
        ts: data_ref.ts,
        source: "screen".into(),
        kind: "screen_capture".into(),
        content: parsed.description,
        metadata: Some(metadata),
        media_ref: Some(MediaRef {
            path: data_ref.path.clone(),
            mime: "image/png".into(),
        }),
    })
}

#[derive(serde::Deserialize)]
struct VisionResponse {
    description: String,
    #[serde(default)]
    actors: Vec<VisionActor>,
}

#[derive(serde::Deserialize)]
struct VisionActor {
    name: String,
    kind: String,
    confidence: f64,
    signal: String,
}

/// Build the vision prompt for external use (e.g., testing prompt content).
pub fn vision_system_prompt() -> &'static str {
    VISION_SYSTEM_PROMPT
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vision_prompt_requests_json_output() {
        let prompt = vision_system_prompt();
        assert!(prompt.contains("Output as JSON"));
        assert!(prompt.contains("description"));
        assert!(prompt.contains("actors"));
    }

    #[test]
    fn vision_prompt_asks_for_actor_identification() {
        let prompt = vision_system_prompt();
        assert!(prompt.contains("Active speaker indicators"));
        assert!(prompt.contains("AI tool output"));
        assert!(prompt.contains("Bot messages"));
        assert!(prompt.contains("System notifications"));
    }

    #[test]
    fn vision_response_parses_with_actors() {
        let json = r#"{
            "description": "VS Code showing main.rs with Rust code.",
            "actors": [
                {"name": "claude", "kind": "agent", "confidence": 0.8, "signal": "Claude Code terminal visible"}
            ]
        }"#;
        let resp: VisionResponse = serde_json::from_str(json).unwrap();
        assert_eq!(resp.description, "VS Code showing main.rs with Rust code.");
        assert_eq!(resp.actors.len(), 1);
        assert_eq!(resp.actors[0].name, "claude");
        assert_eq!(resp.actors[0].kind, "agent");
    }

    #[test]
    fn vision_response_parses_without_actors() {
        let json = r#"{
            "description": "Desktop wallpaper with no applications open.",
            "actors": []
        }"#;
        let resp: VisionResponse = serde_json::from_str(json).unwrap();
        assert_eq!(resp.actors.len(), 0);
    }

    #[test]
    fn vision_response_defaults_empty_actors() {
        let json = r#"{"description": "Just a description."}"#;
        let resp: VisionResponse = serde_json::from_str(json).unwrap();
        assert!(resp.actors.is_empty());
    }

    #[test]
    fn actor_hints_merge_capture_and_vision_layers() {
        // Simulate what describe_screenshot does with metadata merging
        let capture_hints = serde_json::json!([
            {"actor": "self", "kind": "self", "confidence": 0.4, "signal": "screen_active_app"}
        ]);
        let vision_actors = vec![
            VisionActor {
                name: "sarah_chen".into(),
                kind: "person".into(),
                confidence: 0.6,
                signal: "active speaker in Zoom".into(),
            },
        ];

        let mut merged: Vec<serde_json::Value> = Vec::new();

        // Layer 1: capture hints
        if let Some(arr) = capture_hints.as_array() {
            merged.extend(arr.iter().cloned());
        }

        // Layer 2: vision actors
        for actor in &vision_actors {
            merged.push(serde_json::json!({
                "actor": actor.name,
                "kind": actor.kind,
                "confidence": actor.confidence,
                "signal": actor.signal,
            }));
        }

        assert_eq!(merged.len(), 2);
        assert_eq!(merged[0]["actor"], "self");
        assert_eq!(merged[0]["confidence"], 0.4);
        assert_eq!(merged[1]["actor"], "sarah_chen");
        assert_eq!(merged[1]["kind"], "person");
        assert_eq!(merged[1]["confidence"], 0.6);
    }

    #[test]
    fn observation_from_vision_has_correct_fields() {
        let obs = Observation {
            ts: "2026-04-12T09:00:15Z".parse().unwrap(),
            source: "screen".into(),
            kind: "screen_capture".into(),
            content: "VS Code showing main.rs with a Rust function.".into(),
            metadata: Some(serde_json::json!({
                "app": "VS Code",
                "window": "main.rs",
                "trigger": "idle",
                "actor_hints": [
                    {"actor": "self", "kind": "self", "confidence": 0.4, "signal": "screen_active_app"},
                    {"actor": "claude", "kind": "agent", "confidence": 0.7, "signal": "Claude Code terminal visible"}
                ]
            })),
            media_ref: Some(MediaRef {
                path: "screen/images/09-00-15.png".into(),
                mime: "image/png".into(),
            }),
        };

        assert_eq!(obs.source, "screen");
        assert_eq!(obs.kind, "screen_capture");
        let hints = obs.metadata.as_ref().unwrap()["actor_hints"].as_array().unwrap();
        assert_eq!(hints.len(), 2);
        assert!(obs.media_ref.is_some());
    }
}
