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
