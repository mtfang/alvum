//! Screen side of the shared SCK stream: PNG-encode each frame on the
//! delegate queue, plus helpers to identify the frontmost window and
//! the display it sits on so the filter can follow the user across
//! monitors.

use anyhow::{Context, Result, anyhow};
use objc2::rc::Retained;
use objc2_core_media::CMSampleBuffer;
use objc2_core_video::{
    CVPixelBufferGetBaseAddress, CVPixelBufferGetBytesPerRow, CVPixelBufferGetHeight,
    CVPixelBufferGetWidth, CVPixelBufferLockBaseAddress, CVPixelBufferLockFlags,
    CVPixelBufferUnlockBaseAddress,
};
use objc2_screen_capture_kit::{SCDisplay, SCShareableContent, SCWindow};
use tracing::warn;

use crate::stream::SharedState;

pub(crate) fn handle_screen(sample: &CMSampleBuffer, state: &SharedState) {
    match encode_png_from_sample(sample) {
        Ok(png) => {
            if let Ok(mut slot) = state.latest_png.lock() {
                *slot = Some(png);
            }
        }
        Err(e) => warn!(error = %e, "SCK frame encode failed"),
    }
}

fn encode_png_from_sample(sample: &CMSampleBuffer) -> Result<Vec<u8>> {
    let image = unsafe { sample.image_buffer() }.context("CMSampleBuffer has no CVImageBuffer")?;
    let pixel_buffer = &*image;

    let lock_status =
        unsafe { CVPixelBufferLockBaseAddress(pixel_buffer, CVPixelBufferLockFlags::ReadOnly) };
    if lock_status != 0 {
        return Err(anyhow!(
            "CVPixelBufferLockBaseAddress returned {}",
            lock_status
        ));
    }

    let result: Result<Vec<u8>> = (|| {
        let width = CVPixelBufferGetWidth(pixel_buffer);
        let height = CVPixelBufferGetHeight(pixel_buffer);
        let bytes_per_row = CVPixelBufferGetBytesPerRow(pixel_buffer);
        let base = CVPixelBufferGetBaseAddress(pixel_buffer);
        if base.is_null() {
            return Err(anyhow!("CVPixelBufferGetBaseAddress returned null"));
        }

        let mut rgba = Vec::with_capacity(width * height * 4);
        let src = base as *const u8;
        for y in 0..height {
            let row_start = unsafe { src.add(y * bytes_per_row) };
            for x in 0..width {
                let p = unsafe { row_start.add(x * 4) };
                let b = unsafe { *p };
                let g = unsafe { *p.add(1) };
                let r = unsafe { *p.add(2) };
                let a = unsafe { *p.add(3) };
                rgba.extend_from_slice(&[r, g, b, a]);
            }
        }

        let mut png = Vec::new();
        image::ImageEncoder::write_image(
            image::codecs::png::PngEncoder::new(&mut png),
            &rgba,
            width as u32,
            height as u32,
            image::ExtendedColorType::Rgba8,
        )
        .context("PNG encoding failed")?;
        Ok(png)
    })();

    unsafe { CVPixelBufferUnlockBaseAddress(pixel_buffer, CVPixelBufferLockFlags::ReadOnly) };
    result
}

/// Return the frontmost on-screen regular-app window's bounding rect's
/// center, or None if no such window exists.
fn frontmost_window_center(content: &SCShareableContent) -> Option<(f64, f64)> {
    let windows = unsafe { content.windows() };
    for i in 0..windows.count() {
        let window = windows.objectAtIndex(i);
        if !is_frontmost_candidate(&window) {
            continue;
        }
        let frame = unsafe { window.frame() };
        let cx = frame.origin.x + frame.size.width / 2.0;
        let cy = frame.origin.y + frame.size.height / 2.0;
        return Some((cx, cy));
    }
    None
}

/// Same predicate frontmost_window() uses for picking the reported window.
fn is_frontmost_candidate(window: &SCWindow) -> bool {
    if !unsafe { window.isOnScreen() } {
        return false;
    }
    if unsafe { window.windowLayer() } != 0 {
        return false;
    }
    let Some(app) = (unsafe { window.owningApplication() }) else {
        return false;
    };
    let name = unsafe { app.applicationName() }.to_string();
    !(name.is_empty() || name == "Window Server")
}

/// Find the SCDisplay whose frame contains the frontmost window's center.
/// None if no frontmost window, or none of the displays contain it.
pub(crate) fn find_active_display(content: &SCShareableContent) -> Option<Retained<SCDisplay>> {
    let (cx, cy) = frontmost_window_center(content)?;
    let displays = unsafe { content.displays() };
    for i in 0..displays.count() {
        let d = displays.objectAtIndex(i);
        let f = unsafe { d.frame() };
        let x0 = f.origin.x;
        let y0 = f.origin.y;
        let x1 = x0 + f.size.width;
        let y1 = y0 + f.size.height;
        if cx >= x0 && cx < x1 && cy >= y0 && cy < y1 {
            return Some(d);
        }
    }
    None
}

pub(crate) fn frontmost_window(content: &SCShareableContent) -> (String, String) {
    let windows = unsafe { content.windows() };
    for i in 0..windows.count() {
        let window = windows.objectAtIndex(i);
        if !unsafe { window.isOnScreen() } {
            continue;
        }
        if unsafe { window.windowLayer() } != 0 {
            continue;
        }
        let app_name = match unsafe { window.owningApplication() } {
            Some(app) => unsafe { app.applicationName() }.to_string(),
            None => continue,
        };
        if app_name.is_empty() || app_name == "Window Server" {
            continue;
        }
        let window_title = unsafe { window.title() }
            .map(|s| s.to_string())
            .unwrap_or_default();
        return (app_name, window_title);
    }
    (String::new(), String::new())
}
