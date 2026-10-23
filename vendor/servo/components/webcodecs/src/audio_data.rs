// @trace REQ-BRW-051
//! Decoded-audio output frame: a plain-data, DOM-free container the DOM layer
//! turns into an `AudioData`.

/// Audio sample formats (WebCodecs `AudioSampleFormat`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AudioSampleFormat {
    U8,
    S16,
    S32,
    F32,
    U8Planar,
    S16Planar,
    S32Planar,
    F32Planar,
}

impl AudioSampleFormat {
    pub fn bytes_per_sample(&self) -> usize {
        match self {
            AudioSampleFormat::U8 | AudioSampleFormat::U8Planar => 1,
            AudioSampleFormat::S16 | AudioSampleFormat::S16Planar => 2,
            AudioSampleFormat::S32 | AudioSampleFormat::F32 => 4,
            AudioSampleFormat::S32Planar | AudioSampleFormat::F32Planar => 4,
        }
    }
    pub fn is_planar(&self) -> bool {
        matches!(
            self,
            AudioSampleFormat::U8Planar
                | AudioSampleFormat::S16Planar
                | AudioSampleFormat::S32Planar
                | AudioSampleFormat::F32Planar
        )
    }
}

/// One decoded audio buffer.
#[derive(Debug, Clone)]
pub struct DecodedAudioFrame {
    pub format: AudioSampleFormat,
    pub sample_rate: f64,
    /// Number of interleaved channels (number of planes for planar formats).
    pub number_of_frames: u64,
    pub number_of_channels: u64,
    /// Microseconds.
    pub timestamp: i64,
    /// Sample bytes. For interleaved formats one buffer; for planar formats
    /// `number_of_channels` consecutive plane buffers (planes.len() ==
    /// number_of_channels).
    pub data: Vec<Vec<u8>>,
}

impl DecodedAudioFrame {
    /// `allocationSize()` mirror: bytes needed for a copyTo with the given
    /// plane/frame subset.
    pub fn allocation_size(
        &self,
        plane_index: usize,
        frame_offset: u64,
        frame_count: u64,
        format: Option<AudioSampleFormat>,
    ) -> Option<u64> {
        // Interleaved storage exposes a single plane (plane 0): per the
        // WebCodecs allocationSize contract, a planeIndex > 0 is only valid
        // for planar layouts. (Verified against the failing assertion
        // `allocation_size(1, 0, 10, None) == None` on an interleaved
        // stereo frame — the old channel-count bound wrongly accepted it.)
        if plane_index >= self.number_of_channels as usize {
            return None;
        }
        if plane_index > 0 && !self.format.is_planar() {
            return None;
        }
        if frame_offset.checked_add(frame_count)? > self.number_of_frames {
            return None;
        }
        let dst = format.unwrap_or(self.format);
        // Interleaved destinations read all channels; planar destinations read
        // exactly one plane. Mixing an interleaved source into a planar
        // destination (or vice versa) is a conversion the DOM layer performs;
        // this size math is per-destination.
        if dst.is_planar() {
            Some(frame_count * dst.bytes_per_sample() as u64)
        } else {
            Some(
                frame_count
                    * dst.bytes_per_sample() as u64
                    * self.number_of_channels,
            )
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn interleaved_stereo() -> DecodedAudioFrame {
        DecodedAudioFrame {
            format: AudioSampleFormat::F32,
            sample_rate: 48000.0,
            number_of_frames: 100,
            number_of_channels: 2,
            timestamp: 0,
            data: vec![vec![0u8; 100 * 2 * 4]],
        }
    }

    #[test]
    fn allocation_size_interleaved() {
        let f = interleaved_stereo();
        assert_eq!(
            f.allocation_size(0, 0, 10, None),
            Some(10 * 2 * 4)
        );
        assert_eq!(f.allocation_size(0, 95, 10, None), None);
        assert_eq!(f.allocation_size(1, 0, 10, None), None);
    }

    #[test]
    fn allocation_size_planar_dst() {
        let f = interleaved_stereo();
        assert_eq!(
            f.allocation_size(0, 0, 10, Some(AudioSampleFormat::F32Planar)),
            Some(40)
        );
    }
}
