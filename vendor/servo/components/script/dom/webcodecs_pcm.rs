/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

// @trace REQ-BRW-051
//! Pure PCM sample arithmetic for the WebCodecs audio data plane
//! (REQ-BRW-051, Bao fork self-build).
//!
//! The Wave A backend is a raw-PCM pass-through decoder: the PCM codec
//! registry codec strings
//! (<https://www.w3.org/TR/webcodecs-pcm-codec-registration/>) map 1:1 onto
//! interleaved `AudioSampleFormat` values, so "decoding" is a re-interpretation
//! of the encoded chunk bytes as samples with no compression, resampling or
//! priming. The helpers here implement the WebCodecs sample-format conversion
//! and copy arithmetic shared by `AudioData` (`copyTo`/`allocationSize`) and
//! `AudioDecoder` (chunk -> `AudioData`).

use crate::dom::bindings::codegen::Bindings::AudioDataBinding::AudioSampleFormat;

/// Number of bytes per sample for `format`.
pub(crate) fn sample_size(format: AudioSampleFormat) -> usize {
    match format {
        AudioSampleFormat::U8 | AudioSampleFormat::U8_planar => 1,
        AudioSampleFormat::S16 | AudioSampleFormat::S16_planar => 2,
        AudioSampleFormat::S32 | AudioSampleFormat::S32_planar => 4,
        AudioSampleFormat::F32 | AudioSampleFormat::F32_planar => 4,
    }
}

/// Whether `format` stores one plane per channel rather than interleaved.
pub(crate) fn is_planar(format: AudioSampleFormat) -> bool {
    matches!(
        format,
        AudioSampleFormat::U8_planar |
            AudioSampleFormat::S16_planar |
            AudioSampleFormat::S32_planar |
            AudioSampleFormat::F32_planar
    )
}

/// Numeric family of a sample format; the planar variants share the numbers of
/// their interleaved counterparts.
#[derive(Clone, Copy, PartialEq)]
enum SampleFamily {
    U8,
    S16,
    S32,
    F32,
}

fn family(format: AudioSampleFormat) -> SampleFamily {
    match format {
        AudioSampleFormat::U8 | AudioSampleFormat::U8_planar => SampleFamily::U8,
        AudioSampleFormat::S16 | AudioSampleFormat::S16_planar => SampleFamily::S16,
        AudioSampleFormat::S32 | AudioSampleFormat::S32_planar => SampleFamily::S32,
        AudioSampleFormat::F32 | AudioSampleFormat::F32_planar => SampleFamily::F32,
    }
}

/// Map a WebCodecs PCM codec-registry codec string to the `AudioSampleFormat`
/// its chunks decode into.
///
/// Only the raw PCM codecs are backed in Wave A; `pcm-s24`, `ulaw` and `alaw`
/// need sample-width/companding work that is a later slice, so they honestly
/// report unsupported instead of pretending.
pub(crate) fn sample_format_for_pcm_codec(codec: &str) -> Option<AudioSampleFormat> {
    match codec {
        "pcm-u8" => Some(AudioSampleFormat::U8),
        "pcm-s16" => Some(AudioSampleFormat::S16),
        "pcm-s32" => Some(AudioSampleFormat::S32),
        "pcm-f32" => Some(AudioSampleFormat::F32),
        _ => None,
    }
}

/// Read the sample at plane-local `index` of `data` as a `f64` in the source
/// format's numeric range.
pub(crate) fn read_sample_at(format: AudioSampleFormat, data: &[u8], index: usize) -> f64 {
    let size = sample_size(format);
    let start = index * size;
    if start + size > data.len() {
        return 0.0;
    }
    match family(format) {
        SampleFamily::U8 => data[start] as f64,
        SampleFamily::S16 => i16::from_le_bytes([data[start], data[start + 1]]) as f64,
        SampleFamily::S32 => {
            i32::from_le_bytes([
                data[start],
                data[start + 1],
                data[start + 2],
                data[start + 3],
            ]) as f64
        },
        SampleFamily::F32 => {
            f32::from_le_bytes([
                data[start],
                data[start + 1],
                data[start + 2],
                data[start + 3],
            ]) as f64
        },
    }
}

/// Clamp `value` into `[min, max]`.
fn clamp(value: f64, min: f64, max: f64) -> f64 {
    value.max(min).min(max)
}

/// Write `value` (already in the destination format's numeric range) at
/// plane-local `index` of `data`, clamping to the representable range.
pub(crate) fn write_sample_at(
    format: AudioSampleFormat,
    data: &mut [u8],
    index: usize,
    value: f64,
) {
    let size = sample_size(format);
    let start = index * size;
    if start + size > data.len() {
        return;
    }
    match family(format) {
        SampleFamily::U8 => {
            data[start] = clamp(value, 0.0, 255.0) as u8;
        },
        SampleFamily::S16 => {
            let value = clamp(value, -32768.0, 32767.0) as i16;
            data[start..start + 2].copy_from_slice(&value.to_le_bytes());
        },
        SampleFamily::S32 => {
            let value = clamp(value, -2147483648.0, 2147483647.0) as i32;
            data[start..start + 4].copy_from_slice(&value.to_le_bytes());
        },
        SampleFamily::F32 => {
            let value = value as f32;
            data[start..start + 4].copy_from_slice(&value.to_le_bytes());
        },
    }
}

/// Convert one sample between formats following the WebCodecs "audio sample
/// conversion" table (floor rounding; clamping happens on write).
pub(crate) fn convert_sample(value: f64, from: AudioSampleFormat, to: AudioSampleFormat) -> f64 {
    match (family(from), family(to)) {
        (SampleFamily::U8, SampleFamily::U8) |
        (SampleFamily::S16, SampleFamily::S16) |
        (SampleFamily::S32, SampleFamily::S32) |
        (SampleFamily::F32, SampleFamily::F32) => value,
        (SampleFamily::U8, SampleFamily::S16) => (value - 128.0) * 256.0,
        (SampleFamily::U8, SampleFamily::S32) => (value - 128.0) * 16777216.0,
        (SampleFamily::U8, SampleFamily::F32) => (value - 128.0) / 128.0,
        (SampleFamily::S16, SampleFamily::U8) => (value / 256.0).floor() + 128.0,
        (SampleFamily::S16, SampleFamily::S32) => value * 65536.0,
        (SampleFamily::S16, SampleFamily::F32) => value / 32768.0,
        (SampleFamily::S32, SampleFamily::U8) => (value / 16777216.0).floor() + 128.0,
        (SampleFamily::S32, SampleFamily::S16) => (value / 65536.0).floor(),
        (SampleFamily::S32, SampleFamily::F32) => value / 2147483648.0,
        (SampleFamily::F32, SampleFamily::U8) => (value * 128.0).floor() + 128.0,
        (SampleFamily::F32, SampleFamily::S16) => (value * 32768.0).floor(),
        (SampleFamily::F32, SampleFamily::S32) => (value * 2147483648.0).floor(),
    }
}

/// Base sample index of channel `channel` inside a planar buffer that holds
/// `frames` frames per channel; 0 for interleaved buffers.
pub(crate) fn channel_plane_base(
    format: AudioSampleFormat,
    channel: usize,
    frames: usize,
) -> usize {
    if is_planar(format) {
        channel * frames
    } else {
        0
    }
}

/// Copy (and convert) audio samples between two PCM buffers following the
/// WebCodecs copy algorithm.
///
/// * `source` is the whole decoded source buffer holding `source_frames`
///   frames of `channels` channels.
/// * `frame_offset`/`frame_count` select the copied frames.
/// * `destination` is the whole destination buffer; when the destination
///   format is planar only the single plane `destination_plane` is written,
///   when it is interleaved every channel is written (plane 0).
pub(crate) fn copy_samples(
    source_format: AudioSampleFormat,
    source: &[u8],
    channels: usize,
    source_frames: usize,
    frame_offset: usize,
    frame_count: usize,
    destination_format: AudioSampleFormat,
    destination: &mut [u8],
    destination_plane: usize,
) {
    let source_planar = is_planar(source_format);
    let destination_planar = is_planar(destination_format);
    if channels == 0 {
        return;
    }
    if destination_planar {
        let channel = destination_plane.min(channels - 1);
        let base = channel_plane_base(source_format, channel, source_frames);
        for frame_index in 0..frame_count {
            let source_frame = frame_offset + frame_index;
            let source_index = if source_planar {
                base + source_frame
            } else {
                source_frame * channels + channel
            };
            let value = read_sample_at(source_format, source, source_index);
            let value = convert_sample(value, source_format, destination_format);
            write_sample_at(destination_format, destination, frame_index, value);
        }
    } else {
        for frame_index in 0..frame_count {
            let source_frame = frame_offset + frame_index;
            for channel in 0..channels {
                let base = channel_plane_base(source_format, channel, source_frames);
                let source_index = if source_planar {
                    base + source_frame
                } else {
                    source_frame * channels + channel
                };
                let value = read_sample_at(source_format, source, source_index);
                let value = convert_sample(value, source_format, destination_format);
                write_sample_at(
                    destination_format,
                    destination,
                    frame_index * channels + channel,
                    value,
                );
            }
        }
    }
}
