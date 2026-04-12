# Screen Capture + Actor Attribution Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build macOS screen capture (screenshot + vision model) and three-layer actor attribution, enabling true cross-source episodic threading.

**Architecture:** Two new crates: `alvum-capture-screen` (triggers + screenshot + writer) and `alvum-processor-screen` (vision model description + actor hints). LlmProvider gains image support. CLI gets `capture-screen` command and screen data wiring in cross-source extract mode.

**Tech Stack:** Rust, `screencapturekit` (ScreenCaptureKit), `objc2`/`objc2-app-kit` (NSWorkspace), `image` (PNG), `base64` (API encoding), existing alvum crates.

---

## File Structure

```
alvum/
├── crates/
│   ├── alvum-capture-screen/
│   │   ├── Cargo.toml
│   │   └── src/
│   │       ├── lib.rs              re-exports
│   │       ├── writer.rs           save PNG + append DataRef to captures.jsonl
│   │       ├── screenshot.rs       ScreenCaptureKit active window capture
│   │       ├── trigger.rs          NSWorkspace listener + idle timer
│   │       └── daemon.rs           orchestrator: triggers + screenshot + writer
│   ├── alvum-processor-screen/
│   │   ├── Cargo.toml
│   │   └── src/
│   │       ├── lib.rs              re-exports
│   │       └── describe.rs         vision model screenshot description + actor hints
│   ├── alvum-pipeline/             (modified)
│   │   └── src/
│   │       └── llm.rs              add complete_with_image to LlmProvider trait
│   ├── alvum-episode/              (modified)
│   │   └── src/
│   │       └── threading.rs        add attribution instructions to prompt
│   ├── alvum-pipeline/             (modified)
│   │   └── src/
│   │       └── distill.rs          enhance extraction prompt with actor context
│   └── alvum-cli/                  (modified)
│       └── src/
│           └── main.rs             add capture-screen subcommand + screen data in extract
```

---

### Task 1: LlmProvider Vision Extension

Add `complete_with_image` default method to `LlmProvider` trait. Implement real vision support for `OllamaProvider` (primary, free, local) and `AnthropicApiProvider` (premium, paid) using their respective image APIs. `ClaudeCliProvider` gets the default fallback (text-only, Claude CLI doesn't support image input in `-p` mode).

**Files:**
- Modify: `crates/alvum-pipeline/Cargo.toml`
- Modify: `crates/alvum-pipeline/src/llm.rs`

- [ ] **Step 1: Add `base64` dependency to alvum-pipeline**

```toml
# crates/alvum-pipeline/Cargo.toml
[package]
name = "alvum-pipeline"
version = "0.1.0"
edition = "2024"

[dependencies]
alvum-core = { path = "../alvum-core" }
serde.workspace = true
serde_json.workspace = true
chrono.workspace = true
anyhow.workspace = true
tokio.workspace = true
tracing.workspace = true
reqwest.workspace = true
async-trait = "0.1"
base64 = "0.22"
```

- [ ] **Step 2: Add `complete_with_image` to `LlmProvider` trait**

In `crates/alvum-pipeline/src/llm.rs`, add the import and default method:

```rust
// At the top of the file, add:
use std::path::Path;

// Replace the LlmProvider trait with:
/// Provider-agnostic LLM interface. Implementations handle the transport
/// (HTTP API, CLI subprocess, local model) — callers just send prompts.
#[async_trait::async_trait]
pub trait LlmProvider: Send + Sync {
    async fn complete(&self, system: &str, user_message: &str) -> Result<String>;

    /// Complete with an image attachment. Providers that support vision implement
    /// this directly; others fall back to text-only (image is ignored).
    async fn complete_with_image(
        &self,
        system: &str,
        user_message: &str,
        image_path: &Path,
    ) -> Result<String> {
        let _ = image_path; // default: ignore image
        self.complete(system, user_message).await
    }

    fn name(&self) -> &str;
}
```

- [ ] **Step 3: Add multimodal API types for AnthropicApiProvider**

In `crates/alvum-pipeline/src/llm.rs`, add these types after the existing `ApiMessage` struct:

```rust
/// A content block in a multimodal API message (text or image).
#[derive(Serialize)]
#[serde(tag = "type")]
enum ContentBlock2 {
    #[serde(rename = "text")]
    Text { text: String },
    #[serde(rename = "image")]
    Image { source: ImageSource },
}

#[derive(Serialize)]
struct ImageSource {
    #[serde(rename = "type")]
    source_type: String,
    media_type: String,
    data: String,
}

/// API message with multimodal content blocks.
#[derive(Serialize)]
struct ApiMessageMultimodal {
    role: String,
    content: Vec<ContentBlock2>,
}

/// API request that accepts multimodal content.
#[derive(Serialize)]
struct ApiRequestMultimodal {
    model: String,
    max_tokens: u32,
    system: String,
    messages: Vec<ApiMessageMultimodal>,
}
```

- [ ] **Step 4: Implement `complete_with_image` for AnthropicApiProvider**

In `crates/alvum-pipeline/src/llm.rs`, replace the `impl LlmProvider for AnthropicApiProvider` block with:

```rust
#[async_trait::async_trait]
impl LlmProvider for AnthropicApiProvider {
    async fn complete(&self, system: &str, user_message: &str) -> Result<String> {
        let request = ApiRequest {
            model: self.model.clone(),
            max_tokens: 16000,
            system: system.to_string(),
            messages: vec![ApiMessage {
                role: "user".into(),
                content: user_message.to_string(),
            }],
        };

        debug!(model = %self.model, system_len = system.len(), user_len = user_message.len(), "sending to Anthropic API");

        let response = self
            .http
            .post("https://api.anthropic.com/v1/messages")
            .header("x-api-key", &self.api_key)
            .header("anthropic-version", "2023-06-01")
            .header("content-type", "application/json")
            .json(&request)
            .send()
            .await
            .context("failed to send request to Claude API")?;

        let status = response.status();
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            bail!("Claude API error {status}: {body}");
        }

        let api_response: ApiResponse = response
            .json()
            .await
            .context("failed to parse Claude API response")?;

        let text = api_response
            .content
            .iter()
            .filter(|b| b.block_type == "text")
            .filter_map(|b| b.text.as_deref())
            .collect::<Vec<_>>()
            .join("\n");

        debug!(response_len = text.len(), "received API response");
        Ok(text)
    }

    async fn complete_with_image(
        &self,
        system: &str,
        user_message: &str,
        image_path: &Path,
    ) -> Result<String> {
        use base64::Engine;

        let image_bytes = tokio::fs::read(image_path)
            .await
            .with_context(|| format!("failed to read image: {}", image_path.display()))?;

        let b64 = base64::engine::general_purpose::STANDARD.encode(&image_bytes);

        // Infer media type from extension
        let media_type = match image_path.extension().and_then(|e| e.to_str()) {
            Some("png") => "image/png",
            Some("jpg" | "jpeg") => "image/jpeg",
            Some("webp") => "image/webp",
            Some("gif") => "image/gif",
            _ => "image/png",
        };

        let request = ApiRequestMultimodal {
            model: self.model.clone(),
            max_tokens: 16000,
            system: system.to_string(),
            messages: vec![ApiMessageMultimodal {
                role: "user".into(),
                content: vec![
                    ContentBlock2::Image {
                        source: ImageSource {
                            source_type: "base64".into(),
                            media_type: media_type.into(),
                            data: b64,
                        },
                    },
                    ContentBlock2::Text {
                        text: user_message.to_string(),
                    },
                ],
            }],
        };

        debug!(
            model = %self.model,
            image = %image_path.display(),
            "sending image to Anthropic API"
        );

        let response = self
            .http
            .post("https://api.anthropic.com/v1/messages")
            .header("x-api-key", &self.api_key)
            .header("anthropic-version", "2023-06-01")
            .header("content-type", "application/json")
            .json(&request)
            .send()
            .await
            .context("failed to send image request to Claude API")?;

        let status = response.status();
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            bail!("Claude API vision error {status}: {body}");
        }

        let api_response: ApiResponse = response
            .json()
            .await
            .context("failed to parse Claude API vision response")?;

        let text = api_response
            .content
            .iter()
            .filter(|b| b.block_type == "text")
            .filter_map(|b| b.text.as_deref())
            .collect::<Vec<_>>()
            .join("\n");

        debug!(response_len = text.len(), "received API vision response");
        Ok(text)
    }

    fn name(&self) -> &str {
        "anthropic-api"
    }
}
```

- [ ] **Step 5: Implement `complete_with_image` for OllamaProvider**

In `crates/alvum-pipeline/src/llm.rs`, replace the `impl LlmProvider for OllamaProvider` block with:

```rust
#[async_trait::async_trait]
impl LlmProvider for OllamaProvider {
    async fn complete(&self, system: &str, user_message: &str) -> Result<String> {
        debug!(model = %self.model, "sending to Ollama");

        let body = serde_json::json!({
            "model": self.model,
            "stream": false,
            "system": system,
            "prompt": user_message,
        });

        let response = self
            .http
            .post(format!("{}/api/generate", self.base_url))
            .json(&body)
            .send()
            .await
            .context("failed to connect to Ollama — is it running?")?;

        if !response.status().is_success() {
            let body = response.text().await.unwrap_or_default();
            bail!("Ollama error: {body}");
        }

        let resp: serde_json::Value = response.json().await?;
        let text = resp["response"]
            .as_str()
            .unwrap_or("")
            .to_string();

        debug!(response_len = text.len(), "received Ollama response");
        Ok(text)
    }

    async fn complete_with_image(
        &self,
        system: &str,
        user_message: &str,
        image_path: &Path,
    ) -> Result<String> {
        use base64::Engine;

        let image_bytes = tokio::fs::read(image_path)
            .await
            .with_context(|| format!("failed to read image: {}", image_path.display()))?;

        let b64 = base64::engine::general_purpose::STANDARD.encode(&image_bytes);

        debug!(model = %self.model, image = %image_path.display(), "sending image to Ollama");

        let body = serde_json::json!({
            "model": self.model,
            "stream": false,
            "system": system,
            "prompt": user_message,
            "images": [b64],
        });

        let response = self
            .http
            .post(format!("{}/api/generate", self.base_url))
            .json(&body)
            .send()
            .await
            .context("failed to connect to Ollama — is it running?")?;

        if !response.status().is_success() {
            let body = response.text().await.unwrap_or_default();
            bail!("Ollama vision error: {body}");
        }

        let resp: serde_json::Value = response.json().await?;
        let text = resp["response"]
            .as_str()
            .unwrap_or("")
            .to_string();

        debug!(response_len = text.len(), "received Ollama vision response");
        Ok(text)
    }

    fn name(&self) -> &str {
        "ollama"
    }
}
```

- [ ] **Step 6: Add tests for the vision extension**

Add these tests to the existing `#[cfg(test)] mod tests` block in `crates/alvum-pipeline/src/llm.rs`:

```rust
    #[test]
    fn multimodal_request_serializes_correctly() {
        let req = ApiRequestMultimodal {
            model: "claude-sonnet-4-6".into(),
            max_tokens: 4096,
            system: "Describe this image.".into(),
            messages: vec![ApiMessageMultimodal {
                role: "user".into(),
                content: vec![
                    ContentBlock2::Image {
                        source: ImageSource {
                            source_type: "base64".into(),
                            media_type: "image/png".into(),
                            data: "iVBORw0KGgo=".into(),
                        },
                    },
                    ContentBlock2::Text {
                        text: "What is on this screen?".into(),
                    },
                ],
            }],
        };
        let json = serde_json::to_value(&req).unwrap();
        assert_eq!(json["model"], "claude-sonnet-4-6");
        assert_eq!(json["messages"][0]["content"][0]["type"], "image");
        assert_eq!(json["messages"][0]["content"][0]["source"]["type"], "base64");
        assert_eq!(json["messages"][0]["content"][0]["source"]["media_type"], "image/png");
        assert_eq!(json["messages"][0]["content"][1]["type"], "text");
        assert_eq!(json["messages"][0]["content"][1]["text"], "What is on this screen?");
    }

    #[test]
    fn image_source_serializes_type_field() {
        let src = ImageSource {
            source_type: "base64".into(),
            media_type: "image/png".into(),
            data: "abc123".into(),
        };
        let json = serde_json::to_value(&src).unwrap();
        assert_eq!(json["type"], "base64");
        assert_eq!(json["media_type"], "image/png");
    }
```

- [ ] **Step 6: Verify**

```bash
cd /Users/michael/git/alvum && cargo test -p alvum-pipeline
```

- [ ] **Step 7: Commit**

```bash
cd /Users/michael/git/alvum && git add crates/alvum-pipeline/Cargo.toml crates/alvum-pipeline/src/llm.rs && git commit -m "feat: add complete_with_image to LlmProvider trait for vision model support"
```

---

### Task 2: Capture Screen Crate — Types + Writer

Create `alvum-capture-screen` with the writer module that saves PNG files and appends `DataRef` entries to `captures.jsonl`.

**Files:**
- Create: `crates/alvum-capture-screen/Cargo.toml`
- Create: `crates/alvum-capture-screen/src/lib.rs`
- Create: `crates/alvum-capture-screen/src/writer.rs`
- Modify: `Cargo.toml` (workspace members)

- [ ] **Step 1: Create Cargo.toml**

```toml
# crates/alvum-capture-screen/Cargo.toml
[package]
name = "alvum-capture-screen"
version = "0.1.0"
edition = "2024"

[dependencies]
alvum-core = { path = "../alvum-core" }
screencapturekit = { version = "1.5", features = ["macos_14_0"] }
objc2 = "0.6"
objc2-app-kit = { version = "0.3", features = ["NSWorkspace", "NSRunningApplication", "NSNotification"] }
objc2-foundation = { version = "0.3", features = ["NSNotification", "NSString", "NSThread"] }
image = { version = "0.25", default-features = false, features = ["png"] }
anyhow.workspace = true
tokio.workspace = true
tracing.workspace = true
chrono.workspace = true
serde.workspace = true
serde_json.workspace = true

[dev-dependencies]
tempfile = "3"
```

- [ ] **Step 2: Add to workspace**

In root `Cargo.toml`, add `"crates/alvum-capture-screen"` to the `members` list:

```toml
[workspace]
resolver = "2"
members = [
    "crates/alvum-core",
    "crates/alvum-connector-claude",
    "crates/alvum-pipeline",
    "crates/alvum-cli",
    "crates/alvum-capture-audio",
    "crates/alvum-processor-audio",
    "crates/alvum-episode",
    "crates/alvum-knowledge",
    "crates/alvum-capture-screen",
]
```

- [ ] **Step 3: Create lib.rs**

```rust
// crates/alvum-capture-screen/src/lib.rs

//! Screen capture daemon: captures active window screenshots on app focus change
//! and idle timer triggers.
//!
//! Captures are intentionally dumb — save PNG files and record DataRefs.
//! Interpretation (vision model) lives in alvum-processor-screen.

pub mod writer;
pub mod screenshot;
pub mod trigger;
pub mod daemon;
```

- [ ] **Step 4: Create writer.rs**

```rust
// crates/alvum-capture-screen/src/writer.rs

//! Saves PNG screenshots to disk and appends DataRef entries to captures.jsonl.

use alvum_core::data_ref::DataRef;
use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use std::path::{Path, PathBuf};
use tracing::info;

/// Manages writing screenshot files and their DataRef metadata.
pub struct ScreenWriter {
    /// Root capture directory (e.g., capture/2026-04-12)
    capture_dir: PathBuf,
    /// Directory for PNG images (capture_dir/screen/images/)
    images_dir: PathBuf,
    /// Path to captures.jsonl (capture_dir/screen/captures.jsonl)
    captures_jsonl: PathBuf,
}

impl ScreenWriter {
    pub fn new(capture_dir: PathBuf) -> Result<Self> {
        let images_dir = capture_dir.join("screen").join("images");
        let captures_jsonl = capture_dir.join("screen").join("captures.jsonl");
        std::fs::create_dir_all(&images_dir)
            .with_context(|| format!("failed to create images dir: {}", images_dir.display()))?;
        Ok(Self {
            capture_dir,
            images_dir,
            captures_jsonl,
        })
    }

    /// Save a PNG screenshot and record the DataRef.
    ///
    /// Returns the path to the written PNG file.
    pub fn save_screenshot(
        &self,
        png_bytes: &[u8],
        ts: DateTime<Utc>,
        app_name: &str,
        window_title: &str,
        trigger: &str,
    ) -> Result<PathBuf> {
        let filename = format!("{}.png", ts.format("%H-%M-%S"));
        let image_path = self.images_dir.join(&filename);

        std::fs::write(&image_path, png_bytes)
            .with_context(|| format!("failed to write screenshot: {}", image_path.display()))?;

        // Path relative to capture_dir for DataRef (portable across machines)
        let relative_path = format!("screen/images/{filename}");

        let data_ref = DataRef {
            ts,
            source: "screen".into(),
            path: relative_path,
            mime: "image/png".into(),
            metadata: Some(serde_json::json!({
                "app": app_name,
                "window": window_title,
                "trigger": trigger,
                "actor_hints": [{
                    "actor": "self",
                    "kind": "self",
                    "confidence": 0.4,
                    "signal": "screen_active_app"
                }]
            })),
        };

        alvum_core::storage::append_jsonl(&self.captures_jsonl, &data_ref)
            .context("failed to append DataRef to captures.jsonl")?;

        info!(
            path = %image_path.display(),
            app = app_name,
            trigger = trigger,
            "saved screenshot"
        );

        Ok(image_path)
    }

    /// Return the path to captures.jsonl for reading by processors.
    pub fn captures_jsonl_path(&self) -> &Path {
        &self.captures_jsonl
    }

    /// Return the capture directory root.
    pub fn capture_dir(&self) -> &Path {
        &self.capture_dir
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn minimal_png() -> Vec<u8> {
        // 1x1 red PNG — smallest valid PNG for testing
        let mut buf = Vec::new();
        let mut encoder = image::codecs::png::PngEncoder::new(&mut buf);
        image::ImageEncoder::write_image(
            &mut encoder,
            &[255, 0, 0, 255], // RGBA red pixel
            1,
            1,
            image::ExtendedColorType::Rgba8,
        )
        .unwrap();
        buf
    }

    #[test]
    fn writer_creates_directory_structure() {
        let tmp = TempDir::new().unwrap();
        let writer = ScreenWriter::new(tmp.path().to_path_buf()).unwrap();

        assert!(tmp.path().join("screen").join("images").is_dir());
        assert_eq!(
            writer.captures_jsonl_path(),
            tmp.path().join("screen").join("captures.jsonl")
        );
    }

    #[test]
    fn save_screenshot_writes_png_and_dataref() {
        let tmp = TempDir::new().unwrap();
        let writer = ScreenWriter::new(tmp.path().to_path_buf()).unwrap();

        let ts: DateTime<Utc> = "2026-04-12T09:00:15Z".parse().unwrap();
        let png = minimal_png();

        let path = writer
            .save_screenshot(&png, ts, "VS Code", "main.rs", "app_focus")
            .unwrap();

        // PNG file exists
        assert!(path.exists());
        assert_eq!(path.file_name().unwrap(), "09-00-15.png");

        // DataRef recorded in captures.jsonl
        let refs: Vec<DataRef> =
            alvum_core::storage::read_jsonl(writer.captures_jsonl_path()).unwrap();
        assert_eq!(refs.len(), 1);
        assert_eq!(refs[0].source, "screen");
        assert_eq!(refs[0].mime, "image/png");
        assert_eq!(refs[0].path, "screen/images/09-00-15.png");

        let meta = refs[0].metadata.as_ref().unwrap();
        assert_eq!(meta["app"], "VS Code");
        assert_eq!(meta["window"], "main.rs");
        assert_eq!(meta["trigger"], "app_focus");
        assert_eq!(meta["actor_hints"][0]["actor"], "self");
        assert_eq!(meta["actor_hints"][0]["confidence"], 0.4);
    }

    #[test]
    fn save_multiple_screenshots_appends_to_jsonl() {
        let tmp = TempDir::new().unwrap();
        let writer = ScreenWriter::new(tmp.path().to_path_buf()).unwrap();
        let png = minimal_png();

        let ts1: DateTime<Utc> = "2026-04-12T09:00:15Z".parse().unwrap();
        let ts2: DateTime<Utc> = "2026-04-12T09:00:45Z".parse().unwrap();

        writer
            .save_screenshot(&png, ts1, "VS Code", "main.rs", "app_focus")
            .unwrap();
        writer
            .save_screenshot(&png, ts2, "VS Code", "main.rs", "idle")
            .unwrap();

        let refs: Vec<DataRef> =
            alvum_core::storage::read_jsonl(writer.captures_jsonl_path()).unwrap();
        assert_eq!(refs.len(), 2);
        assert_eq!(refs[0].path, "screen/images/09-00-15.png");
        assert_eq!(refs[1].path, "screen/images/09-00-45.png");
    }
}
```

- [ ] **Step 5: Verify**

```bash
cd /Users/michael/git/alvum && cargo test -p alvum-capture-screen -- writer
```

- [ ] **Step 6: Commit**

```bash
cd /Users/michael/git/alvum && git add crates/alvum-capture-screen/Cargo.toml crates/alvum-capture-screen/src/lib.rs crates/alvum-capture-screen/src/writer.rs Cargo.toml && git commit -m "feat: add alvum-capture-screen crate with PNG writer and DataRef tracking"
```

---

### Task 3: Capture Screen — Screenshot Module

Implement active window screenshot via ScreenCaptureKit. This is a thin safe wrapper around the FFI.

**Files:**
- Create: `crates/alvum-capture-screen/src/screenshot.rs`

- [ ] **Step 1: Implement screenshot.rs**

```rust
// crates/alvum-capture-screen/src/screenshot.rs

//! Active window screenshot via ScreenCaptureKit.
//!
//! Wraps ScreenCaptureKit's SCScreenshotManager to capture the frontmost window
//! as a PNG byte buffer. Requires macOS 14+ and Screen Recording permission.

use anyhow::{bail, Context, Result};
use screencapturekit::{
    shareable_content::SCShareableContent,
    sc_screenshot_manager::SCScreenshotManager,
    stream::content_filter::SCContentFilter,
    stream::sc_stream::SCStreamConfiguration,
};
use tracing::debug;

/// Result of a window screenshot: PNG bytes + metadata about what was captured.
pub struct Screenshot {
    /// Raw PNG image bytes, ready to write to disk.
    pub png_bytes: Vec<u8>,
    /// Application name (e.g., "VS Code").
    pub app_name: String,
    /// Window title (e.g., "main.rs").
    pub window_title: String,
}

/// Capture a screenshot of the frontmost window.
///
/// Uses ScreenCaptureKit (macOS 14+). Requires Screen Recording permission.
/// Returns `None` if no capturable window is found (e.g., desktop is focused).
pub fn capture_frontmost_window() -> Result<Option<Screenshot>> {
    // Get all shareable content (windows, displays, apps)
    let content = SCShareableContent::get()
        .context("failed to get shareable content — is Screen Recording permission granted?")?;

    let windows = content.windows();
    if windows.is_empty() {
        debug!("no capturable windows found");
        return Ok(None);
    }

    // Find the frontmost window: ScreenCaptureKit returns windows ordered by
    // layer, with the frontmost on-screen windows first. We take the first
    // window that belongs to an application (skip system UI elements).
    let window = match windows.iter().find(|w| {
        w.owning_application()
            .map(|app| !app.bundle_identifier().unwrap_or_default().is_empty())
            .unwrap_or(false)
    }) {
        Some(w) => w,
        None => {
            debug!("no application windows found");
            return Ok(None);
        }
    };

    let app_name = window
        .owning_application()
        .and_then(|app| app.application_name())
        .unwrap_or_else(|| "Unknown".into());

    let window_title = window.title().unwrap_or_else(|| "Untitled".into());

    debug!(app = %app_name, window = %window_title, "capturing window");

    // Create a filter targeting just this window
    let filter = SCContentFilter::new()
        .with_desktop_independent_window(window);

    // Configure capture resolution to match the window
    let config = SCStreamConfiguration::new()
        .with_width(window.frame().size.width as u32)
        .with_height(window.frame().size.height as u32);

    // Capture a single frame as CGImage
    let cg_image = SCScreenshotManager::capture_image(&filter, &config)
        .context("ScreenCaptureKit screenshot failed")?;

    // Convert CGImage to PNG bytes
    let png_bytes = cgimage_to_png(&cg_image)?;

    Ok(Some(Screenshot {
        png_bytes,
        app_name,
        window_title,
    }))
}

/// Convert a CGImage to PNG bytes using the `image` crate.
fn cgimage_to_png(cg_image: &screencapturekit::sc_screenshot_manager::CGImage) -> Result<Vec<u8>> {
    let width = cg_image.width() as u32;
    let height = cg_image.height() as u32;
    let raw_data = cg_image.data();

    if raw_data.is_empty() {
        bail!("CGImage returned empty pixel data");
    }

    // CGImage data is BGRA; convert to RGBA for the image crate
    let mut rgba = Vec::with_capacity((width * height * 4) as usize);
    for chunk in raw_data.chunks_exact(4) {
        rgba.push(chunk[2]); // R (from B position in BGRA)
        rgba.push(chunk[1]); // G
        rgba.push(chunk[0]); // B (from R position in BGRA)
        rgba.push(chunk[3]); // A
    }

    let mut png_buf = Vec::new();
    let encoder = image::codecs::png::PngEncoder::new(&mut png_buf);
    image::ImageEncoder::write_image(
        &encoder,
        &rgba,
        width,
        height,
        image::ExtendedColorType::Rgba8,
    )
    .context("failed to encode PNG")?;

    Ok(png_buf)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Requires Screen Recording permission — run manually.
    #[test]
    #[ignore]
    fn capture_frontmost_produces_png() {
        let result = capture_frontmost_window().unwrap();
        // There should be at least one window open (the terminal running tests)
        let screenshot = result.expect("expected a capturable window");
        assert!(!screenshot.png_bytes.is_empty());
        assert!(!screenshot.app_name.is_empty());
        // Verify PNG magic bytes
        assert_eq!(&screenshot.png_bytes[..4], &[0x89, 0x50, 0x4E, 0x47]);
    }

    /// Requires Screen Recording permission — run manually.
    #[test]
    #[ignore]
    fn capture_returns_valid_metadata() {
        let result = capture_frontmost_window().unwrap();
        if let Some(screenshot) = result {
            // App name should be a real application
            assert!(!screenshot.app_name.is_empty());
            // PNG should be at least a few KB for a real window
            assert!(screenshot.png_bytes.len() > 100);
        }
    }
}
```

> **Note:** The exact ScreenCaptureKit API surface may differ from what's shown here. The `screencapturekit` crate wraps Apple's Objective-C framework, and method names may use slightly different conventions (e.g., builder pattern vs. direct constructors). At implementation time, check `screencapturekit` docs and adjust the filter/config/capture calls to match the actual API. The overall flow (get content -> find window -> create filter -> capture image -> convert to PNG) is stable.

- [ ] **Step 2: Verify compilation**

```bash
cd /Users/michael/git/alvum && cargo check -p alvum-capture-screen
```

If the `screencapturekit` API differs from what's shown, adjust the code to match the actual crate API. The key contract: `capture_frontmost_window() -> Result<Option<Screenshot>>` with `Screenshot { png_bytes, app_name, window_title }`.

- [ ] **Step 3: Run ignored tests manually (requires Screen Recording permission)**

```bash
cd /Users/michael/git/alvum && cargo test -p alvum-capture-screen -- screenshot --ignored
```

- [ ] **Step 4: Commit**

```bash
cd /Users/michael/git/alvum && git add crates/alvum-capture-screen/src/screenshot.rs && git commit -m "feat: add ScreenCaptureKit active window screenshot capture"
```

---

### Task 4: Capture Screen — Triggers + Daemon

Implement NSWorkspace app focus notification + idle timer, then wire together into a capture daemon loop.

**Files:**
- Create: `crates/alvum-capture-screen/src/trigger.rs`
- Create: `crates/alvum-capture-screen/src/daemon.rs`

- [ ] **Step 1: Implement trigger.rs**

```rust
// crates/alvum-capture-screen/src/trigger.rs

//! Two capture triggers: app focus change (NSWorkspace) and idle timer.
//!
//! The idle timer resets after each app focus event to avoid double-capture.

use anyhow::Result;
use tokio::sync::mpsc;
use tracing::{debug, info, warn};
use std::time::Duration;

/// Which event triggered a capture.
#[derive(Debug, Clone, PartialEq)]
pub enum TriggerKind {
    /// User switched to a different application.
    AppFocus,
    /// No app switch for 30 seconds — capture current state.
    Idle,
}

impl TriggerKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            TriggerKind::AppFocus => "app_focus",
            TriggerKind::Idle => "idle",
        }
    }
}

/// A trigger event with its kind.
#[derive(Debug, Clone)]
pub struct TriggerEvent {
    pub kind: TriggerKind,
    pub ts: chrono::DateTime<chrono::Utc>,
}

const IDLE_INTERVAL: Duration = Duration::from_secs(30);

/// Start the trigger system. Returns a receiver that yields TriggerEvents.
///
/// Spawns two async tasks:
/// 1. NSWorkspace app focus listener (sends AppFocus events)
/// 2. Idle timer (sends Idle events, resets on each AppFocus)
///
/// Both tasks run until the returned receiver is dropped.
pub fn start_triggers() -> Result<mpsc::Receiver<TriggerEvent>> {
    let (tx, rx) = mpsc::channel::<TriggerEvent>(64);

    // Channel for the focus listener to notify the idle timer of resets
    let (reset_tx, mut reset_rx) = mpsc::channel::<()>(16);

    // Task 1: NSWorkspace app focus listener
    let focus_tx = tx.clone();
    let focus_reset_tx = reset_tx;
    std::thread::spawn(move || {
        run_focus_listener(focus_tx, focus_reset_tx);
    });

    // Task 2: Idle timer
    tokio::spawn(async move {
        loop {
            tokio::select! {
                // Wait for idle interval
                _ = tokio::time::sleep(IDLE_INTERVAL) => {
                    let event = TriggerEvent {
                        kind: TriggerKind::Idle,
                        ts: chrono::Utc::now(),
                    };
                    debug!("idle timer fired");
                    if tx.send(event).await.is_err() {
                        break; // receiver dropped
                    }
                }
                // Reset timer when app focus fires
                Some(()) = reset_rx.recv() => {
                    debug!("idle timer reset");
                    continue;
                }
            }
        }
        info!("idle timer stopped");
    });

    Ok(rx)
}

/// Run the NSWorkspace focus listener on the current thread.
/// This must run on a thread with a CFRunLoop (not a tokio task).
fn run_focus_listener(tx: mpsc::Sender<TriggerEvent>, reset_tx: mpsc::Sender<()>) {
    use objc2_foundation::NSString;
    use objc2_app_kit::NSWorkspace;

    info!("starting NSWorkspace focus listener");

    // NSWorkspaceDidActivateApplicationNotification
    let notification_name =
        unsafe { NSString::from_str("NSWorkspaceDidActivateApplicationNotification") };

    let workspace = unsafe { NSWorkspace::sharedWorkspace() };
    let center = unsafe { workspace.notificationCenter() };

    // Use a polling approach: check frontmost app periodically from a dedicated
    // thread. This avoids the complexity of setting up an Objective-C observer
    // block and CFRunLoop from Rust. The poll interval (500ms) is fast enough
    // to catch app switches without meaningful CPU cost.
    let mut last_app = get_frontmost_app_name();
    debug!(app = %last_app, "initial frontmost app");

    loop {
        std::thread::sleep(Duration::from_millis(500));

        let current_app = get_frontmost_app_name();
        if current_app != last_app {
            info!(from = %last_app, to = %current_app, "app focus changed");
            last_app = current_app;

            let event = TriggerEvent {
                kind: TriggerKind::AppFocus,
                ts: chrono::Utc::now(),
            };

            // Blocking send from std::thread into async channel
            if tx.blocking_send(event).is_err() {
                break; // receiver dropped
            }
            // Reset idle timer
            let _ = reset_tx.blocking_send(());
        }
    }

    info!("focus listener stopped");
}

/// Get the name of the frontmost application via NSWorkspace.
fn get_frontmost_app_name() -> String {
    use objc2_app_kit::NSWorkspace;

    let workspace = unsafe { NSWorkspace::sharedWorkspace() };
    unsafe { workspace.frontmostApplication() }
        .and_then(|app| unsafe { app.localizedName() })
        .map(|name| name.to_string())
        .unwrap_or_else(|| "Unknown".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trigger_kind_as_str() {
        assert_eq!(TriggerKind::AppFocus.as_str(), "app_focus");
        assert_eq!(TriggerKind::Idle.as_str(), "idle");
    }

    /// Requires a running macOS GUI session.
    #[test]
    #[ignore]
    fn get_frontmost_app_returns_nonempty() {
        let name = get_frontmost_app_name();
        assert!(!name.is_empty());
        assert_ne!(name, "Unknown");
    }
}
```

- [ ] **Step 2: Implement daemon.rs**

```rust
// crates/alvum-capture-screen/src/daemon.rs

//! Screen capture daemon: orchestrates triggers, screenshots, and file writing.

use anyhow::{Context, Result};
use std::path::PathBuf;
use tracing::{info, warn};

use crate::screenshot;
use crate::trigger;
use crate::writer::ScreenWriter;

/// Configuration for the screen capture daemon.
#[derive(Debug, Clone)]
pub struct ScreenCaptureConfig {
    /// Root capture directory (e.g., capture/2026-04-12).
    pub capture_dir: PathBuf,
}

/// Run the screen capture daemon until the returned handle is stopped.
///
/// Listens for trigger events (app focus change, idle timer), captures
/// a screenshot of the frontmost window, and saves it to disk.
pub async fn run(config: ScreenCaptureConfig) -> Result<()> {
    let writer = ScreenWriter::new(config.capture_dir.clone())
        .context("failed to create screen writer")?;

    let mut triggers = trigger::start_triggers()
        .context("failed to start triggers")?;

    info!(
        capture_dir = %config.capture_dir.display(),
        "screen capture daemon started"
    );

    let mut capture_count: u64 = 0;

    while let Some(event) = triggers.recv().await {
        match screenshot::capture_frontmost_window() {
            Ok(Some(shot)) => {
                match writer.save_screenshot(
                    &shot.png_bytes,
                    event.ts,
                    &shot.app_name,
                    &shot.window_title,
                    event.kind.as_str(),
                ) {
                    Ok(path) => {
                        capture_count += 1;
                        info!(
                            count = capture_count,
                            app = %shot.app_name,
                            trigger = event.kind.as_str(),
                            path = %path.display(),
                            "captured screenshot"
                        );
                    }
                    Err(e) => {
                        warn!(error = %e, "failed to save screenshot");
                    }
                }
            }
            Ok(None) => {
                // No capturable window (e.g., desktop focused) — skip
            }
            Err(e) => {
                warn!(error = %e, "screenshot capture failed");
            }
        }
    }

    info!(total = capture_count, "screen capture daemon stopped");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_stores_capture_dir() {
        let config = ScreenCaptureConfig {
            capture_dir: PathBuf::from("/tmp/test-capture"),
        };
        assert_eq!(config.capture_dir, PathBuf::from("/tmp/test-capture"));
    }
}
```

- [ ] **Step 3: Verify compilation**

```bash
cd /Users/michael/git/alvum && cargo check -p alvum-capture-screen
```

- [ ] **Step 4: Run unit tests (non-ignored)**

```bash
cd /Users/michael/git/alvum && cargo test -p alvum-capture-screen
```

- [ ] **Step 5: Commit**

```bash
cd /Users/michael/git/alvum && git add crates/alvum-capture-screen/src/trigger.rs crates/alvum-capture-screen/src/daemon.rs && git commit -m "feat: add screen capture triggers (app focus + idle timer) and daemon loop"
```

---

### Task 5: CLI `capture-screen` Subcommand

Add `alvum capture-screen` command to the CLI. Same pattern as `alvum record`.

**Files:**
- Modify: `crates/alvum-cli/Cargo.toml`
- Modify: `crates/alvum-cli/src/main.rs`

- [ ] **Step 1: Add dependency**

In `crates/alvum-cli/Cargo.toml`, add to `[dependencies]`:

```toml
alvum-capture-screen = { path = "../alvum-capture-screen" }
```

- [ ] **Step 2: Add subcommand to CLI enum**

In `crates/alvum-cli/src/main.rs`, add to the `Commands` enum:

```rust
    /// Start screen capture (active window screenshots)
    #[command(name = "capture-screen")]
    CaptureScreen {
        /// Capture directory (default: ./capture/<today>)
        #[arg(long)]
        capture_dir: Option<PathBuf>,
    },
```

- [ ] **Step 3: Add match arm in main**

In `crates/alvum-cli/src/main.rs`, add to the `match cli.command` block:

```rust
        Commands::CaptureScreen { capture_dir } => {
            cmd_capture_screen(capture_dir).await
        }
```

- [ ] **Step 4: Implement cmd_capture_screen**

In `crates/alvum-cli/src/main.rs`, add after `cmd_record`:

```rust
async fn cmd_capture_screen(capture_dir: Option<PathBuf>) -> Result<()> {
    let today = chrono::Local::now().format("%Y-%m-%d").to_string();
    let capture_dir = capture_dir
        .unwrap_or_else(|| PathBuf::from("capture").join(&today));

    info!(dir = %capture_dir.display(), "starting screen capture");

    let config = alvum_capture_screen::daemon::ScreenCaptureConfig {
        capture_dir,
    };

    println!("Screen capture running... Press Ctrl-C to stop.");

    tokio::select! {
        result = alvum_capture_screen::daemon::run(config) => {
            result?;
        }
        _ = tokio::signal::ctrl_c() => {
            println!("\nStopping...");
        }
    }

    println!("Done.");
    Ok(())
}
```

- [ ] **Step 5: Verify**

```bash
cd /Users/michael/git/alvum && cargo check -p alvum-cli
```

- [ ] **Step 6: Commit**

```bash
cd /Users/michael/git/alvum && git add crates/alvum-cli/Cargo.toml crates/alvum-cli/src/main.rs && git commit -m "feat: add 'alvum capture-screen' CLI subcommand"
```

---

### Task 6: Processor Screen Crate

Create `alvum-processor-screen` that reads screenshot DataRefs, sends them to a vision model (or OCR fallback), and produces Observations with actor hints. Supports configurable vision modes: `local` (Ollama), `api` (Anthropic), `ocr` (macOS Vision), `off` (skip).

**Files:**
- Create: `crates/alvum-processor-screen/Cargo.toml`
- Create: `crates/alvum-processor-screen/src/lib.rs`
- Create: `crates/alvum-processor-screen/src/describe.rs`
- Create: `crates/alvum-processor-screen/src/ocr.rs`
- Modify: `Cargo.toml` (workspace members)

- [ ] **Step 1: Create Cargo.toml**

```toml
# crates/alvum-processor-screen/Cargo.toml
[package]
name = "alvum-processor-screen"
version = "0.1.0"
edition = "2024"

[dependencies]
alvum-core = { path = "../alvum-core" }
alvum-pipeline = { path = "../alvum-pipeline" }
serde.workspace = true
serde_json.workspace = true
chrono.workspace = true
anyhow.workspace = true
tokio.workspace = true
tracing.workspace = true

[dev-dependencies]
tempfile = "3"
```

- [ ] **Step 2: Add to workspace**

In root `Cargo.toml`, add `"crates/alvum-processor-screen"` to the `members` list:

```toml
[workspace]
resolver = "2"
members = [
    "crates/alvum-core",
    "crates/alvum-connector-claude",
    "crates/alvum-pipeline",
    "crates/alvum-cli",
    "crates/alvum-capture-audio",
    "crates/alvum-processor-audio",
    "crates/alvum-episode",
    "crates/alvum-knowledge",
    "crates/alvum-capture-screen",
    "crates/alvum-processor-screen",
]
```

- [ ] **Step 3: Create lib.rs**

```rust
// crates/alvum-processor-screen/src/lib.rs

//! Screen processor: sends screenshots to a vision model and produces Observations.
//!
//! Reads DataRefs (PNG screenshots from alvum-capture-screen), calls the LLM's
//! vision API, and produces text Observations with actor attribution hints.
//! Supports configurable vision modes: local (Ollama), api (Anthropic), ocr (macOS Vision), off.

pub mod describe;
pub mod ocr;

/// Vision processing mode, selected by `--vision` CLI flag.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum VisionMode {
    /// Ollama vision model (free, local). Default.
    Local,
    /// Anthropic API vision (paid, highest quality).
    Api,
    /// macOS Vision framework OCR only (free, text-only fallback).
    Ocr,
    /// Skip processing. Save screenshots but produce no Observations.
    Off,
}

impl VisionMode {
    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "local" => Some(Self::Local),
            "api" => Some(Self::Api),
            "ocr" => Some(Self::Ocr),
            "off" => Some(Self::Off),
            _ => None,
        }
    }
}
```

- [ ] **Step 4: Create describe.rs**

```rust
// crates/alvum-processor-screen/src/describe.rs

//! Vision model screenshot description with actor attribution hints.

use alvum_core::data_ref::DataRef;
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
    let semaphore = tokio::sync::Semaphore::new(10); // limit concurrent vision calls

    let mut handles = Vec::new();

    for data_ref in data_refs {
        let dr = data_ref.clone();
        let dir = capture_dir.to_path_buf();

        handles.push((dr, dir));
    }

    // Process sequentially for now — parallel via semaphore can be added
    // when we confirm the API handles concurrent calls well.
    for (dr, dir) in handles {
        let _permit = semaphore.acquire().await
            .context("semaphore closed")?;

        match describe_screenshot(provider, &dr, &dir).await {
            Ok(obs) => observations.push(obs),
            Err(e) => {
                warn!(path = %dr.path, error = %e, "failed to process screenshot");
            }
        }
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
    let response = provider
        .complete_with_image(VISION_SYSTEM_PROMPT, user_message, &image_path)
        .await
        .with_context(|| format!("vision call failed for {}", image_path.display()))?;

    // Parse the structured response
    let json_str = alvum_pipeline::util::strip_markdown_fences(&response);
    let parsed: VisionResponse = serde_json::from_str(json_str).unwrap_or_else(|_| {
        // If JSON parsing fails, treat the whole response as description
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
```

- [ ] **Step 5: Create ocr.rs (macOS Vision OCR fallback)**

```rust
// crates/alvum-processor-screen/src/ocr.rs

//! macOS Vision framework OCR fallback.
//!
//! Extracts visible text from screenshots using VNRecognizeTextRequest.
//! Used when no vision model is available (--vision ocr mode).

use alvum_core::data_ref::DataRef;
use alvum_core::observation::{MediaRef, Observation};
use anyhow::{Context, Result};
use std::path::Path;
use tracing::{debug, info, warn};

/// Process screen DataRefs using macOS Vision OCR.
/// Returns one Observation per screenshot with extracted text as content.
pub fn process_screen_data_refs_ocr(
    data_refs: &[DataRef],
    capture_dir: &Path,
) -> Result<Vec<Observation>> {
    info!(screenshots = data_refs.len(), "OCR processing screen captures");

    let mut observations = Vec::new();

    for dr in data_refs {
        let image_path = if Path::new(&dr.path).is_absolute() {
            std::path::PathBuf::from(&dr.path)
        } else {
            capture_dir.join(&dr.path)
        };

        match extract_text(&image_path) {
            Ok(text) if !text.trim().is_empty() => {
                let app = dr.metadata.as_ref()
                    .and_then(|m| m.get("app"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("Unknown");
                let window = dr.metadata.as_ref()
                    .and_then(|m| m.get("window"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("");

                // Build metadata, carrying forward capture hints
                let mut metadata = dr.metadata.clone().unwrap_or(serde_json::json!({}));
                if let Some(obj) = metadata.as_object_mut() {
                    obj.insert("vision_mode".into(), serde_json::json!("ocr"));
                }

                let content = format!("{app} — {window}: {text}");

                observations.push(Observation {
                    ts: dr.ts,
                    source: "screen".into(),
                    kind: "screen_capture".into(),
                    content,
                    metadata: Some(metadata),
                    media_ref: Some(MediaRef {
                        path: dr.path.clone(),
                        mime: "image/png".into(),
                    }),
                });
            }
            Ok(_) => {
                debug!(path = %dr.path, "OCR returned no text, skipping");
            }
            Err(e) => {
                warn!(path = %dr.path, error = %e, "OCR failed");
            }
        }
    }

    info!(observations = observations.len(), "OCR processing complete");
    Ok(observations)
}

/// Extract text from an image using macOS Vision framework.
///
/// Calls VNRecognizeTextRequest via Objective-C FFI. Returns concatenated
/// recognized text. Falls back to empty string if Vision framework unavailable.
fn extract_text(image_path: &Path) -> Result<String> {
    // Shell out to a small Swift helper or use objc2-vision bindings.
    // For MVP, use the `osascript` bridge to call Vision framework:
    let output = std::process::Command::new("osascript")
        .args([
            "-e",
            &format!(
                r#"use framework "Vision"
use framework "AppKit"
set imgPath to POSIX file "{}"
set img to current application's NSImage's alloc()'s initWithContentsOfFile:(POSIX path of imgPath)
set reqHandler to current application's VNImageRequestHandler's alloc()'s initWithData:(img's TIFFRepresentation()) options:(current application's NSDictionary's dictionary())
set req to current application's VNRecognizeTextRequest's alloc()'s init()
reqHandler's performRequests:({{req}}) |error|:(missing value)
set results to req's results()
set output to ""
repeat with obs in results
    set output to output & (obs's topCandidates:(1))'s first item's |string|() & linefeed
end repeat
return output"#,
                image_path.display()
            ),
        ])
        .output()
        .context("failed to run osascript for Vision OCR")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("Vision OCR failed: {stderr}");
    }

    let text = String::from_utf8_lossy(&output.stdout).to_string();
    Ok(text)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_data_refs_returns_empty() {
        let tmp = tempfile::TempDir::new().unwrap();
        let obs = process_screen_data_refs_ocr(&[], tmp.path()).unwrap();
        assert!(obs.is_empty());
    }
}
```

- [ ] **Step 6: Verify**

```bash
cd /Users/michael/git/alvum && cargo test -p alvum-processor-screen
```

- [ ] **Step 7: Commit**

```bash
cd /Users/michael/git/alvum && git add crates/alvum-processor-screen/Cargo.toml crates/alvum-processor-screen/src/lib.rs crates/alvum-processor-screen/src/describe.rs crates/alvum-processor-screen/src/ocr.rs Cargo.toml && git commit -m "feat: add alvum-processor-screen crate with configurable vision modes and actor hints"
```

---

### Task 7: CLI Cross-Source Screen Integration

Wire screen processor into the cross-source extract mode so `alvum extract --capture-dir` picks up screen data alongside audio.

**Files:**
- Modify: `crates/alvum-cli/Cargo.toml`
- Modify: `crates/alvum-cli/src/main.rs`

- [ ] **Step 1: Add dependency**

In `crates/alvum-cli/Cargo.toml`, add to `[dependencies]`:

```toml
alvum-processor-screen = { path = "../alvum-processor-screen" }
```

- [ ] **Step 2: Add `--vision` flag to Extract command**

In the `Extract` variant of the `Commands` enum, add after `relevance_threshold`:

```rust
        /// Vision processing mode for screen captures: local, api, ocr, off
        #[arg(long, default_value = "local")]
        vision: String,
```

Update the match arm in `main()` to pass `vision`, and update `cmd_extract`'s signature to accept `vision: String`. Parse it at the top of `cmd_extract`:

```rust
    let vision_mode = alvum_processor_screen::VisionMode::from_str(&vision)
        .unwrap_or_else(|| {
            tracing::warn!(vision = %vision, "unknown vision mode, defaulting to local");
            alvum_processor_screen::VisionMode::Local
        });
```

- [ ] **Step 3: Add screen data scanning to cross-source mode**

In `crates/alvum-cli/src/main.rs`, inside `cmd_extract`, find the cross-source mode section (the `if source.is_none()` block). Replace the existing `events.jsonl` scan block with screen capture processing:

Replace this existing block:

```rust
        // Scan for screen events
        let events_path = capture_dir.join("events.jsonl");
        if events_path.exists() {
            info!("loading screen events");
            let screen_obs: Vec<alvum_core::observation::Observation> = alvum_core::storage::read_jsonl(&events_path)?;
            all_observations.extend(screen_obs);
        }
```

With:

```rust
        // Scan for screen captures
        let screen_captures_path = capture_dir.join("screen").join("captures.jsonl");
        if screen_captures_path.exists() && vision_mode != alvum_processor_screen::VisionMode::Off {
            info!("loading screen captures");
            let screen_refs: Vec<alvum_core::data_ref::DataRef> =
                alvum_core::storage::read_jsonl(&screen_captures_path)?;
            if !screen_refs.is_empty() {
                let screen_obs = match vision_mode {
                    alvum_processor_screen::VisionMode::Local | alvum_processor_screen::VisionMode::Api => {
                        info!(screenshots = screen_refs.len(), mode = ?vision_mode, "describing screenshots with vision model");
                        alvum_processor_screen::describe::process_screen_data_refs(
                            provider.as_ref(),
                            &screen_refs,
                            &capture_dir,
                        ).await?
                    }
                    alvum_processor_screen::VisionMode::Ocr => {
                        info!(screenshots = screen_refs.len(), "extracting text with OCR");
                        alvum_processor_screen::ocr::process_screen_data_refs_ocr(
                            &screen_refs,
                            &capture_dir,
                        )?
                    }
                    alvum_processor_screen::VisionMode::Off => unreachable!(),
                };
                all_observations.extend(screen_obs);
            }
        }
```

- [ ] **Step 3: Verify**

```bash
cd /Users/michael/git/alvum && cargo check -p alvum-cli
```

- [ ] **Step 4: Commit**

```bash
cd /Users/michael/git/alvum && git add crates/alvum-cli/Cargo.toml crates/alvum-cli/src/main.rs && git commit -m "feat: wire screen processor into cross-source extract mode"
```

---

### Task 8: Actor Attribution in Threading + Extraction

Update the threading prompt and extraction prompt to use actor hints for attribution. This is the Layer 3 resolution — the threading LLM sees all actor hints from Layer 1 (capture) and Layer 2 (processor), and performs final attribution.

**Files:**
- Modify: `crates/alvum-episode/src/threading.rs`
- Modify: `crates/alvum-pipeline/src/distill.rs`

- [ ] **Step 1: Update threading prompt**

In `crates/alvum-episode/src/threading.rs`, replace the `THREADING_SYSTEM_PROMPT` constant with:

```rust
const THREADING_SYSTEM_PROMPT: &str = r#"You are analyzing a full day of captured data from multiple sensors.
The data is organized into 5-minute time blocks, each containing
observations from various sources (audio transcripts, screen events,
location, calendar, etc.).

Identify CONTEXT THREADS — coherent, continuous activities that
may span multiple time blocks and may run concurrently.

For each thread, output:
- id: sequential (thread_001, thread_002, ...)
- label: human-readable name for this activity
- start: ISO 8601 timestamp (start of first relevant observation)
- end: ISO 8601 timestamp (end of last relevant observation)
- thread_type: free-form classification (e.g., "conversation", "solo_work",
  "media_playback", "ambient", "transition", "phone_call")
- sources: which data sources contribute to this thread
- observations: array of objects with {block_index, obs_index} identifying
  which observations belong to this thread
- relevance: 0.0 to 1.0
- relevance_signals: list of reasons for the score
- metadata: structured context including actor attribution (see below)

THREADING RULES:
1. A time block can participate in MULTIPLE concurrent threads.
2. Each observation belongs to EXACTLY ONE thread. Disambiguate.
3. Trace threads across block boundaries — a meeting spanning
   10:00-10:30 is ONE thread across multiple blocks.
4. Split threads when the context genuinely changes.

ACTOR ATTRIBUTION:
Observations may include actor_hints in their metadata. These are signals
from the capture layer and processors about who is acting. Your job is
to RESOLVE these hints into final attribution using cross-source evidence:

- Fuse signals: if system audio says "unknown_person" and screen shows
  "Sarah Chen" as active speaker in Zoom → resolve to sarah_chen (person).
- Use knowledge corpus: if a name appears in known entities, use that entity ID.
- Resolve ambiguity: mic audio (self, 0.3) + screen shows user typing → self (0.9).
- Detect agents: screen shows Claude Code terminal with AI output → agent (0.8).

In the thread metadata, include:
- "speakers": array of actor identifiers who participated
- "primary_actor": who was mainly driving this activity

RELEVANCE SCORING:
High (0.7-1.0):
  - Multi-source convergence (audio + screen + calendar corroborate)
  - Decision language ("let's do X", "I've decided", "we should")
  - Commitment language ("I'll have it by Friday")
  - References to the person's actual projects, people, goals

Medium (0.3-0.7):
  - Single-source conversation with work content
  - Solo work session with sparse self-talk
  - Thinking aloud about real topics

Low (0.0-0.3):
  - Media playback (TV, movies, podcasts, music)
  - Other people's conversations not involving the user
  - Routine transactions ("large coffee please")
  - Transit with no meaningful conversation

Output ONLY a JSON array of threads. No markdown, no explanation."#;
```

- [ ] **Step 2: Update threading prompt test**

In `crates/alvum-episode/src/threading.rs`, update the test:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn threading_prompt_contains_key_instructions() {
        assert!(THREADING_SYSTEM_PROMPT.contains("CONTEXT THREADS"));
        assert!(THREADING_SYSTEM_PROMPT.contains("relevance"));
        assert!(THREADING_SYSTEM_PROMPT.contains("EXACTLY ONE thread"));
        assert!(THREADING_SYSTEM_PROMPT.contains("media_playback"));
    }

    #[test]
    fn threading_prompt_contains_attribution_instructions() {
        assert!(THREADING_SYSTEM_PROMPT.contains("ACTOR ATTRIBUTION"));
        assert!(THREADING_SYSTEM_PROMPT.contains("speakers"));
        assert!(THREADING_SYSTEM_PROMPT.contains("primary_actor"));
        assert!(THREADING_SYSTEM_PROMPT.contains("actor_hints"));
    }
}
```

- [ ] **Step 3: Update extraction prompt**

In `crates/alvum-pipeline/src/distill.rs`, update `format_conversation` to include actor hints in the formatted output so the extraction LLM can see them:

Replace the `format_conversation` function with:

```rust
fn format_conversation(observations: &[Observation]) -> String {
    let mut parts = Vec::new();
    for obs in observations {
        let speaker = obs.speaker().unwrap_or("system").to_string();
        let ts = obs.ts.format("%Y-%m-%d %H:%M");
        let content = if obs.content.len() > 2000 {
            format!("{}...[truncated]", truncate_chars(&obs.content, 2000))
        } else {
            obs.content.clone()
        };

        let mut line = format!("[{ts}] [{source}/{kind}] {speaker}: {content}",
            source = obs.source, kind = obs.kind);

        // Include actor hints if present, so the extraction LLM can attribute decisions
        if let Some(hints) = obs.metadata.as_ref()
            .and_then(|m| m.get("actor_hints"))
            .and_then(|h| h.as_array())
        {
            if !hints.is_empty() {
                let hint_strs: Vec<String> = hints.iter()
                    .filter_map(|h| {
                        let actor = h.get("actor")?.as_str()?;
                        let kind = h.get("kind")?.as_str()?;
                        let conf = h.get("confidence")?.as_f64()?;
                        Some(format!("{actor}({kind},{conf:.1})"))
                    })
                    .collect();
                if !hint_strs.is_empty() {
                    line.push_str(&format!("  [actors: {}]", hint_strs.join(", ")));
                }
            }
        }

        parts.push(line);
    }
    parts.join("\n\n")
}
```

- [ ] **Step 4: Update extraction format test**

In `crates/alvum-pipeline/src/distill.rs`, update the tests:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_conversation_produces_readable_transcript() {
        let obs = vec![
            Observation::dialogue(
                "2026-04-02T04:31:55Z".parse().unwrap(),
                "claude-code",
                "user",
                "Should we use real-time or batch?",
            ),
            Observation::dialogue(
                "2026-04-02T04:33:57Z".parse().unwrap(),
                "claude-code",
                "assistant",
                "Batch processing is better because...",
            ),
        ];
        let formatted = format_conversation(&obs);
        assert!(formatted.contains("[2026-04-02 04:31] [claude-code/dialogue] user:"));
        assert!(formatted.contains("[2026-04-02 04:33] [claude-code/dialogue] assistant:"));
        assert!(formatted.contains("Should we use"));
    }

    #[test]
    fn format_conversation_truncates_long_messages() {
        let long_content = "x".repeat(5000);
        let obs = vec![Observation::dialogue(
            "2026-04-02T04:33:57Z".parse().unwrap(),
            "claude-code",
            "assistant",
            &long_content,
        )];
        let formatted = format_conversation(&obs);
        assert!(formatted.contains("[truncated]"));
        assert!(formatted.len() < 5000);
    }

    #[test]
    fn format_conversation_includes_actor_hints() {
        let obs = vec![Observation {
            ts: "2026-04-12T09:00:15Z".parse().unwrap(),
            source: "screen".into(),
            kind: "screen_capture".into(),
            content: "VS Code showing main.rs".into(),
            metadata: Some(serde_json::json!({
                "app": "VS Code",
                "actor_hints": [
                    {"actor": "self", "kind": "self", "confidence": 0.4, "signal": "screen"},
                    {"actor": "claude", "kind": "agent", "confidence": 0.7, "signal": "terminal"}
                ]
            })),
            media_ref: None,
        }];
        let formatted = format_conversation(&obs);
        assert!(formatted.contains("[actors: self(self,0.4), claude(agent,0.7)]"));
    }

    #[test]
    fn format_conversation_skips_empty_actor_hints() {
        let obs = vec![Observation {
            ts: "2026-04-12T09:00:15Z".parse().unwrap(),
            source: "audio-mic".into(),
            kind: "speech".into(),
            content: "Hello world".into(),
            metadata: Some(serde_json::json!({"actor_hints": []})),
            media_ref: None,
        }];
        let formatted = format_conversation(&obs);
        assert!(!formatted.contains("[actors:"));
    }
}
```

- [ ] **Step 5: Verify**

```bash
cd /Users/michael/git/alvum && cargo test -p alvum-episode && cargo test -p alvum-pipeline
```

- [ ] **Step 6: Commit**

```bash
cd /Users/michael/git/alvum && git add crates/alvum-episode/src/threading.rs crates/alvum-pipeline/src/distill.rs && git commit -m "feat: add actor attribution to threading and extraction prompts (Layer 3 resolution)"
```
