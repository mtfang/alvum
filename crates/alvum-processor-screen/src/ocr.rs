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
        // Advance the shared per-file counter regardless of result.
        alvum_core::progress::tick_stage(alvum_core::progress::STAGE_PROCESS);
    }

    info!(observations = observations.len(), "OCR processing complete");
    Ok(observations)
}

/// Extract text from an image using the macOS Vision framework natively.
/// Uses VNRecognizeTextRequest at accurate level over an NSImage loaded from disk.
#[cfg(target_os = "macos")]
pub fn extract_text(image_path: &Path) -> Result<String> {
    use objc2::AllocAnyThread;
    use objc2::rc::Retained;
    use objc2_app_kit::NSImage;
    use objc2_foundation::{NSArray, NSString, NSURL};
    use objc2_vision::{
        VNImageRequestHandler, VNRecognizeTextRequest, VNRecognizedTextObservation, VNRequest,
        VNRequestTextRecognitionLevel,
    };

    let path_str = image_path
        .to_str()
        .with_context(|| format!("OCR: non-UTF8 path {}", image_path.display()))?;

    // Load the image. NSImage handles PNG decoding via ImageIO under the hood.
    let url = NSURL::fileURLWithPath(&NSString::from_str(path_str));
    let image = NSImage::initWithContentsOfURL(NSImage::alloc(), &url)
        .ok_or_else(|| anyhow::anyhow!("Vision: NSImage failed to load {path_str}"))?;

    // NSImage → CGImage for the Vision request handler.
    let cg_image = unsafe {
        let size = image.size();
        let mut rect = objc2_core_foundation::CGRect {
            origin: objc2_core_foundation::CGPoint { x: 0.0, y: 0.0 },
            size: objc2_core_foundation::CGSize {
                width: size.width,
                height: size.height,
            },
        };
        image
            .CGImageForProposedRect_context_hints(&mut rect, None, None)
            .ok_or_else(|| anyhow::anyhow!("Vision: NSImage has no CGImage representation"))?
    };

    // Build the recognition request. Accurate level is the documented default,
    // setting it explicitly is a no-op but reads cleanly.
    let request = VNRecognizeTextRequest::new();
    request.setRecognitionLevel(VNRequestTextRecognitionLevel::Accurate);

    // Run synchronously through VNImageRequestHandler.
    let handler = unsafe {
        VNImageRequestHandler::initWithCGImage_options(
            VNImageRequestHandler::alloc(),
            &cg_image,
            &objc2_foundation::NSDictionary::new(),
        )
    };
    // VNRecognizeTextRequest → VNImageBasedRequest → VNRequest
    let request_as_vnreq: Retained<VNRequest> = request.clone().into_super().into_super();
    let request_array: Retained<NSArray<VNRequest>> =
        NSArray::from_retained_slice(&[request_as_vnreq]);
    handler
        .performRequests_error(&request_array)
        .map_err(|e| anyhow::anyhow!("Vision performRequests failed: {e}"))?;

    // Concatenate top candidates from each observation in row order.
    let Some(observations) = request.results() else {
        return Ok(String::new());
    };

    let mut lines: Vec<String> = Vec::new();
    let count = observations.count();
    for i in 0..count {
        let any_obj = observations.objectAtIndex(i);
        let obs = any_obj
            .downcast::<VNRecognizedTextObservation>()
            .map_err(|_| anyhow::anyhow!("Vision: observation cast failed"))?;
        let candidates = obs.topCandidates(1);
        if candidates.count() > 0 {
            let cand = candidates.objectAtIndex(0);
            let s = cand.string();
            lines.push(s.to_string());
        }
    }

    Ok(lines.join("\n"))
}

#[cfg(not(target_os = "macos"))]
pub fn extract_text(_image_path: &Path) -> Result<String> {
    anyhow::bail!("native Vision OCR is only supported on macOS")
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
