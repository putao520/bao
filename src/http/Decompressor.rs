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
unsafe fn seat<'a>(input: &'a [u8], out: &'a mut Vec<u8>) -> (&'static [u8], &'static mut Vec<u8>) {
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
    pub fn update_buffers(
        &mut self,
        encoding: Encoding,
        buffer: &[u8],
        body_out_str: &mut MutableString,
    ) -> Result<(), bun_core::Error> {
        if !encoding.is_compressed() {
            return Ok(());
        }

        if matches!(self, Decompressor::None) {
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
                            // zlib.MAX_WBITS = 15
                            // to (de-)compress deflate format, use wbits = -zlib.MAX_WBITS
                            // to (de-)compress deflate format with headers we use wbits = 0 (we can detect the first byte using 120)
                            // to (de-)compress gzip format, use wbits = zlib.MAX_WBITS | 16
                            window_bits: if encoding == Encoding::Gzip {
                                bun_zlib::MAX_WBITS | 16
                            } else if buffer.len() > 1 && buffer[0] == 120 {
                                0
                            } else {
                                -bun_zlib::MAX_WBITS
                            },
                            ..Default::default()
                        },
                    )?;
                    reader.max_output_size = MAX_DECOMPRESSED_BODY_SIZE;
                    *self = Decompressor::Zlib(reader);
                    return Ok(());
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
                    return Ok(());
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
                    return Ok(());
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

        Ok(())
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

// ported from: src/http/Decompressor.zig
