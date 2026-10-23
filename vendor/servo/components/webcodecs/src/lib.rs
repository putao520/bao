// @trace REQ-BRW-051
//! WebCodecs codec plane (REQ-BRW-051 C2): codec-string parsing, decoder
//! config validation + lifecycle, GStreamer video decode + audio decode
//! (opus/AAC, Wave B) backends, a GStreamer audio encode backend
//! (opusenc/avenc_aac, Wave B), and an image-crate ImageDecoder backend. BAO
//! fork-native component (upstream servo has none). Pure-Rust, DOM-free: the
//! DOM glue lives in `components/script/dom/{audio,video}*.rs` and feeds
//! plain-data structs to this crate; the backends emit plain-data output
//! (frames / encoded packets) back to the DOM.

pub mod audio_decoder;
pub mod audio_data;
pub mod config;
pub mod error;
// BAO patch (G1, 2026-10-21): the gst decode backend owns the three optional
// GStreamer crates (`gstreamer` / `gstreamer-app` / `gstreamer-video`, only
// activated by the `media-gstreamer` feature). Gate the module so
// `servo/webcodecs` without `media-gstreamer` stays pure-Rust and compiles on
// Windows (CLAUDE.md Windows 交叉编译准入律). The sole consumer,
// `script/dom/videodecoder.rs`, mirrors this gate with
// `cfg(feature = "webcodecs-gstreamer")` (which implies
// `webcodecs/media-gstreamer`).
#[cfg(feature = "media-gstreamer")]
pub mod gst_video_decoder;
// BAO patch (Wave B, 2026-10-21): GStreamer audio decode/encode backends
// (REQ-BRW-051 C2). Same feature gate as the video backend — the three
// GStreamer crates (plus `gstreamer-audio`) are only linked under
// `media-gstreamer`, keeping `servo/webcodecs` alone pure-Rust on Windows
// (CLAUDE.md Windows 交叉编译准入律). Consumers: `script/dom/audiodecoder.rs`
// and `script/dom/audioencoder.rs` under `cfg(feature = "webcodecs-gstreamer")`.
#[cfg(feature = "media-gstreamer")]
pub mod gst_audio_decoder;
#[cfg(feature = "media-gstreamer")]
pub mod gst_audio_encoder;
pub mod image_decoder;
pub mod image_frame;
pub mod video_decoder;
pub mod video_frame;
pub mod webcodecs_codec;
