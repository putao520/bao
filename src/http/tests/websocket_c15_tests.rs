//! C15 coverage wave — additive `bun_http::websocket` frame-header codec
//! faces (REQ-ENG-010). The C7 `websocket_header_tests` own the happy-path
//! layout contract; this file pins the arms that file leaves open: the
//! `write_header` writer-error propagation, the u7 length truncation at
//! construction, the all-16-opcode wire round-trip, the all-fields-extreme
//! bit image, and extended-length masked-frame assembly.

use bun_http::websocket::{Opcode, WebsocketHeader};

/// Link leg (C7 suites convention): referencing `force_link` keeps
/// bao_native_stubs in the binary; its .init_array ctor drags in the
/// lsquic/boringssl native archives that bun_uws_sys's quic/socket objects
/// reference at link time.
#[test]
fn native_stubs_link_leg_c15() {
    bao_native_stubs::force_link();
}

/// A writer that always fails — drives `write_header`'s `?` propagation arm
/// (C7 only writes into a `Vec`, which cannot fail).
struct FailingWriter;

impl std::io::Write for FailingWriter {
    fn write(&mut self, _buf: &[u8]) -> std::io::Result<usize> {
        Err(std::io::Error::new(std::io::ErrorKind::BrokenPipe, "c15"))
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// `write_header` must propagate the writer's error, not panic and not
/// partially succeed (the `writer.write_all(..)?` arm).
#[test]
fn write_header_propagates_writer_errors_c15() {
    let h = WebsocketHeader::new(3, false, Opcode::Text);
    let err = h
        .write_header(&mut FailingWriter, 3)
        .expect_err("failing writer must propagate");
    assert_eq!(err.kind(), std::io::ErrorKind::BrokenPipe);
}

/// `new` masks the length into the u7 field (RFC 6455: the inline length is
/// 7 bits; extended lengths follow in-band and `pack_length` names them).
#[test]
fn new_len_above_127_truncates_low_seven_bits_c15() {
    let h = WebsocketHeader::new(200, false, Opcode::Binary);
    assert_eq!(h.len(), 200 & 0x7F, "200 truncates to its low 7 bits");
    let h = WebsocketHeader::new(255, true, Opcode::Close);
    assert_eq!(h.len(), 127);
    let h = WebsocketHeader::new(128, false, Opcode::Ping);
    assert_eq!(h.len(), 0);
}

/// Every opcode survives the wire round-trip (`slice` → `from_slice` →
/// accessor), and the control-plane bit is visible in the wire byte itself.
#[test]
fn all_sixteen_opcodes_roundtrip_through_wire_bytes_c15() {
    for nib in 0x0u16..=0xF {
        let op = Opcode::from_raw(nib as u8);
        let h = WebsocketHeader::new(1, false, op);
        let wire = h.slice();
        let parsed = WebsocketHeader::from_slice(wire);
        assert_eq!(parsed.opcode(), op, "nibble {nib:#x}");
        // The control bit is nib & 0x8 — readable straight off the wire.
        assert_eq!(parsed.opcode().is_control(), nib & 0x8 != 0, "nibble {nib:#x}");
    }
}

/// All-fields-extreme image: len=127, mask, rsv=3, compressed (rsv1), not
/// final, opcode ResF → exactly 0x7FFF (bit 15 clear, everything else set).
#[test]
fn all_fields_extreme_bits_image_c15() {
    let mut h = WebsocketHeader::new(127, true, Opcode::ResF);
    h.set_rsv(0b11);
    h.set_compressed(true);
    h.set_final(false);
    assert_eq!(h.bits(), 0x7FFF, "final=0|rsv1=1|rsv=11|op=F|mask=1|len=127");
    let parsed = WebsocketHeader::from_slice(h.slice());
    assert!(!parsed.final_());
    assert!(parsed.compressed());
    assert_eq!(parsed.rsv(), 0b11);
    assert_eq!(parsed.opcode(), Opcode::ResF);
    assert!(parsed.mask());
    assert_eq!(parsed.len(), 127);
}

/// Extended-length masked frame assembly: a 300-byte payload (u16 extension)
/// assembles to exactly `frame_size_including_mask`, and `write_header`'s
/// debug-assert (`len() == pack_length(n)`) holds with the packed length.
#[test]
fn extended_length_masked_frame_assembly_matches_frame_size_c15() {
    let payload = vec![0xABu8; 300];
    let h = WebsocketHeader::new(WebsocketHeader::pack_length(payload.len()), true, Opcode::Binary);
    assert_eq!(h.len(), 126, "u16 extension marker inline");

    let mut frame = Vec::new();
    h.write_header(&mut frame, payload.len()).expect("Vec write");
    // The writer emits only the 2-byte header pair; the caller appends the
    // extended length itself (u16 BE here — `length_byte_count(300) == 2`).
    frame.extend_from_slice(&(payload.len() as u16).to_be_bytes());
    frame.extend_from_slice(&[0x0A, 0x0B, 0x0C, 0x0D]); // mask key
    frame.extend_from_slice(&payload);
    assert_eq!(
        frame.len(),
        WebsocketHeader::frame_size_including_mask(payload.len()),
        "assembled frame must match frame_size_including_mask"
    );
    // byte1 = MASK(0x80) | 126 (u16 extension marker).
    assert_eq!(frame[1], 0x80 | 126);
    // After the 2-byte header: extended length (2) + mask key (4) + payload.
    assert_eq!(frame.len() - 2, 2 + 4 + payload.len());
}
