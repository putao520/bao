// @trace REQ-BRW-002  REQ-CDP-007: Screenshot capture via servo rendering pipeline
use std::io::Cursor;

use image::{ImageFormat, RgbaImage};

use crate::error::BrowserError;

pub enum ScreenshotFormat {
    Png,
    Jpeg,
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
    }
    Ok(buf.into_inner())
}
