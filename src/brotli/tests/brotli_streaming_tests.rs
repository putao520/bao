use bun_brotli::BrotliReaderArrayList;
use std::io::Write;

fn br(input: &[u8]) -> Vec<u8> {
    let mut enc = brotli::CompressorWriter::new(Vec::new(), 4096, 6, 22);
    enc.write_all(input).unwrap();
    enc.into_inner()
}

fn drive(compressed: &[u8], seat_size: usize) -> Vec<u8> {
    let mut out: Vec<u8> = Vec::new();
    let seats: Vec<&[u8]> = compressed.chunks(seat_size).collect();
    let mut reader =
        BrotliReaderArrayList::new_with_options(seats[0], &mut out, &Default::default()).unwrap();
    let last = seats.len() - 1;
    for (i, seat) in seats.iter().enumerate() {
        if i > 0 {
            reader.input = seat; // update_buffers re-seat (delta contract)
        }
        match reader.read_all(i == last) {
            Ok(()) => {}
            Err(e) if i != last && format!("{e}").contains("ShortRead") => {}
            Err(e) => panic!("seat {i}/{last}: {e}"),
        }
    }
    out
}

#[test]
fn multi_chunk_brotli_stream_16k() {
    let raw: Vec<u8> = (0..200_000u32).map(|i| (i % 251) as u8).collect();
    let out = drive(&br(&raw), 16 * 1024);
    assert_eq!(out.len(), raw.len());
    assert_eq!(out, raw, "body mismatch");
}

#[test]
fn multi_chunk_brotli_stream_4k() {
    let raw = b"small but chunked".repeat(400);
    let out = drive(&br(&raw), 4096);
    assert_eq!(out.len(), raw.len());
}

/// The real www.cloudflare.com homepage br body (104 235 bytes captured from
/// the wire, content-encoding: br, no content-length) decoded across h2-sized
/// 16 KiB seats — the exact shape that failed fetch with error.Unexpected
/// before the inner-reader re-point fix.
#[test]
fn real_cloudflare_br_body_multi_chunk() {
    let compressed = include_bytes!("fixtures_cf_br.bin");
    let out = drive(compressed, 16 * 1024);
    assert!(
        out.len() > 90_000,
        "decompressed suspiciously small: {}",
        out.len()
    );
    assert_eq!(&out[..15], b"<!DOCTYPE html>".as_slice());
}

/// Upstream bdbe669b15 (fetch brotli streaming, oven-sh/bun#41439): a flushed
/// chunk that decodes to more than one 16 KiB output window must reach the
/// reader in full. Brotli reports `needs_more_input` even with output left in
/// its ring buffer, so the decoder used to hand over one window and keep the
/// rest until the next compressed chunk arrived.
#[test]
fn flushed_chunk_decoding_past_one_window_is_delivered_in_full() {
    // Shared sink so the flushed prefix can be snapshotted mid-stream.
    #[derive(Clone)]
    struct SharedBuf(std::rc::Rc<std::cell::RefCell<Vec<u8>>>);
    impl std::io::Write for SharedBuf {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0.borrow_mut().extend_from_slice(buf);
            Ok(buf.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    let sink = SharedBuf(std::rc::Rc::new(std::cell::RefCell::new(Vec::new())));
    let first_line = [vec![b'x'; 40_000], b"\n".to_vec()].concat();
    let second_line = b"done\n";

    let mut enc = brotli::CompressorWriter::new(sink.clone(), 4096, 6, 22);
    enc.write_all(&first_line).unwrap();
    enc.flush().unwrap(); // BROTLI_OPERATION_FLUSH — part1 ends here
    let part1 = sink.0.borrow().clone();
    enc.write_all(second_line).unwrap();
    drop(enc); // Drop emits BROTLI_OPERATION_FINISH
    let full = sink.0.borrow().clone();
    let part2 = full[part1.len()..].to_vec();

    assert!(!part1.is_empty(), "flush emitted no bytes");
    assert!(!part2.is_empty(), "finish emitted no bytes");

    let mut out: Vec<u8> = Vec::new();
    let mut reader =
        BrotliReaderArrayList::new_with_options(&part1, &mut out, &Default::default()).unwrap();
    // Not done: the second compressed part has not arrived, yet every byte of
    // the flushed chunk must be delivered now. `total_out` is the delivery
    // count (the `&mut out` seat is still borrowed by the reader).
    match reader.read_all(false) {
        Ok(()) => {}
        Err(e) if format!("{e}").contains("ShortRead") => {}
        Err(e) => panic!("first seat: {e}"),
    }
    let delivered = reader.total_out;

    // Second seat finishes the stream.
    reader.input = &part2;
    reader.total_in = 0;
    reader.read_all(true).unwrap_or_else(|e| panic!("final seat: {e}"));
    drop(reader);

    assert_eq!(delivered, first_line.len(), "decoder kept decoded bytes pending");
    assert_eq!(out.len(), first_line.len() + second_line.len());
    assert_eq!(&out[..first_line.len()], &first_line[..]);
    assert_eq!(&out[first_line.len()..], second_line);
}
