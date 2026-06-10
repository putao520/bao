pub use _impl::{Context, NativeZstd};

mod _impl {
    use core::cell::Cell;
    use core::ffi::{c_int, c_uint, c_void};
    use core::ptr;

    use bun_jsc::{
        self as jsc, CallFrame, JSGlobalObject, JSValue, JsCell, JsResult, StrongOptional,
        WorkPoolTask,
    };
    use bun_zstd::error_codes;
    use bun_zstd::{
        ZSTD_CCtx, ZSTD_DCtx, ZSTD_cParameter, ZSTD_dParameter, ZSTD_EndDirective,
        ZSTD_ResetDirective, ZSTD_DResetDirective, InBuffer, OutBuffer,
    };

    use crate::node::node_zlib_binding::{CompressionStream, CountedKeepAlive, Error};
    use crate::node::util::validators;
    use bun_zlib::NodeMode;

    fn unset_task_callback(_: *mut WorkPoolTask) {
        unreachable!("WorkPoolTask scheduled before CompressionStream set its callback");
    }

    #[bun_jsc::JsClass]
    #[derive(bun_ptr::CellRefCounted)]
    pub struct NativeZstd {
        pub ref_count: Cell<u32>,
        pub global_this: bun_ptr::BackRef<JSGlobalObject>,
        pub stream: JsCell<Context>,
        pub write_result: Cell<Option<*mut u32>>,
        pub poll_ref: JsCell<CountedKeepAlive>,
        pub this_value: JsCell<StrongOptional>,
        pub write_in_progress: Cell<bool>,
        pub pending_close: Cell<bool>,
        pub pending_reset: Cell<bool>,
        pub closed: Cell<bool>,
        pub task: JsCell<WorkPoolTask>,
    }

    impl NativeZstd {
        pub fn constructor(global: &JSGlobalObject, frame: &CallFrame) -> JsResult<Box<Self>> {
            let arguments = frame.arguments_as_array::<1>();

            let mode = arguments[0];
            if !mode.is_number() {
                return Err(global.throw_invalid_argument_type_value("mode", "number", mode));
            }
            let mode_double = mode.as_number();
            if mode_double % 1.0 != 0.0 {
                return Err(global.throw_invalid_argument_type_value("mode", "integer", mode));
            }
            let mode_int: i64 = mode_double as i64;
            if mode_int < 10 || mode_int > 11 {
                return Err(global.throw_range_error(
                    mode_int,
                    jsc::RangeErrorOptions {
                        field_name: b"mode",
                        min: 10,
                        max: 11,
                        msg: b"",
                    },
                ));
            }

            let stream = Context {
                mode: NodeMode::from_int(mode_int as u8),
                ..Default::default()
            };
            Ok(Box::new(Self {
                ref_count: Cell::new(1),
                global_this: bun_ptr::BackRef::new(global),
                stream: JsCell::new(stream),
                write_result: Cell::new(None),
                poll_ref: JsCell::new(CountedKeepAlive::default()),
                this_value: JsCell::new(StrongOptional::empty()),
                write_in_progress: Cell::new(false),
                pending_close: Cell::new(false),
                pending_reset: Cell::new(false),
                closed: Cell::new(false),
                task: JsCell::new(WorkPoolTask {
                    node: Default::default(),
                    callback: unset_task_callback,
                }),
            }))
        }

        pub fn estimated_size(&self) -> usize {
            core::mem::size_of::<Self>()
                + match self.stream.get().mode {
                    NodeMode::ZSTD_COMPRESS => 5272,
                    NodeMode::ZSTD_DECOMPRESS => 95968,
                    _ => 0,
                }
        }

        #[bun_jsc::host_fn(method)]
        pub fn init(&self, global: &JSGlobalObject, frame: &CallFrame) -> JsResult<JSValue> {
            let arguments = frame.arguments_as_array::<4>();
            let this_value = frame.this();
            if frame.arguments_count() != 4 {
                return Err(global
                    .err(
                        jsc::ErrorCode::MISSING_ARGS,
                        format_args!(
                            "init(initParamsArray, pledgedSrcSize, writeState, processCallback)"
                        ),
                    )
                    .throw());
            }

            let init_params_array_value = arguments[0];
            let pledged_src_size_value = arguments[1];
            let write_state_value = arguments[2];
            let process_callback_value = arguments[3];

            let Some(mut write_state) = write_state_value.as_array_buffer(global) else {
                return Err(global.throw_invalid_argument_type_value(
                    "writeState",
                    "Uint32Array",
                    write_state_value,
                ));
            };
            if write_state.typed_array_type != jsc::JSType::Uint32Array {
                return Err(global.throw_invalid_argument_type_value(
                    "writeState",
                    "Uint32Array",
                    write_state_value,
                ));
            }
            let write_state_slice = write_state.as_u32();
            if write_state_slice.len() < 2 {
                return Err(global
                    .err(
                        jsc::ErrorCode::INVALID_ARG_VALUE,
                        format_args!("writeState must be a Uint32Array with at least 2 elements"),
                    )
                    .throw());
            }
            self.write_result.set(Some(write_state_slice.as_mut_ptr()));

            let write_js_callback =
                validators::validate_function(global, "processCallback", process_callback_value)?;
            js::write_callback_set_cached(
                this_value,
                global,
                write_js_callback.with_async_context_if_needed(global),
            );

            let mut pledged_src_size: u64 = u64::MAX;
            if pledged_src_size_value.is_number() {
                pledged_src_size = u64::from(validators::validate_uint32(
                    global,
                    pledged_src_size_value,
                    format_args!("pledgedSrcSize"),
                    false,
                )?);
            }

            let err = self.stream.with_mut(|s| s.init(pledged_src_size));
            if err.is_error() {
                CompressionStream::<Self>::emit_error(self, global, this_value, err);
                return Ok(JSValue::FALSE);
            }

            let Some(mut params_) = init_params_array_value.as_array_buffer(global) else {
                return Err(global.throw_invalid_argument_type_value(
                    "initParamsArray",
                    "Uint32Array",
                    init_params_array_value,
                ));
            };
            if params_.typed_array_type != jsc::JSType::Uint32Array {
                return Err(global.throw_invalid_argument_type_value(
                    "initParamsArray",
                    "Uint32Array",
                    init_params_array_value,
                ));
            }
            for (i, &x) in params_.as_u32().iter().enumerate() {
                if x == u32::MAX {
                    continue;
                }
                let err_ = self
                    .stream
                    .with_mut(|s| s.set_params(c_uint::try_from(i).expect("int cast"), x));
                if err_.is_error() {
                    self.stream.with_mut(|s| s.close());
                    let msg = unsafe { bun_core::ffi::cstr(err_.msg) }.to_bytes();
                    return Err(global
                        .err(
                            jsc::ErrorCode::ZLIB_INITIALIZATION_FAILED,
                            format_args!("{}", bstr::BStr::new(msg)),
                        )
                        .throw());
                }
            }

            Ok(JSValue::TRUE)
        }

        #[bun_jsc::host_fn(method)]
        pub fn params(&self, _global: &JSGlobalObject, _frame: &CallFrame) -> JsResult<JSValue> {
            Ok(JSValue::UNDEFINED)
        }
    }

    impl Drop for NativeZstd {
        fn drop(&mut self) {
            self.stream.with_mut(|s| match s.mode {
                NodeMode::ZSTD_COMPRESS | NodeMode::ZSTD_DECOMPRESS => s.close(),
                _ => {}
            });
        }
    }

    /// Holds the streaming zstd state. Uses an enum to own either a CCtx or DCtx
    /// without raw pointers, enabling safe Drop.
    enum ZstdState {
        Compress(Box<ZSTD_CCtx>),
        Decompress(Box<ZSTD_DCtx>),
        None,
    }

    pub struct Context {
        pub mode: NodeMode,
        state: ZstdState,
        pub flush: c_int,
        pub pledged_src_size: u64,
        pub remaining: u64,
        // Buffer references — set by set_buffers, pointing into caller-owned memory.
        input_ptr: *const u8,
        input_len: usize,
        input_pos: usize,
        output_ptr: *mut u8,
        output_len: usize,
        output_pos: usize,
    }

    impl Default for Context {
        fn default() -> Self {
            Self {
                mode: NodeMode::NONE,
                state: ZstdState::None,
                flush: ZSTD_EndDirective::ZSTD_e_continue as c_int,
                pledged_src_size: u64::MAX,
                remaining: 0,
                input_ptr: ptr::null(),
                input_len: 0,
                input_pos: 0,
                output_ptr: ptr::null_mut(),
                output_len: 0,
                output_pos: 0,
            }
        }
    }

    impl Context {
        pub fn init(&mut self, pledged_src_size: u64) -> Error {
            match self.mode {
                NodeMode::ZSTD_COMPRESS => {
                    self.pledged_src_size = pledged_src_size;
                    let mut cctx = match bun_zstd::ZSTD_createCCtx() {
                        Some(c) => c,
                        None => {
                            return Error::init(
                                c"Could not initialize zstd instance".as_ptr(),
                                -1,
                                c"ERR_ZLIB_INITIALIZATION_FAILED".as_ptr(),
                            );
                        }
                    };
                    let result = bun_zstd::ZSTD_CCtx_setPledgedSrcSize(
                        &mut cctx, pledged_src_size as _,
                    );
                    if bun_zstd::ZSTD_isError(result) {
                        return Error::init(
                            c"Could not set pledged src size".as_ptr(),
                            -1,
                            c"ERR_ZLIB_INITIALIZATION_FAILED".as_ptr(),
                        );
                    }
                    self.state = ZstdState::Compress(cctx);
                    Error::OK
                }
                NodeMode::ZSTD_DECOMPRESS => {
                    let dctx = match bun_zstd::ZSTD_createDCtx() {
                        Some(d) => d,
                        None => {
                            return Error::init(
                                c"Could not initialize zstd instance".as_ptr(),
                                -1,
                                c"ERR_ZLIB_INITIALIZATION_FAILED".as_ptr(),
                            );
                        }
                    };
                    self.state = ZstdState::Decompress(dctx);
                    Error::OK
                }
                _ => unreachable!(),
            }
        }

        pub fn set_params(&mut self, key: c_uint, value: u32) -> Error {
            match (&mut self.state, self.mode) {
                (ZstdState::Compress(cctx), NodeMode::ZSTD_COMPRESS) => {
                    let result = bun_zstd::ZSTD_CCtx_setParameter(
                        cctx,
                        ZSTD_cParameter(key),
                        value as c_int,
                    );
                    if bun_zstd::ZSTD_isError(result) {
                        return Error::init(
                            c"Setting parameter failed".as_ptr(),
                            -1,
                            c"ERR_ZSTD_PARAM_SET_FAILED".as_ptr(),
                        );
                    }
                    Error::OK
                }
                (ZstdState::Decompress(dctx), NodeMode::ZSTD_DECOMPRESS) => {
                    let result = bun_zstd::ZSTD_DCtx_setParameter(
                        dctx,
                        ZSTD_dParameter(key),
                        value as c_int,
                    );
                    if bun_zstd::ZSTD_isError(result) {
                        return Error::init(
                            c"Setting parameter failed".as_ptr(),
                            -1,
                            c"ERR_ZSTD_PARAM_SET_FAILED".as_ptr(),
                        );
                    }
                    Error::OK
                }
                _ => unreachable!(),
            }
        }

        pub fn reset(&mut self) -> Error {
            if !matches!(self.state, ZstdState::None) {
                self.deinit_state();
            }
            self.init(self.pledged_src_size)
        }

        fn deinit_state(&mut self) {
            self.state = ZstdState::None;
        }

        pub fn set_buffers(&mut self, in_: Option<&[u8]>, out: Option<&mut [u8]>) {
            self.input_ptr = in_.map_or(ptr::null(), |p| p.as_ptr());
            self.input_len = in_.map_or(0, |p| p.len());
            self.input_pos = 0;
            match out {
                Some(p) => {
                    self.output_len = p.len();
                    self.output_ptr = p.as_mut_ptr();
                }
                None => {
                    self.output_len = 0;
                    self.output_ptr = ptr::null_mut();
                }
            }
            self.output_pos = 0;
        }

        pub fn set_flush(&mut self, flush: c_int) {
            self.flush = flush;
        }

        pub fn do_work(&mut self) {
            // SAFETY: input_ptr/output_ptr are set by set_buffers pointing to
            // caller-kept-alive memory. We construct temporary slices for the
            // pure Rust API calls.
            let input_slice = unsafe {
                core::slice::from_raw_parts(self.input_ptr.add(self.input_pos), self.input_len - self.input_pos)
            };
            let output_slice = unsafe {
                core::slice::from_raw_parts_mut(self.output_ptr.add(self.output_pos), self.output_len - self.output_pos)
            };

            match (&mut self.state, self.mode) {
                (ZstdState::Compress(cctx), NodeMode::ZSTD_COMPRESS) => {
                    let mut in_pos = 0usize;
                    let mut out_pos = 0usize;
                    let end_op = unsafe { core::mem::transmute::<c_int, ZSTD_EndDirective>(self.flush) };
                    let rc = bun_zstd::ZSTD_compressStream2(
                        cctx,
                        output_slice,
                        &mut out_pos,
                        input_slice,
                        &mut in_pos,
                        end_op,
                    );
                    self.input_pos += in_pos;
                    self.output_pos += out_pos;
                    self.remaining = rc as u64;
                }
                (ZstdState::Decompress(dctx), NodeMode::ZSTD_DECOMPRESS) => {
                    let mut in_pos = 0usize;
                    let mut out_pos = 0usize;
                    let rc = bun_zstd::ZSTD_decompressStream(
                        dctx,
                        output_slice,
                        &mut out_pos,
                        input_slice,
                        &mut in_pos,
                    );
                    self.input_pos += in_pos;
                    self.output_pos += out_pos;
                    self.remaining = rc as u64;
                }
                _ => unreachable!(),
            }
        }

        pub fn update_write_result(&self, avail_in: &mut u32, avail_out: &mut u32) {
            *avail_in = u32::try_from(self.input_len - self.input_pos).expect("int cast");
            *avail_out = u32::try_from(self.output_len - self.output_pos).expect("int cast");
        }

        pub fn get_error_info(&mut self) -> Error {
            let err = bun_zstd::ZSTD_getErrorCode(self.remaining as usize);
            let result = if err == bun_zstd::ZSTD_ErrorCode::NoError {
                Error::OK
            } else {
                let err_u32 = match err {
                    bun_zstd::ZSTD_ErrorCode::NoError => error_codes::ZSTD_error_no_error,
                    bun_zstd::ZSTD_ErrorCode::Generic => error_codes::ZSTD_error_GENERIC,
                    bun_zstd::ZSTD_ErrorCode::PrefixUnknown => error_codes::ZSTD_error_prefix_unknown,
                    bun_zstd::ZSTD_ErrorCode::VersionUnsupported => error_codes::ZSTD_error_version_unsupported,
                    bun_zstd::ZSTD_ErrorCode::FrameParameterUnsupported => error_codes::ZSTD_error_frameParameter_unsupported,
                    bun_zstd::ZSTD_ErrorCode::FrameParameterWindowTooLarge => error_codes::ZSTD_error_frameParameter_windowTooLarge,
                    bun_zstd::ZSTD_ErrorCode::CorruptionDetected => error_codes::ZSTD_error_corruption_detected,
                    bun_zstd::ZSTD_ErrorCode::ChecksumWrong => error_codes::ZSTD_error_checksum_wrong,
                    bun_zstd::ZSTD_ErrorCode::LiteralsHeaderWrong => error_codes::ZSTD_error_literals_headerWrong,
                    bun_zstd::ZSTD_ErrorCode::DictionaryCorrupted => error_codes::ZSTD_error_dictionary_corrupted,
                    bun_zstd::ZSTD_ErrorCode::DictionaryWrong => error_codes::ZSTD_error_dictionary_wrong,
                    bun_zstd::ZSTD_ErrorCode::DictionaryCreationFailed => error_codes::ZSTD_error_dictionaryCreation_failed,
                    bun_zstd::ZSTD_ErrorCode::ParameterUnsupported => error_codes::ZSTD_error_parameter_unsupported,
                    bun_zstd::ZSTD_ErrorCode::ParameterCombinationUnsupported => error_codes::ZSTD_error_parameter_combination_unsupported,
                    bun_zstd::ZSTD_ErrorCode::ParameterOutOfBound => error_codes::ZSTD_error_parameter_outOfBound,
                    bun_zstd::ZSTD_ErrorCode::TableLogTooLarge => error_codes::ZSTD_error_tableLog_tooLarge,
                    bun_zstd::ZSTD_ErrorCode::MaxSymbolValueTooLarge => error_codes::ZSTD_error_maxSymbolValue_tooLarge,
                    bun_zstd::ZSTD_ErrorCode::MaxSymbolValueTooSmall => error_codes::ZSTD_error_maxSymbolValue_tooSmall,
                    bun_zstd::ZSTD_ErrorCode::StabilityConditionNotRespected => error_codes::ZSTD_error_stabilityCondition_notRespected,
                    bun_zstd::ZSTD_ErrorCode::StageWrong => error_codes::ZSTD_error_stage_wrong,
                    bun_zstd::ZSTD_ErrorCode::InitMissing => error_codes::ZSTD_error_init_missing,
                    bun_zstd::ZSTD_ErrorCode::MemoryAllocation => error_codes::ZSTD_error_memory_allocation,
                    bun_zstd::ZSTD_ErrorCode::WorkSpaceTooSmall => error_codes::ZSTD_error_workSpace_tooSmall,
                    bun_zstd::ZSTD_ErrorCode::DstSizeTooSmall => error_codes::ZSTD_error_dstSize_tooSmall,
                    bun_zstd::ZSTD_ErrorCode::SrcSizeWrong => error_codes::ZSTD_error_srcSize_wrong,
                    bun_zstd::ZSTD_ErrorCode::DstBufferNull => error_codes::ZSTD_error_dstBuffer_null,
                    bun_zstd::ZSTD_ErrorCode::NoForwardProgressDestFull => error_codes::ZSTD_error_noForwardProgress_destFull,
                    bun_zstd::ZSTD_ErrorCode::NoForwardProgressInputEmpty => error_codes::ZSTD_error_noForwardProgress_inputEmpty,
                    _ => error_codes::ZSTD_error_GENERIC,
                };
                Error {
                    err: err_u32 as c_int,
                    msg: match err_u32 {
                        error_codes::ZSTD_error_no_error => c"ZSTD_error_no_error",
                        error_codes::ZSTD_error_GENERIC => c"ZSTD_error_GENERIC",
                        error_codes::ZSTD_error_prefix_unknown => c"ZSTD_error_prefix_unknown",
                        error_codes::ZSTD_error_version_unsupported => c"ZSTD_error_version_unsupported",
                        error_codes::ZSTD_error_frameParameter_unsupported => c"ZSTD_error_frameParameter_unsupported",
                        error_codes::ZSTD_error_frameParameter_windowTooLarge => c"ZSTD_error_frameParameter_windowTooLarge",
                        error_codes::ZSTD_error_corruption_detected => c"ZSTD_error_corruption_detected",
                        error_codes::ZSTD_error_checksum_wrong => c"ZSTD_error_checksum_wrong",
                        error_codes::ZSTD_error_literals_headerWrong => c"ZSTD_error_literals_headerWrong",
                        error_codes::ZSTD_error_dictionary_corrupted => c"ZSTD_error_dictionary_corrupted",
                        error_codes::ZSTD_error_dictionary_wrong => c"ZSTD_error_dictionary_wrong",
                        error_codes::ZSTD_error_dictionaryCreation_failed => c"ZSTD_error_dictionaryCreation_failed",
                        error_codes::ZSTD_error_parameter_unsupported => c"ZSTD_error_parameter_unsupported",
                        error_codes::ZSTD_error_parameter_combination_unsupported => c"ZSTD_error_parameter_combination_unsupported",
                        error_codes::ZSTD_error_parameter_outOfBound => c"ZSTD_error_parameter_outOfBound",
                        error_codes::ZSTD_error_tableLog_tooLarge => c"ZSTD_error_tableLog_tooLarge",
                        error_codes::ZSTD_error_maxSymbolValue_tooLarge => c"ZSTD_error_maxSymbolValue_tooLarge",
                        error_codes::ZSTD_error_maxSymbolValue_tooSmall => c"ZSTD_error_maxSymbolValue_tooSmall",
                        error_codes::ZSTD_error_stabilityCondition_notRespected => c"ZSTD_error_stabilityCondition_notRespected",
                        error_codes::ZSTD_error_stage_wrong => c"ZSTD_error_stage_wrong",
                        error_codes::ZSTD_error_init_missing => c"ZSTD_error_init_missing",
                        error_codes::ZSTD_error_memory_allocation => c"ZSTD_error_memory_allocation",
                        error_codes::ZSTD_error_workSpace_tooSmall => c"ZSTD_error_workSpace_tooSmall",
                        error_codes::ZSTD_error_dstSize_tooSmall => c"ZSTD_error_dstSize_tooSmall",
                        error_codes::ZSTD_error_srcSize_wrong => c"ZSTD_error_srcSize_wrong",
                        error_codes::ZSTD_error_dstBuffer_null => c"ZSTD_error_dstBuffer_null",
                        error_codes::ZSTD_error_noForwardProgress_destFull => c"ZSTD_error_noForwardProgress_destFull",
                        error_codes::ZSTD_error_noForwardProgress_inputEmpty => c"ZSTD_error_noForwardProgress_inputEmpty",
                        _ => c"ZSTD_error_GENERIC",
                    }
                    .as_ptr(),
                    code: match err_u32 {
                        error_codes::ZSTD_error_no_error => c"ZSTD_error_no_error",
                        error_codes::ZSTD_error_GENERIC => c"ZSTD_error_GENERIC",
                        error_codes::ZSTD_error_prefix_unknown => c"ZSTD_error_prefix_unknown",
                        error_codes::ZSTD_error_version_unsupported => c"ZSTD_error_version_unsupported",
                        error_codes::ZSTD_error_frameParameter_unsupported => c"ZSTD_error_frameParameter_unsupported",
                        error_codes::ZSTD_error_frameParameter_windowTooLarge => c"ZSTD_error_frameParameter_windowTooLarge",
                        error_codes::ZSTD_error_corruption_detected => c"ZSTD_error_corruption_detected",
                        error_codes::ZSTD_error_checksum_wrong => c"ZSTD_error_checksum_wrong",
                        error_codes::ZSTD_error_literals_headerWrong => c"ZSTD_error_literals_headerWrong",
                        error_codes::ZSTD_error_dictionary_corrupted => c"ZSTD_error_dictionary_corrupted",
                        error_codes::ZSTD_error_dictionary_wrong => c"ZSTD_error_dictionary_wrong",
                        error_codes::ZSTD_error_dictionaryCreation_failed => c"ZSTD_error_dictionaryCreation_failed",
                        error_codes::ZSTD_error_parameter_unsupported => c"ZSTD_error_parameter_unsupported",
                        error_codes::ZSTD_error_parameter_combination_unsupported => c"ZSTD_error_parameter_combination_unsupported",
                        error_codes::ZSTD_error_parameter_outOfBound => c"ZSTD_error_parameter_outOfBound",
                        error_codes::ZSTD_error_tableLog_tooLarge => c"ZSTD_error_tableLog_tooLarge",
                        error_codes::ZSTD_error_maxSymbolValue_tooLarge => c"ZSTD_error_maxSymbolValue_tooLarge",
                        error_codes::ZSTD_error_maxSymbolValue_tooSmall => c"ZSTD_error_maxSymbolValue_tooSmall",
                        error_codes::ZSTD_error_stabilityCondition_notRespected => c"ZSTD_error_stabilityCondition_notRespected",
                        error_codes::ZSTD_error_stage_wrong => c"ZSTD_error_stage_wrong",
                        error_codes::ZSTD_error_init_missing => c"ZSTD_error_init_missing",
                        error_codes::ZSTD_error_memory_allocation => c"ZSTD_error_memory_allocation",
                        error_codes::ZSTD_error_workSpace_tooSmall => c"ZSTD_error_workSpace_tooSmall",
                        error_codes::ZSTD_error_dstSize_tooSmall => c"ZSTD_error_dstSize_tooSmall",
                        error_codes::ZSTD_error_srcSize_wrong => c"ZSTD_error_srcSize_wrong",
                        error_codes::ZSTD_error_dstBuffer_null => c"ZSTD_error_dstBuffer_null",
                        error_codes::ZSTD_error_noForwardProgress_destFull => c"ZSTD_error_noForwardProgress_destFull",
                        error_codes::ZSTD_error_noForwardProgress_inputEmpty => c"ZSTD_error_noForwardProgress_inputEmpty",
                        _ => c"ZSTD_error_GENERIC",
                    }
                    .as_ptr(),
                }
            };
            self.remaining = 0;
            result
        }

        pub fn close(&mut self) {
            match (&mut self.state, self.mode) {
                (ZstdState::Compress(cctx), NodeMode::ZSTD_COMPRESS) => {
                    let _ = bun_zstd::ZSTD_CCtx_reset(cctx, ZSTD_ResetDirective::ZSTD_reset_session_and_parameters);
                }
                (ZstdState::Decompress(dctx), NodeMode::ZSTD_DECOMPRESS) => {
                    let _ = bun_zstd::ZSTD_DCtx_reset(dctx, ZSTD_DResetDirective::ZSTD_reset_session_and_parameters);
                }
                _ => {}
            }
            self.deinit_state();
            self.mode = NodeMode::NONE;
        }
    }

    crate::__impl_compression_stream!(NativeZstd, Context, "NativeZstd");
    crate::__compression_stream_mixin_reexports!(NativeZstd);
}
