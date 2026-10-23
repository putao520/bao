// @trace REQ-BRW-051
//! Decoded-video output frame: a plain-data, DOM-free container the DOM layer
//! turns into a `VideoFrame`. The layout/stride math is pure arithmetic and
//! unit-testable without a decoder backend.

/// One decoded frame as planar buffers. Timestamps are microseconds per the
/// WebCodecs spec.
#[derive(Debug, Clone)]
pub struct DecodedVideoFrame {
    pub format: FrameFormat,
    pub coded_width: u32,
    pub coded_height: u32,
    /// Width the page sees (after crop + rotation).
    pub display_width: u32,
    pub display_height: u32,
    /// Microseconds.
    pub timestamp: i64,
    pub duration: Option<i64>,
    pub color_space: FrameColorSpace,
    /// Plane bytes, `display_height` rows of stride `stride[plane]`. For
    /// tightly-packed planar formats the stride is the padded plane width.
    pub planes: Vec<Vec<u8>>,
    /// Per-plane stride in bytes.
    pub strides: Vec<u32>,
}

/// Planar video formats the decode plane produces. 8-bit planar YUV and
/// RGBA/BGRA are the portable surface (the gst backend narrows everything to
/// these for output; 10/12-bit chroma is truncated, matching the WebCodecs
/// "valid, but lossy" contract — high-bit-depth decodes still succeed).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FrameFormat {
    /// 4:2:0 planar, three planes Y/U/V.
    I420,
    /// 4:2:2 planar, three planes Y/U/V.
    I422,
    /// 4:4:4 planar, three planes Y/U/V.
    I444,
    /// RGBA.
    Rgba,
    /// BGRA.
    Bgra,
}

impl FrameFormat {
    /// Bytes per pixel of plane 0. For planar YUV this is the luma BPP.
    pub fn luma_bytes_per_pixel(&self) -> u32 {
        match self {
            FrameFormat::I420 | FrameFormat::I422 | FrameFormat::I444 => 1,
            FrameFormat::Rgba | FrameFormat::Bgra => 4,
        }
    }
    /// Plane count.
    pub fn num_planes(&self) -> usize {
        match self {
            FrameFormat::I420 | FrameFormat::I422 | FrameFormat::I444 => 3,
            FrameFormat::Rgba | FrameFormat::Bgra => 1,
        }
    }
}

/// Video color space with the WebCodecs enum value names (`primaries`,
/// `transfer`, `matrix`, `fullRange`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FrameColorSpace {
    pub primaries: &'static str,
    pub transfer: &'static str,
    pub matrix: &'static str,
    pub full_range: bool,
}

impl Default for FrameColorSpace {
    /// bt709 / bt709 / bt709 / limited — the WebCodecs default for YUV.
    fn default() -> Self {
        Self {
            primaries: "bt709",
            transfer: "bt709",
            matrix: "bt709",
            full_range: false,
        }
    }
}

/// A rectangle crop region (visible/coded rect in coded pixels).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FrameRect {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}

impl FrameRect {
    pub fn new(x: i32, y: i32, width: u32, height: u32) -> Self {
        Self { x, y, width, height }
    }
}

/// Copy-to layout for a single plane.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PlaneLayout {
    pub offset: usize,
    pub stride: u32,
}

/// Compute the visible-rect sub-copy of plane 0 for a tightly-packed frame.
/// Returns `(src_offset, src_stride, dst_layout)` for `copyTo`.
///
/// Cropping is done in visible pixels. The `src` offset lands on the first
/// visible row of plane 0.
pub fn plane_copy_layout(
    frame: &DecodedVideoFrame,
    plane: usize,
    visible: FrameRect,
) -> Option<(usize, u32, PlaneLayout)> {
    if plane >= frame.strides.len() || plane >= frame.planes.len() {
        return None;
    }
    let stride = frame.strides[plane];
    let bpp = frame.format.luma_bytes_per_pixel();
    if plane == 0 {
        // Sub-pixel crop shifts luma only.
        let row_bytes = visible.width.saturating_mul(bpp);
        let src_row_offset = visible.x as usize * bpp as usize;
        let src_row = visible.y as usize;
        if stride == 0 {
            return None;
        }
        let src_offset = src_row * stride as usize + src_row_offset;
        Some((
            src_offset,
            stride,
            PlaneLayout {
                offset: 0,
                stride: row_bytes,
            },
        ))
    } else {
        // Subsampled chroma: same rounding used by the 4:2:0/4:2:2 chroma
        // siting convention — ceil(crop / 2), clamped to the coded chroma
        // dimensions.
        let subsample = match frame.format {
            FrameFormat::I420 | FrameFormat::I422 => 2,
            FrameFormat::I444 => 1,
            FrameFormat::Rgba | FrameFormat::Bgra => 1,
        };
        let chroma_x = visible.x as usize / subsample;
        let chroma_y = visible.y as usize / subsample;
        let chroma_w = (visible.width as usize + subsample - 1) / subsample;
        let src_row_offset = chroma_x * bpp as usize;
        let src_row = chroma_y;
        let src_offset = src_row * stride as usize + src_row_offset;
        Some((
            src_offset,
            stride,
            PlaneLayout {
                offset: 0,
                stride: (chroma_w * bpp as usize) as u32,
            },
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame_i420() -> DecodedVideoFrame {
        DecodedVideoFrame {
            format: FrameFormat::I420,
            coded_width: 4,
            coded_height: 4,
            display_width: 4,
            display_height: 4,
            timestamp: 0,
            duration: None,
            color_space: FrameColorSpace::default(),
            // Y=16 bytes, U=4, V=4 (4x4 luma, 2x2 chroma).
            planes: vec![vec![0u8; 16], vec![0u8; 4], vec![0u8; 4]],
            strides: vec![4, 2, 2],
        }
    }

    #[test]
    fn plane_layout_luma() {
        let f = frame_i420();
        let (off, src_stride, layout) = plane_copy_layout(&f, 0, FrameRect::new(1, 1, 2, 2)).unwrap();
        assert_eq!(off, 1 * 4 + 1);
        assert_eq!(src_stride, 4);
        assert_eq!(layout.stride, 2);
    }

    #[test]
    fn plane_layout_chroma_subsample() {
        let f = frame_i420();
        let (off, src_stride, layout) = plane_copy_layout(&f, 1, FrameRect::new(2, 2, 2, 2)).unwrap();
        // chroma x=1, y=1 (2/2,2/2); stride=2 → offset = 1*2 + 1 = 3.
        assert_eq!(off, 1 * 2 + 1);
        assert_eq!(src_stride, 2);
        assert_eq!(layout.stride, 1);
    }
}
