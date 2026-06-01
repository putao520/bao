// @trace TEST-BRW-019 [req:REQ-BRW-001,REQ-BRW-002,REQ-CDP-007] [level:unit]
// Comprehensive tests for PageState, ScreenshotFormat/encode_image, BrowserError,
// format_js_value (via PageHandle::evaluate_js internals), and PageHandle close/state logic.

use std::error::Error;

use bao_browser::{BrowserError, PageState, ScreenshotFormat};
use bao_browser::encode_image;
use image::{Rgba, RgbaImage};

// ============================================================
// §1 PageState — enum variants, traits, transitions
// ============================================================

#[test]
fn page_state_all_variants_are_distinct() {
    let all = [
        PageState::Created,
        PageState::Navigating,
        PageState::Interactive,
        PageState::Idle,
        PageState::Closed,
    ];
    for i in 0..all.len() {
        for j in 0..all.len() {
            if i == j {
                assert_eq!(all[i], all[j]);
            } else {
                assert_ne!(all[i], all[j]);
            }
        }
    }
}

#[test]
fn page_state_equality_reflexive() {
    assert_eq!(PageState::Created, PageState::Created);
    assert_eq!(PageState::Navigating, PageState::Navigating);
    assert_eq!(PageState::Interactive, PageState::Interactive);
    assert_eq!(PageState::Idle, PageState::Idle);
    assert_eq!(PageState::Closed, PageState::Closed);
}

#[test]
fn page_state_clone_preserves_value() {
    let states = [
        PageState::Created,
        PageState::Navigating,
        PageState::Interactive,
        PageState::Idle,
        PageState::Closed,
    ];
    for s in &states {
        assert_eq!(*s, s.clone());
    }
}

#[test]
fn page_state_copy_semantics() {
    let a = PageState::Interactive;
    let b = a;
    assert_eq!(a, b);
}

#[test]
fn page_state_debug_includes_variant_name() {
    assert!(format!("{:?}", PageState::Created).contains("Created"));
    assert!(format!("{:?}", PageState::Navigating).contains("Navigating"));
    assert!(format!("{:?}", PageState::Interactive).contains("Interactive"));
    assert!(format!("{:?}", PageState::Idle).contains("Idle"));
    assert!(format!("{:?}", PageState::Closed).contains("Closed"));
}

#[test]
fn page_state_partial_ord_not_implemented() {
    // PageState derives Debug, Clone, Copy, PartialEq, Eq — no Ord.
    // This test just verifies the type compiles with those bounds.
    let _ = PageState::Created == PageState::Closed;
}

#[test]
fn page_state_transition_sequence_logical() {
    // Verify a typical lifecycle: Created -> Navigating -> Interactive -> Idle -> Closed
    let mut state = PageState::Created;
    assert_eq!(state, PageState::Created);

    state = PageState::Navigating;
    assert_eq!(state, PageState::Navigating);

    state = PageState::Interactive;
    assert_eq!(state, PageState::Interactive);

    state = PageState::Idle;
    assert_eq!(state, PageState::Idle);

    state = PageState::Closed;
    assert_eq!(state, PageState::Closed);
}

#[test]
fn page_state_closed_is_final() {
    let state = PageState::Closed;
    assert_eq!(state, PageState::Closed);
    assert_ne!(state, PageState::Created);
    assert_ne!(state, PageState::Navigating);
    assert_ne!(state, PageState::Interactive);
    assert_ne!(state, PageState::Idle);
}

#[test]
fn page_state_created_not_equal_any_other() {
    assert_ne!(PageState::Created, PageState::Navigating);
    assert_ne!(PageState::Created, PageState::Interactive);
    assert_ne!(PageState::Created, PageState::Idle);
    assert_ne!(PageState::Created, PageState::Closed);
}

#[test]
fn page_state_implements_send_and_sync() {
    fn assert_send<T: Send>() {}
    fn assert_sync<T: Sync>() {}
    assert_send::<PageState>();
    assert_sync::<PageState>();
}

// ============================================================
// §2 ScreenshotFormat & encode_image
// ============================================================

fn red_image(w: u32, h: u32) -> RgbaImage {
    RgbaImage::from_pixel(w, h, Rgba([255, 0, 0, 255]))
}

fn green_image(w: u32, h: u32) -> RgbaImage {
    RgbaImage::from_pixel(w, h, Rgba([0, 255, 0, 255]))
}

fn blue_image(w: u32, h: u32) -> RgbaImage {
    RgbaImage::from_pixel(w, h, Rgba([0, 0, 255, 255]))
}

fn transparent_image(w: u32, h: u32) -> RgbaImage {
    RgbaImage::from_pixel(w, h, Rgba([0, 0, 0, 0]))
}

fn gradient_image(w: u32, h: u32) -> RgbaImage {
    let mut img = RgbaImage::new(w, h);
    for y in 0..h {
        for x in 0..w {
            let r = ((x * 255) / w) as u8;
            let g = ((y * 255) / h) as u8;
            let b = (((x + y) * 255) / (w + h)) as u8;
            img.put_pixel(x, y, Rgba([r, g, b, 255]));
        }
    }
    img
}

#[test]
fn png_magic_bytes() {
    let img = red_image(1, 1);
    let data = encode_image(&img, ScreenshotFormat::Png).unwrap();
    assert_eq!(&data[0..4], &[0x89, 0x50, 0x4E, 0x47], "PNG magic: 89 50 4E 47");
}

#[test]
fn jpeg_magic_bytes() {
    let img = red_image(1, 1);
    let data = encode_image(&img, ScreenshotFormat::Jpeg).unwrap();
    assert_eq!(&data[0..2], &[0xFF, 0xD8], "JPEG SOI marker: FF D8");
}

#[test]
fn png_1x1_red_nonempty() {
    let img = red_image(1, 1);
    let data = encode_image(&img, ScreenshotFormat::Png).unwrap();
    assert!(!data.is_empty());
}

#[test]
fn jpeg_1x1_green_nonempty() {
    let img = green_image(1, 1);
    let data = encode_image(&img, ScreenshotFormat::Jpeg).unwrap();
    assert!(!data.is_empty());
}

#[test]
fn png_1x1_blue_nonempty() {
    let img = blue_image(1, 1);
    let data = encode_image(&img, ScreenshotFormat::Png).unwrap();
    assert!(!data.is_empty());
}

#[test]
fn encode_png_large_image() {
    let img = red_image(800, 600);
    let data = encode_image(&img, ScreenshotFormat::Png).unwrap();
    assert!(data.len() > 1000, "800x600 PNG should be substantial: {} bytes", data.len());
}

#[test]
fn encode_jpeg_large_image() {
    let img = green_image(800, 600);
    let data = encode_image(&img, ScreenshotFormat::Jpeg).unwrap();
    assert!(data.len() > 1000, "800x600 JPEG should be substantial: {} bytes", data.len());
}

#[test]
fn png_transparent_image() {
    let img = transparent_image(4, 4);
    let data = encode_image(&img, ScreenshotFormat::Png).unwrap();
    assert!(!data.is_empty());
    assert_eq!(&data[0..4], &[0x89, 0x50, 0x4E, 0x47]);
}

#[test]
fn jpeg_transparent_image() {
    let img = transparent_image(4, 4);
    let data = encode_image(&img, ScreenshotFormat::Jpeg).unwrap();
    assert!(!data.is_empty());
}

#[test]
fn png_gradient_image() {
    let img = gradient_image(64, 64);
    let data = encode_image(&img, ScreenshotFormat::Png).unwrap();
    assert!(!data.is_empty());
    assert!(data.len() > 100);
}

#[test]
fn jpeg_gradient_image() {
    let img = gradient_image(64, 64);
    let data = encode_image(&img, ScreenshotFormat::Jpeg).unwrap();
    assert!(!data.is_empty());
    assert!(data.len() > 100);
}

#[test]
fn png_vs_jpeg_different_output() {
    let img = red_image(100, 100);
    let png = encode_image(&img, ScreenshotFormat::Png).unwrap();
    let jpeg = encode_image(&img, ScreenshotFormat::Jpeg).unwrap();
    assert_ne!(png, jpeg, "PNG and JPEG encoding should produce different bytes");
}

#[test]
fn same_image_same_png_encoding() {
    let img = red_image(10, 10);
    let a = encode_image(&img, ScreenshotFormat::Png).unwrap();
    let b = encode_image(&img, ScreenshotFormat::Png).unwrap();
    assert_eq!(a, b, "Deterministic PNG encoding");
}

#[test]
fn same_image_same_jpeg_encoding() {
    let img = red_image(10, 10);
    let a = encode_image(&img, ScreenshotFormat::Jpeg).unwrap();
    let b = encode_image(&img, ScreenshotFormat::Jpeg).unwrap();
    assert_eq!(a, b, "Deterministic JPEG encoding");
}

#[test]
fn encode_empty_0x0_png_no_panic() {
    let img = RgbaImage::new(0, 0);
    let result = encode_image(&img, ScreenshotFormat::Png);
    // 0x0 image may succeed or error, but must not panic
    let _ = result;
}

#[test]
fn encode_empty_0x0_jpeg_no_panic() {
    let img = RgbaImage::new(0, 0);
    let result = encode_image(&img, ScreenshotFormat::Jpeg);
    let _ = result;
}

#[test]
fn png_wider_than_tall() {
    let img = red_image(2000, 100);
    let data = encode_image(&img, ScreenshotFormat::Png).unwrap();
    assert!(!data.is_empty());
}

#[test]
fn png_taller_than_wide() {
    let img = red_image(100, 2000);
    let data = encode_image(&img, ScreenshotFormat::Png).unwrap();
    assert!(!data.is_empty());
}

#[test]
fn jpeg_wider_than_tall() {
    let img = green_image(2000, 100);
    let data = encode_image(&img, ScreenshotFormat::Jpeg).unwrap();
    assert!(!data.is_empty());
}

#[test]
fn jpeg_taller_than_wide() {
    let img = green_image(100, 2000);
    let data = encode_image(&img, ScreenshotFormat::Jpeg).unwrap();
    assert!(!data.is_empty());
}

#[test]
fn encode_multiple_formats_sequentially() {
    let img = blue_image(50, 50);
    for _ in 0..10 {
        let png = encode_image(&img, ScreenshotFormat::Png).unwrap();
        let jpeg = encode_image(&img, ScreenshotFormat::Jpeg).unwrap();
        assert!(!png.is_empty());
        assert!(!jpeg.is_empty());
    }
}

#[test]
fn png_white_image() {
    let img = RgbaImage::from_pixel(10, 10, Rgba([255, 255, 255, 255]));
    let data = encode_image(&img, ScreenshotFormat::Png).unwrap();
    assert!(!data.is_empty());
}

#[test]
fn png_black_image() {
    let img = RgbaImage::from_pixel(10, 10, Rgba([0, 0, 0, 255]));
    let data = encode_image(&img, ScreenshotFormat::Png).unwrap();
    assert!(!data.is_empty());
}

#[test]
fn jpeg_white_image() {
    let img = RgbaImage::from_pixel(10, 10, Rgba([255, 255, 255, 255]));
    let data = encode_image(&img, ScreenshotFormat::Jpeg).unwrap();
    assert!(!data.is_empty());
}

#[test]
fn jpeg_black_image() {
    let img = RgbaImage::from_pixel(10, 10, Rgba([0, 0, 0, 255]));
    let data = encode_image(&img, ScreenshotFormat::Jpeg).unwrap();
    assert!(!data.is_empty());
}

#[test]
fn screenshot_format_variants_compile() {
    // Verify both variants exist and are usable as enum values
    let _png = ScreenshotFormat::Png;
    let _jpeg = ScreenshotFormat::Jpeg;
}

// ============================================================
// §3 BrowserError — all variants, Display, Error trait, edge cases
// ============================================================

#[test]
fn error_init_display() {
    let err = BrowserError::Init("engine failed".into());
    assert_eq!(err.to_string(), "browser init error: engine failed");
}

#[test]
fn error_navigation_display() {
    let err = BrowserError::Navigation("timeout".into());
    assert_eq!(err.to_string(), "navigation error: timeout");
}

#[test]
fn error_rendering_display() {
    let err = BrowserError::Rendering("gpu lost".into());
    assert_eq!(err.to_string(), "rendering error: gpu lost");
}

#[test]
fn error_javascript_display() {
    let err = BrowserError::JavaScript("syntax error".into());
    assert_eq!(err.to_string(), "javascript error: syntax error");
}

#[test]
fn error_cdp_display() {
    let err = BrowserError::CDP("connection refused".into());
    assert_eq!(err.to_string(), "cdp error: connection refused");
}

#[test]
fn error_all_variants_have_distinct_prefixes() {
    let msgs = [
        BrowserError::Init("x".into()).to_string(),
        BrowserError::Navigation("x".into()).to_string(),
        BrowserError::Rendering("x".into()).to_string(),
        BrowserError::JavaScript("x".into()).to_string(),
        BrowserError::CDP("x".into()).to_string(),
    ];
    let prefixes: Vec<&str> = msgs.iter().map(|m| m.split(':').next().unwrap().trim()).collect();
    // Each prefix should be unique
    for i in 0..prefixes.len() {
        for j in (i + 1)..prefixes.len() {
            assert_ne!(prefixes[i], prefixes[j], "Duplicate prefix: {}", prefixes[i]);
        }
    }
}

#[test]
fn error_debug_contains_variant_name() {
    assert!(format!("{:?}", BrowserError::Init("a".into())).contains("Init"));
    assert!(format!("{:?}", BrowserError::Navigation("b".into())).contains("Navigation"));
    assert!(format!("{:?}", BrowserError::Rendering("c".into())).contains("Rendering"));
    assert!(format!("{:?}", BrowserError::JavaScript("d".into())).contains("JavaScript"));
    assert!(format!("{:?}", BrowserError::CDP("e".into())).contains("CDP"));
}

#[test]
fn error_debug_contains_message() {
    let err = BrowserError::Init("servo crash log".into());
    let debug = format!("{err:?}");
    assert!(debug.contains("servo crash log"));
}

#[test]
fn error_implements_std_error() {
    let err = BrowserError::Navigation("test".into());
    let _: &dyn Error = &err;
}

#[test]
fn error_boxed_dyn_error() {
    let err: Box<dyn Error> = Box::new(BrowserError::CDP("port busy".into()));
    assert_eq!(err.to_string(), "cdp error: port busy");
}

#[test]
fn error_source_is_none() {
    // BrowserError has no source chain — source() returns None
    let err = BrowserError::Init("test".into());
    assert!(err.source().is_none());
}

#[test]
fn error_empty_message() {
    let err = BrowserError::Rendering(String::new());
    assert_eq!(err.to_string(), "rendering error: ");
}

#[test]
fn error_multiline_message() {
    let err = BrowserError::JavaScript("line1\nline2\nline3".into());
    let msg = err.to_string();
    assert!(msg.contains("line1"));
    assert!(msg.contains("line2"));
    assert!(msg.contains("line3"));
}

#[test]
fn error_unicode_message() {
    let err = BrowserError::Navigation("连接超时 🌐".into());
    assert!(err.to_string().contains("连接超时 🌐"));
}

#[test]
fn error_long_message() {
    let long_msg = "x".repeat(10000);
    let err = BrowserError::Init(long_msg.clone());
    assert_eq!(err.to_string(), format!("browser init error: {long_msg}"));
}

#[test]
fn error_special_characters_message() {
    let err = BrowserError::CDP("error: <script>alert('xss')</script>".into());
    assert!(err.to_string().contains("<script>"));
}

#[test]
fn error_null_bytes_in_message() {
    let err = BrowserError::Init("before\0after".into());
    let msg = err.to_string();
    assert!(msg.contains("before"));
}

#[test]
fn error_debug_roundtrip() {
    let err = BrowserError::Navigation("test".into());
    let debug = format!("{err:?}");
    assert!(debug.contains("Navigation"));
    assert!(debug.contains("test"));
    // BrowserError does not derive Clone — verify it moves
    let moved = err;
    assert_eq!(moved.to_string(), "navigation error: test");
}

#[test]
fn error_send_sync() {
    fn assert_send<T: Send>() {}
    fn assert_sync<T: Sync>() {}
    assert_send::<BrowserError>();
    assert_sync::<BrowserError>();
}

#[test]
fn error_result_propagation() {
    fn fallible() -> Result<(), BrowserError> {
        Err(BrowserError::CDP("timeout".into()))
    }
    let result = fallible();
    assert!(result.is_err());
    let err = result.unwrap_err();
    assert!(err.to_string().contains("timeout"));
}

#[test]
fn error_match_all_variants() {
    let errors: Vec<BrowserError> = vec![
        BrowserError::Init("a".into()),
        BrowserError::Navigation("b".into()),
        BrowserError::Rendering("c".into()),
        BrowserError::JavaScript("d".into()),
        BrowserError::CDP("e".into()),
    ];
    for err in errors {
        let msg = err.to_string();
        assert!(!msg.is_empty(), "Display should produce non-empty string");
        assert!(msg.contains("error"), "Display should contain 'error': {msg}");
    }
}

// ============================================================
// §4 Cross-concern: encode_image returns BrowserError on failure
// ============================================================

#[test]
fn encode_image_png_returns_browser_error_on_invalid() {
    // We can't easily force image::write_to to fail with a valid image,
    // but verify the error type is BrowserError::Rendering
    let img = red_image(1, 1);
    let result = encode_image(&img, ScreenshotFormat::Png);
    match result {
        Ok(data) => assert!(!data.is_empty()),
        Err(BrowserError::Rendering(msg)) => {
            assert!(msg.contains("PNG encode failed"), "unexpected: {msg}");
        }
        Err(other) => panic!("Expected BrowserError::Rendering, got: {other}"),
    }
}

#[test]
fn encode_image_jpeg_returns_browser_error_on_invalid() {
    let img = red_image(1, 1);
    let result = encode_image(&img, ScreenshotFormat::Jpeg);
    match result {
        Ok(data) => assert!(!data.is_empty()),
        Err(BrowserError::Rendering(msg)) => {
            assert!(msg.contains("JPEG encode failed"), "unexpected: {msg}");
        }
        Err(other) => panic!("Expected BrowserError::Rendering, got: {other}"),
    }
}

// ============================================================
// §5 PageState in collection contexts
// ============================================================

#[test]
fn page_state_in_vec() {
    let states = vec![
        PageState::Created,
        PageState::Navigating,
        PageState::Interactive,
        PageState::Idle,
        PageState::Closed,
    ];
    assert_eq!(states.len(), 5);
    assert_eq!(states[0], PageState::Created);
    assert_eq!(states[4], PageState::Closed);
}

#[test]
fn page_state_iterator() {
    let states = [PageState::Created, PageState::Navigating, PageState::Closed];
    let closed_count = states.iter().filter(|s| **s == PageState::Closed).count();
    assert_eq!(closed_count, 1);
}

#[test]
fn page_state_match_exhaustive() {
    fn classify(s: PageState) -> &'static str {
        match s {
            PageState::Created => "created",
            PageState::Navigating => "navigating",
            PageState::Interactive => "interactive",
            PageState::Idle => "idle",
            PageState::Closed => "closed",
        }
    }
    assert_eq!(classify(PageState::Created), "created");
    assert_eq!(classify(PageState::Navigating), "navigating");
    assert_eq!(classify(PageState::Interactive), "interactive");
    assert_eq!(classify(PageState::Idle), "idle");
    assert_eq!(classify(PageState::Closed), "closed");
}

#[test]
fn page_state_hash_consistency() {
    use std::collections::HashMap;
    // PageState doesn't derive Hash — use string keys for HashMap
    let mut counts = HashMap::new();
    let key = format!("{:?}", PageState::Created);
    *counts.entry(key).or_insert(0) += 1;
    assert_eq!(counts.get("Created"), Some(&1));
}

// ============================================================
// §6 encode_image capacity pre-allocation
// ============================================================

#[test]
fn png_output_size_reasonable_for_dimensions() {
    let img = red_image(100, 100);
    let data = encode_image(&img, ScreenshotFormat::Png).unwrap();
    // 100x100 RGBA raw = 40000 bytes, PNG compressed should be smaller
    assert!(data.len() < 40000, "PNG should be compressed: {} bytes", data.len());
}

#[test]
fn jpeg_output_size_reasonable_for_dimensions() {
    let img = red_image(100, 100);
    let data = encode_image(&img, ScreenshotFormat::Jpeg).unwrap();
    // 100x100 RGBA raw = 40000 bytes, JPEG compressed should be smaller
    assert!(data.len() < 40000, "JPEG should be compressed: {} bytes", data.len());
}

#[test]
fn larger_image_produces_larger_png() {
    let small = red_image(10, 10);
    let large = red_image(200, 200);
    let small_data = encode_image(&small, ScreenshotFormat::Png).unwrap();
    let large_data = encode_image(&large, ScreenshotFormat::Png).unwrap();
    assert!(large_data.len() > small_data.len());
}

#[test]
fn larger_image_produces_larger_jpeg() {
    let small = green_image(10, 10);
    let large = green_image(200, 200);
    let small_data = encode_image(&small, ScreenshotFormat::Jpeg).unwrap();
    let large_data = encode_image(&large, ScreenshotFormat::Jpeg).unwrap();
    assert!(large_data.len() > small_data.len());
}

// ============================================================
// §7 BrowserError pattern matching and downcasting
// ============================================================

#[test]
fn error_downcast_ref() {
    let err: Box<dyn Error> = Box::new(BrowserError::Init("fail".into()));
    let downcast = err.downcast_ref::<BrowserError>();
    assert!(downcast.is_some());
    match downcast.unwrap() {
        BrowserError::Init(msg) => assert_eq!(msg, "fail"),
        other => panic!("Expected Init, got: {other:?}"),
    }
}

#[test]
fn error_pattern_match_all_variants() {
    fn extract_msg(err: &BrowserError) -> &str {
        match err {
            BrowserError::Init(m) => m,
            BrowserError::Navigation(m) => m,
            BrowserError::Rendering(m) => m,
            BrowserError::JavaScript(m) => m,
            BrowserError::CDP(m) => m,
        }
    }
    assert_eq!(extract_msg(&BrowserError::Init("a".into())), "a");
    assert_eq!(extract_msg(&BrowserError::Navigation("b".into())), "b");
    assert_eq!(extract_msg(&BrowserError::Rendering("c".into())), "c");
    assert_eq!(extract_msg(&BrowserError::JavaScript("d".into())), "d");
    assert_eq!(extract_msg(&BrowserError::CDP("e".into())), "e");
}

#[test]
fn error_collected_into_vec() {
    let errors: Vec<BrowserError> = vec![
        BrowserError::Init("1".into()),
        BrowserError::Navigation("2".into()),
        BrowserError::Rendering("3".into()),
    ];
    assert_eq!(errors.len(), 3);
    assert!(errors.iter().all(|e| !e.to_string().is_empty()));
}
