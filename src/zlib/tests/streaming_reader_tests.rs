// Streaming ZlibReaderArrayList tests — the multi-chunk contract the HTTP
// body pipeline relies on (`bun_http::Decompressor` re-seats `input` with
// each delivery's delta and expects `ShortRead` while the stream is still
// incomplete; a hard error mid-stream killed every content-length-less
// gzip response — see the one.one.one.one fetch failure).
//
// Coverage matrix (6 维度里的 wire-behavior 维):
//   - format × chunking: gzip/zlib/raw, split at hostile boundaries
//     (header split, mid-deflate, trailer split)
//   - error paths: CRC corruption, truncation with is_done, empty input
//   - state: multi-member gzip, max_output_size, single-shot read_all(true)

use bun_zlib::{Options, ZlibReaderArrayList};

/// Drive the reader the way `bun_http::Decompressor` does: create with the
/// first chunk, then for every later chunk re-seat `input` (update_buffers)
/// and `read_all(is_done)`; `ShortRead` mid-stream is expected and tolerated.
fn stream_decode(format_bits: i32, compressed: &[u8], chunks: &[usize]) -> Vec<u8> {
    // `chunks` partitions `compressed` into consecutive seats.
    let mut seats: Vec<&[u8]> = Vec::new();
    let mut off = 0usize;
    for &len in chunks {
        let end = off.saturating_add(len).min(compressed.len());
        seats.push(&compressed[off..end]);
        off = end;
    }
    if off < compressed.len() {
        seats.push(&compressed[off..]);
    }

    let mut out: Vec<u8> = Vec::new();
    let mut reader = ZlibReaderArrayList::init_with_options(
        seats[0],
        &mut out,
        Options { window_bits: format_bits, ..Default::default() },
    )
    .expect("init");
    let last = seats.len() - 1;
    for (i, seat) in seats.iter().enumerate() {
        if i > 0 {
            // Mirror Decompressor::update_buffers' Zlib re-seat: only the
            // input pointer moves; no counters reset.
            reader.input = seat;
        }
        match reader.read_all(i == last) {
            Ok(()) => {}
            // Mid-stream ShortRead is the contract for "need more input".
            Err(e) if i != last && e == bun_zlib::ZlibError::ShortRead => {}
            Err(e) => panic!("read_all(seat {i}/{last}) failed: {e:?}"),
        }
    }
    drop(reader);
    out
}

fn gz(input: &[u8]) -> Vec<u8> {
    let mut enc = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    use std::io::Write;
    enc.write_all(input).unwrap();
    enc.finish().unwrap()
}

fn zlib_stream(input: &[u8]) -> Vec<u8> {
    let mut enc = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
    use std::io::Write;
    enc.write_all(input).unwrap();
    enc.finish().unwrap()
}

fn raw_stream(input: &[u8]) -> Vec<u8> {
    let mut enc = flate2::write::DeflateEncoder::new(Vec::new(), flate2::Compression::default());
    use std::io::Write;
    enc.write_all(input).unwrap();
    enc.finish().unwrap()
}

/// ~192 KiB of pseudo-random (incompressible) bytes: keeps the compressed
/// stream large enough to exercise multi-seat delivery (a run-length body
/// compresses to a few hundred bytes and every seat becomes single-shot).
fn body() -> Vec<u8> {
    let mut x: u64 = 0x9E3779B97F4A7C15;
    (0..(63 * 1024))
        .map(|_| {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            (x >> 33) as u8
        })
        .collect()
}

// ── gzip: format 31 (15|16) ───────────────────────────────────────────────

#[test]
fn gzip_single_shot() {
    let body = body();
    let out = stream_decode(31, &gz(&body), &[usize::MAX]);
    assert_eq!(out, body);
}

#[test]
fn gzip_header_split_across_chunks() {
    // 3 bytes of header, then the rest — header parser must resume.
    let out = stream_decode(31, &gz(&body()), &[3, usize::MAX]);
    assert_eq!(out.len(), body().len());
}

#[test]
fn gzip_trailer_split_across_chunks() {
    let compressed = gz(&body());
    let split = compressed.len() - 4; // into the CRC/ISIZE trailer
    let out = stream_decode(31, &compressed, &[split, usize::MAX]);
    assert_eq!(out.len(), body().len());
}

#[test]
fn gzip_byte_at_a_time() {
    // The most hostile chunking: one byte per delivery. Slow but must be
    // exact — this is the boundary-split stress for header + deflate +
    // trailer state machines.
    let compressed = gz(b"hello streaming gzip world");
    let seats: Vec<usize> = (0..compressed.len()).map(|_| 1usize).collect();
    let out = stream_decode(31, &compressed, &seats);
    assert_eq!(out, b"hello streaming gzip world");
}

#[test]
fn gzip_multi_member() {
    // RFC 1952 §2.2: concatenated members decompress to the concatenation.
    let mut both = gz(b"first-member|").clone();
    both.extend_from_slice(&gz(b"second-member"));
    let out = stream_decode(31, &both, &[usize::MAX]);
    assert_eq!(out, b"first-member|second-member");
}

#[test]
fn gzip_multi_member_split_between_members() {
    let mut both = gz(b"AAA").clone();
    let boundary = both.len();
    both.extend_from_slice(&gz(b"BBB"));
    let out = stream_decode(31, &both, &[boundary, usize::MAX]);
    assert_eq!(out, b"AAABBB");
}

#[test]
fn gzip_mid_deflate_chunks() {
    // 8 KiB seats mirror the h2 DATA frames of a real response.
    let compressed = gz(&body());
    let seats: Vec<usize> = compressed.chunks(8 * 1024).map(|c| c.len()).collect();
    let out = stream_decode(31, &compressed, &seats);
    assert_eq!(out.len(), body().len());
}

// ── zlib / raw ────────────────────────────────────────────────────────────

#[test]
fn zlib_stream_multi_chunk() {
    let compressed = zlib_stream(&body());
    let seats: Vec<usize> = compressed.chunks(5 * 1024).map(|c| c.len()).collect();
    let out = stream_decode(15, &compressed, &seats);
    assert_eq!(out.len(), body().len());
}

#[test]
fn raw_stream_multi_chunk() {
    let compressed = raw_stream(&body());
    let seats: Vec<usize> = compressed.chunks(7 * 1024).map(|c| c.len()).collect();
    let out = stream_decode(-15, &compressed, &seats);
    assert_eq!(out.len(), body().len());
}

// ── auto-detect (window_bits 0 and 47) ────────────────────────────────────

#[test]
fn auto_detect_gzip_and_zlib() {
    assert_eq!(stream_decode(0, &gz(b"auto-gzip"), &[usize::MAX]), b"auto-gzip");
    assert_eq!(stream_decode(0, &zlib_stream(b"auto-zlib"), &[usize::MAX]), b"auto-zlib");
    assert_eq!(stream_decode(47, &gz(b"auto47"), &[usize::MAX]), b"auto47");
}

// ── error paths ───────────────────────────────────────────────────────────

#[test]
fn gzip_corrupt_crc_fails() {
    let mut compressed = gz(&body());
    let last = compressed.len() - 1;
    compressed[last] ^= 0xff; // flip ISIZE bits
    let mut out = Vec::new();
    let mut reader = ZlibReaderArrayList::init_with_options(
        &compressed,
        &mut out,
        Options { window_bits: 31, ..Default::default() },
    )
    .unwrap();
    assert!(reader.read_all(true).is_err());
    drop(reader);
}

#[test]
fn gzip_truncated_final_fails_not_shortread() {
    let compressed = gz(&body());
    let cut = &compressed[..compressed.len() - 5]; // trailer incomplete
    let mut out = Vec::new();
    let mut reader = ZlibReaderArrayList::init_with_options(
        cut,
        &mut out,
        Options { window_bits: 31, ..Default::default() },
    )
    .unwrap();
    match reader.read_all(true) {
        Err(bun_zlib::ZlibError::ZlibError) => {} // truncated at is_done: hard error
        other => panic!("expected ZlibError, got {other:?}"),
    }
}

#[test]
fn gzip_partial_without_is_done_returns_short_read() {
    // THE regression: a valid first 8 KiB of a larger gzip stream must be
    // `ShortRead`, not a fatal error (this is exactly what killed
    // https fetch bodies without content-length).
    let compressed = gz(&body());
    let mut out = Vec::new();
    let mut reader = ZlibReaderArrayList::init_with_options(
        &compressed[..8 * 1024],
        &mut out,
        Options { window_bits: 31, ..Default::default() },
    )
    .unwrap();
    match reader.read_all(false) {
        Err(bun_zlib::ZlibError::ShortRead) => {}
        other => panic!("expected ShortRead, got {other:?}"),
    }
}

#[test]
fn max_output_size_enforced_streaming() {
    let compressed = gz(&body());
    let mut out = Vec::new();
    let mut reader = ZlibReaderArrayList::init_with_options(
        &compressed,
        &mut out,
        Options { window_bits: 31, ..Default::default() },
    )
    .unwrap();
    reader.max_output_size = 1024;
    assert!(reader.read_all(true).is_err());
    drop(reader);
    assert!(out.len() <= 1024 + 32 * 1024); // bounded by one pass past the cap
}
