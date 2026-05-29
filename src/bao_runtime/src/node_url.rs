use ::std::ffi::CString;
use ::std::ptr::NonNull;

use mozjs::conversions::jsstr_to_string;
use mozjs::jsapi::*;
use mozjs::jsval::{JSVal, UndefinedValue, BooleanValue, ObjectValue, PrivateValue, StringValue};
use mozjs::rooted;
use mozjs::rust::wrappers2 as w2;

use crate::require::cache_builtin;

struct UrlState {
    href: String,
    protocol: String,
    username: String,
    password: String,
    host: String,
    hostname: String,
    port: String,
    pathname: String,
    search: String,
    hash: String,
    origin: String,
}

fn parse_url(input: &str, base: Option<&str>) -> Option<UrlState> {
    let input = input.trim();
    if input.is_empty() {
        return None;
    }

    let full_url = if input.starts_with("data:") || input.starts_with("blob:") {
        return Some(UrlState {
            href: input.to_string(),
            protocol: input.split(':').next().unwrap_or("").to_string() + ":",
            username: String::new(),
            password: String::new(),
            host: String::new(),
            hostname: String::new(),
            port: String::new(),
            pathname: input.splitn(2, ':').nth(1).unwrap_or("").to_string(),
            search: String::new(),
            hash: String::new(),
            origin: "null".to_string(),
        });
    } else {
        let (base_parts, _actual_base) = if let Some(b) = base {
            (Some(parse_url(b, None)?), b.to_string())
        } else {
            (None, String::new())
        };

        let has_scheme = input.contains("://") || input.starts_with("//");
        let working = if has_scheme {
            input.to_string()
        } else if input.starts_with("/") {
            if let Some(bp) = &base_parts {
                format!("{}://{}{}", bp.protocol.trim_end_matches(':'), bp.host, input)
            } else {
                return None;
            }
        } else if input.starts_with("?") || input.starts_with("#") {
            if let Some(bp) = &base_parts {
                format!("{}://{}{}{}", bp.protocol.trim_end_matches(':'), bp.host, bp.pathname, input)
            } else {
                return None;
            }
        } else {
            if let Some(bp) = &base_parts {
                let dir = if bp.pathname.contains('/') {
                    bp.pathname.rsplit_once('/').map(|(d, _)| d).unwrap_or("")
                } else {
                    ""
                };
                format!("{}://{}{}/{}", bp.protocol.trim_end_matches(':'), bp.host, dir, input)
            } else {
                return None;
            }
        };
        working
    };

    let full_url = full_url;
    let (scheme_rest, hash) = if let Some(pos) = full_url.find('#') {
        (&full_url[..pos], full_url[pos..].to_string())
    } else {
        (full_url.as_str(), String::new())
    };

    let (scheme_rest, search) = if let Some(pos) = scheme_rest.find('?') {
        (&scheme_rest[..pos], scheme_rest[pos..].to_string())
    } else {
        (scheme_rest, String::new())
    };

    let (scheme, authority_path) = if let Some(pos) = scheme_rest.find("://") {
        (&scheme_rest[..pos], &scheme_rest[pos + 3..])
    } else if scheme_rest.starts_with("//") {
        ("", &scheme_rest[2..])
    } else {
        return None;
    };

    let (authority, pathname) = if let Some(slash_pos) = authority_path.find('/') {
        (&authority_path[..slash_pos], authority_path[slash_pos..].to_string())
    } else {
        (authority_path, "/".to_string())
    };

    let (userinfo, host_port) = if let Some(at_pos) = authority.rfind('@') {
        (&authority[..at_pos], &authority[at_pos + 1..])
    } else {
        ("", authority)
    };

    let (username, password) = if !userinfo.is_empty() {
        if let Some(colon_pos) = userinfo.find(':') {
            (userinfo[..colon_pos].to_string(), userinfo[colon_pos + 1..].to_string())
        } else {
            (userinfo.to_string(), String::new())
        }
    } else {
        (String::new(), String::new())
    };

    let (hostname, port) = if host_port.starts_with('[') {
        if let Some(bracket_end) = host_port.find(']') {
            let host = host_port[..=bracket_end].to_string();
            let port = if bracket_end + 1 < host_port.len() && host_port.as_bytes()[bracket_end + 1] == b':' {
                host_port[bracket_end + 2..].to_string()
            } else {
                String::new()
            };
            (host, port)
        } else {
            (host_port.to_string(), String::new())
        }
    } else if let Some(colon_pos) = host_port.rfind(':') {
        (host_port[..colon_pos].to_string(), host_port[colon_pos + 1..].to_string())
    } else {
        (host_port.to_string(), String::new())
    };

    let host = if port.is_empty() { hostname.clone() } else { format!("{}:{}", hostname, port) };

    let protocol = if scheme.is_empty() { "http:".to_string() } else { format!("{}:", scheme) };

    let origin = if hostname.is_empty() {
        "null".to_string()
    } else {
        format!("{}//{}", protocol, host)
    };

    let href = format!("{}//{}{}{}{}", protocol, host, pathname, search, hash);

    Some(UrlState {
        href,
        protocol,
        username,
        password,
        host,
        hostname,
        port,
        pathname,
        search,
        hash,
        origin,
    })
}

const SLOT_URL: u32 = 0;
const URL_CLASS: JSClass = JSClass {
    name: b"URL\0".as_ptr() as *const ::std::os::raw::c_char,
    flags: (1 << JSCLASS_RESERVED_SLOTS_SHIFT) as u32,
    cOps: ::std::ptr::null(),
    spec: ::std::ptr::null(),
    ext: ::std::ptr::null(),
    oOps: ::std::ptr::null(),
};

const URL_SEARCH_PARAMS_CLASS: JSClass = JSClass {
    name: b"URLSearchParams\0".as_ptr() as *const ::std::os::raw::c_char,
    flags: (1 << JSCLASS_RESERVED_SLOTS_SHIFT) as u32,
    cOps: ::std::ptr::null(),
    spec: ::std::ptr::null(),
    ext: ::std::ptr::null(),
    oOps: ::std::ptr::null(),
};

fn set_url_state(obj: *mut JSObject, state: Box<UrlState>) {
    unsafe {
        let val = PrivateValue(Box::into_raw(state) as *const ::std::os::raw::c_void);
        JS_SetReservedSlot(obj, SLOT_URL, &val);
    }
}

unsafe fn set_string_prop(cx: *mut JSContext, obj: *mut JSObject, name: &str, value: &str) { unsafe {
    let Ok(c_name) = CString::new(name) else { return };
    let js_str = JS_NewStringCopyN(cx, value.as_ptr() as *const ::std::os::raw::c_char, value.len());
    if !js_str.is_null() {
        let val = StringValue(&*js_str);
        let obj_h = Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &obj };
        let val_h = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &val };
        JS_DefineProperty(cx, obj_h, c_name.as_ptr(), val_h, (JSPROP_ENUMERATE | JSPROP_READONLY) as u32);
    }
}}

unsafe fn url_to_js<'a>(cx: *mut JSContext, state: &UrlState) -> *mut JSObject { unsafe {
    let obj = JS_NewObject(cx, &URL_CLASS);
    if obj.is_null() {
        return obj;
    }

    set_string_prop(cx, obj, "href", &state.href);
    set_string_prop(cx, obj, "protocol", &state.protocol);
    set_string_prop(cx, obj, "username", &state.username);
    set_string_prop(cx, obj, "password", &state.password);
    set_string_prop(cx, obj, "host", &state.host);
    set_string_prop(cx, obj, "hostname", &state.hostname);
    set_string_prop(cx, obj, "port", &state.port);
    set_string_prop(cx, obj, "pathname", &state.pathname);
    set_string_prop(cx, obj, "search", &state.search);
    set_string_prop(cx, obj, "hash", &state.hash);
    set_string_prop(cx, obj, "origin", &state.origin);

    // searchParams — create a URLSearchParams object with get/has/toString
    {
        let sp_obj = mozjs_sys::jsapi::JS_NewPlainObject(cx);
        if !sp_obj.is_null() {
            let search_str = if state.search.starts_with('?') { &state.search[1..] } else { &state.search };
            let pairs: Vec<(String, String)> = if search_str.is_empty() {
                Vec::new()
            } else {
                search_str.split('&').filter_map(|p| {
                    let mut parts = p.splitn(2, '=');
                    let k = parts.next()?.to_string();
                    let v = parts.next().unwrap_or("").to_string();
                    Some((k, v))
                }).collect()
            };
            for (k, v) in &pairs {
                let Ok(c_k) = CString::new(k.as_str()) else { continue };
                let Ok(c_v) = CString::new(v.as_str()) else { continue };
                let vs = JS_NewStringCopyZ(cx, c_v.as_ptr());
                if !vs.is_null() {
                    let vv = StringValue(&*vs);
                    let vv_h = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &vv };
                    let sp_h = Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &sp_obj };
                    JS_DefineProperty(cx, sp_h, c_k.as_ptr(), vv_h, JSPROP_ENUMERATE as u32);
                }
            }
            let sp_fn = JS_NewFunction(cx, Some(sp_get), 1, 0, c"get".as_ptr());
            if !sp_fn.is_null() {
                let fn_obj = JS_GetFunctionObject(sp_fn);
                let fv = ObjectValue(fn_obj);
                let fv_h = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &fv };
                let sp_h = Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &sp_obj };
                JS_DefineProperty(cx, sp_h, c"get".as_ptr(), fv_h, JSPROP_ENUMERATE as u32);
            }
            let has_fn = JS_NewFunction(cx, Some(sp_has), 1, 0, c"has".as_ptr());
            if !has_fn.is_null() {
                let fn_obj = JS_GetFunctionObject(has_fn);
                let fv = ObjectValue(fn_obj);
                let fv_h = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &fv };
                let sp_h = Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &sp_obj };
                JS_DefineProperty(cx, sp_h, c"has".as_ptr(), fv_h, JSPROP_ENUMERATE as u32);
            }
            let sp_val = ObjectValue(sp_obj);
            let sp_val_h = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &sp_val };
            let obj_h = Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &obj };
            JS_DefineProperty(cx, obj_h, c"searchParams".as_ptr(), sp_val_h, (JSPROP_ENUMERATE | JSPROP_READONLY) as u32);
        }
    }

    let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    rooted!(&in(wrapped_cx) let obj_r = obj);
    w2::JS_DefineFunction(&mut wrapped_cx, obj_r.handle(), c"toString".as_ptr(), Some(url_to_string), 0, 0);
    w2::JS_DefineFunction(&mut wrapped_cx, obj_r.handle(), c"toJSON".as_ptr(), Some(url_to_string), 0, 0);

    obj
}}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn url_to_string(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let this = args.thisv();
    if !this.is_object() { args.rval().set(UndefinedValue()); return true; }
    let obj = this.to_object();
    let mut href_val = UndefinedValue();
    let obj_h = Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &obj };
    JS_GetProperty(cx, obj_h, c"href".as_ptr(), MutableHandle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &mut href_val });
    args.rval().set(href_val);
    true
}

pub fn install(cx: &mut mozjs::context::JSContext, global: mozjs::rust::Handle<*mut JSObject>) {
    unsafe {
        let url_fun = JS_NewFunction(cx.raw_cx(), Some(url_constructor), 2, JSFUN_CONSTRUCTOR as u32, c"URL".as_ptr());
        if !url_fun.is_null() {
            let url_obj = JS_GetFunctionObject(url_fun);
            if !url_obj.is_null() {
                let val = ObjectValue(url_obj);
                rooted!(&in(cx) let v = val);
                JS_DefineProperty(cx.raw_cx(), global.into(), c"URL".as_ptr(), v.handle().into(), (JSPROP_ENUMERATE | JSPROP_PERMANENT) as u32);
            }
        }

        let sp_fun = JS_NewFunction(cx.raw_cx(), Some(url_search_params_constructor), 1, JSFUN_CONSTRUCTOR as u32, c"URLSearchParams".as_ptr());
        if !sp_fun.is_null() {
            let sp_obj = JS_GetFunctionObject(sp_fun);
            if !sp_obj.is_null() {
                let val = ObjectValue(sp_obj);
                rooted!(&in(cx) let v = val);
                JS_DefineProperty(cx.raw_cx(), global.into(), c"URLSearchParams".as_ptr(), v.handle().into(), (JSPROP_ENUMERATE | JSPROP_PERMANENT) as u32);
            }
        }
    }

    rooted!(&in(cx) let url_mod = unsafe { w2::JS_NewPlainObject(cx) });
    if !url_mod.get().is_null() {
        let mod_h = url_mod.handle().into();
        let url_ctor = unsafe { JS_NewFunction(cx.raw_cx(), Some(url_constructor), 2, JSFUN_CONSTRUCTOR as u32, c"URL".as_ptr()) };
        if !url_ctor.is_null() {
            let ctor_obj = unsafe { JS_GetFunctionObject(url_ctor) };
            if !ctor_obj.is_null() {
                let val = ObjectValue(ctor_obj);
                let val_h = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &val };
                unsafe { JS_DefineProperty(cx.raw_cx(), mod_h, c"URL".as_ptr(), val_h, JSPROP_ENUMERATE as u32); }
            }
        }
        let sp_ctor = unsafe { JS_NewFunction(cx.raw_cx(), Some(url_search_params_constructor), 1, JSFUN_CONSTRUCTOR as u32, c"URLSearchParams".as_ptr()) };
        if !sp_ctor.is_null() {
            let ctor_obj = unsafe { JS_GetFunctionObject(sp_ctor) };
            if !ctor_obj.is_null() {
                let val = ObjectValue(ctor_obj);
                let val_h = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &val };
                unsafe { JS_DefineProperty(cx.raw_cx(), mod_h, c"URLSearchParams".as_ptr(), val_h, JSPROP_ENUMERATE as u32); }
            }
        }
        unsafe {
            JS_DefineFunction(cx.raw_cx(), mod_h, c"parse".as_ptr(), Some(url_parse_fn), 2, JSPROP_ENUMERATE as u32);
            JS_DefineFunction(cx.raw_cx(), mod_h, c"format".as_ptr(), Some(url_format_fn), 1, JSPROP_ENUMERATE as u32);
            JS_DefineFunction(cx.raw_cx(), mod_h, c"resolve".as_ptr(), Some(url_resolve_fn), 2, JSPROP_ENUMERATE as u32);
        }
        cache_builtin("url", url_mod.get());
    }
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn url_constructor(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    if argc == 0 {
        JS_ReportErrorUTF8(cx, b"URL requires at least 1 argument\0".as_ptr() as *const ::std::os::raw::c_char);
        return false;
    }

    let input_val = *args.get(0).ptr;
    if !input_val.is_string() {
        JS_ReportErrorUTF8(cx, b"URL first argument must be a string\0".as_ptr() as *const ::std::os::raw::c_char);
        return false;
    }
    let input = crate::js_to_rust_string(cx, input_val);

    let base = if argc > 1 {
        let base_val = *args.get(1).ptr;
        if base_val.is_string() {
            Some(crate::js_to_rust_string(cx, base_val))
        } else {
            None
        }
    } else {
        None
    };

    let state = match parse_url(&input, base.as_deref()) {
        Some(s) => s,
        None => {
            let msg = format!("Invalid URL: {}", input);
            let c_msg = CString::new(msg).unwrap_or_default();
            JS_ReportErrorUTF8(cx, b"%s\0".as_ptr() as *const ::std::os::raw::c_char, c_msg.as_ptr());
            return false;
        }
    };

    let obj = url_to_js(cx, &state);
    if obj.is_null() {
        args.rval().set(UndefinedValue());
        return true;
    }

    set_url_state(obj, Box::new(state));
    args.rval().set(ObjectValue(obj));
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn url_search_params_constructor(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);

    let obj = JS_NewObject(cx, &URL_SEARCH_PARAMS_CLASS);
    if obj.is_null() {
        args.rval().set(UndefinedValue());
        return true;
    }

    let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;
    rooted!(&in(cx_ref) let obj_r = obj);
    w2::JS_DefineFunction(cx_ref, obj_r.handle(), c"get".as_ptr(), Some(sp_get), 1, 0);
    w2::JS_DefineFunction(cx_ref, obj_r.handle(), c"set".as_ptr(), Some(sp_set), 2, 0);
    w2::JS_DefineFunction(cx_ref, obj_r.handle(), c"has".as_ptr(), Some(sp_has), 1, 0);
    w2::JS_DefineFunction(cx_ref, obj_r.handle(), c"delete".as_ptr(), Some(sp_delete), 1, 0);
    w2::JS_DefineFunction(cx_ref, obj_r.handle(), c"append".as_ptr(), Some(sp_append), 2, 0);
    w2::JS_DefineFunction(cx_ref, obj_r.handle(), c"toString".as_ptr(), Some(sp_to_string), 0, 0);
    w2::JS_DefineFunction(cx_ref, obj_r.handle(), c"keys".as_ptr(), Some(sp_keys), 0, 0);
    w2::JS_DefineFunction(cx_ref, obj_r.handle(), c"values".as_ptr(), Some(sp_values), 0, 0);
    w2::JS_DefineFunction(cx_ref, obj_r.handle(), c"entries".as_ptr(), Some(sp_entries), 0, 0);
    w2::JS_DefineFunction(cx_ref, obj_r.handle(), c"forEach".as_ptr(), Some(sp_for_each), 1, 0);

    if argc > 0 {
        let init_val = *args.get(0).ptr;
        if init_val.is_string() {
            let init_str = crate::js_to_rust_string(cx, init_val);
            let search = init_str.strip_prefix('?').unwrap_or(&init_str);
            for pair in search.split('&') {
                if pair.is_empty() { continue; }
                if let Some((k, v)) = pair.split_once('=') {
                    let _ = set_sp_property(cx, obj, k, v);
                } else {
                    let _ = set_sp_property(cx, obj, pair, "");
                }
            }
        }
    }

    args.rval().set(ObjectValue(obj));
    true
}

unsafe fn set_sp_property(cx: *mut JSContext, obj: *mut JSObject, key: &str, value: &str) -> bool { unsafe {
    let Ok(c_key) = CString::new(key) else { return false };
    let js_str = JS_NewStringCopyN(cx, value.as_ptr() as *const ::std::os::raw::c_char, value.len());
    if js_str.is_null() { return false; }
    let val = StringValue(&*js_str);
    let obj_h = Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &obj };
    let val_h = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &val };
    JS_SetProperty(cx, obj_h, c_key.as_ptr(), val_h)
}}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn sp_get(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let this = args.thisv();
    if !this.is_object() { args.rval().set(UndefinedValue()); return true; }
    let obj = this.to_object();
    if argc == 0 { args.rval().set(UndefinedValue()); return true; }
    let key_val = *args.get(0).ptr;
    if !key_val.is_string() { args.rval().set(UndefinedValue()); return true; }
    let key = crate::js_to_rust_string(cx, key_val);
    let Ok(c_key) = CString::new(key) else { args.rval().set(UndefinedValue()); return true; };
    let obj_h = Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &obj };
    let mut result = UndefinedValue();
    JS_GetProperty(cx, obj_h, c_key.as_ptr(), MutableHandle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &mut result });
    args.rval().set(result);
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn sp_set(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let this = args.thisv();
    if !this.is_object() { args.rval().set(UndefinedValue()); return true; }
    if argc < 2 { args.rval().set(UndefinedValue()); return true; }
    let key_val = *args.get(0).ptr;
    let val_val = *args.get(1).ptr;
    if !key_val.is_string() { args.rval().set(UndefinedValue()); return true; }
    let key = crate::js_to_rust_string(cx, key_val);
    let value = if val_val.is_string() { crate::js_to_rust_string(cx, val_val) } else { String::new() };
    set_sp_property(cx, this.to_object(), &key, &value);
    args.rval().set(UndefinedValue());
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn sp_has(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let this = args.thisv();
    if !this.is_object() { args.rval().set(BooleanValue(false)); return true; }
    if argc == 0 { args.rval().set(BooleanValue(false)); return true; }
    let key_val = *args.get(0).ptr;
    if !key_val.is_string() { args.rval().set(BooleanValue(false)); return true; }
    let key = crate::js_to_rust_string(cx, key_val);
    let Ok(c_key) = CString::new(key) else { args.rval().set(BooleanValue(false)); return true; };
    let obj = this.to_object();
    let obj_h = Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &obj };
    let mut found = false;
    JS_HasProperty(cx, obj_h, c_key.as_ptr(), &mut found);
    args.rval().set(BooleanValue(found));
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn sp_delete(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let this = args.thisv();
    if !this.is_object() { args.rval().set(BooleanValue(false)); return true; }
    if argc == 0 { args.rval().set(BooleanValue(false)); return true; }
    let key_val = *args.get(0).ptr;
    if !key_val.is_string() { args.rval().set(BooleanValue(false)); return true; }
    let key = crate::js_to_rust_string(cx, key_val);
    let Ok(c_key) = CString::new(key) else { args.rval().set(BooleanValue(false)); return true; };
    let obj = this.to_object();
    let obj_h = Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &obj };
    JS_DeleteProperty1(cx, obj_h, c_key.as_ptr());
    args.rval().set(BooleanValue(true));
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn sp_append(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let this = args.thisv();
    if !this.is_object() { args.rval().set(UndefinedValue()); return true; }
    if argc < 2 { args.rval().set(UndefinedValue()); return true; }
    let key_val = *args.get(0).ptr;
    let val_val = *args.get(1).ptr;
    if !key_val.is_string() { args.rval().set(UndefinedValue()); return true; }
    let key = crate::js_to_rust_string(cx, key_val);
    let value = if val_val.is_string() { crate::js_to_rust_string(cx, val_val) } else { String::new() };
    set_sp_property(cx, this.to_object(), &key, &value);
    args.rval().set(UndefinedValue());
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn sp_to_string(cx: *mut JSContext, _argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, _argc);
    let this = args.thisv();
    if !this.is_object() {
        let empty = JS_NewStringCopyZ(cx, b"\0".as_ptr() as *const ::std::os::raw::c_char);
        args.rval().set(if empty.is_null() { UndefinedValue() } else { StringValue(&*empty) });
        return true;
    }
    let obj = this.to_object();
    let obj_h = Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &obj };

    let mut init_val = UndefinedValue();
    JS_GetProperty(cx, obj_h, c"__initString".as_ptr(), MutableHandle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &mut init_val });
    if init_val.is_string() {
        args.rval().set(init_val);
    } else {
        let empty = JS_NewStringCopyZ(cx, b"\0".as_ptr() as *const ::std::os::raw::c_char);
        args.rval().set(if empty.is_null() { UndefinedValue() } else { StringValue(&*empty) });
    }
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn sp_keys(_cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    args.rval().set(UndefinedValue());
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn sp_values(_cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    args.rval().set(UndefinedValue());
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn sp_entries(_cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    args.rval().set(UndefinedValue());
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn sp_for_each(_cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    args.rval().set(UndefinedValue());
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn url_parse_fn(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    if argc == 0 || !(*args.get(0).ptr).is_string() {
        args.rval().set(mozjs::jsval::NullValue());
        return true;
    }
    let input = crate::js_to_rust_string(cx, *args.get(0).ptr);
    let _parse_slashes = if argc > 1 && (*args.get(1).ptr).is_boolean() {
        (*args.get(1).ptr).to_boolean()
    } else {
        false
    };

    let state = match parse_url(&input, None) {
        Some(s) => s,
        None => {
            args.rval().set(mozjs::jsval::NullValue());
            return true;
        }
    };

    let obj = mozjs_sys::jsapi::JS_NewPlainObject(cx);
    if obj.is_null() {
        args.rval().set(mozjs::jsval::NullValue());
        return true;
    }
    let obj_h = Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &obj };
    for (name, value) in [
        ("href", state.href.as_str()),
        ("protocol", state.protocol.as_str()),
        ("host", state.host.as_str()),
        ("hostname", state.hostname.as_str()),
        ("port", state.port.as_str()),
        ("pathname", state.pathname.as_str()),
        ("search", state.search.as_str()),
        ("hash", state.hash.as_str()),
        ("path", format!("{}{}", state.pathname, state.search).as_str()),
    ] {
        let Ok(c_name) = CString::new(name) else { continue };
        let js_str = JS_NewStringCopyN(cx, value.as_ptr() as *const ::std::os::raw::c_char, value.len());
        if !js_str.is_null() {
            let val = StringValue(&*js_str);
            let val_h = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &val };
            JS_DefineProperty(cx, obj_h, c_name.as_ptr(), val_h, JSPROP_ENUMERATE as u32);
        }
    }

    args.rval().set(ObjectValue(obj));
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn url_format_fn(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    if argc == 0 {
        let empty = JS_NewStringCopyZ(cx, b"\0".as_ptr() as *const ::std::os::raw::c_char);
        args.rval().set(if empty.is_null() { UndefinedValue() } else { StringValue(&*empty) });
        return true;
    }
    let input = *args.get(0).ptr;
    if input.is_string() {
        args.rval().set(input);
        return true;
    }
    if input.is_object() {
        let obj = input.to_object();
        let obj_h = Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &obj };
        let mut href_val = UndefinedValue();
        JS_GetProperty(cx, obj_h, c"href".as_ptr(), MutableHandle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &mut href_val });
        if href_val.is_string() {
            args.rval().set(href_val);
            return true;
        }
        let mut proto_val = UndefinedValue();
        let mut host_val = UndefinedValue();
        let mut path_val = UndefinedValue();
        JS_GetProperty(cx, obj_h, c"protocol".as_ptr(), MutableHandle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &mut proto_val });
        JS_GetProperty(cx, obj_h, c"host".as_ptr(), MutableHandle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &mut host_val });
        JS_GetProperty(cx, obj_h, c"path".as_ptr(), MutableHandle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &mut path_val });
        let proto = if proto_val.is_string() { crate::js_to_rust_string(cx, proto_val) } else { "http:".to_string() };
        let host = if host_val.is_string() { crate::js_to_rust_string(cx, host_val) } else { "localhost".to_string() };
        let path = if path_val.is_string() { crate::js_to_rust_string(cx, path_val) } else { "/".to_string() };
        let formatted = format!("{}//{}{}", proto, host, path);
        let Ok(c_str) = CString::new(formatted) else { args.rval().set(UndefinedValue()); return true; };
        let js_str = JS_NewStringCopyZ(cx, c_str.as_ptr());
        args.rval().set(if js_str.is_null() { UndefinedValue() } else { StringValue(&*js_str) });
        return true;
    }
    args.rval().set(UndefinedValue());
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn url_resolve_fn(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    if argc < 2 || !(*args.get(0).ptr).is_string() || !(*args.get(1).ptr).is_string() {
        args.rval().set(*args.get(0).ptr);
        return true;
    }
    let base = crate::js_to_rust_string(cx, *args.get(0).ptr);
    let relative = crate::js_to_rust_string(cx, *args.get(1).ptr);
    let resolved = match parse_url(&relative, Some(&base)) {
        Some(s) => s.href,
        None => relative,
    };
    let Ok(c_str) = CString::new(resolved) else { args.rval().set(UndefinedValue()); return true; };
    let js_str = JS_NewStringCopyZ(cx, c_str.as_ptr());
    args.rval().set(if js_str.is_null() { UndefinedValue() } else { StringValue(&*js_str) });
    true
}

