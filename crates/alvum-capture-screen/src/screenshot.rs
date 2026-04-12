//! Captures a screenshot of the frontmost (active) window using Core Graphics.
//!
//! Uses CGWindowListCopyWindowInfo to enumerate on-screen windows and
//! CGWindowListCreateImage to capture the frontmost application window.
//! The result is returned as PNG bytes along with app/window metadata.

use anyhow::{Context, Result};
use core_foundation::array::CFArrayGetValueAtIndex;
use core_foundation::base::{CFType, TCFType};
use core_foundation::boolean::CFBoolean;
use core_foundation::dictionary::CFDictionary;
use core_foundation::number::CFNumber;
use core_foundation::string::CFString;
use core_graphics::geometry::CGRect;
use core_graphics::window;
use tracing::debug;

/// A captured screenshot with metadata about the source window.
pub struct Screenshot {
    pub png_bytes: Vec<u8>,
    pub app_name: String,
    pub window_title: String,
}

/// Information about a candidate window for capture.
struct WindowInfo {
    id: window::CGWindowID,
    app_name: String,
    window_title: String,
}

/// Check if Screen Recording permission is granted by attempting a test capture.
/// Without permission, CGWindowListCreateImage returns blank images silently.
pub fn check_screen_recording_permission() -> Result<bool> {
    // CGWindowListCopyWindowInfo always works, but window names are hidden
    // without permission. We can detect this: if we find a layer-0 window
    // with an owner name but CGWindowListCreateImage returns all-zero pixels,
    // permission is missing.
    let Some(info) = find_frontmost_window()? else {
        // No windows found at all — can't determine permission status
        return Ok(true);
    };

    let cg_rect_null = CGRect::new(
        &core_graphics::geometry::CGPoint::new(f64::INFINITY, f64::INFINITY),
        &core_graphics::geometry::CGSize::new(0.0, 0.0),
    );

    let Some(cg_image) = window::create_image(
        cg_rect_null,
        window::kCGWindowListOptionIncludingWindow,
        info.id,
        window::kCGWindowImageBoundsIgnoreFraming,
    ) else {
        return Ok(false);
    };

    // Check if the image is all zeros (blank) — sign of missing permission
    let raw_data = cg_image.data();
    let raw_bytes: &[u8] = raw_data.bytes();
    let has_content = raw_bytes.iter().any(|&b| b != 0);

    Ok(has_content)
}

/// Captures the frontmost application window as a PNG screenshot.
///
/// Returns `Ok(None)` if no suitable window is found (e.g., only desktop
/// elements are visible). Requires Screen Recording permission on macOS 10.15+.
pub fn capture_frontmost_window() -> Result<Option<Screenshot>> {
    let Some(info) = find_frontmost_window()? else {
        return Ok(None);
    };

    debug!(
        window_id = info.id,
        app = %info.app_name,
        title = %info.window_title,
        "capturing window"
    );

    let png_bytes = capture_window_png(info.id)?;

    Ok(Some(Screenshot {
        png_bytes,
        app_name: info.app_name,
        window_title: info.window_title,
    }))
}

/// Finds the frontmost on-screen window that belongs to a regular application.
///
/// Core Graphics returns windows in front-to-back order, so the first
/// on-screen, non-desktop window at layer 0 (normal window layer) that has
/// an owner name is the frontmost application window.
fn find_frontmost_window() -> Result<Option<WindowInfo>> {
    let options = window::kCGWindowListOptionOnScreenOnly
        | window::kCGWindowListExcludeDesktopElements;

    let window_list = window::copy_window_info(options, window::kCGNullWindowID)
        .context("CGWindowListCopyWindowInfo returned null")?;

    let count = window_list.len();
    debug!(count, "enumerating on-screen windows");

    let key_owner = CFString::new("kCGWindowOwnerName");
    let key_name = CFString::new("kCGWindowName");
    let key_number = CFString::new("kCGWindowNumber");
    let key_layer = CFString::new("kCGWindowLayer");
    let key_onscreen = CFString::new("kCGWindowIsOnscreen");

    for i in 0..count {
        // Each element in the array is a CFDictionary
        let dict: CFDictionary<CFString, CFType> = unsafe {
            let ptr = CFArrayGetValueAtIndex(
                window_list.as_concrete_TypeRef(),
                i as _,
            );
            TCFType::wrap_under_get_rule(ptr as _)
        };

        // Skip windows not at layer 0 (layer 0 = normal application windows)
        if let Some(layer_val) = dict.find(&key_layer) {
            let layer_ref: CFNumber = unsafe {
                TCFType::wrap_under_get_rule(layer_val.as_CFTypeRef() as _)
            };
            if let Some(layer) = layer_ref.to_i32() {
                if layer != 0 {
                    continue;
                }
            }
        }

        // Skip windows not on screen
        if let Some(onscreen_val) = dict.find(&key_onscreen) {
            let onscreen: CFBoolean = unsafe {
                TCFType::wrap_under_get_rule(onscreen_val.as_CFTypeRef() as _)
            };
            if onscreen == CFBoolean::false_value() {
                continue;
            }
        }

        // Must have an owner name (application name)
        let Some(owner_val) = dict.find(&key_owner) else {
            continue;
        };
        let owner: CFString = unsafe {
            TCFType::wrap_under_get_rule(owner_val.as_CFTypeRef() as _)
        };
        let app_name = owner.to_string();

        // Skip system UI elements (e.g., menu bar, Spotlight)
        if app_name == "Window Server" || app_name.is_empty() {
            continue;
        }

        // Window title is optional (some windows have no title)
        let window_title = dict
            .find(&key_name)
            .map(|v| {
                let s: CFString = unsafe {
                    TCFType::wrap_under_get_rule(v.as_CFTypeRef() as _)
                };
                s.to_string()
            })
            .unwrap_or_default();

        // Window ID
        let Some(number_val) = dict.find(&key_number) else {
            continue;
        };
        let number: CFNumber = unsafe {
            TCFType::wrap_under_get_rule(number_val.as_CFTypeRef() as _)
        };
        let Some(window_id) = number.to_i32() else {
            continue;
        };

        return Ok(Some(WindowInfo {
            id: window_id as window::CGWindowID,
            app_name,
            window_title,
        }));
    }

    Ok(None)
}

/// Captures a specific window by ID and encodes the result as PNG bytes.
fn capture_window_png(window_id: window::CGWindowID) -> Result<Vec<u8>> {
    // CGRectNull tells CG to use the window's own bounds
    let cg_rect_null = CGRect::new(
        &core_graphics::geometry::CGPoint::new(f64::INFINITY, f64::INFINITY),
        &core_graphics::geometry::CGSize::new(0.0, 0.0),
    );

    let cg_image = window::create_image(
        cg_rect_null,
        window::kCGWindowListOptionIncludingWindow,
        window_id,
        window::kCGWindowImageBoundsIgnoreFraming | window::kCGWindowImageBestResolution,
    )
    .context("CGWindowListCreateImage returned null — is Screen Recording permission granted?")?;

    let width = cg_image.width() as u32;
    let height = cg_image.height() as u32;
    let bytes_per_row = cg_image.bytes_per_row();
    let raw_data = cg_image.data();
    let raw_bytes: &[u8] = raw_data.bytes();

    debug!(width, height, bytes_per_row, raw_len = raw_bytes.len(), "captured CGImage");

    // Core Graphics typically returns BGRA (32-bit, premultiplied alpha, big-endian on ARM).
    // The image crate needs RGBA. We convert in-place while also stripping row padding.
    let bpp = cg_image.bits_per_pixel() / 8; // bytes per pixel
    let mut rgba_buf = Vec::with_capacity((width as usize) * (height as usize) * 4);

    for y in 0..height as usize {
        let row_start = y * bytes_per_row;
        for x in 0..width as usize {
            let offset = row_start + x * bpp;
            if offset + 3 >= raw_bytes.len() {
                // Defensive: pad with transparent pixel if data is short
                rgba_buf.extend_from_slice(&[0, 0, 0, 0]);
                continue;
            }
            // macOS CGImage with premultiplied-first alpha is typically BGRA
            let b = raw_bytes[offset];
            let g = raw_bytes[offset + 1];
            let r = raw_bytes[offset + 2];
            let a = raw_bytes[offset + 3];
            rgba_buf.extend_from_slice(&[r, g, b, a]);
        }
    }

    let mut png_bytes: Vec<u8> = Vec::new();
    let encoder = image::codecs::png::PngEncoder::new(&mut png_bytes);
    image::ImageEncoder::write_image(
        encoder,
        &rgba_buf,
        width,
        height,
        image::ExtendedColorType::Rgba8,
    )
    .context("PNG encoding failed")?;

    Ok(png_bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[ignore] // Requires Screen Recording permission; run manually
    fn capture_frontmost_returns_valid_png() {
        let result = capture_frontmost_window().expect("capture should not error");
        let screenshot = result.expect("should find at least one window");

        // PNG magic bytes
        assert!(
            screenshot.png_bytes.starts_with(&[0x89, 0x50, 0x4E, 0x47]),
            "output should be valid PNG"
        );
        assert!(
            !screenshot.app_name.is_empty(),
            "app_name should not be empty"
        );
        eprintln!(
            "captured: app='{}' title='{}' size={}",
            screenshot.app_name,
            screenshot.window_title,
            screenshot.png_bytes.len()
        );
    }
}
