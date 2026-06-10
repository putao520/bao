use bun_brotli::{compress, decompress, BrotliCompressionStream, BrotliReaderArrayList, DecoderOptions};

// ──────────────────────────────────────────────────────────────────────────
// One-shot compress then decompress → original data
// ──────────────────────────────────────────────────────────────────────────

#[test]
fn compress_then_decompress_roundtrip() {
    let input = b"Hello, Brotli! This is a test string for round-trip verification.";
    let compressed = compress(input, 6, 22);
    let decompressed = decompress(&compressed).expect("decompress");
    assert_eq!(decompressed, input);
}

#[test]
fn compress_all_quality_levels() {
    let input = b"The quick brown fox jumps over the lazy dog. 1234567890.";
    for quality in 0..=11 {
        let compressed = compress(input, quality, 22);
        let decompressed = decompress(&compressed).unwrap_or_else(|_| panic!("quality={quality}"));
        assert_eq!(decompressed, input, "quality={quality}");
    }
}

#[test]
fn compress_empty_input() {
    let compressed = compress(b"", 6, 22);
    let decompressed = decompress(&compressed).expect("empty decompress");
    assert_eq!(decompressed, b"");
}

#[test]
fn compress_large_input() {
    let input: Vec<u8> = (0..1_000_000).map(|i| (i % 256) as u8).collect();
    let compressed = compress(&input, 6, 22);
    assert!(compressed.len() < input.len(), "compressed should be smaller");
    let decompressed = decompress(&compressed).expect("large decompress");
    assert_eq!(decompressed, input);
}

#[test]
fn different_lgwin_values() {
    let input = b"Testing different lgwin values for window size.";
    for lgwin in [10, 16, 22, 24] {
        let compressed = compress(input, 11, lgwin);
        let decompressed = decompress(&compressed).expect("decompress");
        assert_eq!(decompressed, input, "lgwin={lgwin}");
    }
}

#[test]
fn compress_is_deterministic() {
    let input = b"Determinism test: compressing the same input with the same params must produce identical output.";
    let a = compress(input, 6, 22);
    let b = compress(input, 6, 22);
    assert_eq!(a, b);
}

#[test]
fn higher_quality_smaller_or_equal_output() {
    let input = b"Different quality levels should generally produce different compressed output. \
                  This text is repeated to make the difference more visible. \
                  Different quality levels should generally produce different compressed output.";
    let low = compress(input, 1, 22);
    let high = compress(input, 11, 22);
    assert!(
        high.len() <= low.len(),
        "quality 11 output ({}) should be <= quality 1 output ({})",
        high.len(),
        low.len()
    );
}

// ──────────────────────────────────────────────────────────────────────────
// Streaming decompress via BrotliReaderArrayList
// ──────────────────────────────────────────────────────────────────────────

#[test]
fn streaming_decompress_matches_oneshot() {
    let input = b"Streaming decompress test: some repeated text to make compression effective. \
                  Streaming decompress test: some repeated text to make compression effective.";
    let compressed = compress(input, 6, 22);

    let mut output = Vec::new();
    let mut reader = BrotliReaderArrayList::new_with_options(
        &compressed[..],
        &mut output,
        &DecoderOptions::default(),
    )
    .expect("new_with_options");
    reader.read_all(true).expect("read_all");

    assert_eq!(output, input);
}

#[test]
fn streaming_decompress_empty() {
    let compressed = compress(b"", 6, 22);
    let mut output = Vec::new();
    let mut reader = BrotliReaderArrayList::new_with_options(
        &compressed[..],
        &mut output,
        &DecoderOptions::default(),
    )
    .expect("new_with_options");
    reader.read_all(true).expect("read_all");
    assert_eq!(output, b"");
}

#[test]
fn streaming_decompress_max_output_size_enforced() {
    let input = vec![0xAB_u8; 100_000];
    let compressed = compress(&input, 6, 22);

    let mut output = Vec::new();
    let mut reader = BrotliReaderArrayList::new_with_options(
        &compressed[..],
        &mut output,
        &DecoderOptions::default(),
    )
    .expect("new_with_options");
    reader.max_output_size = 1000;
    let result = reader.read_all(true);
    assert!(
        result.is_err(),
        "should error when output exceeds max_output_size"
    );
}

// ──────────────────────────────────────────────────────────────────────────
// Streaming compress via BrotliCompressionStream
// ──────────────────────────────────────────────────────────────────────────

#[test]
fn streaming_compress_then_decompress() {
    let input = b"Streaming compression test with enough data to produce multiple blocks. \
                  Streaming compression test with enough data to produce multiple blocks.";

    let mut compressed = Vec::new();
    let mut encoder = BrotliCompressionStream::new(6, 22);
    encoder.write_to_vec(input, false, &mut compressed).expect("write");
    encoder.finish_to_vec(&mut compressed).expect("finish");

    let decompressed = decompress(&compressed).expect("decompress");
    assert_eq!(decompressed, input);
}

#[test]
fn streaming_compress_empty() {
    let mut compressed = Vec::new();
    let mut encoder = BrotliCompressionStream::new(6, 22);
    encoder.finish_to_vec(&mut compressed).expect("finish");
    let decompressed = decompress(&compressed).expect("decompress");
    assert_eq!(decompressed, b"");
}

#[test]
fn streaming_compress_chunked() {
    let chunks: &[&[u8]] = &[b"chunk1 ", b"chunk2 ", b"chunk3 ", b"chunk4 "];
    let expected: Vec<u8> = chunks.iter().flat_map(|c| c.iter().copied()).collect();

    let mut compressed = Vec::new();
    let mut encoder = BrotliCompressionStream::new(6, 22);
    for chunk in chunks {
        encoder.write_to_vec(chunk, false, &mut compressed).expect("write");
    }
    encoder.finish_to_vec(&mut compressed).expect("finish");

    let decompressed = decompress(&compressed).expect("decompress");
    assert_eq!(decompressed, expected);
}

// ──────────────────────────────────────────────────────────────────────────
// DecoderOptions variants
// ──────────────────────────────────────────────────────────────────────────

#[test]
fn decoder_options_large_window() {
    let input = b"Test with large window enabled.";
    let compressed = compress(input, 6, 22);

    let mut output = Vec::new();
    let opts = DecoderOptions {
        large_window: true,
    };
    let mut reader = BrotliReaderArrayList::new_with_options(&compressed[..], &mut output, &opts)
        .expect("new_with_options");
    reader.read_all(true).expect("read_all");
    assert_eq!(output, input);
}

#[test]
fn decoder_options_no_large_window() {
    let input = b"Test without large window.";
    let compressed = compress(input, 6, 22);

    let mut output = Vec::new();
    let opts = DecoderOptions {
        large_window: false,
    };
    let mut reader = BrotliReaderArrayList::new_with_options(&compressed[..], &mut output, &opts)
        .expect("new_with_options");
    reader.read_all(true).expect("read_all");
    assert_eq!(output, input);
}

// ──────────────────────────────────────────────────────────────────────────
// BrotliWriter adapter
// ──────────────────────────────────────────────────────────────────────────

#[test]
fn brotli_writer_roundtrip() {
    let input = b"BrotliWriter round-trip test data.";
    let mut compressed = Vec::new();
    {
        let mut encoder = BrotliCompressionStream::new(6, 22);
        let mut writer = encoder.writer(&mut compressed);
        writer.write(input).expect("write");
        writer.end().expect("end");
    }
    let decompressed = decompress(&compressed).expect("decompress");
    assert_eq!(decompressed, input);
}
