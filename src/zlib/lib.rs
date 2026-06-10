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
// Since all input is available in memory, delegate to inflate_decompress.
// ──────────────────────────────────────────────────────────────────────────

pub struct ZlibReaderArrayList<'a> {
    pub input: &'a [u8],
    pub list_ptr: &'a mut Vec<u8>,
    pub state: ZlibReaderArrayListState,
    pub max_output_size: usize,
    window_bits: c_int,
}

impl<'a> Drop for ZlibReaderArrayList<'a> {
    fn drop(&mut self) {
        self.end();
    }
}

impl<'a> ZlibReaderArrayList<'a> {
    pub fn end(&mut self) {
        self.state = ZlibReaderArrayListState::End;
    }

    pub fn init(input: &'a [u8], list: &'a mut Vec<u8>) -> Result<Box<Self>, ZlibError> {
        Self::init_with_options(input, list, Options { window_bits: 15 + 32, ..Default::default() })
    }

    pub fn init_with_options(
        input: &'a [u8],
        list: &'a mut Vec<u8>,
        options: Options,
    ) -> Result<Box<Self>, ZlibError> {
        Self::init_with_options_and_list_allocator(input, list, options)
    }

    pub fn init_with_options_and_list_allocator(
        input: &'a [u8],
        list: &'a mut Vec<u8>,
        options: Options,
    ) -> Result<Box<Self>, ZlibError> {
        Ok(Box::new(Self {
            input,
            list_ptr: list,
            state: ZlibReaderArrayListState::Uninitialized,
            max_output_size: usize::MAX,
            window_bits: options.window_bits,
        }))
    }

    pub fn error_message(&self) -> Option<&[u8]> {
        None
    }

    pub fn read_all(&mut self, _is_done: bool) -> Result<(), ZlibError> {
        self.state = ZlibReaderArrayListState::Inflating;
        match inflate_decompress(self.input, self.window_bits) {
            Some(decompressed) => {
                if decompressed.len() > self.max_output_size {
                    self.state = ZlibReaderArrayListState::Error;
                    return Err(ZlibError::ZlibError);
                }
                self.list_ptr.extend_from_slice(&decompressed);
                self.state = ZlibReaderArrayListState::End;
                Ok(())
            }
            None => {
                self.state = ZlibReaderArrayListState::Error;
                Err(ZlibError::ZlibError)
            }
        }
    }
}

// ──────────────────────────────────────────────────────────────────────────
// ZlibCompressorArrayList — one-shot compression into Vec<u8>
// ──────────────────────────────────────────────────────────────────────────

pub struct ZlibCompressorArrayList<'a> {
    pub input: &'a [u8],
    pub list_ptr: &'a mut Vec<u8>,
    pub state: ZlibCompressorArrayListState,
    options: Options,
}

impl<'a> Drop for ZlibCompressorArrayList<'a> {
    fn drop(&mut self) {
        self.end();
    }
}

impl<'a> ZlibCompressorArrayList<'a> {
    pub fn end(&mut self) {
        self.state = ZlibCompressorArrayListState::End;
    }

    pub fn init(input: &'a [u8], list: &'a mut Vec<u8>, options: Options) -> Result<Box<Self>, ZlibError> {
        Self::init_with_list_allocator(input, list, options)
    }

    pub fn init_with_list_allocator(input: &'a [u8], list: &'a mut Vec<u8>, options: Options) -> Result<Box<Self>, ZlibError> {
        let bound = compress_bound_for(input.len(), options.gzip);
        list.reserve(bound.saturating_sub(list.capacity()));
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
                self.list_ptr.extend_from_slice(&compressed);
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
