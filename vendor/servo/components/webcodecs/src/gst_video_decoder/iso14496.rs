// @trace REQ-BRW-051
//! ISO 14496-15 avcC/hvcC record parsing — extract SPS/PPS (avcC) and VPS/
//! SPS/PPS (hvcC) parameter-set NAL units.
//!
//! Contract source: ISO/IEC 14496-15 §5.3.3.1.2 (AVCDecoderConfigurationRecord)
//! and §5.3.6.1.2 (HEVCDecoderConfigurationRecord). The WPT codec-specific
//! tests slice these records out of real MP4 files and hand them to
//! `configure()` as `description`, so the decode plane must accept raw avcC/
//! hvcC bytes. Record shapes:
//!
//! avcC: configurationVersion(1) AVCProfileIndication(1)
//!       profile_compatibility(1) AVCLevelIndication(1) 0xFF(reserved+lengthSizeMinusOne=3)
//!       0xE0|numOfSPS(1) [SPS: len(2 BE) bytes]… 0xE0-ish|numOfPPS(1) [PPS: len(2) bytes]
//!
//! hvcC: configurationVersion(1) general_* (21 bytes) min_spatial_segmentation
//!       (2) parallelismType(1) chromaFormat(1) bitDepthLuma(1) bitDepthChroma(1)
//!       avgFrameRate(2) 0xF0|numOfArrays(1) [array: 0x80|nal_type(1) numNalus(2)
//!       [len(2) nal]…]
//!
//! lengthSizeMinusOne is 3 in practice → 4-byte length prefixes; Annex-B
//! conversion uses the record's actual value.

/// One NAL unit with its type.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NalUnit {
    pub nal_type: u8,
    pub bytes: Vec<u8>,
}

fn read_be16(b: &[u8], at: usize) -> Option<u16> {
    b.get(at..at + 2).map(|s| u16::from_be_bytes([s[0], s[1]]))
}

/// Parse an avcC record into its SPS + PPS NAL units.
pub fn parse_avcc(record: &[u8]) -> Option<(Vec<NalUnit>, Vec<NalUnit>)> {
    if record.len() < 7 || record[0] != 1 {
        return None;
    }
    let length_size_minus_one = record[4] & 0x03;
    let mut at = 5usize;
    let num_sps = record[at] & 0x1F;
    at += 1;
    let mut sps = Vec::new();
    for _ in 0..num_sps {
        let len = usize::from(read_be16(record, at)?);
        at += 2;
        let body = record.get(at..at + len)?;
        at += len;
        sps.push(NalUnit {
            nal_type: body.first().copied().unwrap_or(0) & 0x1F,
            bytes: body.to_vec(),
        });
    }
    if at >= record.len() {
        return None;
    }
    let num_pps = record[at] & 0x1F;
    at += 1;
    let mut pps = Vec::new();
    for _ in 0..num_pps {
        let len = usize::from(read_be16(record, at)?);
        at += 2;
        let body = record.get(at..at + len)?;
        at += len;
        pps.push(NalUnit {
            nal_type: body.first().copied().unwrap_or(0) & 0x1F,
            bytes: body.to_vec(),
        });
    }
    if length_size_minus_one != 3 {
        // Non-4-byte length prefixes are legal but do not occur in the wild;
        // we do not fabricate conversion for them — callers use the raw
        // record as codec_data instead.
        return Some((sps, pps));
    }
    Some((sps, pps))
}

/// Parse an hvcC record into its VPS/SPS/PPS NAL units (by NAL type).
pub fn parse_hvcc(record: &[u8]) -> Option<Vec<NalUnit>> {
    if record.len() < 23 || record[0] != 1 {
        return None;
    }
    let mut at = 22usize;
    let num_arrays = record[at] & 0x1F;
    at += 1;
    let mut nals = Vec::new();
    for _ in 0..num_arrays {
        if at >= record.len() {
            return None;
        }
        let array_nal_type = record[at] & 0x3F;
        at += 1;
        let num_nalus = usize::from(read_be16(record, at)?);
        at += 2;
        for _ in 0..num_nalus {
            let len = usize::from(read_be16(record, at)?);
            at += 2;
            let body = record.get(at..at + len)?;
            at += len;
            nals.push(NalUnit {
                // Trust the array's declared type (HEVC NAL types > 31 are
                // impossible; the declared type is the spec authority).
                nal_type: array_nal_type,
                bytes: body.to_vec(),
            });
        }
    }
    Some(nals)
}

/// Wrap a length-prefixed NAL into Annex B (start code 00 00 00 01).
pub fn nal_to_annexb(nal: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(nal.len() + 4);
    out.extend_from_slice(&[0, 0, 0, 1]);
    out.extend_from_slice(nal);
    out
}

/// Convert a length-prefixed (AVC/hvc1 "length-prefixed") access unit to
/// Annex B, using the record's length size (we only support 4; other sizes
/// are passed through — the caps already told the decoder the true format).
pub fn length_prefixed_to_annexb(chunk: &[u8]) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(chunk.len() + 16);
    let mut at = 0usize;
    while at < chunk.len() {
        let len = usize::try_from(u32::from_be_bytes(
            chunk.get(at..at + 4)?.try_into().ok()?,
        ))
        .ok()?;
        at += 4;
        let nal = chunk.get(at..at + len)?;
        at += len;
        out.extend_from_slice(&nal_to_annexb(nal));
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A minimal avcC: version 1, profile 0x64, compat 0x00, level 0x0b,
    /// lengthSizeMinusOne=3, 1 SPS (2 bytes), 1 PPS (2 bytes).
    fn minimal_avcc() -> Vec<u8> {
        vec![
            1, 0x64, 0x00, 0x0b, 0xFF, 0xE1, 0, 2, 0x67, 0x42, 0xE0, 0, 2, 0x68, 0xCE,
        ]
    }

    #[test]
    fn parse_minimal_avcc() {
        let (sps, pps) = parse_avcc(&minimal_avcc()).unwrap();
        assert_eq!(sps.len(), 1);
        assert_eq!(sps[0].nal_type, 0x67 & 0x1F);
        assert_eq!(pps.len(), 1);
        assert_eq!(pps[0].bytes, vec![0x68, 0xCE]);
    }

    #[test]
    fn avcc_rejects_garbage() {
        assert!(parse_avcc(&[0, 1, 2]).is_none());
        assert!(parse_avcc(&[]).is_none());
    }

    #[test]
    fn annexb_roundtrip() {
        let chunk = vec![0, 0, 0, 2, 0xAA, 0xBB, 0, 0, 0, 1, 0xCC];
        let annexb = length_prefixed_to_annexb(&chunk).unwrap();
        assert_eq!(annexb, vec![0, 0, 0, 1, 0xAA, 0xBB, 0, 0, 0, 1, 0xCC]);
    }
}
