//! Unit coverage for `bun_http::websocket` — the ported zhp WS frame-header
//! codec (`Opcode` + `WebsocketHeader`).
//!
//! Distinct from the runtime WS face: `bao_runtime`'s `WsConn` (covered by
//! web_api_ws_edge_tests) speaks `bun_uws::ws_codec`; this module is the
//! self-contained LSB-first packed-struct port with its own bit layout, and
//! its public API is exactly what these tests drive. The layout contract is
//! the RFC 6455 first two header bytes: `FIN|RSV1|RSV2|RSV3|opcode|MASK|len7`
//! on the wire (big-endian u16), with the Zig packed-struct fields mapped
//! LSB-first (`len` bit 0, `final` bit 15).

use bun_http::websocket::{Opcode, WebsocketHeader};

/// All 16 opcodes: `is_control` is `opcode & 0x8` — the control plane is
/// exactly 0x8..=0xF (Close/Ping/Pong + reserved), the data plane 0x0..=0x7.
#[test]
fn opcode_is_control_splits_at_bit_three() {
    let expect = [
        (Opcode::Continue, false),
        (Opcode::Text, false),
        (Opcode::Binary, false),
        (Opcode::Res3, false),
        (Opcode::Res4, false),
        (Opcode::Res5, false),
        (Opcode::Res6, false),
        (Opcode::Res7, false),
        (Opcode::Close, true),
        (Opcode::Ping, true),
        (Opcode::Pong, true),
        (Opcode::ResB, true),
        (Opcode::ResC, true),
        (Opcode::ResD, true),
        (Opcode::ResE, true),
        (Opcode::ResF, true),
    ];
    for (op, control) in expect {
        assert_eq!(op.is_control(), control, "{op:?} is_control wrong");
    }
}

/// `from_raw` round-trips every in-contract 4-bit value (caller guarantees
/// `n <= 0xF` — the `& 0xF` inside only keeps the match total, it is not a
/// license for out-of-contract inputs, which debug-assert).
#[test]
fn opcode_from_raw_roundtrips_every_nibble() {
    let all = [
        Opcode::Continue,
        Opcode::Text,
        Opcode::Binary,
        Opcode::Res3,
        Opcode::Res4,
        Opcode::Res5,
        Opcode::Res6,
        Opcode::Res7,
        Opcode::Close,
        Opcode::Ping,
        Opcode::Pong,
        Opcode::ResB,
        Opcode::ResC,
        Opcode::ResD,
        Opcode::ResE,
        Opcode::ResF,
    ];
    for (n, op) in all.iter().enumerate() {
        assert_eq!(Opcode::from_raw(n as u8), *op, "n={n}");
    }
}

/// `new` carries the Zig packed-struct defaults: final=true, rsv=0,
/// compressed=false, mask=false — and the wire bytes of a 5-byte unmasked
/// Text frame are `0x81 0x05` (RFC 6455 §5.2 example shape).
#[test]
fn header_new_defaults_match_rfc_wire_form() {
    let h = WebsocketHeader::new(5, false, Opcode::Text);
    assert!(h.final_());
    assert!(!h.compressed());
    assert_eq!(h.rsv(), 0);
    assert!(!h.mask());
    assert_eq!(h.opcode(), Opcode::Text);
    assert_eq!(h.len(), 5);
    assert_eq!(h.slice(), [0x81, 0x05], "wire bytes for final text len=5");
}

/// Every setter updates only its own field — the other six must be
/// preserved bit-for-bit (the shift/mask pairs are independent, but a
/// wrong shift would bleed here).
#[test]
fn setters_update_only_their_own_field() {
    let mut h = WebsocketHeader::new(7, false, Opcode::Binary);
    // Baseline: final=1, everything else at defaults.
    assert!(h.final_());

    h.set_len(125);
    assert_eq!(h.len(), 125);
    assert!(h.final_(), "set_len clobbered final");
    assert_eq!(h.opcode(), Opcode::Binary);

    h.set_opcode(Opcode::Ping);
    assert_eq!(h.opcode(), Opcode::Ping);
    assert_eq!(h.len(), 125, "set_opcode clobbered len");
    assert!(h.final_(), "set_opcode clobbered final");

    h.set_mask(true);
    assert!(h.mask());
    assert_eq!(h.len(), 125, "set_mask clobbered len");

    h.set_rsv(0b11);
    assert_eq!(h.rsv(), 0b11);
    assert!(h.final_(), "set_rsv clobbered final");

    h.set_compressed(true);
    assert!(h.compressed());
    assert_eq!(h.rsv(), 0b11, "set_compressed clobbered rsv");

    h.set_final(false);
    assert!(!h.final_());
    assert!(h.compressed(), "set_final clobbered compressed");
    assert_eq!(h.rsv(), 0b11, "set_final clobbered rsv");
    assert!(h.mask(), "set_final clobbered mask");
}

/// `len` is a u7 field: u8 inputs above 127 store their low 7 bits (RFC 6455
/// — the extended length follows in-band, tracked by `length_byte_count`).
#[test]
fn len_field_truncates_to_seven_bits() {
    let mut h = WebsocketHeader::new(0, false, Opcode::Text);
    h.set_len(126);
    assert_eq!(h.len(), 126, "126 fits the u7 field");
    h.set_len(200);
    assert_eq!(h.len(), 72, "200 truncates to 200 & 0x7F");
    h.set_len(255);
    assert_eq!(h.len(), 127);
}

/// `pack_length` boundaries: 0..=125 inline, 126..=0xFFFF → 126 (u16
/// extended), anything larger → 127 (u64 extended).
#[test]
fn pack_length_boundaries() {
    assert_eq!(WebsocketHeader::pack_length(0), 0);
    assert_eq!(WebsocketHeader::pack_length(1), 1);
    assert_eq!(WebsocketHeader::pack_length(125), 125);
    assert_eq!(WebsocketHeader::pack_length(126), 126);
    assert_eq!(WebsocketHeader::pack_length(0xFFFF), 126);
    assert_eq!(WebsocketHeader::pack_length(0x1_0000), 127);
    assert_eq!(WebsocketHeader::pack_length(usize::MAX), 127);
}

/// `length_byte_count` mirrors `pack_length`: 0 extra ≤125, 2 bytes ≤0xFFFF,
/// 8 bytes beyond.
#[test]
fn length_byte_count_boundaries() {
    assert_eq!(WebsocketHeader::length_byte_count(0), 0);
    assert_eq!(WebsocketHeader::length_byte_count(125), 0);
    assert_eq!(WebsocketHeader::pack_length(126), 126);
    assert_eq!(WebsocketHeader::length_byte_count(126), 2);
    assert_eq!(WebsocketHeader::length_byte_count(0xFFFF), 2);
    assert_eq!(WebsocketHeader::length_byte_count(0x1_0000), 8);
    assert_eq!(WebsocketHeader::length_byte_count(usize::MAX), 8);
}

/// `frame_size` = 2 header bytes + payload + length extension; the masked
/// variant adds the 4-byte masking key.
#[test]
fn frame_size_accounts_for_header_length_and_mask() {
    assert_eq!(WebsocketHeader::frame_size(0), 2);
    assert_eq!(WebsocketHeader::frame_size(125), 127);
    assert_eq!(WebsocketHeader::frame_size(126), 2 + 2 + 126);
    assert_eq!(WebsocketHeader::frame_size(0xFFFF), 2 + 2 + 0xFFFF);
    assert_eq!(WebsocketHeader::frame_size(0x1_0000), 2 + 8 + 0x1_0000);

    assert_eq!(WebsocketHeader::frame_size_including_mask(0), 2 + 4);
    assert_eq!(WebsocketHeader::frame_size_including_mask(125), 127 + 4);
    assert_eq!(WebsocketHeader::frame_size_including_mask(0x1_0000), 2 + 8 + 0x1_0000 + 4);
}

/// `write_header` emits the big-endian u16 and debug-asserts the packed
/// length agrees with `pack_length(n)` — a correct call writes 2 bytes
/// matching `slice()`.
#[test]
fn write_header_emits_big_endian_pair() {
    for (payload_len, opcode) in [
        (0u64, Opcode::Text),
        (5, Opcode::Text),
        (125, Opcode::Binary),
        (300, Opcode::Binary),
        (70_000, Opcode::Binary),
    ] {
        let mut buf = Vec::new();
        let h = WebsocketHeader::new(WebsocketHeader::pack_length(payload_len as usize), false, opcode);
        h.write_header(&mut buf, payload_len as usize)
            .expect("write_header must not fail on a Vec");
        assert_eq!(buf, h.slice().to_vec(), "payload_len={payload_len}");
        assert_eq!(buf.len(), 2);
        let wire = u16::from_be_bytes([buf[0], buf[1]]);
        assert_eq!(wire, h.bits(), "wire u16 must equal the packed bits");
    }
}

/// `slice`/`from_slice` round-trip through the wire representation, and the
/// parsed header's accessors read back what was packed.
#[test]
fn slice_from_slice_roundtrip() {
    let mut h = WebsocketHeader::new(100, true, Opcode::Close);
    h.set_final(false);
    h.set_rsv(2);
    let bytes = h.slice();
    assert_eq!(bytes.len(), 2);
    let parsed = WebsocketHeader::from_slice(bytes);
    assert_eq!(parsed.bits(), h.bits());
    assert_eq!(parsed.opcode(), Opcode::Close);
    assert!(parsed.mask());
    assert!(!parsed.final_());
    assert_eq!(parsed.rsv(), 2);
    assert_eq!(parsed.len(), 100);
}

/// The header is a plain u16 value type: Copy/Eq semantics hold (no partial
/// moves, cloned headers compare equal to their source).
#[test]
fn header_is_copy_and_eq() {
    let h = WebsocketHeader::new(42, true, Opcode::Pong);
    let copy = h;
    // WebsocketHeader implements Eq but not Debug, so assert on the raw
    // representation (Eq semantics are the u16 identity).
    assert_eq!(h.bits(), copy.bits());
    // bits() exposes the raw representation — spot-check the field positions:
    // final(bit15) | Pong=0xA (bits 8..11) | mask(bit7) | len=42 → 0x8AAA.
    assert_eq!(h.bits(), 0x8AAA);
}

/// Full frame assembly for a masked client→server Text message: header pair
/// + 4-byte mask key + payload — `frame_size_including_mask` must predict
/// the assembled length exactly.
#[test]
fn masked_client_frame_assembly_matches_frame_size() {
    let payload = b"hi";
    let h = WebsocketHeader::new(payload.len() as u8, true, Opcode::Text);
    assert!(h.final_() && h.mask());
    let mut frame = Vec::new();
    h.write_header(&mut frame, payload.len()).unwrap();
    frame.extend_from_slice(&[0x01, 0x02, 0x03, 0x04]); // mask key
    frame.extend_from_slice(payload);
    assert_eq!(
        frame.len(),
        WebsocketHeader::frame_size_including_mask(payload.len()),
        "assembled frame must match frame_size_including_mask"
    );
    // On-wire shape: byte0 = FIN|Text = 0x81; byte1 = MASK|len7 = 0x80|2.
    assert_eq!(frame[0], 0x81);
    assert_eq!(frame[1], 0x80 | payload.len() as u8);
}
