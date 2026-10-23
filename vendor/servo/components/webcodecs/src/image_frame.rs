// @trace REQ-BRW-051
//! Decoded-image output frame (plain-data, DOM-free) — the ImageDecoder
//! output surface. The DOM layer turns an [`ImageFrame`] into a `VideoFrame`
//! with an RGBA format.

/// One decoded image frame (always RGBA8 — the `image` crate normalizes).
#[derive(Debug, Clone)]
pub struct ImageFrame {
    pub width: u32,
    pub height: u32,
    /// RGBA8 pixels, width*height*4 bytes.
    pub rgba: Vec<u8>,
    /// Frame delay in milliseconds (animation frames; 0 for stills).
    pub duration_ms: u32,
}

/// Image track metadata (ImageTrack mirror).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImageTrack {
    /// Whether this track is animated (frameCount > 1 with timing).
    pub animated: bool,
    /// Number of frames in this track.
    pub frame_count: u32,
    /// Repetition count for animation: `None` = infinite, `Some(n)` = finite
    /// n (0 = play once — the WPT asserts `repetitionCount === 0` for a
    /// still and `Infinity` for a looping GIF).
    pub repetition_count: Option<u32>,
}

impl ImageTrack {
    pub fn still() -> Self {
        Self {
            animated: false,
            frame_count: 1,
            repetition_count: Some(0),
        }
    }
}

/// Full ImageDecoder decode output.
#[derive(Debug, Clone)]
pub struct ImageDecodeResult {
    pub frames: Vec<ImageFrame>,
    pub track: ImageTrack,
}
