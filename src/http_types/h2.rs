//! HTTP/2 wire-format types (RFC 7540 / 9113). Pure-data tier-1 module —
//! zero JSC, socket, or io dependencies. Shared by:
//!   • `bun_http`    (fetch() HTTP/2 client) — re-exported as `h2_frame_parser`
//!   • `bun_runtime` (node:http2 bindings)   — `pub use`d into its
//!     `h2_frame_parser` module, which layers `WireWriter`-based `write()`
//!     and `to_js()` on top as local extension traits.
//!
//! The Zig tree carries TWO copies of these types (`src/http/H2FrameParser.zig`
//! and a private duplicate inside `src/runtime/api/bun/h2_frame_parser.zig`);
//! the http copy's own doc-comment already promised this dedup. This module is
//! that promise kept on the Rust side.
#![allow(non_camel_case_types, non_upper_case_globals)]

// ─── connection / sizing constants ──────────────

pub const CLIENT_PREFACE: &[u8] = b"PRI * HTTP/2.0\r\n\r\nSM\r\n\r\n";

pub const MAX_WINDOW_SIZE: u32 = i32::MAX as u32;
pub const MAX_STREAM_ID: u32 = i32::MAX as u32;
/// `std.math.maxInt(u24)`
pub const MAX_FRAME_SIZE: u32 = 0x00FF_FFFF;
pub const DEFAULT_WINDOW_SIZE: u32 = u16::MAX as u32;
/// PORT NOTE: Zig type was `u24`; Rust has no `u24`, so widened to `u32`.
pub const DEFAULT_MAX_FRAME_SIZE: u32 = 16384;

// ─── frame type / flags ─────────────────────────
//
// PORT NOTE: Zig `enum(u8) { …, _ }` is non-exhaustive (any u8 is a valid
// value). A `#[repr(u8)]` Rust enum is UB for unknown discriminants received
// off the wire, so callers dispatch on the raw `u8` (`FrameHeader.type_`) and
// only ever use this enum for *outbound* frame construction (`X as u8`).

#[repr(u8)]
#[derive(Copy, Clone, PartialEq, Eq)]
pub enum FrameType {
    HTTP_FRAME_DATA = 0x00,
    HTTP_FRAME_HEADERS = 0x01,
    HTTP_FRAME_PRIORITY = 0x02,
    HTTP_FRAME_RST_STREAM = 0x03,
    HTTP_FRAME_SETTINGS = 0x04,
    HTTP_FRAME_PUSH_PROMISE = 0x05,
    HTTP_FRAME_PING = 0x06,
    HTTP_FRAME_GOAWAY = 0x07,
    HTTP_FRAME_WINDOW_UPDATE = 0x08,
    /// RFC 7540 §6.10: continues a header block fragment.
    HTTP_FRAME_CONTINUATION = 0x09,
    /// <https://datatracker.ietf.org/doc/html/rfc7838#section-7.2>
    HTTP_FRAME_ALTSVC = 0x0A,
    /// <https://datatracker.ietf.org/doc/html/rfc8336#section-2>
    HTTP_FRAME_ORIGIN = 0x0C,
}

#[repr(u8)]
#[derive(Copy, Clone, PartialEq, Eq)]
pub enum PingFrameFlags {
    ACK = 0x1,
}

#[repr(u8)]
#[derive(Copy, Clone, PartialEq, Eq)]
pub enum DataFrameFlags {
    END_STREAM = 0x1,
    PADDED = 0x8,
}

#[repr(u8)]
#[derive(Copy, Clone, PartialEq, Eq)]
pub enum HeadersFrameFlags {
    END_STREAM = 0x1,
    END_HEADERS = 0x4,
    PADDED = 0x8,
    PRIORITY = 0x20,
}

#[repr(u8)]
#[derive(Copy, Clone, PartialEq, Eq)]
pub enum SettingsFlags {
    ACK = 0x1,
}

// ─── error / setting codes ──────────────────────
//
// Non-exhaustive in Zig (`_` catch-all). Newtype-over-int instead of
// `#[repr]` enums so any value off the wire is well-defined; consumers match
// on `.0` or the associated consts.

#[repr(transparent)]
#[derive(Copy, Clone, PartialEq, Eq)]
pub struct ErrorCode(pub u32);
impl ErrorCode {
    pub const NO_ERROR: Self = Self(0x0);
    pub const PROTOCOL_ERROR: Self = Self(0x1);
    pub const INTERNAL_ERROR: Self = Self(0x2);
    pub const FLOW_CONTROL_ERROR: Self = Self(0x3);
    pub const SETTINGS_TIMEOUT: Self = Self(0x4);
    pub const STREAM_CLOSED: Self = Self(0x5);
    pub const FRAME_SIZE_ERROR: Self = Self(0x6);
    pub const REFUSED_STREAM: Self = Self(0x7);
    pub const CANCEL: Self = Self(0x8);
    pub const COMPRESSION_ERROR: Self = Self(0x9);
    pub const CONNECT_ERROR: Self = Self(0xa);
    pub const ENHANCE_YOUR_CALM: Self = Self(0xb);
    pub const INADEQUATE_SECURITY: Self = Self(0xc);
    pub const HTTP_1_1_REQUIRED: Self = Self(0xd);
    pub const MAX_PENDING_SETTINGS_ACK: Self = Self(0xe);
}

#[repr(transparent)]
#[derive(Copy, Clone, PartialEq, Eq)]
pub struct SettingsType(pub u16);
impl SettingsType {
    pub const SETTINGS_HEADER_TABLE_SIZE: Self = Self(0x1);
    pub const SETTINGS_ENABLE_PUSH: Self = Self(0x2);
    pub const SETTINGS_MAX_CONCURRENT_STREAMS: Self = Self(0x3);
    pub const SETTINGS_INITIAL_WINDOW_SIZE: Self = Self(0x4);
    pub const SETTINGS_MAX_FRAME_SIZE: Self = Self(0x5);
    pub const SETTINGS_MAX_HEADER_LIST_SIZE: Self = Self(0x6);
    // Non-standard extension settings (still unsupported):
    pub const SETTINGS_ENABLE_CONNECT_PROTOCOL: Self = Self(0x8);
    pub const SETTINGS_NO_RFC7540_PRIORITIES: Self = Self(0x9);
}

// ─── wire helpers ───────────────────────────────

#[inline]
pub fn u32_from_bytes(src: &[u8]) -> u32 {
    debug_assert!(src.len() == 4);
    u32::from_be_bytes([src[0], src[1], src[2], src[3]])
}

/// Zig: `packed struct(u32) { reserved: bool = false, uint31: u31 = 0 }`.
///
/// PORT NOTE (intentional divergence): Zig's `toUInt32()` is `@bitCast` of
/// `packed struct(u32){ reserved: bool, uint31: u31 }`, which on little-endian
/// places `reserved` in bit 0 and yields `(uint31 << 1) | reserved`. That is a
/// latent RFC 7540 §6.3 bug in Zig's deprecated PRIORITY path — the wire
/// format wants the reserved/E bit at bit 31. We keep the RFC-compliant
/// `(reserved << 31) | uint31` layout here, which already matches
/// `from_bytes`/`encode_into` and the on-wire `StreamPriority.stream_identifier`.
#[repr(transparent)]
#[derive(Copy, Clone, Default, PartialEq, Eq)]
pub struct UInt31WithReserved(u32);

impl UInt31WithReserved {
    #[inline]
    pub const fn reserved(self) -> bool {
        self.0 & 0x8000_0000 != 0
    }
    #[inline]
    pub const fn uint31(self) -> u32 {
        self.0 & 0x7fff_ffff
    }
    #[inline]
    pub const fn from(value: u32) -> Self {
        Self(value)
    }
    #[inline]
    pub const fn init(value: u32, reserved: bool) -> Self {
        Self((value & 0x7fff_ffff) | if reserved { 0x8000_0000 } else { 0 })
    }
    #[inline]
    pub const fn to_uint32(self) -> u32 {
        self.0
    }
    #[inline]
    pub fn from_bytes(src: &[u8]) -> Self {
        Self(u32_from_bytes(src))
    }
    #[inline]
    pub fn encode_into(self, dst: &mut [u8; 4]) {
        *dst = self.0.to_be_bytes();
    }
}

// ─── packed wire structs ────────────────────────
//
// `StreamPriority`, `SettingsPayloadUnit` and `FullSettingsPayload` are
// `#[repr(C, packed)]` with integer-only fields and therefore have no padding
// bytes and no niches. They implement `bytemuck::Pod`, so the per-`from()`
// byte-view that the Zig parser did via `@ptrCast` is the safe
// `bytemuck::bytes_of_mut`.

/// Zig: `packed struct(u40) { streamIdentifier: u32 = 0, weight: u8 = 0 }`.
#[repr(C, packed)]
#[derive(Copy, Clone, Default)]
pub struct StreamPriority {
    pub stream_identifier: u32,
    pub weight: u8,
}
// SAFETY: `#[repr(C, packed)]` with `u32 + u8` fields — no padding, no niches,
// every 5-byte pattern is a valid value.
unsafe impl bytemuck::Zeroable for StreamPriority {}
// SAFETY: see `Zeroable` impl above; additionally `Copy + 'static`.
unsafe impl bytemuck::Pod for StreamPriority {}
const _: () = assert!(core::mem::size_of::<StreamPriority>() == StreamPriority::BYTE_SIZE);

impl StreamPriority {
    pub const BYTE_SIZE: usize = 5;

    #[inline]
    pub fn from(dst: &mut StreamPriority, src: &[u8]) {
        bytemuck::bytes_of_mut(dst).copy_from_slice(src);
        // std.mem.byteSwapAllFields(StreamPriority, dst) — `weight: u8` is a no-op.
        // PORT NOTE: brace-expr `{packed.field}` performs an unaligned copy;
        // assignment to a packed field is an unaligned store. No `unsafe`.
        dst.stream_identifier = u32::swap_bytes(dst.stream_identifier);
    }

    #[inline]
    pub fn encode_into(self, dst: &mut [u8; Self::BYTE_SIZE]) {
        let mut swap = self;
        swap.stream_identifier = u32::swap_bytes(swap.stream_identifier);
        dst.copy_from_slice(bytemuck::bytes_of(&swap));
    }
}

/// Zig: `packed struct(u72) { length: u24, type: u8, flags: u8, streamIdentifier: u32 }`.
///
/// NOT `#[repr(packed)]` — the `u24` length is widened to a native `u32`
/// in-memory; wire encoding is handled in `decode()`/`encode_into()` instead
/// of by punning the struct bytes. Callers assemble the 9 raw wire bytes on
/// the stack and hand them to `decode()`.
#[derive(Copy, Clone)]
pub struct FrameHeader {
    /// `u24` on the wire.
    pub length: u32,
    pub type_: u8,
    pub flags: u8,
    pub stream_identifier: u32,
}
impl Default for FrameHeader {
    fn default() -> Self {
        Self {
            length: 0,
            type_: FrameType::HTTP_FRAME_SETTINGS as u8,
            flags: 0,
            stream_identifier: 0,
        }
    }
}
impl FrameHeader {
    pub const BYTE_SIZE: usize = 9;

    /// Decode a complete 9-byte big-endian frame header.
    #[inline]
    pub fn decode(raw: &[u8; Self::BYTE_SIZE]) -> Self {
        Self {
            length: ((raw[0] as u32) << 16) | ((raw[1] as u32) << 8) | (raw[2] as u32),
            type_: raw[3],
            flags: raw[4],
            stream_identifier: u32::from_be_bytes([raw[5], raw[6], raw[7], raw[8]]),
        }
    }

    #[inline]
    pub fn encode_into(&self, dst: &mut [u8; Self::BYTE_SIZE]) {
        // std.mem.byteSwapAllFields on `packed struct(u72)` — emit BE manually.
        dst[0] = ((self.length >> 16) & 0xFF) as u8;
        dst[1] = ((self.length >> 8) & 0xFF) as u8;
        dst[2] = (self.length & 0xFF) as u8;
        dst[3] = self.type_;
        dst[4] = self.flags;
        dst[5..9].copy_from_slice(&self.stream_identifier.to_be_bytes());
    }
}

/// Zig: `packed struct(u48) { type: u16, value: u32 }`.
#[repr(C, packed)]
#[derive(Copy, Clone, Default)]
pub struct SettingsPayloadUnit {
    pub type_: u16,
    pub value: u32,
}
// SAFETY: `#[repr(C, packed)]` with `u16 + u32` fields — no padding, no
// niches, every 6-byte pattern is a valid value.
unsafe impl bytemuck::Zeroable for SettingsPayloadUnit {}
// SAFETY: see `Zeroable` impl above; additionally `Copy + 'static`.
unsafe impl bytemuck::Pod for SettingsPayloadUnit {}
const _: () =
    assert!(core::mem::size_of::<SettingsPayloadUnit>() == SettingsPayloadUnit::BYTE_SIZE);

impl SettingsPayloadUnit {
    pub const BYTE_SIZE: usize = 6;

    #[inline]
    pub fn from<const END: bool>(dst: &mut SettingsPayloadUnit, src: &[u8], offset: usize) {
        let bytes = bytemuck::bytes_of_mut(dst);
        bytes[offset..src.len() + offset].copy_from_slice(src);
        if END {
            // std.mem.byteSwapAllFields(SettingsPayloadUnit, dst)
            dst.type_ = u16::swap_bytes(dst.type_);
            dst.value = u32::swap_bytes(dst.value);
        }
    }

    #[inline]
    pub fn encode(dst: &mut [u8; Self::BYTE_SIZE], setting: SettingsType, value: u32) {
        dst[0..2].copy_from_slice(&setting.0.to_be_bytes());
        dst[2..6].copy_from_slice(&value.to_be_bytes());
    }
}

/// Zig: `packed struct(u336)` — 7 × (`u16` type + `u32` value) = 42 bytes.
#[repr(C, packed)]
#[derive(Copy, Clone)]
pub(crate) struct FullSettingsPayload {
    _header_table_size_type: u16,
    pub header_table_size: u32,
    _enable_push_type: u16,
    pub enable_push: u32,
    _max_concurrent_streams_type: u16,
    pub max_concurrent_streams: u32,
    _initial_window_size_type: u16,
    pub initial_window_size: u32,
    _max_frame_size_type: u16,
    pub max_frame_size: u32,
    _max_header_list_size_type: u16,
    pub max_header_list_size: u32,
    _enable_connect_protocol_type: u16,
    pub enable_connect_protocol: u32,
}
// SAFETY: `#[repr(C, packed)]` with only `u16`/`u32` fields — no padding, no
// niches, every 42-byte pattern is a valid value.
unsafe impl bytemuck::Zeroable for FullSettingsPayload {}
// SAFETY: see `Zeroable` impl above; additionally `Copy + 'static`.
unsafe impl bytemuck::Pod for FullSettingsPayload {}
const _: () =
    assert!(core::mem::size_of::<FullSettingsPayload>() == FullSettingsPayload::BYTE_SIZE);

impl Default for FullSettingsPayload {
    fn default() -> Self {
        Self {
            _header_table_size_type: SettingsType::SETTINGS_HEADER_TABLE_SIZE.0,
            header_table_size: 4096,
            _enable_push_type: SettingsType::SETTINGS_ENABLE_PUSH.0,
            enable_push: 1,
            _max_concurrent_streams_type: SettingsType::SETTINGS_MAX_CONCURRENT_STREAMS.0,
            max_concurrent_streams: u32::MAX,
            _initial_window_size_type: SettingsType::SETTINGS_INITIAL_WINDOW_SIZE.0,
            initial_window_size: 65535,
            _max_frame_size_type: SettingsType::SETTINGS_MAX_FRAME_SIZE.0,
            max_frame_size: 16384,
            _max_header_list_size_type: SettingsType::SETTINGS_MAX_HEADER_LIST_SIZE.0,
            max_header_list_size: 65535,
            _enable_connect_protocol_type: SettingsType::SETTINGS_ENABLE_CONNECT_PROTOCOL.0,
            enable_connect_protocol: 0,
        }
    }
}
impl FullSettingsPayload {
    pub(crate) const BYTE_SIZE: usize = 42;
}

// ported from: src/http/H2FrameParser.zig + src/runtime/api/bun/h2_frame_parser.zig (wire types)

#[cfg(test)]
mod tests {
    use super::*;

    // ─── CLIENT_PREFACE ────────────────────────────
    #[test]
    fn client_preface_is_24_bytes() {
        assert_eq!(CLIENT_PREFACE.len(), 24);
    }

    #[test]
    fn client_preface_starts_with_pri_method() {
        assert!(CLIENT_PREFACE.starts_with(b"PRI * HTTP/2.0\r\n\r\nSM\r\n\r\n"));
    }

    // ─── constants ─────────────────────────────────
    #[test]
    fn max_window_size_is_i31_max() {
        assert_eq!(MAX_WINDOW_SIZE, 0x7FFF_FFFF);
    }

    #[test]
    fn max_stream_id_is_i31_max() {
        assert_eq!(MAX_STREAM_ID, 0x7FFF_FFFF);
    }

    #[test]
    fn default_window_size_is_65535() {
        assert_eq!(DEFAULT_WINDOW_SIZE, 65535);
    }

    #[test]
    fn default_max_frame_size_is_16384() {
        assert_eq!(DEFAULT_MAX_FRAME_SIZE, 16384);
    }

    #[test]
    fn max_frame_size_is_u24_max() {
        assert_eq!(MAX_FRAME_SIZE, 0x00FF_FFFF);
    }

    // ─── FrameType ────────────────────────────────
    #[test]
    fn frame_type_discriminants_rfc7540() {
        assert_eq!(FrameType::HTTP_FRAME_DATA as u8, 0x00);
        assert_eq!(FrameType::HTTP_FRAME_HEADERS as u8, 0x01);
        assert_eq!(FrameType::HTTP_FRAME_PRIORITY as u8, 0x02);
        assert_eq!(FrameType::HTTP_FRAME_RST_STREAM as u8, 0x03);
        assert_eq!(FrameType::HTTP_FRAME_SETTINGS as u8, 0x04);
        assert_eq!(FrameType::HTTP_FRAME_PUSH_PROMISE as u8, 0x05);
        assert_eq!(FrameType::HTTP_FRAME_PING as u8, 0x06);
        assert_eq!(FrameType::HTTP_FRAME_GOAWAY as u8, 0x07);
        assert_eq!(FrameType::HTTP_FRAME_WINDOW_UPDATE as u8, 0x08);
        assert_eq!(FrameType::HTTP_FRAME_CONTINUATION as u8, 0x09);
    }

    // ─── ErrorCode ─────────────────────────────────
    #[test]
    fn error_code_rfc7540_values() {
        assert_eq!(ErrorCode::NO_ERROR.0, 0x0);
        assert_eq!(ErrorCode::PROTOCOL_ERROR.0, 0x1);
        assert_eq!(ErrorCode::INTERNAL_ERROR.0, 0x2);
        assert_eq!(ErrorCode::FLOW_CONTROL_ERROR.0, 0x3);
        assert_eq!(ErrorCode::SETTINGS_TIMEOUT.0, 0x4);
        assert_eq!(ErrorCode::STREAM_CLOSED.0, 0x5);
        assert_eq!(ErrorCode::FRAME_SIZE_ERROR.0, 0x6);
        assert_eq!(ErrorCode::REFUSED_STREAM.0, 0x7);
        assert_eq!(ErrorCode::CANCEL.0, 0x8);
        assert_eq!(ErrorCode::COMPRESSION_ERROR.0, 0x9);
        assert_eq!(ErrorCode::CONNECT_ERROR.0, 0xa);
        assert_eq!(ErrorCode::ENHANCE_YOUR_CALM.0, 0xb);
        assert_eq!(ErrorCode::INADEQUATE_SECURITY.0, 0xc);
        assert_eq!(ErrorCode::HTTP_1_1_REQUIRED.0, 0xd);
    }

    #[test]
    fn error_code_is_newtype_not_enum() {
        // Any u32 value off the wire is valid — non-exhaustive
        let custom = ErrorCode(0xFF);
        assert_eq!(custom.0, 0xFF);
    }

    // ─── SettingsType ──────────────────────────────
    #[test]
    fn settings_type_rfc7540_ids() {
        assert_eq!(SettingsType::SETTINGS_HEADER_TABLE_SIZE.0, 0x1);
        assert_eq!(SettingsType::SETTINGS_ENABLE_PUSH.0, 0x2);
        assert_eq!(SettingsType::SETTINGS_MAX_CONCURRENT_STREAMS.0, 0x3);
        assert_eq!(SettingsType::SETTINGS_INITIAL_WINDOW_SIZE.0, 0x4);
        assert_eq!(SettingsType::SETTINGS_MAX_FRAME_SIZE.0, 0x5);
        assert_eq!(SettingsType::SETTINGS_MAX_HEADER_LIST_SIZE.0, 0x6);
    }

    #[test]
    fn settings_type_extension_ids() {
        assert_eq!(SettingsType::SETTINGS_ENABLE_CONNECT_PROTOCOL.0, 0x8);
        assert_eq!(SettingsType::SETTINGS_NO_RFC7540_PRIORITIES.0, 0x9);
    }

    #[test]
    fn settings_type_is_newtype() {
        let unknown = SettingsType(0xDEAD);
        assert_eq!(unknown.0, 0xDEAD);
    }

    // ─── FrameHeader decode/encode roundtrip ───────
    #[test]
    fn frame_header_decode_zero_length() {
        let raw: [u8; 9] = [0, 0, 0, 0x04, 0, 0, 0, 0, 0];
        let hdr = FrameHeader::decode(&raw);
        assert_eq!(hdr.length, 0);
        assert_eq!(hdr.type_, 0x04);
        assert_eq!(hdr.flags, 0);
        assert_eq!(hdr.stream_identifier, 0);
    }

    #[test]
    fn frame_header_decode_max_u24_length() {
        // u24 max = 0xFFFFFF
        let raw: [u8; 9] = [0xFF, 0xFF, 0xFF, 0x01, 0x05, 0x00, 0x00, 0x00, 0x01];
        let hdr = FrameHeader::decode(&raw);
        assert_eq!(hdr.length, 0x00FF_FFFF);
        assert_eq!(hdr.type_, 0x01);
        assert_eq!(hdr.flags, 0x05);
        assert_eq!(hdr.stream_identifier, 1);
    }

    #[test]
    fn frame_header_decode_ignores_reserved_bit() {
        // Stream ID with reserved bit set (bit 31) → must be ignored per RFC 7540 §4.1
        let raw: [u8; 9] = [0, 0, 0, 0x04, 0, 0x80, 0x00, 0x00, 0x01];
        let hdr = FrameHeader::decode(&raw);
        // u32::from_be_bytes includes the reserved bit — caller must mask
        assert_eq!(hdr.stream_identifier & 0x7FFF_FFFF, 1);
        assert!(hdr.stream_identifier & 0x8000_0000 != 0); // reserved bit present
    }

    #[test]
    fn frame_header_encode_roundtrip() {
        let original = FrameHeader {
            length: 16384,
            type_: FrameType::HTTP_FRAME_HEADERS as u8,
            flags: 0x05,
            stream_identifier: 42,
        };
        let mut buf = [0u8; 9];
        original.encode_into(&mut buf);
        let decoded = FrameHeader::decode(&buf);
        assert_eq!(decoded.length, original.length);
        assert_eq!(decoded.type_, original.type_);
        assert_eq!(decoded.flags, original.flags);
        assert_eq!(decoded.stream_identifier, original.stream_identifier);
    }

    #[test]
    fn frame_header_encode_zero_fields() {
        let hdr = FrameHeader::default();
        let mut buf = [0u8; 9];
        hdr.encode_into(&mut buf);
        // Default: type=SETTINGS(0x04), length=0, flags=0, stream_id=0
        assert_eq!(buf, [0, 0, 0, 0x04, 0, 0, 0, 0, 0]);
    }

    #[test]
    fn frame_header_encode_large_length() {
        let hdr = FrameHeader {
            length: 0x00FF_FFFF,
            type_: FrameType::HTTP_FRAME_DATA as u8,
            flags: 0x01,
            stream_identifier: 0,
        };
        let mut buf = [0u8; 9];
        hdr.encode_into(&mut buf);
        assert_eq!(buf[0..3], [0xFF, 0xFF, 0xFF]); // u24 max
        assert_eq!(buf[3], 0x00); // DATA
        assert_eq!(buf[4], 0x01); // END_STREAM
    }

    // ─── UInt31WithReserved ────────────────────────
    #[test]
    fn uint31_reserved_bit_extraction() {
        let val = UInt31WithReserved::init(42, true);
        assert!(val.reserved());
        assert_eq!(val.uint31(), 42);
    }

    #[test]
    fn uint31_no_reserved() {
        let val = UInt31WithReserved::init(100, false);
        assert!(!val.reserved());
        assert_eq!(val.uint31(), 100);
    }

    #[test]
    fn uint31_reserved_masks_value() {
        // Value > u31 max gets masked
        let val = UInt31WithReserved::init(0xFFFF_FFFF, false);
        assert_eq!(val.uint31(), 0x7FFF_FFFF);
        assert!(!val.reserved());
    }

    #[test]
    fn uint31_from_bytes_roundtrip() {
        let original = UInt31WithReserved::init(12345, true);
        let mut dst = [0u8; 4];
        original.encode_into(&mut dst);
        let decoded = UInt31WithReserved::from_bytes(&dst);
        assert_eq!(decoded.uint31(), 12345);
        assert!(decoded.reserved());
    }

    #[test]
    fn uint31_to_uint32_preserves_layout() {
        let val = UInt31WithReserved::init(1, true);
        assert_eq!(val.to_uint32(), 0x8000_0001);
    }

    // ─── SettingsPayloadUnit encode ────────────────
    #[test]
    fn settings_payload_unit_encode_header_table_size() {
        let mut buf = [0u8; 6];
        SettingsPayloadUnit::encode(&mut buf, SettingsType::SETTINGS_HEADER_TABLE_SIZE, 4096);
        assert_eq!(buf[0..2], [0x00, 0x01]); // ID = 1
        assert_eq!(u32::from_be_bytes([buf[2], buf[3], buf[4], buf[5]]), 4096);
    }

    #[test]
    fn settings_payload_unit_encode_initial_window_size() {
        let mut buf = [0u8; 6];
        SettingsPayloadUnit::encode(&mut buf, SettingsType::SETTINGS_INITIAL_WINDOW_SIZE, 65535);
        assert_eq!(buf[0..2], [0x00, 0x04]); // ID = 4
        assert_eq!(u32::from_be_bytes([buf[2], buf[3], buf[4], buf[5]]), 65535);
    }

    #[test]
    fn settings_payload_unit_encode_max_value() {
        let mut buf = [0u8; 6];
        SettingsPayloadUnit::encode(&mut buf, SettingsType::SETTINGS_INITIAL_WINDOW_SIZE, 0x7FFF_FFFF);
        assert_eq!(buf[2..6], [0x7F, 0xFF, 0xFF, 0xFF]);
    }

    #[test]
    fn settings_payload_unit_encode_zero_value() {
        // Critical: NUL bytes must be preserved (not truncated)
        let mut buf = [0u8; 6];
        SettingsPayloadUnit::encode(&mut buf, SettingsType::SETTINGS_ENABLE_PUSH, 0);
        assert_eq!(buf[0..2], [0x00, 0x02]);
        assert_eq!(buf[2..6], [0x00, 0x00, 0x00, 0x00]);
    }

    // ─── StreamPriority ────────────────────────────
    #[test]
    fn stream_priority_byte_size_is_5() {
        assert_eq!(core::mem::size_of::<StreamPriority>(), 5);
    }

    #[test]
    fn stream_priority_from_and_encode_roundtrip() {
        let mut sp = StreamPriority::default();
        let src: [u8; 5] = [0x00, 0x00, 0x00, 0x05, 16]; // stream_id=5, weight=16
        StreamPriority::from(&mut sp, &src);
        // Packed struct — read via byte view to avoid unaligned references (E0793)
        let bytes = bytemuck::bytes_of(&sp);
        // from() does swap_bytes on stream_identifier (BE→native), weight stays raw
        let stream_id = u32::from_ne_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
        assert_eq!(stream_id, 5);
        assert_eq!(bytes[4], 16);

        let mut encoded = [0u8; 5];
        sp.encode_into(&mut encoded);
        assert_eq!(encoded, src);
    }

    #[test]
    fn stream_priority_from_byte_swap() {
        let mut sp = StreamPriority::default();
        // Wire: big-endian stream_identifier + raw weight
        let src: [u8; 5] = [0x80, 0x00, 0x00, 0x01, 255];
        StreamPriority::from(&mut sp, &src);
        let bytes = bytemuck::bytes_of(&sp);
        // from() copies raw bytes then swap_bytes stream_identifier (BE→native)
        // Wire BE [0x80,0x00,0x00,0x01] → native value 0x80000001 after swap
        let stream_id = u32::from_ne_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
        assert_eq!(stream_id, 0x80000001);
        assert_eq!(bytes[4], 255);
    }

    // ─── FullSettingsPayload ───────────────────────
    #[test]
    fn full_settings_payload_byte_size_is_42() {
        assert_eq!(core::mem::size_of::<FullSettingsPayload>(), 42);
    }

    #[test]
    fn full_settings_payload_default_values() {
        let d = FullSettingsPayload::default();
        // Packed struct — use byte view to avoid unaligned references (E0793)
        let bytes = bytemuck::bytes_of(&d);
        // Each setting: 2-byte type (BE u16) + 4-byte value (native endian since Default assigns directly)
        // On LE: value bytes are LE, on BE: value bytes are BE
        // header_table_size (offset 2, 4 bytes)
        let hts = u32::from_ne_bytes([bytes[2], bytes[3], bytes[4], bytes[5]]);
        assert_eq!(hts, 4096);
        // enable_push (offset 8, 4 bytes)
        let ep = u32::from_ne_bytes([bytes[8], bytes[9], bytes[10], bytes[11]]);
        assert_eq!(ep, 1);
        // initial_window_size (offset 20, 4 bytes)
        let iws = u32::from_ne_bytes([bytes[20], bytes[21], bytes[22], bytes[23]]);
        assert_eq!(iws, 65535);
        // max_frame_size (offset 26, 4 bytes)
        let mfs = u32::from_ne_bytes([bytes[26], bytes[27], bytes[28], bytes[29]]);
        assert_eq!(mfs, 16384);
        // max_header_list_size (offset 32, 4 bytes)
        let mhls = u32::from_ne_bytes([bytes[32], bytes[33], bytes[34], bytes[35]]);
        assert_eq!(mhls, 65535);
        // enable_connect_protocol (offset 38, 4 bytes)
        let ecp = u32::from_ne_bytes([bytes[38], bytes[39], bytes[40], bytes[41]]);
        assert_eq!(ecp, 0);
    }

    #[test]
    fn full_settings_payload_type_fields_match_settings_type() {
        let d = FullSettingsPayload::default();
        let bytes = bytemuck::bytes_of(&d);
        // Each setting starts with 2-byte type field (u16, native endian in packed struct)
        let hts_type = u16::from_ne_bytes([bytes[0], bytes[1]]);
        let ep_type = u16::from_ne_bytes([bytes[6], bytes[7]]);
        let mcs_type = u16::from_ne_bytes([bytes[12], bytes[13]]);
        let iws_type = u16::from_ne_bytes([bytes[18], bytes[19]]);
        let mfs_type = u16::from_ne_bytes([bytes[24], bytes[25]]);
        let mhls_type = u16::from_ne_bytes([bytes[30], bytes[31]]);
        let ecp_type = u16::from_ne_bytes([bytes[36], bytes[37]]);
        assert_eq!(hts_type, SettingsType::SETTINGS_HEADER_TABLE_SIZE.0);
        assert_eq!(ep_type, SettingsType::SETTINGS_ENABLE_PUSH.0);
        assert_eq!(mcs_type, SettingsType::SETTINGS_MAX_CONCURRENT_STREAMS.0);
        assert_eq!(iws_type, SettingsType::SETTINGS_INITIAL_WINDOW_SIZE.0);
        assert_eq!(mfs_type, SettingsType::SETTINGS_MAX_FRAME_SIZE.0);
        assert_eq!(mhls_type, SettingsType::SETTINGS_MAX_HEADER_LIST_SIZE.0);
        assert_eq!(ecp_type, SettingsType::SETTINGS_ENABLE_CONNECT_PROTOCOL.0);
    }

    // ─── u32_from_bytes ────────────────────────────
    #[test]
    fn u32_from_bytes_be() {
        assert_eq!(u32_from_bytes(&[0x00, 0x01, 0x02, 0x03]), 0x00010203);
    }

    #[test]
    fn u32_from_bytes_zero() {
        assert_eq!(u32_from_bytes(&[0, 0, 0, 0]), 0);
    }

    #[test]
    fn u32_from_bytes_max() {
        assert_eq!(u32_from_bytes(&[0xFF, 0xFF, 0xFF, 0xFF]), u32::MAX);
    }
}
