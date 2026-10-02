use bun_core::MutableString;
use bun_http_types::Encoding::Encoding;

use bun_brotli::BrotliReaderArrayList;
use bun_zlib::ZlibReaderArrayList;
use bun_zstd::ZstdReaderArrayList;

// PORT NOTE: the `*ReaderArrayList<'a>` types carry a `&'a mut Vec<u8>` borrow
// of the output buffer (and a `&'a [u8]` of the input). The Zig held them by
// value with the `ArrayListUnmanaged` aliased into the reader (raw ptr/len/cap
// triple). In Rust we erase the borrow to `'static` and uphold the same
// invariant the Zig code relied on: the reader never outlives the
// `body_out_str`/`buffer` it was constructed with — both are owned by the
// surrounding `HTTPClient` request lifecycle and the `Decompressor` is dropped
// (or reset to `None`) in `InternalState::deinit` before either buffer is
// freed. All construction goes through `update_buffers`, which is the single
// place the lifetime is erased.
#[derive(Default)]
pub enum Decompressor {
    Zlib(Box<ZlibReaderArrayList<'static>>),
    Brotli(Box<BrotliReaderArrayList<'static>>),
    Zstd(Box<ZstdReaderArrayList<'static>>),
    #[default]
    None,
}

pub(crate) fn has_zlib_header(buffer: &[u8]) -> bool {
    let &[cmf, flg, ..] = buffer else {
        return false;
    };
    (cmf & 0x0f) == 8 && (cmf >> 4) <= 7 && u16::from_be_bytes([cmf, flg]).is_multiple_of(31)
}

/// Erase the lifetimes of an `(input, output)` pair to `'static` for storage
/// in a `*ReaderArrayList` variant.
///
/// # Safety
/// MODULE INVARIANT: the `Decompressor` is owned by the surrounding
/// `HTTPClient` request lifecycle and is dropped (or reset to `None`) in
/// `InternalState::deinit` *before* either `compressed_body` or `body_out_str`
/// is freed. Callers MUST pass exactly that pair so the erased borrows never
/// dangle. The output `Vec` MUST be uniquely borrowed by the active reader
/// (the only other access is the immediate re-seat on the next chunk, which
/// overwrites `list_ptr`).
#[inline(always)]
unsafe fn seat<'a>(
    input: &'a [u8],
    out: &'a mut bun_core::vec::ChanVec<u8>,
) -> (&'static [u8], &'static mut bun_core::vec::ChanVec<u8>) {
    // SAFETY: (`Interned::assume` — Population B, holder-backed) `input` is
    // `InternalState::compressed_body` (or the caller's body chunk), owned by
    // the surrounding `HTTPClient` request and freed in `InternalState::deinit`
    // strictly after the `Decompressor` is dropped/reset. NOT process-lifetime;
    // `assume` makes the holder explicit and grep-able. The output `Vec<u8>` is
    // a `&'static mut` forge — sibling `static-widen-mut` pattern, routed
    // through `detach_lifetime_mut` so the unsafe stays centralised in
    // `bun_ptr`.
    unsafe {
        (
            bun_ptr::Interned::assume(input).as_bytes(),
            bun_ptr::detach_lifetime_mut(out),
        )
    }
}

/// Decompression-bomb guard for response bodies inflated on the HTTP thread:
/// a hostile server must not be able to expand a tiny compressed payload into
/// an unbounded allocation.
const MAX_DECOMPRESSED_BODY_SIZE: usize = 1024 * 1024 * 1024;

impl Decompressor {
    // PORT NOTE: Zig `deinit` called `that.deinit()` on the active reader and
    // reset to `.none`. The boxed readers' `Drop` impls call `end()`, so an
    // explicit `Drop` is unnecessary. Callers that want a mid-lifecycle reset
    // assign `*self = Decompressor::None`.

    // TODO(port): narrow error set
    /// Seats (creating on first call) the streaming decoder for `buffer`.
    /// Returns `false` and creates no decoder while a deflate body start is
    /// too short to tell zlib from raw deflate and the body has not ended —
    /// the caller must keep the bytes and present them again (upstream
    /// faac63e6d4 returns `Ok(0)` from `decompress_chunk` for the same wait).
    pub fn update_buffers(
        &mut self,
        encoding: Encoding,
        buffer: &[u8],
        body_out_str: &mut MutableString,
        is_done: bool,
    ) -> Result<bool, bun_core::Error> {
        if !encoding.is_compressed() {
            return Ok(true);
        }

        if matches!(self, Decompressor::None) {
            // RFC 1950 needs the two-byte CMF/FLG pair to tell zlib-wrapped
            // from raw deflate; a shorter body start with more to come decides
            // nothing. Create no decoder — the caller keeps the bytes.
            if encoding == Encoding::Deflate && buffer.len() < 2 && !is_done {
                return Ok(false);
            }
            // SAFETY: `buffer`/`body_out_str` are the request's compressed_body
            // and caller-owned output; both outlive `self` (see `seat` contract).
            let (input, out) = unsafe { seat(buffer, &mut body_out_str.list) };
            match encoding {
                Encoding::Gzip | Encoding::Deflate => {
                    let mut reader = ZlibReaderArrayList::init_with_options_and_list_allocator(
                        input,
                        out,
                        // PORT NOTE: Zig passed `body_out_str.allocator` and
                        // `bun.http.default_allocator`; dropped per §Allocators.
                        bun_zlib::Options {
                            window_bits: if encoding == Encoding::Gzip {
                                bun_zlib::MAX_WBITS | 16
                            } else if has_zlib_header(buffer) {
                                0
                            } else {
                                -bun_zlib::MAX_WBITS
                            },
                            ..Default::default()
                        },
                    )?;
                    reader.max_output_size = MAX_DECOMPRESSED_BODY_SIZE;
                    *self = Decompressor::Zlib(reader);
                    return Ok(true);
                }
                Encoding::Brotli => {
                    let mut reader = BrotliReaderArrayList::new_with_options(
                        input,
                        out,
                        // PORT NOTE: Zig passed `body_out_str.allocator`; dropped per §Allocators.
                        &Default::default(),
                    )?;
                    reader.max_output_size = MAX_DECOMPRESSED_BODY_SIZE;
                    *self = Decompressor::Brotli(reader);
                    return Ok(true);
                }
                Encoding::Zstd => {
                    let mut reader = ZstdReaderArrayList::init_with_list_allocator(
                        input,
                        out,
                        // PORT NOTE: Zig passed `body_out_str.allocator` and
                        // `bun.http.default_allocator`; dropped per §Allocators.
                    )?;
                    reader.max_output_size = MAX_DECOMPRESSED_BODY_SIZE;
                    *self = Decompressor::Zstd(reader);
                    return Ok(true);
                }
                _ => unreachable!("Invalid encoding. This code should not be reachable"),
            }
        }

        match self {
            Decompressor::Zlib(reader) => {
                // SAFETY: see `seat` contract — same buffer pair as initial seat.
                let (input, out) = unsafe { seat(buffer, &mut body_out_str.list) };
                reader.input = input;
                reader.list_ptr = out;
            }
            Decompressor::Brotli(reader) => {
                let initial = body_out_str.list.len();
                // SAFETY: see `seat` contract — same buffer pair as initial seat.
                let (input, out) = unsafe { seat(buffer, &mut body_out_str.list) };
                reader.input = input;
                reader.total_in = 0;
                // PORT NOTE: Zig aliased the ArrayList header; re-seat list_ptr instead.
                reader.list_ptr = out;
                reader.total_out = initial;
            }
            Decompressor::Zstd(reader) => {
                let initial = body_out_str.list.len();
                // SAFETY: see `seat` contract — same buffer pair as initial seat.
                let (input, out) = unsafe { seat(buffer, &mut body_out_str.list) };
                reader.input = input;
                reader.total_in = 0;
                // PORT NOTE: Zig aliased the ArrayList header; re-seat list_ptr instead.
                reader.list_ptr = out;
                reader.total_out = initial;
            }
            Decompressor::None => {
                unreachable!("Invalid encoding. This code should not be reachable")
            }
        }

        Ok(true)
    }

    // TODO(port): narrow error set
    pub fn read_all(&mut self, is_done: bool) -> Result<(), bun_core::Error> {
        match self {
            Decompressor::Zlib(zlib) => zlib.read_all(is_done)?,
            Decompressor::Brotli(brotli) => brotli.read_all(is_done)?,
            Decompressor::Zstd(reader) => reader.read_all(is_done)?,
            Decompressor::None => {}
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    // Upstream faac63e6d4 (test/js/web/fetch/fetch-gzip.test.ts): with
    // `Content-Encoding: deflate`, the decoder must decide zlib vs raw
    // deflate by the RFC 1950 header — for both decoders — and the result
    // must not depend on how the body splits across reads. One body byte
    // with more to come decides nothing: no decoder is created, the caller
    // keeps the byte and presents it again with the next delivery.
    use super::{Decompressor, has_zlib_header};
    use bun_core::MutableString;
    use bun_http_types::Encoding::Encoding;

    const PLAIN: &[u8] =
        b"deflate autodetect: the same bytes must decode the same way, whole or split.";
    /// RFC 1950 zlib wrapper (`78 9c ..`).
    const ZLIB_BODY: &[u8] = &[
        0x78, 0x9c, 0x45, 0xc9, 0xc1, 0x0d, 0x80, 0x20, 0x10, 0x04, 0xc0, 0x56, 0xb6, 0x00,
        0x63, 0x01, 0x76, 0x73, 0xc2, 0x1a, 0x4c, 0x8e, 0x40, 0xbc, 0x25, 0x84, 0xee, 0xf5,
        0xe7, 0x7c, 0x27, 0xf3, 0x72, 0x13, 0x61, 0x43, 0x2d, 0x53, 0x4c, 0x3a, 0xa0, 0x42,
        0x84, 0x55, 0xe2, 0x5c, 0x62, 0xa0, 0x8e, 0x10, 0x32, 0xd3, 0xf7, 0x7f, 0x4d, 0x5b,
        0x1b, 0x66, 0x69, 0x4e, 0xb4, 0x07, 0xd1, 0xfd, 0xd6, 0xfe, 0x02, 0x3c, 0x56, 0x1b,
        0xbb,
    ];
    /// Bare deflate stream (`45 c9 ..`).
    const RAW_BODY: &[u8] = &[
        0x45, 0xc9, 0xc1, 0x0d, 0x80, 0x20, 0x10, 0x04, 0xc0, 0x56, 0xb6, 0x00, 0x63, 0x01,
        0x76, 0x73, 0xc2, 0x1a, 0x4c, 0x8e, 0x40, 0xbc, 0x25, 0x84, 0xee, 0xf5, 0xe7, 0x7c,
        0x27, 0xf3, 0x72, 0x13, 0x61, 0x43, 0x2d, 0x53, 0x4c, 0x3a, 0xa0, 0x42, 0x84, 0x55,
        0xe2, 0x5c, 0x62, 0xa0, 0x8e, 0x10, 0x32, 0xd3, 0xf7, 0x7f, 0x4d, 0x5b, 0x1b, 0x66,
        0x69, 0x4e, 0xb4, 0x07, 0xd1, 0xfd, 0xd6, 0xfe, 0x02,
    ];
    /// zlib wrapper with an honest 512-byte window (`18 95 ..`, CINFO=1) —
    /// the old `first byte == 120` heuristic misread this as raw deflate.
    const WB9_BODY: &[u8] = &[
        0x18, 0x95, 0x45, 0xc9, 0xc1, 0x0d, 0x80, 0x20, 0x10, 0x04, 0xc0, 0x56, 0xb6, 0x00,
        0x63, 0x01, 0x76, 0x73, 0xc2, 0x1a, 0x4c, 0x8e, 0x40, 0xbc, 0x25, 0x84, 0xee, 0xf5,
        0xe7, 0x7c, 0x27, 0xf3, 0x72, 0x13, 0x61, 0x43, 0x2d, 0x53, 0x4c, 0x3a, 0xa0, 0x42,
        0x84, 0x55, 0xe2, 0x5c, 0x62, 0xa0, 0x8e, 0x10, 0x32, 0xd3, 0xf7, 0x7f, 0x4d, 0x5b,
        0x1b, 0x66, 0x69, 0x4e, 0xb4, 0x07, 0xd1, 0xfd, 0xd6, 0xfe, 0x02, 0x3c, 0x56, 0x1b,
        0xbb,
    ];

    /// Raw deflate whose first byte is 0x78: a stored block with padding bits
    /// set. The `78 e8` pair fails the RFC 1950 check (not a multiple of 31),
    /// so the stream must take the raw path.
    fn raw78_stored_block() -> Vec<u8> {
        let mut v = vec![0x78, 0xE8, 0x00, 0x17, 0xFF];
        v.extend(core::iter::repeat(b'A').take(0xE8));
        v.extend_from_slice(&[0x01, 0x00, 0x00, 0xFF, 0xFF]);
        v
    }

    /// Mirrors `HTTPClient`'s delivery loop. Each network read hands over
    /// `chunk` FRESH bytes; the seat the decompressor sees is everything
    /// accumulated since the previous successful seat (`compressed_body` is
    /// `take`n and cleared on every non-held delivery — the zlib reader's
    /// pending-carry contract expects incremental seats). A held delivery
    /// (too short to classify, body not done) keeps its bytes in the
    /// accumulator and presents them again with the next chunk.
    fn deliver_in(chunks: &[&[u8]]) -> MutableString {
        let mut d = Decompressor::None;
        let mut out = MutableString::init_empty();
        let mut carry: Vec<u8> = Vec::new();
        for (i, chunk) in chunks.iter().enumerate() {
            let done = i + 1 == chunks.len();
            let mut buffer = std::mem::take(&mut carry);
            buffer.extend_from_slice(chunk);
            if !d
                .update_buffers(Encoding::Deflate, &buffer, &mut out, done)
                .unwrap()
            {
                // Held: one byte and more to come — no decoder exists yet;
                // the caller keeps the bytes for the next delivery.
                assert!(!done);
                assert!(matches!(d, Decompressor::None));
                carry = buffer;
                continue;
            }
            // Seat succeeded: the production pipeline takes (clears)
            // `compressed_body` here, so nothing carries into the next seat.
            if let Err(err) = d.read_all(done) {
                // Mid-stream `ShortRead` is the only tolerated error (the
                // production caller ignores it while the body continues).
                assert!(
                    !done,
                    "final read_all failed: {} (chunk {}/{})",
                    err.name(),
                    i + 1,
                    chunks.len()
                );
            }
        }
        out
    }

    #[test]
    fn zlib_header_check_follows_rfc1950() {
        assert!(has_zlib_header(&[0x78, 0x9c])); // standard wrapper
        assert!(has_zlib_header(&[0x78, 0x01])); // FCHECK-only pair
        assert!(has_zlib_header(&[0x18, 0x95])); // CINFO=1: 512-byte window
        assert!(has_zlib_header(&[0x08, 0x1d])); // CINFO=0: 256-byte window
        assert!(!has_zlib_header(&[0x78, 0xe8])); // fails the x31 check
        assert!(!has_zlib_header(&[0xed, 0x8d])); // CM != 8
        assert!(!has_zlib_header(&[0x88, 0x1d])); // CINFO > 7
        assert!(!has_zlib_header(&[0x78])); // one byte decides nothing
        assert!(!has_zlib_header(&[]));
    }

    #[test]
    fn zlib_body_decodes_identically_whole_one_byte_first_and_two_byte_first() {
        let out = deliver_in(&[ZLIB_BODY]);
        assert_eq!(out.list.as_slice(), PLAIN);

        // First read holds ONE byte: held (the old code picked raw deflate
        // from the length and failed); the second network read delivers the
        // REST of the body, and the pair decides.
        let out = deliver_in(&[&ZLIB_BODY[..1], &ZLIB_BODY[1..]]);
        assert_eq!(out.list.as_slice(), PLAIN);

        // First read holds the two header bytes: the decoder is created and
        // consumes them; the rest arrives as an incremental seat.
        let out = deliver_in(&[&ZLIB_BODY[..2], &ZLIB_BODY[2..]]);
        assert_eq!(out.list.as_slice(), PLAIN);
    }

    #[test]
    fn small_window_zlib_body_decodes() {
        // `18 95 ..`: honest 512-byte window. The old first-byte-120
        // heuristic read it as raw deflate and failed on every delivery.
        let out = deliver_in(&[WB9_BODY]);
        assert_eq!(out.list.as_slice(), PLAIN);
        let out = deliver_in(&[&WB9_BODY[..1], &WB9_BODY[1..]]);
        assert_eq!(out.list.as_slice(), PLAIN);
    }

    #[test]
    fn raw_deflate_bodies_decode_and_are_not_misread_as_zlib() {
        let out = deliver_in(&[RAW_BODY]);
        assert_eq!(out.list.as_slice(), PLAIN);

        let raw78 = raw78_stored_block();
        let out = deliver_in(&[&raw78]);
        assert_eq!(out.list.len(), 0xE8);
        assert_eq!(&out.list.as_slice()[..5], b"AAAAA");

        // 0x78 first byte + more to come: `78 e8` fails the RFC 1950 check,
        // so the wait resolves to the raw path.
        let out = deliver_in(&[&raw78[..1], &raw78[1..]]);
        assert_eq!(out.list.len(), 0xE8);
    }

    #[test]
    fn one_byte_deflate_body_that_ends_fails_rather_than_waits() {
        // Done at one byte: classified raw, truncated stream -> error
        // (upstream keeps this failing; Node resolves an empty body).
        let mut d = Decompressor::None;
        let mut out = MutableString::init_empty();
        assert!(
            d.update_buffers(Encoding::Deflate, &[0x78], &mut out, true)
                .unwrap()
        );
        assert!(d.read_all(true).is_err());
    }
}

// ported from: src/http/Decompressor.zig
