//! End-to-end test of the native Vision-framework OCR path.
//! macOS only — Vision.framework is not available elsewhere.

#[cfg(target_os = "macos")]
#[test]
fn native_ocr_extracts_text_from_fixture() {
    let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/ocr_sample.png");
    let text = alvum_processor_screen::ocr::extract_text(&path)
        .expect("OCR call should succeed");
    let lower = text.to_lowercase();
    assert!(
        lower.contains("alvum") && lower.contains("test"),
        "expected to recognize 'alvum test' in fixture, got {text:?}"
    );
}
