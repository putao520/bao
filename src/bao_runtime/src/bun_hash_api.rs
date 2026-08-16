// @trace REQ-ENG-006 [api:Bun.hash] — non-cryptographic hash face.
//
// Upstream semantics (runtime/api/HashObject.zig + bun.com/docs/runtime/hashing):
//   * `Bun.hash(input)` — wyhash (`std.hash.Wyhash` final4, seed 0) → **BigInt**
//     (64-bit hashes return BigInt, 32-bit return number).
//   * `Bun.hash(input, seed)` — optional integer seed.
//   * Algorithm variants are FUNCTION PROPERTIES on Bun.hash (NOT a string
//     algorithm parameter): wyhash / crc32 / adler32 / cityHash32 / cityHash64 /
//     xxHash32 / xxHash64 / xxHash3 / murmur32v2 / murmur32v3 / murmur64v2 /
//     rapidhash.
//
// Engines: bun_wyhash (the same Wyhash the parser/router HashMaps hash with)
// + bun_hash (xxhash/cityhash/murmur/rapidhash/adler32) + crc32fast. The old
// sha256-hex-string face was a silent fake (audit: "returns sha256 hex STRING
// not wyhash number") — cryptographic hashing lives on Bun.CryptoHasher.
use mozjs::jsapi::*;
use mozjs::jsval::{DoubleValue, Int32Value, JSVal, UndefinedValue};
use mozjs::rooted;
use mozjs::rust::wrappers2::{BigIntFromUint64, JS_DefineProperty3};

/// Marshal a JSVal to hash input bytes: string (UTF-8) or
/// TypedArray/DataView/Buffer/ArrayBuffer (byte-exact via the shared
/// collect_byte_view). None for unrecognized input.
#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn hash_input_bytes(cx: *mut JSContext, val: JSVal) -> Option<Vec<u8>> {
    if val.is_string() {
        return Some(crate::js_to_rust_string(cx, val).into_bytes());
    }
    if val.is_object() {
        return crate::node_buffer::collect_byte_view(cx, val);
    }
    None
}

/// Optional integer seed (arg 1): number → u64 (negative i32 → two's
/// complement, matching upstream `toUInt64NoTruncate` on number inputs).
#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn hash_seed_arg(args: &CallArgs, arg_index: u32) -> u64 {
    if args.argc_ > arg_index {
        let v = *args.get(arg_index).ptr;
        if v.is_int32() {
            return v.to_int32() as i64 as u64;
        }
        if v.is_double() {
            let n = v.to_double();
            if n.is_finite() {
                return n as u64;
            }
        }
    }
    0
}

/// Emit a hash result with upstream 32/64-bit semantics:
/// u32 → JS number; u64 → JS BigInt (no-truncate, docs: "64-bit hashes
/// return a bigint").
#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn set_hash_result_u32(args: &CallArgs, v: u32) {
    args.rval().set(Int32Value(v as i32));
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn set_hash_result_u64(cx: *mut JSContext, args: &CallArgs, v: u64) {
    let mut wrapped = mozjs::context::JSContext::from_ptr(::std::ptr::NonNull::new_unchecked(cx));
    let big = BigIntFromUint64(&mut wrapped, v);
    if !big.is_null() {
        // BigIntValue builds the tagged JSVal directly from the BigInt
        // pointer. No allocation happens between creation and the rval
        // store, so the value stays reachable (rval is a rooted handle).
        args.rval().set(mozjs::jsval::BigIntValue(&*big));
        return;
    }
    // BigInt allocation failure — fall back to the double form rather than
    // crashing the call (explicit degradation, value identical modulo f64).
    args.rval().set(DoubleValue(v as f64));
}

/// The dispatch shape shared by Bun.hash and every named variant.
#[derive(Copy, Clone)]
enum HashVariant {
    Wyhash,
    Crc32,
    Adler32,
    CityHash32,
    CityHash64,
    XxHash32,
    XxHash64,
    XxHash3,
    Murmur32v2,
    Murmur32v3,
    Murmur64v2,
    Rapidhash,
}

impl HashVariant {
    fn is_64bit(self) -> bool {
        matches!(
            self,
            HashVariant::Wyhash
                | HashVariant::CityHash64
                | HashVariant::XxHash64
                | HashVariant::XxHash3
                | HashVariant::Murmur64v2
                | HashVariant::Rapidhash
        )
    }
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn run_hash(cx: *mut JSContext, args: &CallArgs, variant: HashVariant) -> bool {
    if args.argc_ == 0 {
        JS_ReportErrorUTF8(cx, c"Bun.hash() requires input".as_ptr());
        return false;
    }
    let input_val = *args.get(0).ptr;
    let Some(bytes) = hash_input_bytes(cx, input_val) else {
        JS_ReportErrorUTF8(
            cx,
            c"Bun.hash() input must be a string, TypedArray, DataView or ArrayBuffer".as_ptr(),
        );
        return false;
    };
    let seed = hash_seed_arg(args, 1);

    if variant.is_64bit() {
        let v: u64 = match variant {
            HashVariant::Wyhash => bun_wyhash::Wyhash::hash(seed, &bytes),
            HashVariant::CityHash64 => bun_hash::CityHash64::hash(&bytes),
            HashVariant::XxHash64 => bun_hash::XxHash64::hash(seed, &bytes),
            HashVariant::XxHash3 => bun_hash::XxHash3::hash(seed, &bytes),
            HashVariant::Murmur64v2 => bun_hash::Murmur2_64::hash_with_seed(&bytes, seed),
            HashVariant::Rapidhash => bun_hash::RapidHash::hash(seed, &bytes),
            _ => unreachable!("64-bit discriminant"),
        };
        set_hash_result_u64(cx, args, v);
    } else {
        let v: u32 = match variant {
            HashVariant::Crc32 => {
                let mut h = crc32fast::Hasher::new();
                h.update(&bytes);
                h.finalize()
            }
            HashVariant::Adler32 => bun_hash::Adler32::hash(&bytes),
            HashVariant::CityHash32 => bun_hash::CityHash32::hash(&bytes),
            HashVariant::XxHash32 => bun_hash::XxHash32::hash(seed as u32, &bytes),
            HashVariant::Murmur32v2 => bun_hash::Murmur2_32::hash_with_seed(&bytes, seed as u32),
            HashVariant::Murmur32v3 => bun_hash::Murmur3_32::hash_with_seed(&bytes, seed as u32),
            _ => unreachable!("32-bit discriminant"),
        };
        set_hash_result_u32(args, v);
    }
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn bun_hash_fn(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    run_hash(cx, &args, HashVariant::Wyhash)
}

macro_rules! hash_variant_fn {
    ($fname:ident, $variant:expr) => {
        #[allow(unsafe_op_in_unsafe_fn)]
        unsafe extern "C" fn $fname(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
            let args = CallArgs::from_vp(vp, argc);
            run_hash(cx, &args, $variant)
        }
    };
}

hash_variant_fn!(hash_wyhash, HashVariant::Wyhash);
hash_variant_fn!(hash_crc32, HashVariant::Crc32);
hash_variant_fn!(hash_adler32, HashVariant::Adler32);
hash_variant_fn!(hash_city32, HashVariant::CityHash32);
hash_variant_fn!(hash_city64, HashVariant::CityHash64);
hash_variant_fn!(hash_xx32, HashVariant::XxHash32);
hash_variant_fn!(hash_xx64, HashVariant::XxHash64);
hash_variant_fn!(hash_xx3, HashVariant::XxHash3);
hash_variant_fn!(hash_murmur32v2, HashVariant::Murmur32v2);
hash_variant_fn!(hash_murmur32v3, HashVariant::Murmur32v3);
hash_variant_fn!(hash_murmur64v2, HashVariant::Murmur64v2);
hash_variant_fn!(hash_rapidhash, HashVariant::Rapidhash);

type NativeFn = unsafe extern "C" fn(*mut JSContext, u32, *mut JSVal) -> bool;

/// Install `Bun.hash` (+ named algorithm variants) on the Bun object.
///
/// # Safety
/// Caller must ensure `cx` is a valid JSContext and `bun_obj` a live object.
pub unsafe fn install(
    cx: &mut mozjs::context::JSContext,
    bun_obj: mozjs::rust::Handle<*mut JSObject>,
) {
    let hash_fn = JS_NewFunction(cx.raw_cx(), Some(bun_hash_fn), 2, 0, c"hash".as_ptr());
    if hash_fn.is_null() {
        return;
    }
    let fn_obj = JS_GetFunctionObject(hash_fn);
    if fn_obj.is_null() {
        return;
    }
    rooted!(&in(cx) let hash_obj = fn_obj);

    // Named variants (upstream `fns` tuple in HashObject.create).
    let variants: &[(&::std::ffi::CStr, NativeFn)] = &[
        (c"wyhash", hash_wyhash),
        (c"crc32", hash_crc32),
        (c"adler32", hash_adler32),
        (c"cityHash32", hash_city32),
        (c"cityHash64", hash_city64),
        (c"xxHash32", hash_xx32),
        (c"xxHash64", hash_xx64),
        (c"xxHash3", hash_xx3),
        (c"murmur32v2", hash_murmur32v2),
        (c"murmur32v3", hash_murmur32v3),
        (c"murmur64v2", hash_murmur64v2),
        (c"rapidhash", hash_rapidhash),
    ];
    for (name, fp) in variants {
        let vfn = JS_NewFunction(cx.raw_cx(), Some(*fp), 2, 0, name.as_ptr());
        if vfn.is_null() {
            continue;
        }
        let vobj = JS_GetFunctionObject(vfn);
        if vobj.is_null() {
            continue;
        }
        rooted!(&in(cx) let vobj_root = vobj);
        JS_DefineProperty3(
            cx,
            hash_obj.handle(),
            name.as_ptr(),
            vobj_root.handle(),
            JSPROP_ENUMERATE as u32,
        );
    }

    JS_DefineProperty3(
        cx,
        bun_obj,
        c"hash".as_ptr(),
        hash_obj.handle(),
        JSPROP_ENUMERATE as u32,
    );
    let _ = UndefinedValue();
}
