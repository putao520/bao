use std::io::Read as StdRead;
use std::io::Write as StdWrite;

use bun_core::{Error, err};

// ──────────────────────────────────────────────────────────────────────────
// Re-export brotli crate types for downstream compatibility
// ──────────────────────────────────────────────────────────────────────────

pub use brotli::enc::encode::BrotliEncoderOperation;

// ──────────────────────────────────────────────────────────────────────────
// Compatibility types matching the old bun_brotli_sys::brotli_c API
//
// These types provide the same enum discriminants and semantics as the
// C brotli library, so that downstream code (NativeBrotli.rs) can migrate
// incrementally. Pure Rust implementations underneath.
// ──────────────────────────────────────────────────────────────────────────

#[repr(u32)]
#[derive(Copy, Clone, Eq, PartialEq, Debug)]
pub enum BrotliDecoderResult {
    Error = 0,
    Success = 1,
    NeedsMoreInput = 2,
    NeedsMoreOutput = 3,
}

#[repr(i32)]
#[derive(Copy, Clone, Eq, PartialEq, Debug)]
pub enum BrotliDecoderErrorCode {
    NoError = 0,
    Success = 1,
    NeedsMoreInput = 2,
    NeedsMoreOutput = 3,
    ErrorFormatExuberantNibble = -1,
    ErrorFormatReserved = -2,
    ErrorFormatExuberantMetaNibble = -3,
    ErrorFormatSimpleHuffmanAlphabet = -4,
    ErrorFormatSimpleHuffmanSame = -5,
    ErrorFormatClSpace = -6,
    ErrorFormatHuffmanSpace = -7,
    ErrorFormatContextMapRepeat = -8,
    ErrorFormatBlockLength1 = -9,
    ErrorFormatBlockLength2 = -10,
    ErrorFormatTransform = -11,
    ErrorFormatDictionary = -12,
    ErrorFormatWindowBits = -13,
    ErrorFormatPadding1 = -14,
    ErrorFormatPadding2 = -15,
    ErrorFormatDistance = -16,
    ErrorCompoundDictionary = -18,
    ErrorDictionaryNotSet = -19,
    ErrorInvalidArguments = -20,
    ErrorAllocContextModes = -21,
    ErrorAllocTreeGroups = -22,
    ErrorAllocContextMap = -25,
    ErrorAllocRingBuffer1 = -26,
    ErrorAllocRingBuffer2 = -27,
    ErrorAllocBlockTypeTrees = -30,
    ErrorUnreachable = -31,
}

#[repr(u32)]
#[derive(Copy, Clone, Eq, PartialEq, Debug)]
pub enum BrotliEncoderMode {
    Generic = 0,
    Text = 1,
    Font = 2,
}

#[repr(u32)]
#[derive(Copy, Clone, Eq, PartialEq, Debug)]
pub enum BrotliEncoderParameter {
    Mode = 0,
    Quality = 1,
    Lgwin = 2,
    Lgblock = 3,
    DisableLiteralContextModeling = 4,
    SizeHint = 5,
    LargeWindow = 6,
    Npostfix = 7,
    Ndirect = 8,
    StreamOffset = 9,
}

#[repr(u32)]
#[derive(Copy, Clone, Eq, PartialEq, Debug)]
pub enum BrotliDecoderParameter {
    DisableRingBufferReallocation = 0,
    LargeWindow = 1,
}

// ──────────────────────────────────────────────────────────────────────────
// One-shot compress / decompress
// ──────────────────────────────────────────────────────────────────────────

pub fn compress(input: &[u8], quality: u32, lgwin: u32) -> Vec<u8> {
    let mut writer = brotli::CompressorWriter::new(Vec::new(), input.len().max(4096), quality, lgwin);
    writer.write_all(input).expect("brotli compress write_all");
    writer.into_inner()
}

pub fn decompress(input: &[u8]) -> Result<Vec<u8>, Error> {
    let mut output = Vec::new();
    let mut reader = brotli::Decompressor::new(input, 4096);
    reader.read_to_end(&mut output)?;
    Ok(output)
}

// ──────────────────────────────────────────────────────────────────────────
// DecoderOptions
// ──────────────────────────────────────────────────────────────────────────

#[derive(Default)]
pub struct DecoderOptions {
    pub large_window: bool,
}

// ──────────────────────────────────────────────────────────────────────────
// BrotliReaderArrayList — streaming decompressor
// ──────────────────────────────────────────────────────────────────────────

pub use bun_core::compress::State as ReaderState;

pub struct BrotliReaderArrayList<'a> {
    pub input: &'a [u8],
    pub list_ptr: &'a mut Vec<u8>,
    pub state: ReaderState,
    pub total_out: usize,
    pub total_in: usize,
    pub max_output_size: usize,
    decompressor: brotli::Decompressor<&'a [u8]>,
}

impl<'a> BrotliReaderArrayList<'a> {
    pub fn new(value: Self) -> Box<Self> {
        Box::new(value)
    }

    pub fn new_with_options(
        input: &'a [u8],
        list: &'a mut Vec<u8>,
        options: &DecoderOptions,
    ) -> Result<Box<Self>, Error> {
        Ok(Self::new(Self::init_with_options(
            input,
            list,
            options,
        )?))
    }

    pub fn init_with_options(
        input: &'a [u8],
        list: &'a mut Vec<u8>,
        _options: &DecoderOptions,
    ) -> Result<Self, Error> {
        let buf_size = input.len().max(4096);
        let decompressor = brotli::Decompressor::new(input, buf_size);

        Ok(Self {
            input,
            list_ptr: list,
            state: ReaderState::Uninitialized,
            total_out: 0,
            total_in: 0,
            max_output_size: usize::MAX,
            decompressor,
        })
    }

    pub fn end(&mut self) {
        self.state = ReaderState::End;
    }

    pub fn read_all(&mut self, is_done: bool) -> Result<(), Error> {
        if self.state == ReaderState::End || self.state == ReaderState::Error {
            return Ok(());
        }

        let mut buf = [0u8; 4096];
        loop {
            match self.decompressor.read(&mut buf) {
                Ok(0) => {
                    self.end();
                    return Ok(());
                }
                Ok(n) => {
                    self.total_out += n;
                    self.list_ptr.extend_from_slice(&buf[..n]);

                    if self.list_ptr.len() > self.max_output_size {
                        self.state = ReaderState::Error;
                        return Err(err!("BrotliDecompressionError"));
                    }
                    self.state = ReaderState::Inflating;
                }
                Err(ref _e) if _e.kind() == std::io::ErrorKind::UnexpectedEof => {
                    if is_done {
                        self.state = ReaderState::Error;
                        return Err(err!("BrotliDecompressionError"));
                    }
                    self.state = ReaderState::Inflating;
                    return Err(err!("ShortRead"));
                }
                Err(ref _e) if _e.kind() == std::io::ErrorKind::WriteZero => {
                    if self.list_ptr.len() >= self.max_output_size {
                        self.state = ReaderState::Error;
                        return Err(err!("BrotliDecompressionError"));
                    }
                    self.list_ptr.reserve(4096);
                    self.state = ReaderState::Inflating;
                }
                Err(io_err) => {
                    self.state = ReaderState::Error;
                    return Err(Error::from(io_err));
                }
            }
        }
    }
}

// ──────────────────────────────────────────────────────────────────────────
// BrotliCompressionStream — streaming compressor
//
// Accumulates all input bytes, then compresses in one shot when finish
// is called. Produces a single valid brotli stream.
// ──────────────────────────────────────────────────────────────────────────

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum CompressionState {
    Inflating,
    End,
    Error,
}

pub struct BrotliCompressionStream {
    quality: u32,
    lgwin: u32,
    pub state: CompressionState,
    pub total_out: usize,
    pub total_in: usize,
    buffer: Vec<u8>,
}

impl BrotliCompressionStream {
    pub fn new(quality: u32, lgwin: u32) -> Self {
        Self {
            quality,
            lgwin,
            state: CompressionState::Inflating,
            total_out: 0,
            total_in: 0,
            buffer: Vec::new(),
        }
    }

    pub fn write_to_vec(&mut self, input: &[u8], _last: bool, _output: &mut Vec<u8>) -> Result<(), Error> {
        if self.state == CompressionState::End || self.state == CompressionState::Error {
            return Ok(());
        }
        self.total_in += input.len();
        self.buffer.extend_from_slice(input);
        Ok(())
    }

    pub fn finish_to_vec(&mut self, output: &mut Vec<u8>) -> Result<(), Error> {
        if matches!(self.state, CompressionState::End | CompressionState::Error) {
            self.state = CompressionState::End;
            return Ok(());
        }

        let compressed = compress(&self.buffer, self.quality, self.lgwin);
        self.total_out = compressed.len();
        output.extend_from_slice(&compressed);
        self.buffer.clear();
        self.state = CompressionState::End;
        Ok(())
    }

    pub fn writer<W: bun_io::Write>(&mut self, writable: W) -> BrotliWriter<'_, W> {
        BrotliWriter::init(self, writable)
    }
}

// ──────────────────────────────────────────────────────────────────────────
// BrotliWriter
// ──────────────────────────────────────────────────────────────────────────

pub struct BrotliWriter<'a, W> {
    pub compressor: &'a mut BrotliCompressionStream,
    pub input_writer: W,
}

impl<'a, W: bun_io::Write> BrotliWriter<'a, W> {
    pub fn init(compressor: &'a mut BrotliCompressionStream, input_writer: W) -> Self {
        Self {
            compressor,
            input_writer,
        }
    }

    pub fn write(&mut self, to_compress: &[u8]) -> Result<usize, Error> {
        let mut sink = Vec::new();
        self.compressor.write_to_vec(to_compress, false, &mut sink)?;
        Ok(to_compress.len())
    }

    pub fn end(&mut self) -> Result<(), Error> {
        let mut compressed = Vec::new();
        self.compressor.finish_to_vec(&mut compressed)?;
        self.input_writer.write_all(&compressed)?;
        Ok(())
    }
}
