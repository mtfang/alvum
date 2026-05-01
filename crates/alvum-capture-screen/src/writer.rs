//! Saves PNG screenshots to disk and appends DataRef entries to captures.jsonl.

use alvum_core::data_ref::DataRef;
use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use std::path::{Path, PathBuf};
use tracing::info;

/// Manages writing screenshot files and their DataRef metadata.
///
/// Resolves the daily subdir on each call, not at construction, so a
/// long-running screen source rolls into tomorrow's dir once local
/// midnight passes. Mirrors AudioEncoder's behaviour.
pub struct ScreenWriter {
    root: PathBuf,
}

impl ScreenWriter {
    pub fn new(root: PathBuf) -> Result<Self> {
        Ok(Self { root })
    }

    fn day_dir(&self) -> PathBuf {
        let today = chrono::Local::now().format("%Y-%m-%d").to_string();
        self.root.join(&today)
    }

    fn day_dir_for_ts(&self, ts: DateTime<Utc>) -> PathBuf {
        let (day, _) = Self::local_day_and_filename(ts);
        self.root.join(day)
    }

    fn local_day_and_filename(ts: DateTime<Utc>) -> (String, String) {
        let local = ts.with_timezone(&chrono::Local);
        (
            local.format("%Y-%m-%d").to_string(),
            format!("{}.png", local.format("%H-%M-%S")),
        )
    }

    /// Save a PNG screenshot and record the DataRef.
    /// Returns the path to the written PNG file.
    pub fn save_screenshot(
        &self,
        png_bytes: &[u8],
        ts: DateTime<Utc>,
        app_name: &str,
        window_title: &str,
        trigger: &str,
    ) -> Result<PathBuf> {
        let day_dir = self.day_dir_for_ts(ts);
        let images_dir = day_dir.join("screen").join("images");
        std::fs::create_dir_all(&images_dir)
            .with_context(|| format!("failed to create images dir: {}", images_dir.display()))?;

        let (_, filename) = Self::local_day_and_filename(ts);
        let image_path = images_dir.join(&filename);

        std::fs::write(&image_path, png_bytes)
            .with_context(|| format!("failed to write screenshot: {}", image_path.display()))?;

        let relative_path = format!("screen/images/{filename}");

        let data_ref = DataRef {
            ts,
            source: "screen".into(),
            producer: "alvum.screen/snapshot".into(),
            schema: "alvum.screen.image.v1".into(),
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

        let captures_jsonl = day_dir.join("screen").join("captures.jsonl");
        alvum_core::storage::append_jsonl(&captures_jsonl, &data_ref)
            .context("failed to append DataRef to captures.jsonl")?;

        info!(
            path = %image_path.display(),
            app = app_name,
            trigger = trigger,
            "saved screenshot"
        );

        Ok(image_path)
    }

    /// Captures JSONL for today. Resolves lazily so callers always see the
    /// current day's log even after midnight.
    pub fn captures_jsonl_path(&self) -> PathBuf {
        self.day_dir().join("screen").join("captures.jsonl")
    }

    pub fn capture_dir(&self) -> &Path {
        &self.root
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn minimal_png() -> Vec<u8> {
        let mut buf = Vec::new();
        let encoder = image::codecs::png::PngEncoder::new(&mut buf);
        image::ImageEncoder::write_image(
            encoder,
            &[255, 0, 0, 255],
            1,
            1,
            image::ExtendedColorType::Rgba8,
        )
        .unwrap();
        buf
    }

    fn captures_jsonl_for_image(path: &Path) -> PathBuf {
        path.parent()
            .and_then(Path::parent)
            .expect("image path should be under screen/images")
            .join("captures.jsonl")
    }

    #[test]
    fn writer_creates_day_dir_on_first_save() {
        // Construction no longer creates any directories — the day dir is
        // created lazily at first save so that crossing local midnight
        // while the daemon runs silently rotates into the next day.
        let tmp = TempDir::new().unwrap();
        let writer = ScreenWriter::new(tmp.path().to_path_buf()).unwrap();
        let ts: DateTime<Utc> = "2026-04-12T09:00:15Z".parse().unwrap();
        let (local_day, _) = ScreenWriter::local_day_and_filename(ts);
        assert!(!tmp.path().join(&local_day).exists());
        let path = writer
            .save_screenshot(&minimal_png(), ts, "app", "win", "trigger")
            .unwrap();
        assert!(
            tmp.path()
                .join(local_day)
                .join("screen")
                .join("images")
                .is_dir()
        );
        assert!(captures_jsonl_for_image(&path).exists());
    }

    #[test]
    fn save_screenshot_writes_png_and_dataref() {
        let tmp = TempDir::new().unwrap();
        let writer = ScreenWriter::new(tmp.path().to_path_buf()).unwrap();

        let ts: DateTime<Utc> = "2026-04-12T09:00:15Z".parse().unwrap();
        let (_local_day, local_filename) = ScreenWriter::local_day_and_filename(ts);
        let png = minimal_png();

        let path = writer
            .save_screenshot(&png, ts, "VS Code", "main.rs", "app_focus")
            .unwrap();

        assert!(path.exists());
        assert_eq!(path.file_name().unwrap(), local_filename.as_str());

        let refs: Vec<DataRef> =
            alvum_core::storage::read_jsonl(&captures_jsonl_for_image(&path)).unwrap();
        assert_eq!(refs.len(), 1);
        assert_eq!(refs[0].source, "screen");
        assert_eq!(refs[0].mime, "image/png");
        assert_eq!(refs[0].path, format!("screen/images/{local_filename}"));

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

        let path1 = writer
            .save_screenshot(&png, ts1, "VS Code", "main.rs", "app_focus")
            .unwrap();
        let path2 = writer
            .save_screenshot(&png, ts2, "VS Code", "main.rs", "idle")
            .unwrap();

        let refs: Vec<DataRef> =
            alvum_core::storage::read_jsonl(&captures_jsonl_for_image(&path1)).unwrap();
        let (_, filename1) = ScreenWriter::local_day_and_filename(ts1);
        let (_, filename2) = ScreenWriter::local_day_and_filename(ts2);
        assert_eq!(refs.len(), 2);
        assert_eq!(refs[0].path, format!("screen/images/{filename1}"));
        assert_eq!(refs[1].path, format!("screen/images/{filename2}"));
        assert_eq!(
            captures_jsonl_for_image(&path1),
            captures_jsonl_for_image(&path2)
        );
    }
}
