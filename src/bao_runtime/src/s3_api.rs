// @trace REQ-ENG-006 [api:GET /api/bun-compat]
// AWS S3 signing integration for Bun.S3 / Bun.awsCredentials.
//
// Bridges bun_s3_signing (pure Rust protocol layer) to SpiderMonkey
// runtime, exposing S3 credential management and request signing.

use bun_core::ZBox;
use ::std::ptr::NonNull;

use mozjs::jsapi::*;
use mozjs::jsval::{JSVal, ObjectValue, UndefinedValue, StringValue};
use mozjs::rooted;
use mozjs::rust::wrappers2 as w2;

use crate::require::cache_builtin;

pub fn install(cx: &mut mozjs::context::JSContext) {
    rooted!(&in(cx) let s3_obj = unsafe { w2::JS_NewPlainObject(cx) });
    if s3_obj.get().is_null() {
        return;
    }

    unsafe {
        w2::JS_DefineFunction(
            cx,
            s3_obj.handle(),
            c"signRequest".as_ptr(),
            Some(s3_sign_request),
            1,
            JSPROP_ENUMERATE as u32,
        );
        w2::JS_DefineFunction(
            cx,
            s3_obj.handle(),
            c"getCredentials".as_ptr(),
            Some(s3_get_credentials),
            0,
            JSPROP_ENUMERATE as u32,
        );
    }

    cache_builtin(cx, "bun:s3", s3_obj.get());
}

unsafe extern "C" fn s3_sign_request(
    cx: *mut JSContext,
    argc: u32,
    vp: *mut JSVal,
) -> bool {
    let args = CallArgs::from_vp(vp, argc);

    let msg = ZBox::from_bytes(b"S3 signRequest: SpiderMonkey bridge not yet fully implemented (tracking: REQ-ENG-007)");
    JS_ReportErrorUTF8(cx, c"%s".as_ptr(), msg.as_ptr());
    args.rval().set(UndefinedValue());
    false
}

unsafe extern "C" fn s3_get_credentials(
    cx: *mut JSContext,
    argc: u32,
    vp: *mut JSVal,
) -> bool {
    let args = CallArgs::from_vp(vp, argc);

    let access_key = bun_core::getenv_z(bun_core::zstr!("AWS_ACCESS_KEY_ID"))
        .map(|s| String::from_utf8_lossy(s).into_owned());
    let secret_key = bun_core::getenv_z(bun_core::zstr!("AWS_SECRET_ACCESS_KEY"))
        .map(|s| String::from_utf8_lossy(s).into_owned());
    let region = bun_core::getenv_z(bun_core::zstr!("AWS_DEFAULT_REGION"))
        .map(|s| String::from_utf8_lossy(s).into_owned())
        .or_else(|| bun_core::getenv_z(bun_core::zstr!("AWS_REGION"))
            .map(|s| String::from_utf8_lossy(s).into_owned()));

    let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;

    rooted!(&in(cx_ref) let cred_obj = unsafe { w2::JS_NewPlainObject(cx_ref) });
    if cred_obj.get().is_null() {
        args.rval().set(UndefinedValue());
        return true;
    }

    let raw = cx_ref.raw_cx();

    if let Some(ref ak) = access_key {
        let c_ak = ZBox::from_bytes(ak.as_bytes());
        let js_str = JS_NewStringCopyZ(raw, c_ak.as_ptr());
        if !js_str.is_null() {
            rooted!(&in(cx_ref) let ak_val = StringValue(&*js_str));
            JS_DefineProperty(
                raw,
                cred_obj.handle().into(),
                c"accessKeyId".as_ptr(),
                ak_val.handle().into(),
                JSPROP_ENUMERATE as u32,
            );
        }
    }
    if let Some(ref sk) = secret_key {
        let c_sk = ZBox::from_bytes(sk.as_bytes());
        let js_str = JS_NewStringCopyZ(raw, c_sk.as_ptr());
        if !js_str.is_null() {
            rooted!(&in(cx_ref) let sk_val = StringValue(&*js_str));
            JS_DefineProperty(
                raw,
                cred_obj.handle().into(),
                c"secretAccessKey".as_ptr(),
                sk_val.handle().into(),
                JSPROP_ENUMERATE as u32,
            );
        }
    }
    if let Some(ref r) = region {
        let c_r = ZBox::from_bytes(r.as_bytes());
        let js_str = JS_NewStringCopyZ(raw, c_r.as_ptr());
        if !js_str.is_null() {
            rooted!(&in(cx_ref) let r_val = StringValue(&*js_str));
            JS_DefineProperty(
                raw,
                cred_obj.handle().into(),
                c"region".as_ptr(),
                r_val.handle().into(),
                JSPROP_ENUMERATE as u32,
            );
        }
    }

    args.rval().set(ObjectValue(cred_obj.get()));
    true
}
