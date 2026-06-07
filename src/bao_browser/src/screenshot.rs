// @trace REQ-BRW-002  REQ-CDP-007: Screenshot capture via servo rendering pipeline
use std::io::Cursor;

use image::{ImageFormat, RgbaImage};

use crate::error::BrowserError;

pub enum ScreenshotFormat {
    Png,
    Jpeg,
    WebP,
}

pub fn encode_image(image: &RgbaImage, format: ScreenshotFormat) -> Result<Vec<u8>, BrowserError> {
    let mut buf = Cursor::new(Vec::with_capacity(image.width() as usize * image.height() as usize));
    match format {
        ScreenshotFormat::Png => image
            .write_to(&mut buf, ImageFormat::Png)
            .map_err(|e| BrowserError::Rendering(format!("PNG encode failed: {e}")))?,
        ScreenshotFormat::Jpeg => {
            let rgb = image::DynamicImage::ImageRgba8(image.clone()).to_rgb8();
            rgb.write_to(&mut buf, ImageFormat::Jpeg)
                .map_err(|e| BrowserError::Rendering(format!("JPEG encode failed: {e}")))?;
        }
        ScreenshotFormat::WebP => image
            .write_to(&mut buf, ImageFormat::WebP)
            .map_err(|e| BrowserError::Rendering(format!("WebP encode failed: {e}")))?,
    }
    Ok(buf.into_inner())
}

#[cfg(test)]
mod tests {
    // @trace REQ-BRW-002 [req:REQ-BRW-002,REQ-CDP-007] [level:unit]
    use super::*;
    use image::Rgba;

    fn red_image(w: u32, h: u32) -> RgbaImage {
        RgbaImage::from_pixel(w, h, Rgba([255, 0, 0, 255]))
    }

    #[test]
    fn encode_1x1_png_returns_nonempty_with_magic() {
        let img = red_image(1, 1);
        let out = encode_image(&img, ScreenshotFormat::Png).unwrap();
        assert!(!out.is_empty());
        assert_eq!(&out[0..4], &[0x89, 0x50, 0x4E, 0x47]);
    }

    #[test]
    fn encode_1x1_jpeg_returns_nonempty_with_magic() {
        let img = red_image(1, 1);
        let out = encode_image(&img, ScreenshotFormat::Jpeg).unwrap();
        assert!(!out.is_empty());
        assert_eq!(&out[0..2], &[0xFF, 0xD8]);
    }

    #[test]
    fn encode_100x100_png_produces_valid_data() {
        let img = red_image(100, 100);
        let out = encode_image(&img, ScreenshotFormat::Png).unwrap();
        assert!(!out.is_empty());
        assert_eq!(&out[0..4], &[0x89, 0x50, 0x4E, 0x47]);
    }

    #[test]
    fn encode_100x100_jpeg_produces_valid_data() {
        let img = red_image(100, 100);
        let out = encode_image(&img, ScreenshotFormat::Jpeg).unwrap();
        assert!(!out.is_empty());
        assert_eq!(&out[0..2], &[0xFF, 0xD8]);
    }

    #[test]
    fn encode_1x1_webp_returns_nonempty_with_riff_magic() {
        let img = red_image(1, 1);
        let out = encode_image(&img, ScreenshotFormat::WebP).unwrap();
        assert!(!out.is_empty());
        // WebP files start with "RIFF" header
        assert_eq!(&out[0..4], b"RIFF");
    }

    #[test]
    fn encode_100x100_webp_produces_valid_data() {
        let img = red_image(100, 100);
        let out = encode_image(&img, ScreenshotFormat::WebP).unwrap();
        assert!(!out.is_empty());
        assert_eq!(&out[0..4], b"RIFF");
    }

    #[test]
    fn encode_empty_image_no_panic() {
        let img = red_image(0, 0);
        let _ = encode_image(&img, ScreenshotFormat::Png);
        let _ = encode_image(&img, ScreenshotFormat::Jpeg);
        let _ = encode_image(&img, ScreenshotFormat::WebP);
    }

    #[test]
    fn all_formats_produce_nontrivial_output() {
        let img = red_image(200, 200);
        let png = encode_image(&img, ScreenshotFormat::Png).unwrap();
        let jpeg = encode_image(&img, ScreenshotFormat::Jpeg).unwrap();
        let webp = encode_image(&img, ScreenshotFormat::WebP).unwrap();
        assert!(png.len() > 100, "PNG should be nontrivial: {} bytes", png.len());
        assert!(jpeg.len() > 100, "JPEG should be nontrivial: {} bytes", jpeg.len());
        assert!(webp.len() > 100, "WebP should be nontrivial: {} bytes", webp.len());
    }
}
