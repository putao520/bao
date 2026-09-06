// @trace REQ-PURE-003 [level:library] [entity:ZstdEngine,ZstdCompressConfig]
//! bun_zstd - pure Rust zstd via zstd_pure_rs (replaces vendor/zstd C library).
//! All compress/decompress delegates to zstd_pure_rs::prelude.
#![warn(unused_must_use)]
#![allow(non_upper_case_globals)]

use zstd_pure_rs::prelude as zstd_api;

/// No-op function retained for link-propagation compatibility.
#[inline(never)]
pub fn force_link() {}

pub enum Result {
    Success(usize),
    Err(&'static str),
}

#[derive(strum::IntoStaticStr, Debug)]
pub enum ZstdError {
    /// The output, or the decoder state the frame's window size dictates, could not be allocated.
    OutOfMemory,
    InvalidZstdData,
    DecompressionFailed,
    ZstdFailedToCreateInstance,
    ZstdDecompressionError,
    ShortRead,
}

bun_core::impl_tag_error!(ZstdError);
bun_core::named_error_set!(ZstdError);

impl ZstdError {
    /// The error for a failed (`ZSTD_isError`) decompression call; `other` is the non-allocation failure.
    fn for_decompression(rc: usize, other: ZstdError) -> ZstdError {
        if zstd_api::ZSTD_getErrorCode(rc) == zstd_api::ErrorCode::MemoryAllocation {
            ZstdError::OutOfMemory
        } else {
            other
        }
    }
}

/// Minimum spare output capacity offered to `ZSTD_decompressStream` per call.
const STREAMING_OUTPUT_STEP: usize = 4096;

pub fn compress(dest: &mut [u8], src: &[u8], level: Option<i32>) -> Result {
    let level = level.unwrap_or_else(|| zstd_api::ZSTD_defaultCLevel());
    let n = zstd_api::ZSTD_compress(dest, src, level);
    if zstd_api::ZSTD_isError(n) {
        return Result::Err(zstd_api::ZSTD_getErrorName(n));
    }
    Result::Success(n)
}

pub fn compress_bound(src_size: usize) -> usize {
    zstd_api::ZSTD_compressBound(src_size)
}

pub fn decompress(dest: &mut [u8], src: &[u8]) -> Result {
    let n = zstd_api::ZSTD_decompress(dest, src);
    if zstd_api::ZSTD_isError(n) {
        return Result::Err(zstd_api::ZSTD_getErrorName(n));
    }
    Result::Success(n)
}

pub fn decompress_alloc(src: &[u8]) -> core::result::Result<Vec<u8>, ZstdError> {
    let size = get_decompressed_size(src);

    const ZSTD_CONTENTSIZE_UNKNOWN: u64 = u64::MAX;
    const ZSTD_CONTENTSIZE_ERROR: u64 = u64::MAX - 1;
    const MAX_PREALLOCATE_SIZE: usize = 16 * 1024 * 1024;

    if size as u64 == ZSTD_CONTENTSIZE_ERROR {
        return Err(ZstdError::InvalidZstdData);
    }

    if size as u64 == ZSTD_CONTENTSIZE_UNKNOWN || size > MAX_PREALLOCATE_SIZE {
        let initial_capacity = if size as u64 == ZSTD_CONTENTSIZE_UNKNOWN {
            // A frame's output is rarely smaller than its input.
            src.len().clamp(STREAMING_OUTPUT_STEP, MAX_PREALLOCATE_SIZE)
        } else {
            // The header size is untrusted: reserve no more than the fast path below would.
            MAX_PREALLOCATE_SIZE
        };
        let mut list: bun_core::vec::ChanVec<u8> = bun_core::vec::ChanVec::new();
        if list.try_reserve_exact(initial_capacity).is_err() {
            return Err(ZstdError::OutOfMemory);
        }
        let mut reader = ZstdReaderArrayList::init(src, &mut list)?;
        reader.read_all(true)?;
        drop(reader);
        return Ok(list.into_iter().collect::<::std::vec::Vec<u8>>());
    }

    // Fast path: size is known and within reasonable limits. zstd_pure_rs's
    // safe `&mut [u8]` API cannot receive uninitialized spare capacity (the
    // C-FFI `spare_capacity_mut` path upstream 8bc4d2a88 uses is unreachable
    // here), so the reserve is fallible (OOM-hardened) but the fill stays.
    let mut output: Vec<u8> = Vec::new();
    if output.try_reserve_exact(size).is_err() {
        return Err(ZstdError::OutOfMemory);
    }
    output.spare_capacity_mut().fill(core::mem::MaybeUninit::new(0));
    // SAFETY: every byte of `[0..size]` was initialized by the fill above.
    unsafe { output.set_len(size) };

    let rc = zstd_api::ZSTD_decompress(&mut output, src);
    if zstd_api::ZSTD_isError(rc) {
        // `output` is freed by Drop.
        return Err(ZstdError::for_decompression(
            rc,
            ZstdError::DecompressionFailed,
        ));
    }
    output.truncate(rc);
    Ok(output)
}

pub fn get_decompressed_size(src: &[u8]) -> usize {
    zstd_api::ZSTD_findDecompressedSize(src) as usize
}

pub use bun_core::compress::State;

pub struct ZstdReaderArrayList<'a, V: bun_core::vec::SpareBytesVec = bun_core::vec::ChanVec<u8>> {
    pub input: &'a [u8],
    pub list_ptr: &'a mut V,
    zstd: Option<Box<zstd_api::ZSTD_DStream>>,
    pub state: State,
    pub total_out: usize,
    pub total_in: usize,
    pub max_output_size: usize,
}

impl<'a, V: bun_core::vec::SpareBytesVec> ZstdReaderArrayList<'a, V> {
    pub fn init(
        input: &'a [u8],
        list: &'a mut V,
    ) -> core::result::Result<Box<ZstdReaderArrayList<'a, V>>, ZstdError> {
        Self::init_with_list_allocator(input, list)
    }

    pub fn init_with_list_allocator(
        input: &'a [u8],
        list: &'a mut V,
    ) -> core::result::Result<Box<ZstdReaderArrayList<'a, V>>, ZstdError> {
        let mut dstream = zstd_api::ZSTD_createDStream().ok_or(ZstdError::ZstdFailedToCreateInstance)?;
        let _ = zstd_api::ZSTD_initDStream(&mut *dstream);

        Ok(Box::new(ZstdReaderArrayList {
            input,
            list_ptr: list,
            zstd: Some(dstream),
            state: State::Uninitialized,
            total_out: 0,
            total_in: 0,
            max_output_size: usize::MAX,
        }))
    }

    pub fn end(&mut self) {
        if self.state != State::End {
            self.zstd.take();
            self.state = State::End;
        }
    }

    pub fn read_all(&mut self, is_done: bool) -> core::result::Result<(), ZstdError> {
        if self.state == State::End || self.state == State::Error {
            return Ok(());
        }

        let dstream = self.zstd.as_mut().ok_or(ZstdError::ZstdFailedToCreateInstance)?;

        // zstd may hold decoded bytes it could not fit into the last output
        // window. Call it again with no input until it leaves the window short.
        let mut output_full = false;
        while self.state == State::Uninitialized || self.state == State::Inflating {
            let next_in = &self.input[self.total_in..];

            if next_in.is_empty() && !output_full {
                if is_done {
                    if self.state == State::Inflating {
                        self.state = State::Error;
                        return Err(ZstdError::ZstdDecompressionError);
                    }
                    self.end();
                }
                return Ok(());
            }

            let remaining_output = self.max_output_size.saturating_sub(self.list_ptr.sb_len());
            if remaining_output == 0 {
                self.state = State::Error;
                return Err(ZstdError::ZstdDecompressionError);
            }

            if !self.list_ptr.sb_try_reserve(STREAMING_OUTPUT_STEP) {
                self.state = State::Error;
                return Err(ZstdError::OutOfMemory);
            }
            let spare = unsafe { bun_core::vec::spare_bytes_mut(self.list_ptr) };
            let out_cap = spare.len().min(remaining_output);

            let mut out_pos = 0usize;
            let mut in_pos = 0usize;
            let rc = zstd_api::ZSTD_decompressStream(
                dstream,
                &mut spare[..out_cap],
                &mut out_pos,
                next_in,
                &mut in_pos,
            );

            if zstd_api::ZSTD_isError(rc) {
                self.state = State::Error;
                return Err(ZstdError::for_decompression(
                    rc,
                    ZstdError::ZstdDecompressionError,
                ));
            }

            let bytes_written = out_pos;
            let bytes_read = in_pos;
            output_full = bytes_written == out_cap;

            unsafe { bun_core::vec::commit_spare(self.list_ptr, bytes_written) };
            self.total_in += bytes_read;
            self.total_out += bytes_written;

            if rc == 0 {
                self.state = State::Uninitialized;
                if self.total_in >= self.input.len() {
                    if is_done {
                        self.end();
                        return Ok(());
                    }
                    return Ok(());
                }
                let _ = zstd_api::ZSTD_initDStream(dstream);
                continue;
            }

            if rc > 0 {
                self.state = State::Inflating;
            }

            if bytes_read == next_in.len() {
                if output_full {
                    continue;
                }
                if is_done {
                    self.state = State::Error;
                    return Err(ZstdError::ZstdDecompressionError);
                }
                return Err(ZstdError::ShortRead);
            }
        }
        Ok(())
    }
}

impl<V: bun_core::vec::SpareBytesVec> Drop for ZstdReaderArrayList<'_, V> {
    fn drop(&mut self) {
        self.end();
    }
}

// ─── Advanced API for NativeZstd (streaming compress/decompress) ──────────

pub use zstd_pure_rs::prelude::{
    ZSTD_CCtx, ZSTD_DStream,
    ZSTD_cParameter, ZSTD_dParameter,
    ZSTD_EndDirective, ZSTD_ResetDirective, ZSTD_DResetDirective,
    ZSTD_CONTENTSIZE_UNKNOWN, ZSTD_CONTENTSIZE_ERROR,
    ZSTD_createCCtx, ZSTD_freeCCtx,
    ZSTD_createDCtx, ZSTD_freeDCtx,
    ZSTD_CCtx_setPledgedSrcSize, ZSTD_CCtx_setParameter,
    ZSTD_DCtx_setParameter, ZSTD_CCtx_reset, ZSTD_DCtx_reset,
    ZSTD_compressStream2, ZSTD_decompressStream,
    ZSTD_getErrorCode, ZSTD_getErrorString,
    ZSTD_isError, ZSTD_getErrorName,
    ZSTD_defaultCLevel,
};
pub use zstd_pure_rs::prelude::ErrorCode as ZSTD_ErrorCode;
pub use zstd_pure_rs::prelude::ZSTD_EndDirective::ZSTD_e_continue;
pub use zstd_pure_rs::prelude::ZSTD_ResetDirective::ZSTD_reset_session_and_parameters;

/// Rust-native buffer types replacing the C FFI ZSTD_inBuffer/ZSTD_outBuffer.
/// NativeZstd's Context uses these instead of raw pointers.
#[repr(C)]
pub struct InBuffer<'a> {
    pub src: &'a [u8],
    pub pos: usize,
}

#[repr(C)]
pub struct OutBuffer<'a> {
    pub dst: &'a mut [u8],
    pub pos: usize,
}

/// Error code constants — kept as c_uint values for NativeZstd compatibility.
pub mod error_codes {
    pub const ZSTD_error_no_error: u32 = 0;
    pub const ZSTD_error_GENERIC: u32 = 1;
    pub const ZSTD_error_prefix_unknown: u32 = 10;
    pub const ZSTD_error_version_unsupported: u32 = 12;
    pub const ZSTD_error_frameParameter_unsupported: u32 = 14;
    pub const ZSTD_error_frameParameter_windowTooLarge: u32 = 16;
    pub const ZSTD_error_corruption_detected: u32 = 20;
    pub const ZSTD_error_checksum_wrong: u32 = 22;
    pub const ZSTD_error_literals_headerWrong: u32 = 24;
    pub const ZSTD_error_dictionary_corrupted: u32 = 30;
    pub const ZSTD_error_dictionary_wrong: u32 = 32;
    pub const ZSTD_error_dictionaryCreation_failed: u32 = 34;
    pub const ZSTD_error_parameter_unsupported: u32 = 40;
    pub const ZSTD_error_parameter_combination_unsupported: u32 = 41;
    pub const ZSTD_error_parameter_outOfBound: u32 = 42;
    pub const ZSTD_error_tableLog_tooLarge: u32 = 44;
    pub const ZSTD_error_maxSymbolValue_tooLarge: u32 = 46;
    pub const ZSTD_error_maxSymbolValue_tooSmall: u32 = 48;
    pub const ZSTD_error_stabilityCondition_notRespected: u32 = 50;
    pub const ZSTD_error_stage_wrong: u32 = 60;
    pub const ZSTD_error_init_missing: u32 = 62;
    pub const ZSTD_error_memory_allocation: u32 = 64;
    pub const ZSTD_error_workSpace_tooSmall: u32 = 66;
    pub const ZSTD_error_dstSize_tooSmall: u32 = 70;
    pub const ZSTD_error_srcSize_wrong: u32 = 72;
    pub const ZSTD_error_dstBuffer_null: u32 = 74;
    pub const ZSTD_error_noForwardProgress_destFull: u32 = 80;
    pub const ZSTD_error_noForwardProgress_inputEmpty: u32 = 82;
}
