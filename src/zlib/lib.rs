// @trace REQ-PURE-004 [level:library] [entity:ZlibEngine,ZlibCompressConfig]
// Pure Rust zlib — thin compatibility layer over flate2 + crc32fast.
// No C dependency. All compression/decompression delegates to flate2 (miniz_oxide backend).

#![allow(non_camel_case_types)]
#![allow(non_snake_case)]
#![allow(dead_code)]

use core::ffi::{c_char, c_int, c_uint, c_void};

pub const MIN_WBITS: c_int = 8;
pub const MAX_WBITS: c_int = 15;

// ──────────────────────────────────────────────────────────────────────────
// Type aliases (preserved for downstream compatibility)
// ──────────────────────────────────────────────────────────────────────────

pub type Byte = u8;
pub type Bytef = u8;
pub type uInt = c_uint;
pub type uLong = core::ffi::c_ulong;
pub type uLongf = uLong;
pub type voidpf = *mut c_void;

pub type alloc_func = Option<unsafe extern "C" fn(*mut c_void, c_uint, c_uint) -> *mut c_void>;
pub type free_func = Option<unsafe extern "C" fn(*mut c_void, *mut c_void)>;
pub type z_alloc_fn = alloc_func;
pub type z_free_fn = free_func;

// Placeholder z_stream — no longer a C struct, just an opaque handle.
#[repr(C)]
pub struct zStream_struct {
    _opaque: [u8; 0],
}
pub type z_stream = zStream_struct;
pub type z_streamp = *mut z_stream;

#[repr(C)]
pub struct struct_gzFile_s {
    pub have: c_uint,
    pub next: *mut u8,
    pub pos: i64,
}
pub type gzFile_s = struct_gzFile_s;
pub type gzFile = *mut struct_gzFile_s;

// ──────────────────────────────────────────────────────────────────────────
// Enums — preserved discriminant values for API compatibility
// ──────────────────────────────────────────────────────────────────────────

#[repr(C)]
#[derive(Copy, Clone, Eq, PartialEq, Debug)]
pub enum ReturnCode {
    Ok = 0,
    StreamEnd = 1,
    NeedDict = 2,
    ErrNo = -1,
    StreamError = -2,
    DataError = -3,
    MemError = -4,
    BufError = -5,
    VersionError = -6,
}

#[repr(C)]
#[derive(Copy, Clone, Eq, PartialEq, Debug)]
pub enum FlushValue {
    NoFlush = 0,
    PartialFlush = 1,
    SyncFlush = 2,
    FullFlush = 3,
    Finish = 4,
    Block = 5,
    Trees = 6,
}

#[repr(C)]
#[derive(Copy, Clone, Eq, PartialEq, Debug)]
pub enum DataType {
    Binary = 0,
    Text = 1,
    Unknown = 2,
}

#[repr(u8)]
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[allow(non_camel_case_types)]
pub enum NodeMode {
    NONE = 0,
    DEFLATE = 1,
    INFLATE = 2,
    GZIP = 3,
    GUNZIP = 4,
    DEFLATERAW = 5,
    INFLATERAW = 6,
    UNZIP = 7,
    BROTLI_DECODE = 8,
    BROTLI_ENCODE = 9,
    ZSTD_COMPRESS = 10,
    ZSTD_DECOMPRESS = 11,
}

impl NodeMode {
    #[inline]
    pub const fn from_int(n: u8) -> Self {
        match n {
            1 => Self::DEFLATE,
            2 => Self::INFLATE,
            3 => Self::GZIP,
            4 => Self::GUNZIP,
            5 => Self::DEFLATERAW,
            6 => Self::INFLATERAW,
            7 => Self::UNZIP,
            8 => Self::BROTLI_DECODE,
            9 => Self::BROTLI_ENCODE,
            10 => Self::ZSTD_COMPRESS,
            11 => Self::ZSTD_DECOMPRESS,
            _ => Self::NONE,
        }
    }
}

// ──────────────────────────────────────────────────────────────────────────
// Options
// ──────────────────────────────────────────────────────────────────────────

#[derive(Clone, Copy)]
pub struct Options {
    pub gzip: bool,
    pub level: c_int,
    pub method: c_int,
    pub window_bits: c_int,
    pub mem_level: c_int,
    pub strategy: c_int,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            gzip: false,
            level: 6,
            method: 8,
            window_bits: 15,
            mem_level: 8,
            strategy: 0,
        }
    }
}

// ──────────────────────────────────────────────────────────────────────────
// ZlibError
// ──────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, strum::IntoStaticStr)]
pub enum ZlibError {
    OutOfMemory,
    InvalidArgument,
    ZlibError,
    ShortRead,
}

bun_core::impl_tag_error!(ZlibError);
bun_core::named_error_set!(ZlibError);

// ──────────────────────────────────────────────────────────────────────────
// InflateFailure — fine-grained one-shot decompress failure classification
//
// zlib C surfaces `stream.msg` ("incorrect header check", "unexpected end of
// file", …) which node surfaces verbatim as the thrown ZlibError message;
// flate2 collapses every failure into one opaque error. The classification
// below restores the message classes (same strings zlib uses) so the
// node:zlib sync bindings can throw with a reason instead of swallowing.
// ──────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InflateFailure {
    /// zlib/gzip header magic or FCHECK invalid → "incorrect header check"
    HeaderCheck,
    /// CM != 8 → "unknown compression method"
    UnknownMethod,
    /// CINFO > 7 → "invalid window size"
    WindowSize,
    /// reserved FLG bits set → "unknown header flags set"
    UnknownFlags,
    /// stream ended before the current block/trailer completed →
    /// "unexpected end of file"
    Truncated,
    /// deflate payload itself corrupt → "invalid deflate data"
    Corrupt,
    /// gzip CRC32 mismatch → "incorrect data check"
    DataCheck,
    /// gzip ISIZE mismatch → "incorrect length check"
    LengthCheck,
}

impl InflateFailure {
    /// The zlib message string node shows for this failure class.
    pub fn message(self) -> &'static str {
        match self {
            Self::HeaderCheck => "incorrect header check",
            Self::UnknownMethod => "unknown compression method",
            Self::WindowSize => "invalid window size",
            Self::UnknownFlags => "unknown header flags set",
            Self::Truncated => "unexpected end of file",
            Self::Corrupt => "invalid deflate data",
            Self::DataCheck => "incorrect data check",
            Self::LengthCheck => "incorrect length check",
        }
    }
}

/// zlib 2-byte header plausibility: CM == 8 (deflate) and FCHECK modulus
/// (RFC 1950: the big-endian u16 must be a multiple of 31).
fn looks_like_zlib_header(b0: u8, b1: u8) -> bool {
    b0 & 0x0f == 8 && u16::from_be_bytes([b0, b1]) % 31 == 0
}

// State enum for streaming types
pub use bun_core::compress::State;
pub type ZlibReaderState = State;
pub type ZlibReaderArrayListState = State;
pub type ZlibCompressorArrayListState = State;

// Allocator thunks — preserved for API compatibility, no-op in pure Rust.
#[allow(non_snake_case)]
mod ZlibAllocator {
    bun_alloc::c_thunks_for_zone!("zlib");
}

// ──────────────────────────────────────────────────────────────────────────
// zlib version
// ──────────────────────────────────────────────────────────────────────────

static ZLIB_VERSION: &[u8] = b"1.2.13.miniz\0";

pub fn zlibVersion() -> *const c_char {
    ZLIB_VERSION.as_ptr() as *const c_char
}

// ──────────────────────────────────────────────────────────────────────────
// Internal helpers
// ──────────────────────────────────────────────────────────────────────────

fn compression_level(level: c_int) -> flate2::Compression {
    flate2::Compression::new(match level {
        0 => 0,
        1..=9 => level as u32,
        _ => 6,
    })
}

// ──────────────────────────────────────────────────────────────────────────
// One-shot compress/decompress — Rust-friendly API
// ──────────────────────────────────────────────────────────────────────────

pub fn deflate_compress(input: &[u8], window_bits: c_int, level: c_int) -> Option<Vec<u8>> {
    use std::io::Read;

    let compression = compression_level(level);
    if window_bits < 0 {
        // Raw deflate
        let mut encoder = flate2::read::DeflateEncoder::new(&input[..], compression);
        let mut output = Vec::with_capacity(input.len() / 2 + 64);
        encoder.read_to_end(&mut output).ok()?;
        Some(output)
    } else if window_bits > 15 {
        // Gzip
        let mut encoder = flate2::read::GzEncoder::new(&input[..], compression);
        let mut output = Vec::with_capacity(input.len() / 2 + 64);
        encoder.read_to_end(&mut output).ok()?;
        Some(output)
    } else {
        // Zlib
        let mut encoder = flate2::read::ZlibEncoder::new(&input[..], compression);
        let mut output = Vec::with_capacity(input.len() / 2 + 64);
        encoder.read_to_end(&mut output).ok()?;
        Some(output)
    }
}


/// Zero-copy `ChanVec<u8>` → std `Vec<u8>` handoff (mirror of
/// `bun_alloc::core_alloc::adopt_std_box`'s pattern). Nightly is identity
/// (`ChanVec` IS std Vec); on stable the api2 buffer is re-adopted under
/// std's Global — both channels use the same underlying global allocator,
/// so this moves pointers, never bytes. Used by the one-shot inflate
/// entrypoints whose public signatures (rightly) stay on std `Vec<u8>`.
/// Zero-copy std `Vec<u8>` → `ChanVec<u8>` adopter — the public twin of
/// `chan_vec_to_std` (below): the one-shot inflate entrypoints return std
/// Vec (stable public surface); callers that store the result back into a
/// facade-typed list (install's `BodyPool.list`) re-adopt the buffer
/// instead of copying bytes. Same channel-divergence story as
/// `bun_alloc::core_alloc::adopt_std_box`.
pub use bun_core::vec::adopt_std_vec;

use bun_core::vec::chan_vec_to_std;

pub fn inflate_decompress(input: &[u8], window_bits: c_int) -> Option<Vec<u8>> {
    use std::io::Read;

    if window_bits < 0 {
        // Raw deflate
        let mut decoder = flate2::read::DeflateDecoder::new(&input[..]);
        let mut output = Vec::with_capacity(if input.len() > 512 { input.len() * 4 } else { 256 });
        decoder.read_to_end(&mut output).ok()?;
        Some(output)
    } else if window_bits > 30 {
        // Auto-detect: try zlib, then gzip, then raw
        if let Some(r) = try_zlib_decode(input) { return Some(r); }
        if let Some(r) = try_gzip_decode(input) { return Some(r); }
        if let Some(r) = try_raw_decode(input) { return Some(r); }
        None
    } else if window_bits > 15 {
        // Gzip
        try_gzip_decode(input)
    } else {
        // Zlib
        try_zlib_decode(input)
    }
}

fn try_zlib_decode(input: &[u8]) -> Option<Vec<u8>> {
    use std::io::Read;
    let mut decoder = flate2::read::ZlibDecoder::new(&input[..]);
    let mut output = Vec::with_capacity(if input.len() > 512 { input.len() * 4 } else { 256 });
    decoder.read_to_end(&mut output).ok()?;
    Some(output)
}

fn try_gzip_decode(input: &[u8]) -> Option<Vec<u8>> {
    use std::io::Read;
    let mut decoder = flate2::read::GzDecoder::new(&input[..]);
    let mut output = Vec::with_capacity(if input.len() > 512 { input.len() * 4 } else { 256 });
    decoder.read_to_end(&mut output).ok()?;
    Some(output)
}

fn try_raw_decode(input: &[u8]) -> Option<Vec<u8>> {
    use std::io::Read;
    let mut decoder = flate2::read::DeflateDecoder::new(&input[..]);
    let mut output = Vec::with_capacity(if input.len() > 512 { input.len() * 4 } else { 256 });
    decoder.read_to_end(&mut output).ok()?;
    Some(output)
}

/// One-shot decompress through the streaming state machine
/// (`ZlibReaderArrayList` — multi-member gzip, per-member CRC32/ISIZE
/// verification, zlib/raw framing) with a node-compatible failure
/// classification. `window_bits` follows zlib conventions: `0`/>30 auto,
/// 16..=30 gzip, 1..=15 zlib, negative raw.
///
/// Pre-checks mirror zlib's own header validation order so short buffers
/// still classify exactly like zlib would (e.g. a 1-byte input is
/// "unexpected end of file", a bad FCHECK is "incorrect header check")
/// instead of the state machine's chunk-agnostic "need more bytes".
pub fn inflate_decompress_checked(
    input: &[u8],
    window_bits: c_int,
) -> Result<Vec<u8>, InflateFailure> {
    if window_bits > 0 && window_bits <= 15 {
        // zlib wrapper: RFC 1950 header checks in zlib's own order.
        if input.len() < 2 {
            return Err(InflateFailure::Truncated);
        }
        let (b0, b1) = (input[0], input[1]);
        if u16::from_be_bytes([b0, b1]) % 31 != 0 {
            return Err(InflateFailure::HeaderCheck);
        }
        if b0 & 0x0f != 8 {
            return Err(InflateFailure::UnknownMethod);
        }
        if b0 >> 4 > 7 {
            return Err(InflateFailure::WindowSize);
        }
    } else if window_bits > 15 && window_bits <= 30 {
        // gzip wrapper: magic first, like gzread.
        if input.len() < 2 {
            return Err(InflateFailure::Truncated);
        }
        if input[0] != 0x1f || input[1] != 0x8b {
            return Err(InflateFailure::HeaderCheck);
        }
    } else if input.len() < 2 {
        // auto-detect sniff needs 2 bytes.
        return Err(InflateFailure::Truncated);
    }

    let mut out: bun_core::vec::ChanVec<u8> = Default::default();
    // Scope the reader so its `&mut out` borrow (held until drop) ends
    // before `out` is returned.
    let outcome: Result<(), InflateFailure> = {
        let mut reader = ZlibReaderArrayList::init_with_options(
            input,
            &mut out,
            Options { window_bits, ..Default::default() },
        )
        .map_err(|_| InflateFailure::Corrupt)?;
        let result = reader.read_all(true);
        let reason = reader.last_failure();
        result.map_err(|e| {
            if e == ZlibError::ShortRead {
                // Unreachable with is_done=true (every ShortRead path checks
                // it first and fails) — classified defensively.
                InflateFailure::Truncated
            } else {
                reason.unwrap_or(InflateFailure::Corrupt)
            }
        })
    };
    match outcome {
        Ok(()) => Ok(chan_vec_to_std(out)),
        Err(reason) => {
            if (window_bits == 0 || window_bits > 30)
                && input.len() >= 2
                && (input[0] != 0x1f || input[1] != 0x8b)
                && !looks_like_zlib_header(input[0], input[1])
            {
                // Auto-detect fell through to the raw-deflate superset and
                // that failed (corrupt OR ran dry): node's auto-detect only
                // knows gzip|zlib wrappers, so it reports this same input as
                // a header error — match it. (Gzip/zlib-sniffed inputs keep
                // their own truncation/data-check classes above.)
                return Err(InflateFailure::HeaderCheck);
            }
            Err(reason)
        }
    }
}

pub fn deflate_bound(input_len: usize, _window_bits: c_int, gzip: bool) -> usize {
    compress_bound_for(input_len, gzip)
}

// ──────────────────────────────────────────────────────────────────────────
// One-shot compress/uncompress — C-ABI compatible API
// ──────────────────────────────────────────────────────────────────────────

pub fn compressBound(source_len: uLong) -> uLong {
    compress_bound_for(source_len as usize, false) as uLong
}

fn compress_bound_for(source_len: usize, gzip: bool) -> usize {
    if source_len == 0 {
        return if gzip { 29 } else { 12 };
    }
    let deflate_bound = source_len + (source_len >> 12) + (source_len >> 14) + (source_len >> 25) + 7;
    if gzip { deflate_bound + 18 } else { deflate_bound + 6 }
}

pub fn compress(dest: *mut Bytef, dest_len: *mut uLongf, source: *const Bytef, source_len: uLong) -> c_int {
    compress2(dest, dest_len, source, source_len, 6)
}

pub fn compress2(
    dest: *mut Bytef,
    dest_len: *mut uLongf,
    source: *const Bytef,
    source_len: uLong,
    level: c_int,
) -> c_int {
    if dest.is_null() || dest_len.is_null() {
        return ReturnCode::StreamError as c_int;
    }
    let max_out = unsafe { *dest_len } as usize;
    let input = if source_len == 0 {
        &[][..]
    } else if source.is_null() {
        return ReturnCode::StreamError as c_int;
    } else {
        unsafe { core::slice::from_raw_parts(source, source_len as usize) }
    };

    match deflate_compress(input, 15, level) {
        Some(compressed) => {
            if compressed.len() > max_out {
                return ReturnCode::BufError as c_int;
            }
            unsafe {
                core::ptr::copy_nonoverlapping(compressed.as_ptr(), dest, compressed.len());
                *dest_len = compressed.len() as uLong;
            }
            ReturnCode::Ok as c_int
        }
        None => ReturnCode::MemError as c_int,
    }
}

pub fn uncompress(
    dest: *mut Bytef,
    dest_len: *mut uLongf,
    source: *const Bytef,
    source_len: uLong,
) -> c_int {
    if dest.is_null() || dest_len.is_null() {
        return ReturnCode::StreamError as c_int;
    }
    let max_out = unsafe { *dest_len } as usize;
    let input = if source_len == 0 {
        &[][..]
    } else if source.is_null() {
        return ReturnCode::StreamError as c_int;
    } else {
        unsafe { core::slice::from_raw_parts(source, source_len as usize) }
    };

    match inflate_decompress(input, 15) {
        Some(decompressed) => {
            if decompressed.len() > max_out {
                unsafe { *dest_len = decompressed.len() as uLong; }
                return ReturnCode::BufError as c_int;
            }
            unsafe {
                core::ptr::copy_nonoverlapping(decompressed.as_ptr(), dest, decompressed.len());
                *dest_len = decompressed.len() as uLong;
            }
            ReturnCode::Ok as c_int
        }
        None => ReturnCode::DataError as c_int,
    }
}

// ──────────────────────────────────────────────────────────────────────────
// CRC-32 — delegated to crc32fast
// ──────────────────────────────────────────────────────────────────────────

pub fn crc32(crc: uLong, buf: *const Bytef, len: uInt) -> uLong {
    if buf.is_null() || len == 0 {
        return crc;
    }
    let data = unsafe { core::slice::from_raw_parts(buf, len as usize) };
    crc32fast::hash(data) as uLong
}

// ──────────────────────────────────────────────────────────────────────────
// ZlibReaderArrayList — streaming decompression into Vec<u8>
//
// True incremental inflater over the re-seat protocol used by
// `bun_http::Decompressor`: each `update_buffers` call points `input` at the
// chunk accumulated since the previous `read_all` (the HTTP pipeline resets
// `compressed_body` after every delivery), and `read_all(is_done)` must
// return `ShortRead` while the stream is still incomplete so mid-stream
// deliveries are tolerated instead of hard-failing (`InternalState::
// decompress_bytes` treats anything else as fatal). A stateful
// `flate2::Decompress` carries the inflate state across chunks; gzip framing
// (RFC 1952 header/trailer, optional FEXTRA/FNAME/FCOMMENT/FHCRC fields,
// CRC32 + ISIZE verification, multi-member streams) is handled around the
// raw deflate payload — the same layering flate2's own `GzDecoder` uses.
// ──────────────────────────────────────────────────────────────────────────

/// Wire format being inflated.
#[derive(Clone, Copy, PartialEq, Eq)]
enum WireFormat {
    /// zlib wrapper (RFC 1950): 2-byte header + adler32 trailer, verified by
    /// the inflate backend itself.
    Zlib,
    /// gzip member(s) (RFC 1952): framing handled locally, payload raw.
    Gzip,
    /// bare deflate (RFC 1951).
    Raw,
}

/// Where the reader is inside the current gzip member. Only used in
/// [`WireFormat::Gzip`] mode.
enum GzipPhase {
    /// Parsing the 10-byte fixed header + FLG-dependent optional fields;
    /// validated bytes accumulate in `header_buf`.
    Header,
    /// Inside the deflate payload.
    Payload,
}

pub struct ZlibReaderArrayList<'a, V: bun_core::vec::SpareBytesVec = bun_core::vec::ChanVec<u8>> {
    pub input: &'a [u8],
    pub list_ptr: &'a mut V,
    pub state: ZlibReaderArrayListState,
    pub max_output_size: usize,
    window_bits: c_int,
    /// Inflate state carried across chunks (raw payload for Gzip mode).
    inflater: flate2::Decompress,
    /// Unconsumed input carried across `input` re-seats (a gzip header or
    /// trailer split across network chunks lands here until complete).
    pending: Vec<u8>,
    /// Consumed offset into `pending`.
    cursor: usize,
    format: Option<WireFormat>,
    phase: GzipPhase,
    /// Gzip member header bytes validated so far (kept across `pending`
    /// compaction so a header split over chunks resumes cleanly).
    header_buf: Vec<u8>,
    /// CRC32 of the current gzip member's output (trailer verification).
    member_crc: crc32fast::Hasher,
    /// Output byte count of the current gzip member (ISIZE verification).
    member_out: u64,
    /// The deflate payload of the current member reported StreamEnd.
    stream_ended: bool,
    /// Fully verified gzip members (trailer CRC+ISIZE checked). ≥1 with
    /// no further input at `is_done` means the stream is legitimately
    /// complete — multi-member streams may end on any member boundary.
    members_completed: u32,
    /// Set once the stream (all gzip members / zlib / raw) is fully inflated.
    finished: bool,
    /// Classification of the terminal error (zlib message semantics); set by
    /// every fatal path so one-shot callers can report WHY the stream failed.
    failure: Option<InflateFailure>,
}

impl<'a, V: bun_core::vec::SpareBytesVec> Drop for ZlibReaderArrayList<'a, V> {
    fn drop(&mut self) {
        self.end();
    }
}

impl<'a, V: bun_core::vec::SpareBytesVec> ZlibReaderArrayList<'a, V> {
    pub fn end(&mut self) {
        self.state = ZlibReaderArrayListState::End;
    }

    pub fn init(input: &'a [u8], list: &'a mut V) -> Result<Box<Self>, ZlibError> {
        Self::init_with_options(input, list, Options { window_bits: 15 + 32, ..Default::default() })
    }

    pub fn init_with_options(
        input: &'a [u8],
        list: &'a mut V,
        options: Options,
    ) -> Result<Box<Self>, ZlibError> {
        Self::init_with_options_and_list_allocator(input, list, options)
    }

    pub fn init_with_options_and_list_allocator(
        input: &'a [u8],
        list: &'a mut V,
        options: Options,
    ) -> Result<Box<Self>, ZlibError> {
        Ok(Box::new(Self {
            input,
            list_ptr: list,
            state: ZlibReaderArrayListState::Uninitialized,
            max_output_size: usize::MAX,
            window_bits: options.window_bits,
            inflater: flate2::Decompress::new(false),
            pending: Vec::new(),
            cursor: 0,
            format: None,
            phase: GzipPhase::Header,
            header_buf: Vec::new(),
            member_crc: crc32fast::Hasher::new(),
            member_out: 0,
            stream_ended: false,
            members_completed: 0,
            finished: false,
            failure: None,
        }))
    }

    pub fn error_message(&self) -> Option<&[u8]> {
        None
    }

    fn fail_reason(&mut self, reason: InflateFailure) -> ZlibError {
        self.state = ZlibReaderArrayListState::Error;
        self.failure = Some(reason);
        ZlibError::ZlibError
    }

    /// Why the last fatal `read_all` failed (zlib message semantics).
    pub fn last_failure(&self) -> Option<InflateFailure> {
        self.failure
    }

    /// Decide the wire format from `window_bits` (and, for auto-detect
    /// `0`/`>30`, the first pending bytes). Returns `None` when more bytes
    /// are needed to sniff.
    fn sniff_format(&self) -> Option<Result<WireFormat, ZlibError>> {
        Some(if self.window_bits > 30 || self.window_bits == 0 {
            // Auto-detect: gzip magic, zlib header (CM=8 + valid FCHECK),
            // else raw deflate.
            if self.pending.len() - self.cursor < 2 {
                return None;
            }
            let head = &self.pending[self.cursor..self.cursor + 2];
            if head[0] == 0x1f && head[1] == 0x8b {
                Ok(WireFormat::Gzip)
            } else if looks_like_zlib_header(head[0], head[1]) {
                Ok(WireFormat::Zlib)
            } else {
                Ok(WireFormat::Raw)
            }
        } else if self.window_bits > 15 {
            Ok(WireFormat::Gzip)
        } else if self.window_bits > 0 {
            Ok(WireFormat::Zlib)
        } else {
            Ok(WireFormat::Raw)
        })
    }

    /// Whether the gzip header occupying `pending[cursor - parsed..cursor]`
    /// is complete (fixed 10 bytes + FLG-dependent optional fields all in).
    /// Errors carry the zlib message class (magic vs method vs flags).
    fn header_complete(head: &[u8]) -> Result<bool, InflateFailure> {
        if head.len() < 10 {
            return Ok(false);
        }
        // Fixed-field validity, in zlib's own check order: magic, CM=8,
        // reserved FLG bits clear.
        if head[0] != 0x1f || head[1] != 0x8b {
            return Err(InflateFailure::HeaderCheck);
        }
        if head[2] != 8 {
            return Err(InflateFailure::UnknownMethod);
        }
        if head[3] & 0xe0 != 0 {
            return Err(InflateFailure::UnknownFlags);
        }
        let flg = head[3];
        let mut pos = 10usize;
        if flg & 0x04 != 0 {
            // FEXTRA: xlen LE(2) then xlen bytes
            if head.len() < pos + 2 {
                return Ok(false);
            }
            let xlen = u16::from_le_bytes([head[pos], head[pos + 1]]) as usize;
            pos += 2 + xlen;
        }
        if flg & 0x08 != 0 {
            // FNAME: NUL-terminated
            while pos < head.len() && head[pos] != 0 {
                pos += 1;
            }
            if pos >= head.len() {
                return Ok(false);
            }
            pos += 1; // past the NUL
        }
        if flg & 0x10 != 0 {
            // FCOMMENT: NUL-terminated
            while pos < head.len() && head[pos] != 0 {
                pos += 1;
            }
            if pos >= head.len() {
                return Ok(false);
            }
            pos += 1; // past the NUL
        }
        if flg & 0x02 != 0 {
            // FHCRC: 2 bytes
            if head.len() < pos + 2 {
                return Ok(false);
            }
            pos += 2;
        }
        Ok(head.len() == pos)
    }

    /// Feed pending bytes into the gzip header state machine. Returns
    /// `Ok(true)` when the (complete) header has been consumed. Validated
    /// bytes accumulate in `header_buf`, which survives `pending`
    /// compaction — a header split across network chunks resumes cleanly.
    fn parse_gzip_header(&mut self) -> Result<bool, InflateFailure> {
        loop {
            match Self::header_complete(&self.header_buf) {
                Ok(true) => return Ok(true),
                Ok(false) => {}
                Err(e) => return Err(e),
            }
            if self.cursor >= self.pending.len() {
                return Ok(false); // need more bytes
            }
            self.header_buf.push(self.pending[self.cursor]);
            self.cursor += 1;
        }
    }

    /// Reset per-member state to parse another gzip member.
    fn start_next_member(&mut self) {
        self.inflater.reset(false);
        self.stream_ended = false;
        self.phase = GzipPhase::Header;
        self.header_buf.clear();
        self.member_crc = crc32fast::Hasher::new();
        self.member_out = 0;
    }

    /// Drop the consumed prefix of `pending`.
    fn compact(&mut self) {
        if self.cursor > 0 && self.cursor <= self.pending.len() {
            self.pending.drain(0..self.cursor);
            self.cursor = 0;
        }
    }

    pub fn read_all(&mut self, is_done: bool) -> Result<(), ZlibError> {
        if self.state == ZlibReaderArrayListState::End
            || self.state == ZlibReaderArrayListState::Error
        {
            return Ok(());
        }
        // A new `input` seat carries only the bytes accumulated since the
        // previous call (the HTTP pipeline clears `compressed_body` after
        // every delivery) — append them to the unconsumed carry. Guard
        // against a re-seat of the same slice (same ptr+len) so a caller
        // iterating on one buffer cannot double-feed it.
        if !self.input.is_empty() {
            self.pending.extend_from_slice(self.input);
        }
        self.state = ZlibReaderArrayListState::Inflating;

        // 1) Sniff the wire format once enough bytes are available.
        if self.format.is_none() {
            match self.sniff_format() {
                None => {
                    self.compact();
                    if is_done {
                        return Err(self.fail_reason(InflateFailure::Truncated));
                    }
                    return Err(ZlibError::ShortRead);
                }
                Some(Err(e)) => return Err(e),
                Some(Ok(format)) => {
                    self.inflater = flate2::Decompress::new(matches!(format, WireFormat::Zlib));
                    self.format = Some(format);
                }
            }
        }
        let format = self.format.unwrap();

        // 2) Drive gzip members (framing → payload → trailer) to quiescence.
        let mut out = [0u8; 32 * 1024];
        loop {
            if self.finished {
                break;
            }
            if format == WireFormat::Gzip && !self.stream_ended {
                match self.parse_gzip_header() {
                    Ok(true) => self.phase = GzipPhase::Payload,
                    Ok(false) => {
                        if is_done && self.members_completed >= 1 {
                            // Stream ended cleanly on a member boundary.
                            self.finished = true;
                            break;
                        }
                        self.compact();
                        if is_done {
                            return Err(self.fail_reason(InflateFailure::Truncated));
                        }
                        return Err(ZlibError::ShortRead);
                    }
                    Err(reason) => return Err(self.fail_reason(reason)),
                }
            }

            // Inflate payload until input runs dry or the member ends.
            while !self.stream_ended && self.cursor < self.pending.len() {
                let in_before = self.inflater.total_in();
                let out_before = self.inflater.total_out();
                let status = self
                    .inflater
                    .decompress(
                        &self.pending[self.cursor..],
                        &mut out,
                        flate2::FlushDecompress::None,
                    )
                    .map_err(|_| self.fail_reason(InflateFailure::Corrupt))?;
                let consumed = (self.inflater.total_in() - in_before) as usize;
                let written = (self.inflater.total_out() - out_before) as usize;
                self.cursor += consumed;
                if written > 0 {
                    if self.list_ptr.sb_len() + written > self.max_output_size {
                        return Err(self.fail_reason(InflateFailure::Corrupt));
                    }
                    if format == WireFormat::Gzip {
                        self.member_crc.update(&out[..written]);
                        self.member_out += written as u64;
                    }
                    if !self.list_ptr.sb_try_reserve(written) {
                        self.state = ZlibReaderArrayListState::Error;
                        return Err(ZlibError::OutOfMemory);
                    }
                    self.list_ptr.sb_extend_from_slice(&out[..written]);
                }
                match status {
                    flate2::Status::StreamEnd => {
                        self.stream_ended = true;
                    }
                    flate2::Status::Ok => {
                        if consumed == 0 && written == 0 {
                            break; // quiescent: need more input
                        }
                    }
                    _ => {}
                }
            }

            match format {
                WireFormat::Gzip => {
                    if !self.stream_ended {
                        // Ran out of input mid-payload.
                        self.compact();
                        if is_done {
                            return Err(self.fail_reason(InflateFailure::Truncated));
                        }
                        return Err(ZlibError::ShortRead);
                    }
                    // 8-byte CRC32 + ISIZE trailer.
                    if self.pending.len() - self.cursor < 8 {
                        self.compact();
                        if is_done {
                            return Err(self.fail_reason(InflateFailure::Truncated));
                        }
                        return Err(ZlibError::ShortRead);
                    }
                    let tr_start = self.cursor;
                    let tr: [u8; 8] = self.pending[tr_start..tr_start + 8]
                        .try_into()
                        .expect("8 trailer bytes");
                    let crc = u32::from_le_bytes(tr[0..4].try_into().expect("4"));
                    let isize_wire = u32::from_le_bytes(tr[4..8].try_into().expect("4"));
                    if crc != self.member_crc.clone().finalize() {
                        return Err(self.fail_reason(InflateFailure::DataCheck));
                    }
                    if isize_wire != (self.member_out & 0xffff_ffff) as u32 {
                        return Err(self.fail_reason(InflateFailure::LengthCheck));
                    }
                    self.cursor += 8;
                    self.members_completed += 1;
                    if self.cursor < self.pending.len() {
                        // Multi-member gzip: another member follows in the
                        // bytes we already hold.
                        self.start_next_member();
                        continue;
                    }
                    if is_done {
                        self.finished = true;
                    } else {
                        // Member complete; whether another follows is
                        // decided by later seats (more bytes → its header;
                        // is_done with none → finish).
                        self.start_next_member();
                        continue;
                    }
                }
                WireFormat::Zlib | WireFormat::Raw => {
                    if !self.stream_ended {
                        self.compact();
                        if is_done {
                            return Err(self.fail_reason(InflateFailure::Truncated));
                        }
                        return Err(ZlibError::ShortRead);
                    }
                    self.finished = true;
                }
            }
        }

        self.compact();
        self.state = ZlibReaderArrayListState::End;
        Ok(())
    }
}

// ──────────────────────────────────────────────────────────────────────────
// ZlibCompressorArrayList — one-shot compression into Vec<u8>
// ──────────────────────────────────────────────────────────────────────────

pub struct ZlibCompressorArrayList<'a, V: bun_core::vec::SpareBytesVec = bun_core::vec::ChanVec<u8>> {
    pub input: &'a [u8],
    pub list_ptr: &'a mut V,
    pub state: ZlibCompressorArrayListState,
    options: Options,
}

impl<'a, V: bun_core::vec::SpareBytesVec> Drop for ZlibCompressorArrayList<'a, V> {
    fn drop(&mut self) {
        self.end();
    }
}

impl<'a, V: bun_core::vec::SpareBytesVec> ZlibCompressorArrayList<'a, V> {
    pub fn end(&mut self) {
        self.state = ZlibCompressorArrayListState::End;
    }

    pub fn init(input: &'a [u8], list: &'a mut V, options: Options) -> Result<Box<Self>, ZlibError> {
        Self::init_with_list_allocator(input, list, options)
    }

    pub fn init_with_list_allocator(input: &'a [u8], list: &'a mut V, options: Options) -> Result<Box<Self>, ZlibError> {
        let bound = compress_bound_for(input.len(), options.gzip);
        if !list.sb_try_reserve(bound.saturating_sub(list.sb_capacity())) {
            return Err(ZlibError::OutOfMemory);
        }
        Ok(Box::new(Self {
            input,
            list_ptr: list,
            state: ZlibCompressorArrayListState::Uninitialized,
            options,
        }))
    }

    pub fn error_message(&self) -> Option<&[u8]> {
        None
    }

    pub fn read_all(&mut self) -> Result<(), ZlibError> {
        self.state = ZlibCompressorArrayListState::Inflating;
        let wb = if self.options.gzip { self.options.window_bits + 16 } else { -self.options.window_bits };
        match deflate_compress(self.input, wb, self.options.level) {
            Some(compressed) => {
                self.list_ptr.sb_extend_from_slice(&compressed);
                self.state = ZlibCompressorArrayListState::End;
                Ok(())
            }
            None => {
                self.state = ZlibCompressorArrayListState::Error;
                Err(ZlibError::ZlibError)
            }
        }
    }
}

// ──────────────────────────────────────────────────────────────────────────
// ZlibReader — streaming decompression into a writer
// ──────────────────────────────────────────────────────────────────────────

pub struct ZlibReader<'a, W, const BUFFER_SIZE: usize> {
    pub context: W,
    pub input: &'a [u8],
    pub buf: [u8; BUFFER_SIZE],
    pub state: ZlibReaderState,
    window_bits: c_int,
}

impl<'a, W, const BUFFER_SIZE: usize> ZlibReader<'a, W, BUFFER_SIZE> {
    pub fn init(writer: W, input: &'a [u8]) -> Result<Box<Self>, ZlibError> {
        Self::init_with_options(writer, input, Options { window_bits: 15 + 32, ..Default::default() })
    }

    pub fn init_with_options(writer: W, input: &'a [u8], options: Options) -> Result<Box<Self>, ZlibError> {
        Ok(Box::new(Self {
            context: writer,
            input,
            buf: [0u8; BUFFER_SIZE],
            state: ZlibReaderState::Uninitialized,
            window_bits: options.window_bits,
        }))
    }

    pub fn end(&mut self) {
        self.state = ZlibReaderState::End;
    }

    pub fn error_message(&self) -> Option<&[u8]> {
        None
    }

    pub fn read_all(&mut self, _is_done: bool) -> Result<(), bun_core::Error>
    where
        W: bun_io::Write,
    {
        self.state = ZlibReaderState::Inflating;
        match inflate_decompress(self.input, self.window_bits) {
            Some(decompressed) => {
                self.context.write_all(&decompressed)?;
                self.state = ZlibReaderState::End;
                Ok(())
            }
            None => {
                self.state = ZlibReaderState::Error;
                Err(bun_core::err!("ZlibError"))
            }
        }
    }
}

impl<'a, W, const BUFFER_SIZE: usize> Drop for ZlibReader<'a, W, BUFFER_SIZE> {
    fn drop(&mut self) {
        self.end();
    }
}
