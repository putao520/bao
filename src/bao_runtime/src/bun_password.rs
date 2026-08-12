// @trace REQ-ENG-14 [api:Bun.password] — password hashing (argon2id/argon2i/argon2d/bcrypt).
//
// Bridges workspace `rust-argon2` and `bcrypt` crates to SpiderMonkey host
// functions. Bun.password is a namespace object with four methods:
//   - hash(password, options?)       → Promise<string>  (async)
//   - hashSync(password, options?)   → string           (sync)
//   - verify(password, hash, options?) → Promise<bool>  (async)
//   - verifySync(password, hash, options?) → bool       (sync)
//
// Async versions currently resolve synchronously (compute + resolve Promise
// immediately). They will become truly async once integrated with the event
// loop's microtask queue.

use ::std::borrow::Cow;
use ::std::ptr::{self, NonNull};

use mozjs::jsapi::*;
use mozjs::jsval::{BooleanValue, JSVal, ObjectValue, StringValue, UndefinedValue};
use mozjs::rooted;
use mozjs::rust::wrappers2::{JS_DefineFunction, JS_DefineProperty3, JS_NewPlainObject};

use bun_core::ZBox;

// ── Algorithm identifiers ──────────────────────────────────────────────

const ALGO_ARGON2ID: &str = "argon2id";
const ALGO_ARGON2I: &str = "argon2i";
const ALGO_ARGON2D: &str = "argon2d";
const ALGO_BCRYPT: &str = "bcrypt";

// ── Default parameters (matching Bun upstream) ─────────────────────────

/// argon2id defaults: time_cost=2, memory_cost=65536 KiB (64 MiB), parallelism=1
const ARGON2_DEFAULT_TIME_COST: u32 = 2;
const ARGON2_DEFAULT_MEMORY_COST: u32 = 65536; // KiB
const ARGON2_DEFAULT_PARALLELISM: u32 = 1;
const ARGON2_HASH_LENGTH: u32 = 32;
const ARGON2_SALT_LEN: usize = 32;

/// bcrypt default cost (log2 rounds)
const BCRYPT_DEFAULT_COST: u32 = 10;

// ── Verification safety limits (matching Bun upstream pwhash.rs) ───────

const MAX_VERIFY_TIME_COST: u32 = 1 << 16;
const MAX_VERIFY_MEMORY_COST: u32 = 1 << 22;
const MAX_VERIFY_PARALLELISM: u32 = 64;

// ── Install Bun.password namespace ─────────────────────────────────────

/// Install `Bun.password` namespace object on the given `bun_obj`.
///
/// # Safety
///
/// Caller must ensure `cx` is a valid JSContext pointer and `bun_obj` is a
/// valid handle to a JSObject.
pub unsafe fn install(
    cx: &mut mozjs::context::JSContext,
    bun_obj: mozjs::rust::Handle<*mut JSObject>,
) {
    rooted!(&in(cx) let pwd_obj = JS_NewPlainObject(cx));
    if pwd_obj.get().is_null() {
        return;
    }

    // Bun.password.hash(password, options?) → Promise<string>
    JS_DefineFunction(
        cx,
        pwd_obj.handle(),
        c"hash".as_ptr(),
        Some(pwd_hash),
        2,
        JSPROP_ENUMERATE as u32,
    );
    // Bun.password.hashSync(password, options?) → string
    JS_DefineFunction(
        cx,
        pwd_obj.handle(),
        c"hashSync".as_ptr(),
        Some(pwd_hash_sync),
        2,
        JSPROP_ENUMERATE as u32,
    );
    // Bun.password.verify(password, hash, options?) → Promise<bool>
    JS_DefineFunction(
        cx,
        pwd_obj.handle(),
        c"verify".as_ptr(),
        Some(pwd_verify),
        3,
        JSPROP_ENUMERATE as u32,
    );
    // Bun.password.verifySync(password, hash, options?) → bool
    JS_DefineFunction(
        cx,
        pwd_obj.handle(),
        c"verifySync".as_ptr(),
        Some(pwd_verify_sync),
        3,
        JSPROP_ENUMERATE as u32,
    );

    JS_DefineProperty3(
        cx,
        bun_obj,
        c"password".as_ptr(),
        pwd_obj.handle(),
        JSPROP_ENUMERATE as u32,
    );
}

// ── JS argument helpers ────────────────────────────────────────────────

/// Extract a JS string argument as Rust String. Returns None if not a string
/// or index out of bounds.
unsafe fn get_string_arg(
    cx: *mut JSContext,
    args: &CallArgs,
    index: u32,
    argc: u32,
) -> Option<String> {
    if index >= argc {
        return None;
    }
    let val = *args.get(index).ptr;
    if !val.is_string() {
        return None;
    }
    let ptr = val.to_string();
    NonNull::new(ptr).map(|nn| mozjs::conversions::jsstr_to_string(cx, nn))
}

/// Algorithm and parameters parsed from the optional `options` argument.
struct HashOptions {
    algorithm: String,
    time_cost: u32,
    memory_cost: u32,
    parallelism: u32,
    cost: u32, // bcrypt cost (log2 rounds)
}

impl Default for HashOptions {
    fn default() -> Self {
        HashOptions {
            algorithm: ALGO_ARGON2ID.to_string(),
            time_cost: ARGON2_DEFAULT_TIME_COST,
            memory_cost: ARGON2_DEFAULT_MEMORY_COST,
            parallelism: ARGON2_DEFAULT_PARALLELISM,
            cost: BCRYPT_DEFAULT_COST,
        }
    }
}

/// Parse the optional second argument (options object or algorithm string).
unsafe fn parse_hash_options(cx: *mut JSContext, args: &CallArgs, argc: u32) -> HashOptions {
    let mut opts = HashOptions::default();

    if argc < 2 {
        return opts;
    }

    let val = *args.get(1).ptr;

    // Short form: Bun.password.hash("pw", "argon2id")
    if val.is_string() {
        let ptr = val.to_string();
        if let Some(nn) = NonNull::new(ptr) {
            opts.algorithm = mozjs::conversions::jsstr_to_string(cx, nn);
        }
        return opts;
    }

    // Object form: Bun.password.hash("pw", { algorithm: "argon2id", ... })
    if val.is_object() {
        let obj = val.to_object();
        let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
        let cx_ref = &mut wrapped_cx;

        // algorithm
        if let Some(algo) = get_string_property(cx_ref, obj, "algorithm") {
            opts.algorithm = algo;
        }

        // timeCost
        if let Some(v) = get_uint_property(cx_ref, obj, "timeCost") {
            opts.time_cost = v;
        }

        // memoryCost
        if let Some(v) = get_uint_property(cx_ref, obj, "memoryCost") {
            opts.memory_cost = v;
        }

        // parallelism
        if let Some(v) = get_uint_property(cx_ref, obj, "parallelism") {
            opts.parallelism = v;
        }

        // cost (bcrypt)
        if let Some(v) = get_uint_property(cx_ref, obj, "cost") {
            opts.cost = v;
        }
    }

    opts
}

/// Parse verify options (third argument). Only `algorithm` is read;
/// if absent, the algorithm is auto-detected from the hash prefix.
unsafe fn parse_verify_options(cx: *mut JSContext, args: &CallArgs, argc: u32) -> Option<String> {
    if argc < 3 {
        return None;
    }
    let val = *args.get(2).ptr;

    if val.is_string() {
        let ptr = val.to_string();
        return NonNull::new(ptr).map(|nn| mozjs::conversions::jsstr_to_string(cx, nn));
    }

    if val.is_object() {
        let obj = val.to_object();
        let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
        let cx_ref = &mut wrapped_cx;
        return get_string_property(cx_ref, obj, "algorithm");
    }

    None
}

/// Read a string property from a JS object.
/// Pattern matches node_fs.rs `get_hidden_int` / `define_*_prop` style.
unsafe fn get_string_property(
    cx: &mut mozjs::context::JSContext,
    obj: *mut JSObject,
    name: &str,
) -> Option<String> {
    let c_name = ZBox::from_bytes(name.as_bytes());
    rooted!(&in(cx) let obj_rooted = obj);
    let mut val = UndefinedValue();
    JS_GetProperty(
        cx.raw_cx(),
        obj_rooted.handle().into(),
        c_name.as_ptr(),
        MutableHandle::<Value> {
            _phantom_0: ::std::marker::PhantomData,
            ptr: &mut val,
        },
    );
    if !val.is_string() {
        return None;
    }
    let ptr = val.to_string();
    NonNull::new(ptr).map(|nn| mozjs::conversions::jsstr_to_string(cx.raw_cx(), nn))
}

/// Read a u32 property from a JS object.
unsafe fn get_uint_property(
    cx: &mut mozjs::context::JSContext,
    obj: *mut JSObject,
    name: &str,
) -> Option<u32> {
    let c_name = ZBox::from_bytes(name.as_bytes());
    rooted!(&in(cx) let obj_rooted = obj);
    let mut val = UndefinedValue();
    JS_GetProperty(
        cx.raw_cx(),
        obj_rooted.handle().into(),
        c_name.as_ptr(),
        MutableHandle::<Value> {
            _phantom_0: ::std::marker::PhantomData,
            ptr: &mut val,
        },
    );
    if val.is_int32() {
        Some(val.to_int32() as u32)
    } else if val.is_double() {
        Some(val.to_double() as u32)
    } else {
        None
    }
}

/// Throw a JS Error with the given message.
unsafe fn throw_error(cx: *mut JSContext, msg: &str) {
    let c_msg = ZBox::from_bytes(msg.as_bytes());
    JS_ReportErrorUTF8(cx, c"%s".as_ptr(), c_msg.as_ptr());
}

/// Auto-detect algorithm from the hash string prefix.
fn detect_algorithm(hash: &str) -> &'static str {
    if hash.starts_with("$argon2") {
        if hash.starts_with("$argon2id$") {
            ALGO_ARGON2ID
        } else if hash.starts_with("$argon2i$") {
            ALGO_ARGON2I
        } else if hash.starts_with("$argon2d$") {
            ALGO_ARGON2D
        } else {
            ALGO_ARGON2ID
        }
    } else if hash.starts_with("$2") || hash.starts_with("$bcrypt$") {
        ALGO_BCRYPT
    } else {
        ALGO_ARGON2ID
    }
}

// ── Core hash/verify logic ─────────────────────────────────────────────

/// Compute a password hash. Returns the PHC-encoded (argon2) or modular-crypt
/// (bcrypt) hash string.
fn compute_hash(password: &str, opts: &HashOptions) -> ::std::result::Result<String, String> {
    match opts.algorithm.as_str() {
        ALGO_ARGON2ID | ALGO_ARGON2I | ALGO_ARGON2D => compute_argon2_hash(password, opts),
        ALGO_BCRYPT => compute_bcrypt_hash(password, opts),
        other => Err(format!("Unknown algorithm: {}", other)),
    }
}

fn compute_argon2_hash(
    password: &str,
    opts: &HashOptions,
) -> ::std::result::Result<String, String> {
    use argon2::{Config, ThreadMode, Variant, Version};

    let variant = match opts.algorithm.as_str() {
        ALGO_ARGON2ID => Variant::Argon2id,
        ALGO_ARGON2I => Variant::Argon2i,
        ALGO_ARGON2D => Variant::Argon2d,
        _ => Variant::Argon2id,
    };

    let config = Config {
        ad: &[],
        hash_length: ARGON2_HASH_LENGTH,
        lanes: opts.parallelism,
        mem_cost: opts.memory_cost,
        secret: &[],
        thread_mode: ThreadMode::Sequential,
        time_cost: opts.time_cost,
        variant,
        version: Version::Version13,
    };

    let mut salt = [0u8; ARGON2_SALT_LEN];
    getrandom::fill(&mut salt).map_err(|e| format!("Failed to generate salt: {}", e))?;

    argon2::hash_encoded(password.as_bytes(), &salt, &config)
        .map_err(|e| format!("argon2 hash failed: {}", e))
}

fn compute_bcrypt_hash(
    password: &str,
    opts: &HashOptions,
) -> ::std::result::Result<String, String> {
    bcrypt::hash(password, opts.cost).map_err(|e| format!("bcrypt hash failed: {}", e))
}

/// Verify a password against a hash. Returns true on match.
fn compute_verify(
    password: &str,
    hash: &str,
    algorithm: &str,
) -> ::std::result::Result<bool, String> {
    match algorithm {
        ALGO_ARGON2ID | ALGO_ARGON2I | ALGO_ARGON2D => compute_argon2_verify(password, hash),
        ALGO_BCRYPT => compute_bcrypt_verify(password, hash),
        other => Err(format!("Unknown algorithm: {}", other)),
    }
}

fn compute_argon2_verify(
    password: &str,
    encoded_hash: &str,
) -> ::std::result::Result<bool, String> {
    if !encoded_hash.is_ascii() {
        return Err("InvalidEncoding: hash must be ASCII".to_string());
    }

    // Normalise version: if no v= segment, splice in v=19$ so rust-argon2 uses
    // Version13 (matching Zig/Bun semantics). Also validate v=19 only.
    let normalised: Cow<'_, str> = 'norm: {
        let Some(after_dollar) = encoded_hash.strip_prefix('$') else {
            break 'norm Cow::Borrowed(encoded_hash);
        };
        let Some(sep) = after_dollar.find('$') else {
            break 'norm Cow::Borrowed(encoded_hash);
        };
        let alg_end = 1 + sep;
        let rest = &encoded_hash[alg_end + 1..];
        if let Some(v) = rest.strip_prefix("v=") {
            let end = v.find('$').unwrap_or(v.len());
            if &v[..end] != "19" {
                return Err("InvalidEncoding: only argon2 v=19 (0x13) is supported".to_string());
            }
            Cow::Borrowed(encoded_hash)
        } else {
            let mut s = String::with_capacity(encoded_hash.len() + 5);
            s.push_str(&encoded_hash[..=alg_end]);
            s.push_str("v=19$");
            s.push_str(rest);
            Cow::Owned(s)
        }
    };

    // Validate parameter safety limits before delegating to avoid DoS.
    if let Some(after_dollar) = normalised.strip_prefix('$') {
        if let Some(sep) = after_dollar.find('$') {
            let mut rest = &after_dollar[sep + 1..];
            if let Some(after_version) = rest.strip_prefix("v=") {
                rest = match after_version.find('$') {
                    Some(end) => &after_version[end + 1..],
                    None => "",
                };
            }
            let params = &rest[..rest.find('$').unwrap_or(rest.len())];
            for pair in params.split(',') {
                let Some((key, value)) = pair.split_once('=') else {
                    continue;
                };
                let Ok(value) = value.parse::<u32>() else {
                    continue;
                };
                let limit = match key {
                    "m" => MAX_VERIFY_MEMORY_COST,
                    "t" => MAX_VERIFY_TIME_COST,
                    "p" => MAX_VERIFY_PARALLELISM,
                    _ => continue,
                };
                if value > limit {
                    return Err(format!(
                        "WeakParameters: {}={} exceeds limit {}",
                        key, value, limit
                    ));
                }
            }
        }
    }

    match argon2::verify_encoded(&normalised, password.as_bytes()) {
        Ok(matched) => Ok(matched),
        Err(e) => Err(format!("argon2 verify failed: {}", e)),
    }
}

fn compute_bcrypt_verify(password: &str, hash: &str) -> ::std::result::Result<bool, String> {
    if !hash.is_ascii() {
        return Err("InvalidEncoding: hash must be ASCII".to_string());
    }
    match bcrypt::verify(password, hash) {
        Ok(matched) => Ok(matched),
        Err(e) => Err(format!("bcrypt verify failed: {}", e)),
    }
}

// ── Promise helpers ────────────────────────────────────────────────────
// Pattern follows node_fs.rs resolve_undefined / reject_with_error.

/// Create a new Promise, resolve it with a string value, and return it.
unsafe fn resolved_promise_string(cx: *mut JSContext, value: &str) -> *mut JSObject {
    let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;

    rooted!(&in(cx_ref) let promise = mozjs_sys::jsapi::JS::NewPromiseObject(cx, HandleObject::null()));
    if promise.get().is_null() {
        return ptr::null_mut();
    }

    let c_val = ZBox::from_bytes(value.as_bytes());
    let js_str = JS_NewStringCopyZ(cx, c_val.as_ptr());
    if js_str.is_null() {
        return promise.get();
    }
    rooted!(&in(cx_ref) let val = StringValue(&*js_str));
    mozjs_sys::jsapi::JS::ResolvePromise(cx, promise.handle().into(), val.handle().into());

    promise.get()
}

/// Create a new Promise, resolve it with a boolean value, and return it.
unsafe fn resolved_promise_bool(cx: *mut JSContext, value: bool) -> *mut JSObject {
    let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;

    rooted!(&in(cx_ref) let promise = mozjs_sys::jsapi::JS::NewPromiseObject(cx, HandleObject::null()));
    if promise.get().is_null() {
        return ptr::null_mut();
    }

    rooted!(&in(cx_ref) let val = BooleanValue(value));
    mozjs_sys::jsapi::JS::ResolvePromise(cx, promise.handle().into(), val.handle().into());

    promise.get()
}

/// Create a new Promise, reject it with an Error object, and return it.
unsafe fn rejected_promise(cx: *mut JSContext, msg: &str) -> *mut JSObject {
    let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;

    rooted!(&in(cx_ref) let promise = mozjs_sys::jsapi::JS::NewPromiseObject(cx, HandleObject::null()));
    if promise.get().is_null() {
        return ptr::null_mut();
    }

    rooted!(&in(cx_ref) let err_obj = JS_NewPlainObject(cx_ref));
    if !err_obj.get().is_null() {
        let c_msg = ZBox::from_bytes(msg.as_bytes());
        let js_str = JS_NewStringCopyZ(cx, c_msg.as_ptr());
        if !js_str.is_null() {
            rooted!(&in(cx_ref) let msg_val = StringValue(&*js_str));
            JS_DefineProperty(
                cx,
                err_obj.handle().into(),
                c"message".as_ptr(),
                msg_val.handle().into(),
                JSPROP_ENUMERATE as u32,
            );
        }
    }
    rooted!(&in(cx_ref) let err_val = ObjectValue(err_obj.get()));
    mozjs_sys::jsapi::JS::RejectPromise(cx, promise.handle().into(), err_val.handle().into());

    promise.get()
}

// ── Host functions ─────────────────────────────────────────────────────

/// Bun.password.hash(password, options?) → Promise<string>
#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn pwd_hash(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);

    let password = match get_string_arg(cx, &args, 0, argc) {
        Some(p) => p,
        None => {
            throw_error(cx, "Bun.password.hash requires a password string");
            return false;
        }
    };

    let opts = parse_hash_options(cx, &args, argc);

    match compute_hash(&password, &opts) {
        ::std::result::Result::Ok(hash) => {
            let promise = resolved_promise_string(cx, &hash);
            if promise.is_null() {
                args.rval().set(UndefinedValue());
            } else {
                args.rval().set(ObjectValue(promise));
            }
        }
        ::std::result::Result::Err(e) => {
            let promise = rejected_promise(cx, &e);
            if promise.is_null() {
                args.rval().set(UndefinedValue());
            } else {
                args.rval().set(ObjectValue(promise));
            }
        }
    }
    true
}

/// Bun.password.hashSync(password, options?) → string
#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn pwd_hash_sync(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);

    let password = match get_string_arg(cx, &args, 0, argc) {
        Some(p) => p,
        None => {
            throw_error(cx, "Bun.password.hashSync requires a password string");
            return false;
        }
    };

    let opts = parse_hash_options(cx, &args, argc);

    match compute_hash(&password, &opts) {
        ::std::result::Result::Ok(hash) => {
            let c_hash = ZBox::from_bytes(hash.as_bytes());
            let js_str = JS_NewStringCopyZ(cx, c_hash.as_ptr());
            args.rval().set(if js_str.is_null() {
                UndefinedValue()
            } else {
                StringValue(&*js_str)
            });
        }
        ::std::result::Result::Err(e) => {
            throw_error(cx, &e);
            return false;
        }
    }
    true
}

/// Bun.password.verify(password, hash, options?) → Promise<bool>
#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn pwd_verify(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);

    let password = match get_string_arg(cx, &args, 0, argc) {
        Some(p) => p,
        None => {
            throw_error(cx, "Bun.password.verify requires a password string");
            return false;
        }
    };

    let hash = match get_string_arg(cx, &args, 1, argc) {
        Some(h) => h,
        None => {
            throw_error(cx, "Bun.password.verify requires a hash string");
            return false;
        }
    };

    let algorithm = parse_verify_options(cx, &args, argc)
        .unwrap_or_else(|| detect_algorithm(&hash).to_string());

    match compute_verify(&password, &hash, &algorithm) {
        ::std::result::Result::Ok(matched) => {
            let promise = resolved_promise_bool(cx, matched);
            if promise.is_null() {
                args.rval().set(UndefinedValue());
            } else {
                args.rval().set(ObjectValue(promise));
            }
        }
        ::std::result::Result::Err(e) => {
            let promise = rejected_promise(cx, &e);
            if promise.is_null() {
                args.rval().set(UndefinedValue());
            } else {
                args.rval().set(ObjectValue(promise));
            }
        }
    }
    true
}

/// Bun.password.verifySync(password, hash, options?) → bool
#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn pwd_verify_sync(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);

    let password = match get_string_arg(cx, &args, 0, argc) {
        Some(p) => p,
        None => {
            throw_error(cx, "Bun.password.verifySync requires a password string");
            return false;
        }
    };

    let hash = match get_string_arg(cx, &args, 1, argc) {
        Some(h) => h,
        None => {
            throw_error(cx, "Bun.password.verifySync requires a hash string");
            return false;
        }
    };

    let algorithm = parse_verify_options(cx, &args, argc)
        .unwrap_or_else(|| detect_algorithm(&hash).to_string());

    match compute_verify(&password, &hash, &algorithm) {
        ::std::result::Result::Ok(matched) => {
            args.rval().set(BooleanValue(matched));
        }
        ::std::result::Result::Err(e) => {
            throw_error(cx, &e);
            return false;
        }
    }
    true
}
