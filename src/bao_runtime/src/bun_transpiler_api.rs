// @trace REQ-ENG-19 [api:Bun.Transpiler]
// Bun.Transpiler — TypeScript/JSX 转译 API(SPEC REQ-ENG-19 C1-C5,内核 bun_transpiler crate)。
//
// 桥接 workspace `bun_transpiler` crate(零修改复用):`transpile_ts` 用 SWC
// 官方 TypeScript strip 管线(resolver → strip → hygiene → fixer)做
// 类型擦除,注释保留,TSX flag 由文件名扩展名驱动。
//
// JS 面(上游 JSTranspiler.rs 对齐面):
//   * `new Bun.Transpiler({ loader })` — loader ∈ {js, jsx, ts, tsx};
//     省略 = "jsx"(上游 `default_loader: Loader::Jsx`,
//     bun src/runtime/api/JSTranspiler.rs:98)
//   * `transpiler.transformSync(code[, { loader }])` — 同步转译,返回 JS 字符串
//   * `transpiler.transform(code[, { loader }])` — 同版转译,返回 Promise<string>
//
// 逐调用 loader 覆盖构造器默认(上游 transformSync options 同语义);
// 未知 loader fail-closed 抛错(上游 loader_from_js 同为报错不静默)。
use mozjs::jsapi::*;
use mozjs::jsval::{JSVal, ObjectValue, StringValue, UndefinedValue};
use mozjs::rooted;
use mozjs::rust::wrappers2::{JS_DefineFunction, JS_DefineProperty3, JS_NewPlainObject};

use bun_core::ZBox;
use bun_transpiler as swc_transpile;

// `mozjs::jsapi::*` glob 导入了零泛参的 `mozilla::Result` 别名,与本文件
// 的 `Result` 语义冲突——统一显式路径(node_dns.rs 同款纪律)。
use ::std::result::Result;

/// JSFUN_CONSTRUCTOR (0x400): constructible so `new Bun.Transpiler(opts)`
/// dispatches here (node_buffer.rs / bun_glob_api.rs pattern — SM's
/// [[Construct]] for natives honours the rval we set).
const JSFUN_CONSTRUCTOR: u32 = 0x400;

/// JavaScript-like loader set (upstream loader_from_js 接受集的 Bao 面;
/// 非转译 loader 如 css/toml 不属于 Bun.Transpiler 面)。
const LOADERS: [&str; 4] = ["js", "jsx", "ts", "tsx"];

/// 省略 loader 时的默认值 — 上游 Bun JSTranspiler 默认 `Loader::Jsx`。
const DEFAULT_LOADER: &str = "jsx";

/// loader → 交给 `transpile_ts` 的内部文件名。
///
/// `bun_transpiler::transpile_ts` 以文件名扩展名为唯一 TSX 解析档开关
/// (`TsSyntax { tsx }`),这是 crate 暴露的全部 JSX 面:两个 JSX loader
/// 都走 TSX 解析档(JSX 语法可解析,TS 注记同样被 strip);js/ts 保持
/// 各自的无 JSX 解析档。
fn parse_filename_for_loader(loader: &str) -> Option<&'static str> {
    match loader {
        "js" => Some("input.js"),
        "ts" => Some("input.ts"),
        "jsx" | "tsx" => Some("input.tsx"),
        _ => None,
    }
}

/// Read the loader stored on the instance (`_loader` private prop).
#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn transpiler_loader_of(
    cx: *mut JSContext,
    obj: mozjs::rust::Handle<*mut JSObject>,
) -> Option<String> {
    let mut v = UndefinedValue();
    if !JS_GetProperty(
        cx,
        obj.into(),
        c"_loader".as_ptr(),
        MutableHandle::<Value> {
            _phantom_0: ::std::marker::PhantomData,
            ptr: &mut v,
        },
    ) || !v.is_string()
    {
        JS_ClearPendingException(cx);
        return None;
    }
    Some(crate::js_to_rust_string(cx, v))
}

/// Resolve the effective loader for one call: per-call `options.loader`
/// override, else the constructor default stored on the instance.
/// `Ok(None)` = loader missing/unreadable on the instance (caller reports).
#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn resolve_loader(
    cx: *mut JSContext,
    this: *mut JSObject,
    argc: u32,
    args: &CallArgs,
) -> Result<::std::string::String, ()> {
    let mut wrapped = mozjs::context::JSContext::from_ptr(::std::ptr::NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped;
    if argc >= 2 {
        let opts = *args.get(1).ptr;
        if opts.is_object() {
            rooted!(&in(cx_ref) let opts_root = opts.to_object());
            let mut v = UndefinedValue();
            if !JS_GetProperty(
                cx,
                opts_root.handle().into(),
                c"loader".as_ptr(),
                MutableHandle::<Value> {
                    _phantom_0: ::std::marker::PhantomData,
                    ptr: &mut v,
                },
            ) {
                JS_ClearPendingException(cx);
            } else if v.is_string() {
                let l = crate::js_to_rust_string(cx, v);
                if LOADERS.contains(&l.as_str()) {
                    return Ok(l);
                }
                return Err(());
            }
        } else if !opts.is_undefined() && !opts.is_null() {
            return Err(());
        }
    }
    rooted!(&in(cx_ref) let this_root = this);
    transpiler_loader_of(cx, this_root.handle()).ok_or(())
}

/// 转译执行体:`transpile_ts(code, filename)` — Err 载体为 crate 的
/// `TranspileError.message`(真实解析失败,非占位)。
fn transpile(loader: &str, code: &str) -> Result<::std::string::String, ::std::string::String> {
    let Some(filename) = parse_filename_for_loader(loader) else {
        return Err(format!("Unsupported loader \"{}\"", loader));
    };
    swc_transpile::transpile_ts(code, filename)
        .map_err(|e| e.message)
}

/// Shared body of transformSync/transform: resolve the loader, transpile,
/// produce the JS string value. `Err(msg)` = caller reports and returns false.
#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn transpiler_transform_body(
    cx: *mut JSContext,
    this: *mut JSObject,
    argc: u32,
    args: &CallArgs,
) -> Result<*mut JSString, ::std::string::String> {
    if argc == 0 || !(*args.get(0).ptr).is_string() {
        return Err("Bun.Transpiler.transform expects a code string".to_string());
    }
    let code = crate::js_to_rust_string(cx, *args.get(0).ptr);
    let loader = resolve_loader(cx, this, argc, args).map_err(|()| {
        "Unsupported loader: only \"js\", \"jsx\", \"ts\", \"tsx\" are supported".to_string()
    })?;
    let out = transpile(&loader, &code)?;
    Ok(JS_NewStringCopyZ(cx, ZBox::from_bytes(out.as_bytes()).as_ptr()))
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn transpiler_ctor(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);

    let mut wrapped = mozjs::context::JSContext::from_ptr(::std::ptr::NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped;

    // 可选 options 对象;显式 loader 在此校验(未知值即刻报错,fail-closed)。
    let mut loader = DEFAULT_LOADER.to_string();
    if args.argc_ > 0 {
        let opts = *args.get(0).ptr;
        if opts.is_object() {
            rooted!(&in(cx_ref) let opts_root = opts.to_object());
            let mut v = UndefinedValue();
            if !JS_GetProperty(
                cx,
                opts_root.handle().into(),
                c"loader".as_ptr(),
                MutableHandle::<Value> {
                    _phantom_0: ::std::marker::PhantomData,
                    ptr: &mut v,
                },
            ) {
                JS_ClearPendingException(cx);
            } else if !v.is_undefined() {
                if !v.is_string() {
                    JS_ReportErrorUTF8(
                        cx,
                        c"Bun.Transpiler: loader must be a string".as_ptr(),
                    );
                    return false;
                }
                let l = crate::js_to_rust_string(cx, v);
                if !LOADERS.contains(&l.as_str()) {
                    JS_ReportErrorUTF8(
                        cx,
                        c"Bun.Transpiler: unsupported loader (expected \"js\", \"jsx\", \"ts\", or \"tsx\")"
                            .as_ptr(),
                    );
                    return false;
                }
                loader = l;
            }
        } else if !opts.is_undefined() && !opts.is_null() {
            JS_ReportErrorUTF8(cx, c"Bun.Transpiler expects an options object".as_ptr());
            return false;
        }
    }

    rooted!(&in(cx_ref) let g = JS_NewPlainObject(cx_ref));
    if g.get().is_null() {
        args.rval().set(UndefinedValue());
        return true;
    }
    let c_loader = ZBox::from_bytes(loader.as_bytes());
    let js_loader = JS_NewStringCopyZ(cx, c_loader.as_ptr());
    if !js_loader.is_null() {
        rooted!(&in(cx_ref) let lv = StringValue(&*js_loader));
        JS_DefineProperty(
            cx,
            g.handle().into(),
            c"_loader".as_ptr(),
            lv.handle().into(),
            0,
        );
    }
    JS_DefineFunction(
        cx_ref,
        g.handle(),
        c"transformSync".as_ptr(),
        Some(transpiler_transform_sync),
        1,
        JSPROP_ENUMERATE as u32,
    );
    JS_DefineFunction(
        cx_ref,
        g.handle(),
        c"transform".as_ptr(),
        Some(transpiler_transform),
        1,
        JSPROP_ENUMERATE as u32,
    );
    args.rval().set(ObjectValue(g.get()));
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn transpiler_transform_sync(
    cx: *mut JSContext,
    argc: u32,
    vp: *mut JSVal,
) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let this = args.thisv();
    if !this.is_object() {
        JS_ReportErrorUTF8(
            cx,
            c"Bun.Transpiler.prototype.transformSync requires a Transpiler instance".as_ptr(),
        );
        return false;
    }
    match transpiler_transform_body(cx, this.to_object(), argc, &args) {
        Ok(js_out) if !js_out.is_null() => {
            let mut wrapped = mozjs::context::JSContext::from_ptr(::std::ptr::NonNull::new_unchecked(cx));
            let cx_ref = &mut wrapped;
            rooted!(&in(cx_ref) let ov = StringValue(&*js_out));
            args.rval().set(ov.get());
            true
        }
        Ok(_) => {
            // JS_NewStringCopyZ 失败(OOM):如实报错,不静默返空。
            JS_ReportErrorUTF8(cx, c"Bun.Transpiler: failed to allocate result string".as_ptr());
            false
        }
        Err(msg) => {
            let m = ZBox::from_vec(msg.into_bytes());
            JS_ReportErrorUTF8(cx, c"%s".as_ptr(), m.as_ptr());
            false
        }
    }
}

/// `transform` — 同一转译内核的 Promise 形态(Bun.file.exists 同款同步
/// 计算 + 同步 resolve 惯形;转译本身是纯 CPU 面无 IO 等待)。
#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn transpiler_transform(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let this = args.thisv();
    if !this.is_object() {
        JS_ReportErrorUTF8(
            cx,
            c"Bun.Transpiler.prototype.transform requires a Transpiler instance".as_ptr(),
        );
        return false;
    }
    let mut wrapped = mozjs::context::JSContext::from_ptr(::std::ptr::NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped;

    let js_out = match transpiler_transform_body(cx, this.to_object(), argc, &args) {
        Ok(s) => s,
        Err(msg) => {
            let m = ZBox::from_vec(msg.into_bytes());
            JS_ReportErrorUTF8(cx, c"%s".as_ptr(), m.as_ptr());
            return false;
        }
    };
    if js_out.is_null() {
        JS_ReportErrorUTF8(cx, c"Bun.Transpiler: failed to allocate result string".as_ptr());
        return false;
    }

    rooted!(&in(cx_ref) let out_val = StringValue(&*js_out));
    rooted!(&in(cx_ref) let promise = JS::NewPromiseObject(cx, HandleObject::null()));
    if promise.get().is_null() {
        args.rval().set(out_val.get());
        return true;
    }
    let ok = JS::ResolvePromise(cx, promise.handle().into(), out_val.handle().into());
    args.rval().set(if ok {
        ObjectValue(promise.get())
    } else {
        JS_ClearPendingException(cx);
        out_val.get()
    });
    true
}

/// Install `Bun.Transpiler` (constructor) on the Bun object.
///
/// # Safety
/// Caller must ensure `cx` is a valid JSContext and `bun_obj` a live object.
pub unsafe fn install(
    cx: &mut mozjs::context::JSContext,
    bun_obj: mozjs::rust::Handle<*mut JSObject>,
) {
    let ctor = JS_NewFunction(
        cx.raw_cx(),
        Some(transpiler_ctor),
        1,
        JSFUN_CONSTRUCTOR,
        c"Transpiler".as_ptr(),
    );
    if ctor.is_null() {
        return;
    }
    let ctor_obj = JS_GetFunctionObject(ctor);
    if ctor_obj.is_null() {
        return;
    }
    rooted!(&in(cx) let ctor_root = ctor_obj);
    JS_DefineProperty3(
        cx,
        bun_obj,
        c"Transpiler".as_ptr(),
        ctor_root.handle(),
        JSPROP_ENUMERATE as u32,
    );
}
