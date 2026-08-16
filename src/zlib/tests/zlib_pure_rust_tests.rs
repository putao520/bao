use bun_zlib::{
    compress, compress2, compressBound, crc32, uncompress,
    deflate_compress, inflate_decompress, deflate_bound,
    ZlibReaderArrayList, ZlibCompressorArrayList, Options,
    NodeMode, ReturnCode,
};

// ──────────────────────────────────────────────────────────────────────────
// One-shot compress / uncompress roundtrip (zlib-wrapped)
// ──────────────────────────────────────────────────────────────────────────

#[test]
fn compress_then_uncompress_roundtrip() {
    let input = b"Hello, zlib! This is a pure Rust roundtrip test.";
    let bound = compressBound(input.len() as _);
    let mut dest = vec![0u8; bound as usize];
    let mut dest_len = bound;

    let rc = compress(dest.as_mut_ptr(), &mut dest_len, input.as_ptr(), input.len() as _);
    assert_eq!(rc, ReturnCode::Ok as i32);

    let compressed = &dest[..dest_len as usize];
    // Small inputs may expand after compression (zlib header overhead); just verify non-empty

    let mut decomp = vec![0u8; input.len() * 2];
    let mut decomp_len = decomp.len() as _;
    let rc2 = uncompress(decomp.as_mut_ptr(), &mut decomp_len, compressed.as_ptr(), compressed.len() as _);
    assert_eq!(rc2, ReturnCode::Ok as i32);
    assert_eq!(&decomp[..decomp_len as usize], input);
}

#[test]
fn compress2_all_levels() {
    let input = b"The quick brown fox jumps over the lazy dog. 0123456789.";
    for level in 1..=9 {
        let bound = compressBound(input.len() as _);
        let mut dest = vec![0u8; bound as usize];
        let mut dest_len = bound;

        let rc = compress2(dest.as_mut_ptr(), &mut dest_len, input.as_ptr(), input.len() as _, level);
        assert_eq!(rc, ReturnCode::Ok as i32, "level={level}");

        let mut decomp = vec![0u8; input.len() * 2];
        let mut decomp_len = decomp.len() as _;
        let rc2 = uncompress(decomp.as_mut_ptr(), &mut decomp_len, dest.as_ptr(), dest_len as _);
        assert_eq!(rc2, ReturnCode::Ok as i32, "level={level}");
        assert_eq!(&decomp[..decomp_len as usize], input, "level={level}");
    }
}

#[test]
fn compress_empty_input() {
    let bound = compressBound(0);
    let mut dest = vec![0u8; bound as usize];
    let mut dest_len = bound;

    let rc = compress(dest.as_mut_ptr(), &mut dest_len, [].as_ptr(), 0);
    assert_eq!(rc, ReturnCode::Ok as i32);

    let mut decomp = vec![0u8; 256];
    let mut decomp_len = decomp.len() as _;
    let rc2 = uncompress(decomp.as_mut_ptr(), &mut decomp_len, dest.as_ptr(), dest_len as _);
    assert_eq!(rc2, ReturnCode::Ok as i32);
    assert_eq!(decomp_len as usize, 0);
}

#[test]
fn compress_large_input() {
    let input: Vec<u8> = (0..1_000_000).map(|i| (i % 256) as u8).collect();
    let bound = compressBound(input.len() as _);
    let mut dest = vec![0u8; bound as usize];
    let mut dest_len = bound;

    let rc = compress(dest.as_mut_ptr(), &mut dest_len, input.as_ptr(), input.len() as _);
    assert_eq!(rc, ReturnCode::Ok as i32);
    assert!((dest_len as usize) < input.len());

    let mut decomp = vec![0u8; input.len() + 1024];
    let mut decomp_len = decomp.len() as _;
    let rc2 = uncompress(decomp.as_mut_ptr(), &mut decomp_len, dest.as_ptr(), dest_len as _);
    assert_eq!(rc2, ReturnCode::Ok as i32);
    assert_eq!(decomp_len as usize, input.len());
    assert_eq!(&decomp[..input.len()], input);
}

// ──────────────────────────────────────────────────────────────────────────
// CRC-32
// ──────────────────────────────────────────────────────────────────────────

#[test]
fn crc32_known_value() {
    let data = b"123456789";
    let crc = crc32(0, data.as_ptr(), data.len() as _);
    assert_eq!(crc, 0xCBF43926);
}

#[test]
fn crc32_empty() {
    let crc = crc32(0, [].as_ptr(), 0);
    assert_eq!(crc, 0);
}

#[test]
fn crc32_null_buf() {
    let crc = crc32(42, std::ptr::null(), 0);
    assert_eq!(crc, 42);
}

// ──────────────────────────────────────────────────────────────────────────
// compressBound
// ──────────────────────────────────────────────────────────────────────────

#[test]
fn compress_bound_sufficient() {
    let input = b"test data for compress bound check";
    let bound = compressBound(input.len() as _) as usize;
    let mut dest = vec![0u8; bound];
    let mut dest_len = bound as _;
    let rc = compress(dest.as_mut_ptr(), &mut dest_len, input.as_ptr(), input.len() as _);
    assert_eq!(rc, ReturnCode::Ok as i32, "compressBound should be sufficient");
    assert!((dest_len as usize) <= bound);
}

// ──────────────────────────────────────────────────────────────────────────
// deflate_compress / inflate_decompress (one-shot Rust-friendly API)
// ──────────────────────────────────────────────────────────────────────────

#[test]
fn deflate_compress_zlib_roundtrip() {
    let input = b"Zlib-wrapped deflate roundtrip test.";
    let compressed = deflate_compress(input, 15, 6).expect("compress");
    let decompressed = inflate_decompress(&compressed, 15).expect("decompress");
    assert_eq!(decompressed, input);
}

#[test]
fn deflate_compress_raw_roundtrip() {
    let input = b"Raw deflate roundtrip test.";
    let compressed = deflate_compress(input, -15, 6).expect("compress");
    let decompressed = inflate_decompress(&compressed, -15).expect("decompress");
    assert_eq!(decompressed, input);
}

#[test]
fn deflate_compress_gzip_roundtrip() {
    let input = b"Gzip-wrapped roundtrip test.";
    let compressed = deflate_compress(input, 31, 6).expect("compress");
    let decompressed = inflate_decompress(&compressed, 31).expect("decompress");
    assert_eq!(decompressed, input);
}

#[test]
fn deflate_compress_empty() {
    let compressed = deflate_compress(b"", -15, 6).expect("compress empty");
    let decompressed = inflate_decompress(&compressed, -15).expect("decompress empty");
    assert_eq!(decompressed, b"");
}

#[test]
fn deflate_compress_all_levels() {
    let input = b"Testing all compression levels for deflate_compress.";
    for level in 0..=9 {
        let compressed = match deflate_compress(input, -15, level) {
            Some(c) => c,
            None => panic!("compress level={level}"),
        };
        let decompressed = match inflate_decompress(&compressed, -15) {
            Some(d) => d,
            None => panic!("decompress level={level}"),
        };
        assert_eq!(decompressed, input, "level={level}");
    }
}

// ──────────────────────────────────────────────────────────────────────────
// Streaming decompression via ZlibReaderArrayList
// ──────────────────────────────────────────────────────────────────────────

#[test]
fn streaming_decompress_zlib() {
    let input = b"Streaming zlib decompress test: repeated data repeated data repeated data.";
    let compressed = deflate_compress(input, 15, 6).expect("compress");

    let mut output = Vec::new();
    {
        let mut reader = ZlibReaderArrayList::init(&compressed, &mut output).expect("init");
        reader.read_all(true).expect("read_all");
    }
    assert_eq!(output, input);
}

#[test]
fn streaming_decompress_gzip() {
    let input = b"Streaming gzip decompress test.";
    let compressed = deflate_compress(input, 31, 6).expect("compress");

    let mut output = Vec::new();
    let opts = Options { window_bits: 15 + 32, ..Default::default() };
    {
        let mut reader = ZlibReaderArrayList::init_with_options(&compressed, &mut output, opts).expect("init");
        reader.read_all(true).expect("read_all");
    }
    assert_eq!(output, input);
}

#[test]
fn streaming_decompress_empty() {
    let compressed = deflate_compress(b"", 15, 6).expect("compress");
    let mut output = Vec::new();
    {
        let mut reader = ZlibReaderArrayList::init(&compressed, &mut output).expect("init");
        reader.read_all(true).expect("read_all");
    }
    assert_eq!(output, b"");
}

#[test]
fn streaming_decompress_large() {
    let input: Vec<u8> = (0..500_000).map(|i| (i % 256) as u8).collect();
    let compressed = deflate_compress(&input, 15, 6).expect("compress");

    let mut output = Vec::new();
    {
        let mut reader = ZlibReaderArrayList::init(&compressed, &mut output).expect("init");
        reader.read_all(true).expect("read_all");
    }
    assert_eq!(output, input);
}

#[test]
fn streaming_decompress_max_output_size_enforced() {
    let input = vec![0xAB_u8; 100_000];
    let compressed = deflate_compress(&input, 15, 6).expect("compress");

    let mut output = Vec::new();
    let mut reader = ZlibReaderArrayList::init(&compressed, &mut output).expect("init");
    reader.max_output_size = 1000;
    let result = reader.read_all(true);
    assert!(result.is_err(), "should error when output exceeds max_output_size");
}

// ──────────────────────────────────────────────────────────────────────────
// Streaming compression via ZlibCompressorArrayList
// ──────────────────────────────────────────────────────────────────────────

#[test]
fn streaming_compress_zlib_roundtrip() {
    let input = b"Streaming compression test: enough data to exercise the compressor.";
    let mut compressed = Vec::new();
    let opts = Options { gzip: false, window_bits: 15, level: 6, ..Default::default() };
    {
        let mut compressor = ZlibCompressorArrayList::init(input, &mut compressed, opts).expect("init");
        compressor.read_all().expect("read_all");
    }

    let mut decomp = Vec::new();
    {
        let mut reader = ZlibReaderArrayList::init(&compressed, &mut decomp).expect("init");
        reader.read_all(true).expect("read_all");
    }
    assert_eq!(decomp, input);
}

#[test]
fn streaming_compress_gzip_roundtrip() {
    let input = b"Streaming gzip compression test.";
    let mut compressed = Vec::new();
    let opts = Options { gzip: true, window_bits: 15, level: 6, ..Default::default() };
    {
        let mut compressor = ZlibCompressorArrayList::init(input, &mut compressed, opts).expect("init");
        compressor.read_all().expect("read_all");
    }

    let mut decomp = Vec::new();
    let decomp_opts = Options { window_bits: 15 + 32, ..Default::default() };
    {
        let mut reader = ZlibReaderArrayList::init_with_options(&compressed, &mut decomp, decomp_opts).expect("init");
        reader.read_all(true).expect("read_all");
    }
    assert_eq!(decomp, input);
}

#[test]
fn streaming_compress_empty() {
    let mut compressed = Vec::new();
    let opts = Options { gzip: false, window_bits: 15, level: 6, ..Default::default() };
    {
        let mut compressor = ZlibCompressorArrayList::init(b"", &mut compressed, opts).expect("init");
        compressor.read_all().expect("read_all");
    }

    let mut decomp = Vec::new();
    {
        let mut reader = ZlibReaderArrayList::init(&compressed, &mut decomp).expect("init");
        reader.read_all(true).expect("read_all");
    }
    assert_eq!(decomp, b"");
}

// ──────────────────────────────────────────────────────────────────────────
// NodeMode mapping
// ──────────────────────────────────────────────────────────────────────────

#[test]
fn node_mode_from_int() {
    assert_eq!(NodeMode::from_int(0), NodeMode::NONE);
    assert_eq!(NodeMode::from_int(1), NodeMode::DEFLATE);
    assert_eq!(NodeMode::from_int(2), NodeMode::INFLATE);
    assert_eq!(NodeMode::from_int(3), NodeMode::GZIP);
    assert_eq!(NodeMode::from_int(4), NodeMode::GUNZIP);
    assert_eq!(NodeMode::from_int(5), NodeMode::DEFLATERAW);
    assert_eq!(NodeMode::from_int(6), NodeMode::INFLATERAW);
    assert_eq!(NodeMode::from_int(7), NodeMode::UNZIP);
    assert_eq!(NodeMode::from_int(8), NodeMode::BROTLI_DECODE);
    assert_eq!(NodeMode::from_int(9), NodeMode::BROTLI_ENCODE);
    assert_eq!(NodeMode::from_int(10), NodeMode::ZSTD_COMPRESS);
    assert_eq!(NodeMode::from_int(11), NodeMode::ZSTD_DECOMPRESS);
    assert_eq!(NodeMode::from_int(99), NodeMode::NONE);
}

// ──────────────────────────────────────────────────────────────────────────
// deflate_bound
// ──────────────────────────────────────────────────────────────────────────

#[test]
fn deflate_bound_sufficient() {
    let input = b"test data for deflate bound";
    let bound = deflate_bound(input.len(), -15, false);
    let compressed = deflate_compress(input, -15, 6).expect("compress");
    assert!(compressed.len() <= bound, "deflate_bound should be >= actual compressed size");
}

// ──────────────────────────────────────────────────────────────────────────
// Error handling
// ──────────────────────────────────────────────────────────────────────────

#[test]
fn uncompress_corrupt_data() {
    let mut dest = vec![0u8; 1024];
    let mut dest_len = dest.len() as _;
    let rc = uncompress(dest.as_mut_ptr(), &mut dest_len, b"not valid zlib data".as_ptr(), 20);
    assert_ne!(rc, ReturnCode::Ok as i32, "corrupt data should fail");
}

#[test]
fn uncompress_buffer_too_small() {
    let input = b"Buffer too small test data.";
    let compressed = deflate_compress(input, 15, 6).expect("compress");

    let mut dest = vec![0u8; 1];
    let mut dest_len = dest.len() as _;
    let rc = uncompress(dest.as_mut_ptr(), &mut dest_len, compressed.as_ptr(), compressed.len() as _);
    assert_ne!(rc, ReturnCode::Ok as i32, "should fail with too-small buffer");
}

// ──────────────────────────────────────────────────────────────────────────
// inflate_decompress_checked — node-classified failures + multi-member gzip
// (the node:zlib sync bindings route through this; returning Err with a
// reason instead of None is what makes gunzipSync etc. throw ZlibError
// rather than silently returning undefined).
// ──────────────────────────────────────────────────────────────────────────

use bun_zlib::{inflate_decompress_checked, InflateFailure};

fn gz_member(input: &[u8]) -> Vec<u8> {
    use std::io::Write;
    let mut enc = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::new(6));
    enc.write_all(input).unwrap();
    enc.finish().unwrap()
}

fn zlib_member(input: &[u8]) -> Vec<u8> {
    use std::io::Write;
    let mut enc = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::new(6));
    enc.write_all(input).unwrap();
    enc.finish().unwrap()
}

fn raw_member(input: &[u8]) -> Vec<u8> {
    use std::io::Write;
    let mut enc = flate2::write::DeflateEncoder::new(Vec::new(), flate2::Compression::new(6));
    enc.write_all(input).unwrap();
    enc.finish().unwrap()
}

#[test]
fn checked_zlib_roundtrip_ok() {
    let body = b"checked zlib roundtrip payload";
    let out = inflate_decompress_checked(&zlib_member(body), 15).expect("decode");
    assert_eq!(out, body);
}

#[test]
fn checked_zlib_truncated_is_unexpected_eof() {
    let mut compressed = zlib_member(b"truncation target payload, repeated repeated");
    compressed.truncate(compressed.len() - 5); // cut mid-stream
    let err = inflate_decompress_checked(&compressed, 15).unwrap_err();
    assert_eq!(err, InflateFailure::Truncated);
    assert_eq!(err.message(), "unexpected end of file");
}

#[test]
fn checked_zlib_bad_fcheck_is_header_check() {
    // CM=8 but the u16 is not a multiple of 31 → zlib's first header check.
    let err = inflate_decompress_checked(&[0x78, 0x9e, 0x00, 0x00], 15).unwrap_err();
    assert_eq!(err, InflateFailure::HeaderCheck);
    assert_eq!(err.message(), "incorrect header check");
}

#[test]
fn checked_zlib_bad_cm_is_unknown_method() {
    // 0x7918: FCHECK ok (31000 = 31 * 1000) but CM = 9.
    let err = inflate_decompress_checked(&[0x79, 0x18, 0x00, 0x00], 15).unwrap_err();
    assert_eq!(err, InflateFailure::UnknownMethod);
    assert_eq!(err.message(), "unknown compression method");
}

#[test]
fn checked_zlib_cinfo_too_large_is_window_size() {
    // 0x881c: FCHECK ok (34844 = 31 * 1124), CM = 8, but CINFO = 8 > 7.
    let err = inflate_decompress_checked(&[0x88, 0x1c, 0x00, 0x00], 15).unwrap_err();
    assert_eq!(err, InflateFailure::WindowSize);
    assert_eq!(err.message(), "invalid window size");
}

#[test]
fn checked_zlib_short_input_is_unexpected_eof() {
    let one = inflate_decompress_checked(&[0x78], 15).unwrap_err();
    assert_eq!(one, InflateFailure::Truncated);
    let empty = inflate_decompress_checked(&[], 15).unwrap_err();
    assert_eq!(empty, InflateFailure::Truncated);
}

#[test]
fn checked_gzip_roundtrip_ok() {
    let body = b"checked gzip roundtrip payload";
    let out = inflate_decompress_checked(&gz_member(body), 16).expect("decode");
    assert_eq!(out, body);
}

#[test]
fn checked_gzip_bad_magic_is_header_check() {
    let mut member = gz_member(b"magic corruption target");
    member[0] ^= 0xff;
    let err = inflate_decompress_checked(&member, 16).unwrap_err();
    assert_eq!(err, InflateFailure::HeaderCheck);
    assert_eq!(err.message(), "incorrect header check");
}

#[test]
fn checked_gzip_garbage_is_header_check() {
    let garbage = [0xff, 0xfe, 0xfd, 0xfc, 0xfb, 0xfa, 0xf9, 0xf8, 0xf7, 0xf6, 0xf5, 0xf4];
    let err = inflate_decompress_checked(&garbage, 16).unwrap_err();
    assert_eq!(err, InflateFailure::HeaderCheck);
}

#[test]
fn checked_gzip_truncated_trailer_is_unexpected_eof() {
    let mut member = gz_member(b"trailer truncation target payload");
    member.truncate(member.len() - 4); // into the CRC/ISIZE trailer
    let err = inflate_decompress_checked(&member, 16).unwrap_err();
    assert_eq!(err, InflateFailure::Truncated);
}

#[test]
fn checked_gzip_corrupt_crc_is_data_check() {
    let mut member = gz_member(b"crc corruption target payload");
    let last = member.len() - 1;
    member[last] ^= 0xff; // flip ISIZE/CRC trailer bits
    let err = inflate_decompress_checked(&member, 16).unwrap_err();
    assert!(
        err == InflateFailure::DataCheck || err == InflateFailure::LengthCheck,
        "trailer corruption must surface, got {err:?}"
    );
}

#[test]
fn checked_gzip_multi_member_all_members() {
    // RFC 1952 §2.2: concatenated members decompress to the concatenation —
    // the sync path previously stopped after the FIRST member.
    let mut two = gz_member(b"first-member|");
    two.extend_from_slice(&gz_member(b"second-member"));
    let out = inflate_decompress_checked(&two, 16).expect("both members");
    assert_eq!(out, b"first-member|second-member");

    let mut three = gz_member(b"A-");
    three.extend_from_slice(&gz_member(b"B-"));
    three.extend_from_slice(&gz_member(b"C"));
    let out = inflate_decompress_checked(&three, 16).expect("three members");
    assert_eq!(out, b"A-B-C");
}

#[test]
fn checked_gzip_multi_member_corrupt_second_crc_fails() {
    // CRC verification must span EVERY member: corrupt only the second
    // member's trailer — the whole call must fail (the old single-member
    // decode silently returned member one's data).
    let first = gz_member(b"member-one-");
    let mut second = gz_member(b"member-two");
    let crc_off = second.len() - 8;
    second[crc_off] ^= 0xff;
    let mut both = first.clone();
    both.extend_from_slice(&second);
    let err = inflate_decompress_checked(&both, 16).unwrap_err();
    assert!(
        err == InflateFailure::DataCheck || err == InflateFailure::LengthCheck,
        "second-member CRC must be verified, got {err:?}"
    );
}

#[test]
fn checked_gzip_empty_member_decodes_empty() {
    // Empty output is a legitimate result, not a failure.
    let out = inflate_decompress_checked(&gz_member(b""), 16).expect("empty member is valid");
    assert!(out.is_empty());
}

#[test]
fn checked_raw_roundtrip_and_garbage() {
    let body = b"checked raw deflate roundtrip payload";
    let out = inflate_decompress_checked(&raw_member(body), -15).expect("decode");
    assert_eq!(out, body);

    let garbage = [0xff, 0xfe, 0xfd, 0xfc, 0xfb, 0xfa, 0xf9, 0xf8];
    let err = inflate_decompress_checked(&garbage, -15).unwrap_err();
    assert_eq!(err, InflateFailure::Corrupt, "0xff block header is reserved BTYPE=3");
    assert_eq!(err.message(), "invalid deflate data");
}

#[test]
fn checked_raw_truncated_is_unexpected_eof() {
    let mut compressed = raw_member(b"raw truncation target payload repeated repeated");
    compressed.truncate(compressed.len() - 3);
    let err = inflate_decompress_checked(&compressed, -15).unwrap_err();
    assert_eq!(err, InflateFailure::Truncated);
}

#[test]
fn checked_auto_detect_gzip_zlib_raw() {
    let gz = inflate_decompress_checked(&gz_member(b"auto-gzip"), 47).expect("auto gzip");
    assert_eq!(gz, b"auto-gzip");
    let zl = inflate_decompress_checked(&zlib_member(b"auto-zlib"), 47).expect("auto zlib");
    assert_eq!(zl, b"auto-zlib");
    // raw fallback is bao's superset of node's gzip|zlib auto-detect
    let rw = inflate_decompress_checked(&raw_member(b"auto-raw"), 47).expect("auto raw");
    assert_eq!(rw, b"auto-raw");
}

#[test]
fn checked_auto_garbage_is_header_check() {
    // node's auto-detect only knows gzip|zlib wrappers: undetectable input
    // reports as a header error.
    let garbage = [0x33, 0x32, 0x31, 0x30, 0x2f, 0x2e, 0x2d, 0x2c];
    let err = inflate_decompress_checked(&garbage, 47).unwrap_err();
    assert_eq!(err, InflateFailure::HeaderCheck);
    assert!(inflate_decompress_checked(&[], 47).is_err());
}
