use bun_zstd::{compress, compress_bound, decompress, decompress_alloc, get_decompressed_size, Result, ZstdReaderArrayList};

#[test]
fn compress_then_decompress_roundtrip() {
    let data = b"Hello, World! This is a test of zstd compression and decompression.";
    let bound = compress_bound(data.len());
    let mut compressed = vec![0u8; bound];
    let n = match compress(&mut compressed, data, Some(3)) {
        Result::Success(n) => n,
        Result::Err(e) => panic!("compress failed: {}", e),
    };
    compressed.truncate(n);

    let mut decompressed = vec![0u8; data.len()];
    let dn = match decompress(&mut decompressed, &compressed) {
        Result::Success(n) => n,
        Result::Err(e) => panic!("decompress failed: {}", e),
    };
    assert_eq!(dn, data.len());
    assert_eq!(&decompressed[..dn], data);
}

#[test]
fn compress_levels_1_to_22() {
    let data = b"Test data for all compression levels.";
    for level in 1..=22 {
        let bound = compress_bound(data.len());
        let mut compressed = vec![0u8; bound];
        let n = match compress(&mut compressed, data, Some(level)) {
            Result::Success(n) => n,
            Result::Err(e) => panic!("level {} compress failed: {}", level, e),
        };
        let mut decompressed = vec![0u8; data.len()];
        let dn = match decompress(&mut decompressed, &compressed[..n]) {
            Result::Success(n) => n,
            Result::Err(e) => panic!("level {} decompress failed: {}", level, e),
        };
        assert_eq!(&decompressed[..dn], data, "level {} roundtrip failed", level);
    }
}

#[test]
fn empty_input() {
    let bound = compress_bound(0);
    let mut compressed = vec![0u8; bound];
    let n = match compress(&mut compressed, &[][..], Some(3)) {
        Result::Success(n) => n,
        Result::Err(e) => panic!("empty compress failed: {}", e),
    };
    assert!(n > 0, "compressed empty input should produce a frame header");

    let mut decompressed = vec![0u8; 0];
    let dn = match decompress(&mut decompressed, &compressed[..n]) {
        Result::Success(n) => n,
        Result::Err(_) => 0,
    };
    assert_eq!(dn, 0, "decompressed empty input should be 0 bytes");
}

#[test]
fn large_input_1mb() {
    let data = vec![0xABu8; 1024 * 1024];
    let bound = compress_bound(data.len());
    let mut compressed = vec![0u8; bound];
    let n = match compress(&mut compressed, &data, Some(3)) {
        Result::Success(n) => n,
        Result::Err(e) => panic!("large compress failed: {}", e),
    };
    let mut decompressed = vec![0u8; data.len()];
    let dn = match decompress(&mut decompressed, &compressed[..n]) {
        Result::Success(n) => n,
        Result::Err(e) => panic!("large decompress failed: {}", e),
    };
    assert_eq!(dn, data.len());
    assert_eq!(&decompressed[..dn], &data[..]);
}

#[test]
fn compress_bound_accuracy() {
    for size in [0, 1, 100, 1024, 65536, 1024 * 1024] {
        let bound = compress_bound(size);
        let data = vec![0u8; size];
        let mut compressed = vec![0u8; bound];
        let n = match compress(&mut compressed, &data, Some(1)) {
            Result::Success(n) => n,
            Result::Err(e) => panic!("compress_bound test failed for size {}: {}", size, e),
        };
        assert!(n <= bound, "compressed size {} exceeds bound {} for input size {}", n, bound, size);
    }
}

#[test]
fn get_decompressed_size_after_compression() {
    let data = b"Test data for decompressed size query.";
    let bound = compress_bound(data.len());
    let mut compressed = vec![0u8; bound];
    let n = match compress(&mut compressed, data, Some(3)) {
        Result::Success(n) => n,
        Result::Err(e) => panic!("compress failed: {}", e),
    };
    let decomp_size = get_decompressed_size(&compressed[..n]);
    assert_eq!(decomp_size, data.len());
}

#[test]
fn decompress_alloc_known_size() {
    let data = b"Test data for decompress_alloc.";
    let bound = compress_bound(data.len());
    let mut compressed = vec![0u8; bound];
    let n = match compress(&mut compressed, data, Some(3)) {
        Result::Success(n) => n,
        Result::Err(e) => panic!("compress failed: {}", e),
    };
    let decompressed = decompress_alloc(&compressed[..n]).expect("decompress_alloc failed");
    assert_eq!(&decompressed[..], data);
}

#[test]
fn streaming_decompress_matches_oneshot() {
    let data = b"Test data for streaming decompression. Needs to be long enough to exercise multiple chunks.";
    let bound = compress_bound(data.len());
    let mut compressed = vec![0u8; bound];
    let n = match compress(&mut compressed, data, Some(3)) {
        Result::Success(n) => n,
        Result::Err(e) => panic!("compress failed: {}", e),
    };
    let compressed = &compressed[..n];

    // Streaming
    let mut output = Vec::new();
    let mut reader = ZstdReaderArrayList::init(compressed, &mut output).expect("init failed");
    reader.read_all(true).expect("read_all failed");
    drop(reader);

    // One-shot
    let mut oneshot = vec![0u8; data.len()];
    let dn = match decompress(&mut oneshot, compressed) {
        Result::Success(n) => n,
        Result::Err(e) => panic!("decompress failed: {}", e),
    };

    assert_eq!(&output[..], &oneshot[..dn]);
    assert_eq!(&output[..], data);
}

#[test]
fn default_compression_level() {
    let data = b"Test with default level.";
    let bound = compress_bound(data.len());
    let mut compressed = vec![0u8; bound];
    let n = match compress(&mut compressed, data, None) {
        Result::Success(n) => n,
        Result::Err(e) => panic!("default level compress failed: {}", e),
    };
    let mut decompressed = vec![0u8; data.len()];
    let dn = match decompress(&mut decompressed, &compressed[..n]) {
        Result::Success(n) => n,
        Result::Err(e) => panic!("default level decompress failed: {}", e),
    };
    assert_eq!(&decompressed[..dn], data);
}
