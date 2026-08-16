// @trace REQ-ENG-006 [api:Bun.Mime] — MIME type utility.
//
//   * `Bun.Mime.getType(pathOrExt)` — extension → MIME string (null when
//     unknown), backed by the workspace `bun_http_types` MIME table
//     (1000+ extensions, the Bun server table). npm-`mime` semantics:
//     unknown → null (NOT application/octet-stream).
//   * `Bun.Mime.getExtension(type)` — type → canonical extension (null when
//     unknown). The reverse mapping carries the npm-`mime` canonical choice
//     for a type (the forward table is many-to-one and not iterable from
//     outside bun_http_types); every entry is consistency-checked against
//     the forward table in the e2e tests.
//   * `Bun.Mime.normalizeKind(kind)` — short kind → full MIME (Cloudflare
//     Workers normalizeKind semantics; values already containing "/" pass
//     through, unknown values return as-is).
//   * `new Bun.Mime(type, subtype, params)` — structured MIME instance with
//     `essence()` / `toString()` (kept from the prior face).
use mozjs::jsapi::*;
use mozjs::jsval::{JSVal, NullValue, StringValue, UndefinedValue};
use mozjs::rooted;
use mozjs::rust::wrappers2::{JS_DefineFunction, JS_DefineProperty3};

use bun_core::ZBox;

/// Extract the extension from a path or bare extension: basename after the
/// last `/`, then the segment after the last `.` (lowercased). When the
/// basename carries no dot the WHOLE basename is the extension (npm-mime
/// semantics: `getType("html")` and `getType("x.html")` both resolve).
fn path_extension(input: &str) -> Option<String> {
    let base = input.rsplit('/').next().unwrap_or(input);
    let ext = match base.rfind('.') {
        Some(dot) if dot + 1 < base.len() => &base[dot + 1..],
        // Bare extension ("html") or dot-file (".gitignore" → whole name).
        _ => base,
    };
    if ext.is_empty() {
        None
    } else {
        Some(ext.to_ascii_lowercase())
    }
}

/// npm-`mime` canonical type → extension table (the distinct reverse data
/// of the forward ext→type table: which extension a type canonicalizes to).
const CANONICAL_EXTENSIONS: &[(&str, &str)] = &[
    ("application/json", "json"),
    ("application/xml", "xml"),
    ("application/pdf", "pdf"),
    ("application/zip", "zip"),
    ("application/gzip", "gz"),
    ("application/x-tar", "tar"),
    ("application/x-7z-compressed", "7z"),
    ("application/x-rar-compressed", "rar"),
    ("application/wasm", "wasm"),
    ("application/webassembly", "wasm"), // bao server-table name for wasm
    ("application/javascript", "js"), // bao server-table name for js
    ("application/octet-stream", "bin"),
    ("application/manifest+json", "webmanifest"),
    ("application/msword", "doc"),
    ("application/vnd.ms-excel", "xls"),
    ("application/vnd.ms-powerpoint", "ppt"),
    (
        "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
        "docx",
    ),
    (
        "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
        "xlsx",
    ),
    (
        "application/vnd.openxmlformats-officedocument.presentationml.presentation",
        "pptx",
    ),
    ("application/sql", "sql"),
    ("application/graphql", "graphql"),
    ("font/woff", "woff"),
    ("font/woff2", "woff2"),
    ("font/ttf", "ttf"),
    ("font/otf", "otf"),
    ("image/png", "png"),
    ("image/jpeg", "jpg"),
    ("image/gif", "gif"),
    ("image/svg+xml", "svg"),
    ("image/webp", "webp"),
    ("image/avif", "avif"),
    ("image/x-icon", "ico"),
    ("image/bmp", "bmp"),
    ("image/tiff", "tif"),
    ("image/heic", "heic"),
    ("text/html", "html"),
    ("text/css", "css"),
    ("text/javascript", "js"),
    ("text/plain", "txt"),
    ("text/markdown", "md"),
    ("text/csv", "csv"),
    ("text/xml", "xml"),
    ("text/yaml", "yaml"),
    ("text/tab-separated-values", "tsv"),
    ("text/vtt", "vtt"),
    ("audio/mpeg", "mp3"),
    ("audio/mp4", "m4a"),
    ("audio/ogg", "ogg"),
    ("audio/wav", "wav"),
    ("audio/flac", "flac"),
    ("audio/aac", "aac"),
    ("video/mp4", "mp4"),
    ("video/webm", "webm"),
    ("video/quicktime", "mov"),
    ("video/x-msvideo", "avi"),
    ("video/mpeg", "mpeg"),
    ("video/ogg", "ogv"),
    ("model/gltf-binary", "glb"),
    ("model/gltf+json", "gltf"),
];

/// normalizeKind short-kind map (Cloudflare Workers semantics).
const NORMALIZE_KINDS: &[(&str, &str)] = &[
    ("text", "text/plain"),
    ("binary", "application/octet-stream"),
    ("bytes", "application/octet-stream"),
    ("byte", "application/octet-stream"),
    ("arraybuffer", "application/octet-stream"),
    ("arrayBuffer", "application/octet-stream"),
    ("form", "application/x-www-form-urlencoded"),
    ("form-data", "multipart/form-data"),
    ("json", "application/json"),
    ("html", "text/html"),
    ("js", "text/javascript"),
    ("css", "text/css"),
    ("xml", "application/xml"),
    ("plain", "text/plain"),
];

/// Set a JS string result (or undefined on allocation failure).
#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn set_string_result(cx: *mut JSContext, args: &CallArgs, s: &str) {
    let c_s = ZBox::from_bytes(s.as_bytes());
    let js_str = JS_NewStringCopyZ(cx, c_s.as_ptr());
    args.rval().set(if js_str.is_null() {
        UndefinedValue()
    } else {
        StringValue(&*js_str)
    });
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn mime_get_type(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    if args.argc_ == 0 || !(*args.get(0).ptr).is_string() {
        JS_ReportErrorUTF8(cx, c"Bun.Mime.getType expects a string path or extension".as_ptr());
        return false;
    }
    let input = crate::js_to_rust_string(cx, *args.get(0).ptr);
    let Some(ext) = path_extension(&input) else {
        args.rval().set(NullValue());
        return true;
    };
    match bun_http_types::MimeType::by_extension_no_default(ext.as_bytes()) {
        Some(t) => {
            // npm-mime getType returns the bare type (essence, no params) —
            // the server table carries `;charset=utf-8` decorations on some
            // entries.
            let full = String::from_utf8_lossy(&t.value).into_owned();
            let essence = full.split(';').next().unwrap_or(&full).trim().to_string();
            set_string_result(cx, &args, &essence);
        }
        None => args.rval().set(NullValue()),
    }
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn mime_get_extension(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    if args.argc_ == 0 || !(*args.get(0).ptr).is_string() {
        JS_ReportErrorUTF8(cx, c"Bun.Mime.getExtension expects a string MIME type".as_ptr());
        return false;
    }
    let input = crate::js_to_rust_string(cx, *args.get(0).ptr);
    // Essence only (parameters after `;` ignored), lowercased.
    let essence = input
        .split(';')
        .next()
        .unwrap_or(&input)
        .trim()
        .to_ascii_lowercase();
    for (t, ext) in CANONICAL_EXTENSIONS {
        if *t == essence.as_str() {
            set_string_result(cx, &args, ext);
            return true;
        }
    }
    args.rval().set(NullValue());
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn mime_normalize_kind(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    if args.argc_ == 0 || !(*args.get(0).ptr).is_string() {
        JS_ReportErrorUTF8(cx, c"Bun.Mime.normalizeKind expects a string kind".as_ptr());
        return false;
    }
    let kind = crate::js_to_rust_string(cx, *args.get(0).ptr);
    // Already a full MIME type → pass through.
    if kind.contains('/') {
        set_string_result(cx, &args, &kind);
        return true;
    }
    let lower = kind.to_ascii_lowercase();
    for (k, full) in NORMALIZE_KINDS {
        if *k == lower.as_str() {
            set_string_result(cx, &args, full);
            return true;
        }
    }
    // Unknown short kind: returned as-is (Workers semantics).
    set_string_result(cx, &args, &kind);
    true
}

/// Install `Bun.Mime` (class ctor + statics) on the Bun object.
///
/// # Safety
/// Caller must ensure `cx` is a valid JSContext and `bun_obj` a live object.
pub unsafe fn install(
    cx: &mut mozjs::context::JSContext,
    bun_obj: mozjs::rust::Handle<*mut JSObject>,
) {
    // Instance class: `new Mime(type, subtype, params)` with essence()/toString().
    let ctor_src = r#"(function() {
  function Mime(type, subtype, params) {
    this.type = String(type || '');
    this.subtype = String(subtype || '');
    this.params = (params && typeof params === 'object') ? params : {};
  }
  Mime.prototype.toString = function() {
    var s = this.type + '/' + this.subtype;
    var keys = Object.keys(this.params);
    if (keys.length > 0) {
      s += '; ' + keys.map(function(k) { return k + '=' + this.params[k]; }.bind(this)).join('; ');
    }
    return s;
  };
  Mime.prototype.essence = function() {
    return this.type + '/' + this.subtype;
  };
  return Mime;
})()"#;
    let mut text = mozjs::rust::transform_str_to_source_text(ctor_src);
    let opts = mozjs::glue::NewCompileOptions(cx.raw_cx(), c"<bun:Mime>".as_ptr(), 1);
    if opts.is_null() {
        return;
    }
    let mut rval = UndefinedValue();
    let rval_h = MutableHandle::<Value> {
        _phantom_0: ::std::marker::PhantomData,
        ptr: &mut rval,
    };
    let ok = mozjs_sys::jsapi::JS::Evaluate2(cx.raw_cx(), opts, &mut text, rval_h);
    libc::free(opts as *mut _);
    if !ok || !rval.is_object() {
        JS_ClearPendingException(cx.raw_cx());
        return;
    }
    rooted!(&in(cx) let mime_ctor = rval.to_object());

    // Statics (npm-mime + Workers normalizeKind surface).
    JS_DefineFunction(
        cx,
        mime_ctor.handle(),
        c"getType".as_ptr(),
        Some(mime_get_type),
        1,
        JSPROP_ENUMERATE as u32,
    );
    JS_DefineFunction(
        cx,
        mime_ctor.handle(),
        c"getExtension".as_ptr(),
        Some(mime_get_extension),
        1,
        JSPROP_ENUMERATE as u32,
    );
    JS_DefineFunction(
        cx,
        mime_ctor.handle(),
        c"normalizeKind".as_ptr(),
        Some(mime_normalize_kind),
        1,
        JSPROP_ENUMERATE as u32,
    );

    JS_DefineProperty3(
        cx,
        bun_obj,
        c"Mime".as_ptr(),
        mime_ctor.handle(),
        JSPROP_ENUMERATE as u32,
    );
}
