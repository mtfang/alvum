//! macOS Vision framework OCR fallback.

use alvum_core::data_ref::DataRef;
use alvum_core::observation::{MediaRef, Observation};
use anyhow::{Context, Result};
use std::path::Path;
use tracing::{debug, info, warn};

/// Process screen DataRefs using macOS Vision OCR.
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
            Ok(_) => debug!(path = %dr.path, "OCR returned no text, skipping"),
            Err(e) => warn!(path = %dr.path, error = %e, "OCR failed"),
        }
    }

    info!(observations = observations.len(), "OCR processing complete");
    Ok(observations)
}

/// Extract text from an image using macOS Vision framework via osascript.
fn extract_text(image_path: &Path) -> Result<String> {
    let script = format!(
        r#"use framework "Vision"
use framework "AppKit"
set imgPath to "{}"
set img to current application's NSImage's alloc()'s initWithContentsOfFile:imgPath
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
    );

    let output = std::process::Command::new("osascript")
        .args(["-e", &script])
        .output()
        .context("failed to run osascript for Vision OCR")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("Vision OCR failed: {stderr}");
    }

    Ok(String::from_utf8_lossy(&output.stdout).to_string())
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
