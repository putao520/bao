use ::std::ffi::CString;
use ::std::ptr::NonNull;

use mozjs::conversions::jsstr_to_string;
use mozjs::glue::JS_GetReservedSlot;
use mozjs::jsapi::*;
use mozjs::jsval::{JSVal, UndefinedValue, BooleanValue, ObjectValue, PrivateValue, StringValue, Int32Value};
use mozjs::rooted;
use mozjs::rust::wrappers2 as w2;
use mozjs::rust::IdVector;

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

/// Get the UrlState from a URL object's reserved slot.
unsafe fn get_url_state(obj: *mut JSObject) -> Option<Box<UrlState>> {
    unsafe {
        let mut slot = UndefinedValue();
        JS_GetReservedSlot(obj, SLOT_URL, &mut slot);
        if slot.is_double() {
            let ptr = slot.to_private() as *mut UrlState;
            if !ptr.is_null() { return Some(Box::from_raw(ptr)); }
        }
        None
    }
}

fn set_url_state(obj: *mut JSObject, state: Box<UrlState>) {
    unsafe {
        let val = PrivateValue(Box::into_raw(state) as *const ::std::os::raw::c_void);
        JS_SetReservedSlot(obj, SLOT_URL, &val);
    }
}

/// Define a read-only string property on a JS object (for origin etc).
unsafe fn set_string_prop(cx: *mut JSContext, obj: *mut JSObject, name: &str, value: &str) { unsafe {
    let Ok(c_name) = CString::new(name) else { return };
    let js_str = JS_NewStringCopyN(cx, value.as_ptr() as *const ::std::os::raw::c_char, value.len());
    if !js_str.is_null() {
        let val = StringValue(&*js_str);
        let obj_h = Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &obj };
        let val_h = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &val };
        JS_DefineProperty(cx, obj_h, c_name.as_ptr(), val_h, JSPROP_ENUMERATE as u32);
    }
}}

/// Helper: read a string field from UrlState by name.
fn url_state_get_field(state: &UrlState, field: &str) -> String {
    match field {
        "href" => state.href.clone(),
        "protocol" => state.protocol.clone(),
        "username" => state.username.clone(),
        "password" => state.password.clone(),
        "host" => state.host.clone(),
        "hostname" => state.hostname.clone(),
        "port" => state.port.clone(),
        "pathname" => state.pathname.clone(),
        "search" => state.search.clone(),
        "hash" => state.hash.clone(),
        "origin" => state.origin.clone(),
        _ => String::new(),
    }
}

/// Helper: build a new href by replacing one field in the UrlState.
fn rebuild_href(state: &UrlState, field: &str, new_val: &str) -> String {
    let protocol = if field == "protocol" { new_val } else { &state.protocol };
    let username = if field == "username" { new_val } else { &state.username };
    let password = if field == "password" { new_val } else { &state.password };
    let hostname = if field == "hostname" { new_val } else { &state.hostname };
    let port = if field == "port" { new_val } else { &state.port };
    let pathname = if field == "pathname" { new_val } else { &state.pathname };
    let search = if field == "search" { new_val } else { &state.search };
    let hash = if field == "hash" { new_val } else { &state.hash };

    if field == "href" {
        return new_val.to_string();
    }

    let host = if port.is_empty() { hostname.to_string() } else { format!("{}:{}", hostname, port) };
    let auth = if username.is_empty() {
        String::new()
    } else if password.is_empty() {
        format!("{}@", username)
    } else {
        format!("{}:{}@", username, password)
    };

    if field == "protocol" {
        format!("{}//{}{}{}{}{}", protocol, auth, host, pathname, search, hash)
    } else if field == "hostname" || field == "port" || field == "username" || field == "password" {
        format!("{}//{}{}{}{}{}", protocol, auth, host, pathname, search, hash)
    } else {
        format!("{}//{}{}{}{}{}", protocol, auth, host, pathname, search, hash)
    }
}

/// Generic URL property setter — modifies UrlState, re-parses, and syncs all properties.
#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn url_prop_set(cx: *mut JSContext, obj: *mut JSObject, field: &str, new_val: &str) -> bool {
    let state = match get_url_state(obj) {
        Some(s) => s,
        None => return false,
    };

    let new_href = rebuild_href(&state, field, new_val);

    // Re-parse from the new href
    let new_state = if let Some(parsed) = parse_url(&new_href, None) {
        parsed
    } else {
        // If re-parse fails, just update the field directly in existing state
        let mut updated = *state;
        match field {
            "href" => updated.href = new_val.to_string(),
            "protocol" => updated.protocol = new_val.to_string(),
            "username" => updated.username = new_val.to_string(),
            "password" => updated.password = new_val.to_string(),
            "hostname" => updated.hostname = new_val.to_string(),
            "port" => updated.port = new_val.to_string(),
            "pathname" => updated.pathname = new_val.to_string(),
            "search" => updated.search = new_val.to_string(),
            "hash" => updated.hash = new_val.to_string(),
            _ => {}
        }
        updated
    };

    // Store updated state — getters will automatically return new values
    set_url_state(obj, Box::new(new_state));

    // Update the computed read-only properties (host, origin) that don't have getters
    let updated_state = match get_url_state(obj) {
        Some(s) => s,
        None => return true,
    };
    let obj_h = Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &obj };
    for (name, value) in [
        ("host", updated_state.host.as_str()),
        ("origin", updated_state.origin.as_str()),
    ] {
        let Ok(c_name) = CString::new(name) else { continue };
        let js_str = JS_NewStringCopyN(cx, value.as_ptr() as *const ::std::os::raw::c_char, value.len());
        if !js_str.is_null() {
            let val = StringValue(&*js_str);
            let val_h = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &val };
            JS_SetProperty(cx, obj_h, c_name.as_ptr(), val_h);
        }
    }
    set_url_state(obj, updated_state);
    true
}

/// Define a URL property with getter and optional setter.
/// The getter reads from UrlState; the setter updates UrlState and syncs computed props.
unsafe fn define_url_prop(cx: *mut JSContext, obj: *mut JSObject, name: &str, _initial_value: &str, getter: JSNative, setter: JSNative) { unsafe {
    let Ok(c_name) = CString::new(name) else { return };

    let attrs = if setter.is_none() {
        (JSPROP_ENUMERATE | JSPROP_READONLY) as u32
    } else {
        JSPROP_ENUMERATE as u32
    };

    let obj_h = Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &obj };
    JS_DefineProperty1(cx, obj_h, c_name.as_ptr(), getter, setter, attrs);
}}

// Individual getter/setter for each URL property.
// Each getter reads the UrlState from reserved slot and returns the field.
// Each setter modifies the field, rebuilds href, re-parses, and syncs.

macro_rules! url_prop_accessors {
    ($($name:ident => $field:literal),* $(,)?) => {
        $(
            #[allow(unsafe_op_in_unsafe_fn)]
            unsafe extern "C" fn $name(cx: *mut JSContext, _argc: u32, vp: *mut JSVal) -> bool {
                let args = CallArgs::from_vp(vp, _argc);
                let this = args.thisv();
                if !this.is_object() { args.rval().set(UndefinedValue()); return true; }
                let obj = this.to_object();
                let state = get_url_state(obj);
                if let Some(state) = state {
                    let val = url_state_get_field(&state, $field);
                    let js_str = JS_NewStringCopyN(cx, val.as_ptr() as *const ::std::os::raw::c_char, val.len());
                    set_url_state(obj, state);
                    if !js_str.is_null() {
                        args.rval().set(StringValue(&*js_str));
                    } else {
                        args.rval().set(UndefinedValue());
                    }
                } else {
                    args.rval().set(UndefinedValue());
                }
                true
            }
        )*
    };
}

// Generate getter functions for all URL properties
url_prop_accessors! {
    url_get_href => "href",
    url_get_protocol => "protocol",
    url_get_username => "username",
    url_get_password => "password",
    url_get_host => "host",
    url_get_hostname => "hostname",
    url_get_port => "port",
    url_get_pathname => "pathname",
    url_get_search => "search",
    url_get_hash => "hash",
    url_get_origin => "origin",
}

macro_rules! url_prop_setters {
    ($($name:ident => $field:literal),* $(,)?) => {
        $(
            #[allow(unsafe_op_in_unsafe_fn)]
            unsafe extern "C" fn $name(cx: *mut JSContext, _argc: u32, vp: *mut JSVal) -> bool {
                let args = CallArgs::from_vp(vp, _argc);
                let this = args.thisv();
                if !this.is_object() { return true; }
                let obj = this.to_object();
                if _argc == 0 { return true; }
                let val = *args.get(0).ptr;
                let new_val = if val.is_string() { crate::js_to_rust_string(cx, val) } else { String::new() };
                url_prop_set(cx, obj, $field, &new_val);
                true
            }
        )*
    };
}

// Generate setter functions for mutable URL properties
url_prop_setters! {
    url_set_href => "href",
    url_set_protocol => "protocol",
    url_set_username => "username",
    url_set_password => "password",
    url_set_hostname => "hostname",
    url_set_port => "port",
    url_set_pathname => "pathname",
    url_set_search => "search",
    url_set_hash => "hash",
}


unsafe fn url_to_js<'a>(cx: *mut JSContext, state: &UrlState) -> *mut JSObject { unsafe {
    let obj = JS_NewObject(cx, &URL_CLASS);
    if obj.is_null() {
        return obj;
    }

    // Store UrlState in reserved slot for getter/setter access
    set_url_state(obj, Box::new(UrlState {
        href: state.href.clone(),
        protocol: state.protocol.clone(),
        username: state.username.clone(),
        password: state.password.clone(),
        host: state.host.clone(),
        hostname: state.hostname.clone(),
        port: state.port.clone(),
        pathname: state.pathname.clone(),
        search: state.search.clone(),
        hash: state.hash.clone(),
        origin: state.origin.clone(),
    }));

    // Define mutable properties with getter/setter
    define_url_prop(cx, obj, "href", &state.href, Some(url_get_href), Some(url_set_href));
    define_url_prop(cx, obj, "protocol", &state.protocol, Some(url_get_protocol), Some(url_set_protocol));
    define_url_prop(cx, obj, "username", &state.username, Some(url_get_username), Some(url_set_username));
    define_url_prop(cx, obj, "password", &state.password, Some(url_get_password), Some(url_set_password));
    define_url_prop(cx, obj, "hostname", &state.hostname, Some(url_get_hostname), Some(url_set_hostname));
    define_url_prop(cx, obj, "port", &state.port, Some(url_get_port), Some(url_set_port));
    define_url_prop(cx, obj, "pathname", &state.pathname, Some(url_get_pathname), Some(url_set_pathname));
    define_url_prop(cx, obj, "search", &state.search, Some(url_get_search), Some(url_set_search));
    define_url_prop(cx, obj, "hash", &state.hash, Some(url_get_hash), Some(url_set_hash));

    // host and origin are computed from other fields, read-only with getter only
    define_url_prop(cx, obj, "host", &state.host, Some(url_get_host), None);
    define_url_prop(cx, obj, "origin", &state.origin, Some(url_get_origin), None);

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
                let sp_h = Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &sp_obj };
                let mut existing = UndefinedValue();
                JS_GetProperty(cx, sp_h, c_k.as_ptr(), MutableHandle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &mut existing });
                let combined = if existing.is_string() {
                    let prev = crate::js_to_rust_string(cx, existing);
                    format!("{}\x01{}", prev, v)
                } else {
                    v.clone()
                };
                let Ok(c_v) = CString::new(combined) else { continue };
                let vs = JS_NewStringCopyZ(cx, c_v.as_ptr());
                if !vs.is_null() {
                    let vv = StringValue(&*vs);
                    let vv_h = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &vv };
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
            let set_fn = JS_NewFunction(cx, Some(sp_set), 2, 0, c"set".as_ptr());
            if !set_fn.is_null() {
                let fn_obj = JS_GetFunctionObject(set_fn);
                let fv = ObjectValue(fn_obj);
                let fv_h = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &fv };
                let sp_h = Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &sp_obj };
                JS_DefineProperty(cx, sp_h, c"set".as_ptr(), fv_h, JSPROP_ENUMERATE as u32);
            }
            let delete_fn = JS_NewFunction(cx, Some(sp_delete), 1, 0, c"delete".as_ptr());
            if !delete_fn.is_null() {
                let fn_obj = JS_GetFunctionObject(delete_fn);
                let fv = ObjectValue(fn_obj);
                let fv_h = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &fv };
                let sp_h = Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &sp_obj };
                JS_DefineProperty(cx, sp_h, c"delete".as_ptr(), fv_h, JSPROP_ENUMERATE as u32);
            }
            let append_fn = JS_NewFunction(cx, Some(sp_append), 2, 0, c"append".as_ptr());
            if !append_fn.is_null() {
                let fn_obj = JS_GetFunctionObject(append_fn);
                let fv = ObjectValue(fn_obj);
                let fv_h = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &fv };
                let sp_h = Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &sp_obj };
                JS_DefineProperty(cx, sp_h, c"append".as_ptr(), fv_h, JSPROP_ENUMERATE as u32);
            }
            let getall_fn = JS_NewFunction(cx, Some(sp_get_all), 1, 0, c"getAll".as_ptr());
            if !getall_fn.is_null() {
                let fn_obj = JS_GetFunctionObject(getall_fn);
                let fv = ObjectValue(fn_obj);
                let fv_h = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &fv };
                let sp_h = Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &sp_obj };
                JS_DefineProperty(cx, sp_h, c"getAll".as_ptr(), fv_h, JSPROP_ENUMERATE as u32);
            }
            let tostr_fn = JS_NewFunction(cx, Some(sp_to_string), 0, 0, c"toString".as_ptr());
            if !tostr_fn.is_null() {
                let fn_obj = JS_GetFunctionObject(tostr_fn);
                let fv = ObjectValue(fn_obj);
                let fv_h = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &fv };
                let sp_h = Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &sp_obj };
                JS_DefineProperty(cx, sp_h, c"toString".as_ptr(), fv_h, JSPROP_ENUMERATE as u32);
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
                let url_obj_h = Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &url_obj };
                JS_DefineFunction(cx.raw_cx(), url_obj_h, c"canParse".as_ptr(), Some(url_can_parse), 1, JSPROP_ENUMERATE as u32);
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

    rooted!(&in(cx) let url_mod = unsafe { mozjs_sys::jsapi::JS_NewPlainObject(cx.raw_cx()) });
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
        cache_builtin(cx, "url", url_mod.get());
    }
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn url_can_parse(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    if argc == 0 || !(*args.get(0).ptr).is_string() {
        args.rval().set(BooleanValue(false));
        return true;
    }
    let input = crate::js_to_rust_string(cx, *args.get(0).ptr);
    let base = if argc > 1 && (*args.get(1).ptr).is_string() {
        Some(crate::js_to_rust_string(cx, *args.get(1).ptr))
    } else {
        None
    };
    let can_parse = parse_url(&input, base.as_deref()).is_some();
    args.rval().set(BooleanValue(can_parse));
    true
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
    w2::JS_DefineFunction(cx_ref, obj_r.handle(), c"getAll".as_ptr(), Some(sp_get_all), 1, 0);

    if argc > 0 {
        let init_val = *args.get(0).ptr;
        if init_val.is_string() {
            let init_str = crate::js_to_rust_string(cx, init_val);
            let search = init_str.strip_prefix('?').unwrap_or(&init_str);
            for pair in search.split('&') {
                if pair.is_empty() { continue; }
                let (k, v) = pair.split_once('=').unwrap_or((pair, ""));
                let _ = append_sp_value(cx, obj, k, v);
            }
        } else if init_val.is_object() {
            let init_obj = init_val.to_object();
            // Check if it's array-like (has numeric indices) or a plain object
            let init_obj_h = Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &init_obj };
            let mut length_val = UndefinedValue();
            JS_GetProperty(cx, init_obj_h, c"length".as_ptr(), MutableHandle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &mut length_val });

            if length_val.is_number() {
                // Array form: [["key", "val"], ["key2", "val2"]]
                let len = if length_val.is_int32() { length_val.to_int32() as u32 } else { 0 };
                for i in 0..len {
                    let mut elem = UndefinedValue();
                    JS_GetElement(cx, init_obj_h, i, MutableHandle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &mut elem });
                    if !elem.is_object() { continue; }
                    let pair_obj = elem.to_object();
                    let pair_h = Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &pair_obj };
                    let mut k_val = UndefinedValue();
                    let mut v_val = UndefinedValue();
                    JS_GetElement(cx, pair_h, 0, MutableHandle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &mut k_val });
                    JS_GetElement(cx, pair_h, 1, MutableHandle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &mut v_val });
                    let key = if k_val.is_string() { crate::js_to_rust_string(cx, k_val) } else { continue };
                    let val = if v_val.is_string() { crate::js_to_rust_string(cx, v_val) } else { String::new() };
                    let _ = append_sp_value(cx, obj, &key, &val);
                }
            } else {
                // Object form: {key: "val", key2: "val2"}
                let mut ids = IdVector::new(cx);
                let ok = GetPropertyKeys(cx, init_obj_h, JSITER_OWNONLY as u32, ids.handle_mut());
                if ok {
                    for jsid in &*ids {
                        if !jsid.is_string() { continue; }
                        let key_str = jsid.to_string();
                        let key = jsstr_to_string(cx, NonNull::new_unchecked(key_str));
                        let Ok(c_key) = CString::new(&*key) else { continue };
                        let mut v_val = UndefinedValue();
                        JS_GetProperty(cx, init_obj_h, c_key.as_ptr(), MutableHandle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &mut v_val });
                        let val = if v_val.is_string() { crate::js_to_rust_string(cx, v_val) } else { String::new() };
                        let _ = append_sp_value(cx, obj, &key, &val);
                    }
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
    // Use JS_DefineProperty with JSPROP_ENUMERATE so GetPropertyKeys can find these
    // First try to set existing property (for update), fall back to define
    let mut found = false;
    JS_HasProperty(cx, obj_h, c_key.as_ptr(), &mut found);
    if found {
        JS_SetProperty(cx, obj_h, c_key.as_ptr(), val_h)
    } else {
        JS_DefineProperty(cx, obj_h, c_key.as_ptr(), val_h, (JSPROP_ENUMERATE as u32) | (JSPROP_RESOLVING as u32))
    }
}}

/// Append a value to a URLSearchParams key, joining with \x01 for multi-values.
unsafe fn append_sp_value(cx: *mut JSContext, obj: *mut JSObject, key: &str, value: &str) -> bool { unsafe {
    let Ok(c_key) = CString::new(key) else { return false };
    let obj_h = Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &obj };
    let mut existing = UndefinedValue();
    JS_GetProperty(cx, obj_h, c_key.as_ptr(), MutableHandle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &mut existing });
    let combined = if existing.is_string() {
        let prev = crate::js_to_rust_string(cx, existing);
        format!("{}\x01{}", prev, value)
    } else {
        value.to_string()
    };
    set_sp_property(cx, obj, key, &combined)
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
    if result.is_string() {
        let full = crate::js_to_rust_string(cx, result);
        let first = full.split('\x01').next().unwrap_or(&full);
        let decoded = url_decode(first);
        let Ok(c_dec) = CString::new(decoded) else { args.rval().set(UndefinedValue()); return true; };
        let js_str = JS_NewStringCopyZ(cx, c_dec.as_ptr());
        args.rval().set(if js_str.is_null() { UndefinedValue() } else { StringValue(&*js_str) });
    } else {
        args.rval().set(result);
    }
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
    if !found {
        args.rval().set(BooleanValue(false));
        return true;
    }
    // has(name, value) — check if a specific value exists for the key
    if argc >= 2 {
        let value_val = *args.get(1).ptr;
        let mut stored = UndefinedValue();
        JS_GetProperty(cx, obj_h, c_key.as_ptr(), MutableHandle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &mut stored });
        if !stored.is_string() {
            args.rval().set(BooleanValue(false));
            return true;
        }
        let stored_str = crate::js_to_rust_string(cx, stored);
        let target = if value_val.is_string() { url_decode(&crate::js_to_rust_string(cx, value_val)) } else { String::new() };
        let has_match = stored_str.split('\x01').any(|v| url_decode(v) == target);
        args.rval().set(BooleanValue(has_match));
        return true;
    }
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
    let obj = this.to_object();
    let Ok(c_key) = CString::new(&*key) else { args.rval().set(UndefinedValue()); return true; };
    let obj_h = Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &obj };
    let mut existing = UndefinedValue();
    JS_GetProperty(cx, obj_h, c_key.as_ptr(), MutableHandle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &mut existing });
    let combined = if existing.is_string() {
        let prev = crate::jsstr_to_rust_string(cx, existing.to_string());
        format!("{}\x01{}", prev, value)
    } else {
        value
    };
    set_sp_property(cx, obj, &key, &combined);
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

    let mut parts: Vec<String> = Vec::new();
    let mut ids = IdVector::new(cx);
    let ok = GetPropertyKeys(cx, obj_h, JSITER_OWNONLY as u32, ids.handle_mut());
    if ok {
        for jsid in &*ids {
            if !jsid.is_string() { continue; }
            let key_str = jsid.to_string();
            let key = jsstr_to_string(cx, NonNull::new_unchecked(key_str));
            if key.starts_with("__") { continue; }
            let Ok(c_key) = CString::new(&*key) else { continue };
            let mut val = UndefinedValue();
            JS_GetProperty(cx, obj_h, c_key.as_ptr(), MutableHandle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &mut val });
            if !val.is_string() { continue; }
            let val_str = crate::js_to_rust_string(cx, val);
            for v in val_str.split('\x01') {
                parts.push(format!("{}={}", url_encode(&key), url_encode(v)));
            }
        }
    }
    let result = parts.join("&");
    let c_result = CString::new(result).unwrap_or_default();
    let js_str = JS_NewStringCopyZ(cx, c_result.as_ptr());
    args.rval().set(if js_str.is_null() { UndefinedValue() } else { StringValue(&*js_str) });
    true
}

fn url_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => out.push(b as char),
            b' ' => out.push_str("+"),
            _ => out.push_str(&format!("%{:02X}", b)),
        }
    }
    out
}

fn url_decode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'+' {
            out.push(' ');
            i += 1;
        } else if bytes[i] == b'%' && i + 2 < bytes.len() {
            let hi = (bytes[i + 1] as char).to_digit(16);
            let lo = (bytes[i + 2] as char).to_digit(16);
            if let (Some(h), Some(l)) = (hi, lo) {
                out.push(char::from_u32(h * 16 + l).unwrap_or('?'));
                i += 3;
            } else {
                out.push(bytes[i] as char);
                i += 1;
            }
        } else {
            out.push(bytes[i] as char);
            i += 1;
        }
    }
    out
}

/// Collect (key, value) pairs from a URLSearchParams object using GetPropertyKeys.
/// Returns Vec of (key, all_values_joined_with_\x01).
unsafe fn sp_collect_entries(cx: *mut JSContext, obj: *mut JSObject) -> Vec<(String, String)> {
    let mut result: Vec<(String, String)> = Vec::new();
    let obj_h = Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &obj };
    let mut ids = IdVector::new(cx);
    let ok = GetPropertyKeys(cx, obj_h, JSITER_OWNONLY as u32, ids.handle_mut());
    if !ok { return result; }
    for jsid in &*ids {
        if !jsid.is_string() { continue; }
        let key_str = jsid.to_string();
        let key = jsstr_to_string(cx, NonNull::new_unchecked(key_str));
        if key.starts_with("__") { continue; }
        let Ok(c_key) = CString::new(&*key) else { continue };
        let mut val = UndefinedValue();
        JS_GetProperty(cx, obj_h, c_key.as_ptr(), MutableHandle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &mut val });
        if !val.is_string() { continue; }
        let val_str = crate::js_to_rust_string(cx, val);
        result.push((key, val_str));
    }
    result
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn sp_keys(cx: *mut JSContext, _argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, _argc);
    let this = args.thisv();
    if !this.is_object() { args.rval().set(UndefinedValue()); return true; }
    let obj = this.to_object();

    let wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    rooted!(&in(wrapped_cx) let arr_root = mozjs_sys::jsapi::JS_NewPlainObject(cx));
    if arr_root.get().is_null() { args.rval().set(UndefinedValue()); return true; }
    let arr = arr_root.get();
    let mut idx: u32 = 0;
    let entries = sp_collect_entries(cx, obj);
    for (key, val_str) in &entries {
        for v in val_str.split('\x01') {
            let _ = v;
            let js_key = JS_NewStringCopyN(cx, key.as_ptr() as *const ::std::os::raw::c_char, key.len());
            if js_key.is_null() { continue; }
            let key_val = StringValue(&*js_key);
            JS_DefineElement(cx, Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &arr },
                idx, Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &key_val }, JSPROP_ENUMERATE as u32);
            idx += 1;
        }
    }
    let len_val = Int32Value(idx as i32);
    JS_DefineProperty(cx, Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &arr },
        c"length".as_ptr(), Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &len_val }, JSPROP_ENUMERATE as u32);
    args.rval().set(ObjectValue(arr));
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn sp_values(cx: *mut JSContext, _argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, _argc);
    let this = args.thisv();
    if !this.is_object() { args.rval().set(UndefinedValue()); return true; }
    let obj = this.to_object();

    let wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    rooted!(&in(wrapped_cx) let arr_root = mozjs_sys::jsapi::JS_NewPlainObject(cx));
    if arr_root.get().is_null() { args.rval().set(UndefinedValue()); return true; }
    let arr = arr_root.get();
    let mut idx: u32 = 0;
    let entries = sp_collect_entries(cx, obj);
    for (_key, val_str) in &entries {
        for v in val_str.split('\x01') {
            let decoded = url_decode(v);
            let Ok(c_dec) = CString::new(decoded) else { continue };
            let js_val = JS_NewStringCopyZ(cx, c_dec.as_ptr());
            if js_val.is_null() { continue; }
            let v_val = StringValue(&*js_val);
            JS_DefineElement(cx, Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &arr },
                idx, Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &v_val }, JSPROP_ENUMERATE as u32);
            idx += 1;
        }
    }
    let len_val = Int32Value(idx as i32);
    JS_DefineProperty(cx, Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &arr },
        c"length".as_ptr(), Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &len_val }, JSPROP_ENUMERATE as u32);
    args.rval().set(ObjectValue(arr));
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn sp_entries(cx: *mut JSContext, _argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, _argc);
    let this = args.thisv();
    if !this.is_object() { args.rval().set(UndefinedValue()); return true; }
    let obj = this.to_object();

    let wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    rooted!(&in(wrapped_cx) let arr_root = mozjs_sys::jsapi::JS_NewPlainObject(cx));
    if arr_root.get().is_null() { args.rval().set(UndefinedValue()); return true; }
    let arr = arr_root.get();
    let mut idx: u32 = 0;
    let entries = sp_collect_entries(cx, obj);
    for (key, val_str) in &entries {
        for v in val_str.split('\x01') {
            let pair = mozjs_sys::jsapi::JS_NewPlainObject(cx);
            if pair.is_null() { continue; }
            let Ok(c_key) = CString::new(&**key) else { continue };
            let decoded = url_decode(v);
            let Ok(c_dec) = CString::new(decoded) else { continue };
            let js_key = JS_NewStringCopyZ(cx, c_key.as_ptr());
            let js_val = JS_NewStringCopyZ(cx, c_dec.as_ptr());
            if js_key.is_null() || js_val.is_null() { continue; }
            let key_val = StringValue(&*js_key);
            let v_val = StringValue(&*js_val);
            JS_DefineElement(cx, Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &pair },
                0u32, Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &key_val }, JSPROP_ENUMERATE as u32);
            JS_DefineElement(cx, Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &pair },
                1u32, Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &v_val }, JSPROP_ENUMERATE as u32);
            let pair_val = ObjectValue(pair);
            JS_DefineElement(cx, Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &arr },
                idx, Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &pair_val }, JSPROP_ENUMERATE as u32);
            idx += 1;
        }
    }
    let len_val = Int32Value(idx as i32);
    JS_DefineProperty(cx, Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &arr },
        c"length".as_ptr(), Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &len_val }, JSPROP_ENUMERATE as u32);
    args.rval().set(ObjectValue(arr));
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn sp_for_each(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let this = args.thisv();
    if !this.is_object() { args.rval().set(UndefinedValue()); return true; }
    if argc == 0 { args.rval().set(UndefinedValue()); return true; }
    let callback_val = *args.get(0).ptr;
    if !callback_val.is_object() { args.rval().set(UndefinedValue()); return true; }
    let callback_obj = callback_val.to_object();

    let this_obj = this.to_object();
    let this_obj_h = Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &this_obj };

    let mut ids = IdVector::new(cx);
    let ok = GetPropertyKeys(cx, this_obj_h, JSITER_OWNONLY as u32, ids.handle_mut());
    if !ok { args.rval().set(UndefinedValue()); return true; }

    for jsid in &*ids {
        if !jsid.is_string() { continue; }
        let key_str = jsid.to_string();
        let key = jsstr_to_string(cx, NonNull::new_unchecked(key_str));
        let Ok(c_key) = CString::new(&*key) else { continue };

        let mut val = UndefinedValue();
        JS_GetProperty(cx, this_obj_h, c_key.as_ptr(), MutableHandle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &mut val });
        if !val.is_string() { continue; }
        let val_rust = crate::js_to_rust_string(cx, val);

        for v in val_rust.split('\x01') {
            let decoded = url_decode(v);
            let Ok(c_dec) = CString::new(decoded) else { continue };
            let Ok(c_key2) = CString::new(&*key) else { continue };
            let v_js = JS_NewStringCopyZ(cx, c_dec.as_ptr());
            let key_js = JS_NewStringCopyZ(cx, c_key2.as_ptr());
            if v_js.is_null() || key_js.is_null() { continue; }

            let mut args_arr: [JSVal; 3] = [
                StringValue(&*v_js),
                StringValue(&*key_js),
                ObjectValue(this_obj),
            ];
            let handle_arr = HandleValueArray {
                length_: 3,
                elements_: args_arr.as_mut_ptr(),
            };
            let mut rval = UndefinedValue();
            JS_CallFunctionValue(
                cx,
                Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &this_obj }.into(),
                Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &ObjectValue(callback_obj) },
                &handle_arr,
                MutableHandle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &mut rval },
            );
        }
    }

    args.rval().set(UndefinedValue());
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn sp_get_all(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    // Return empty array if no key argument
    if argc == 0 || !(*args.get(0).ptr).is_string() {
        let arr = mozjs::jsapi::NewArrayObject1(cx, 0);
        args.rval().set(if arr.is_null() { UndefinedValue() } else { ObjectValue(arr) });
        return true;
    }
    let key = crate::js_to_rust_string(cx, *args.get(0).ptr);
    let this = args.thisv();
    if !this.is_object() {
        let arr = mozjs::jsapi::NewArrayObject1(cx, 0);
        args.rval().set(if arr.is_null() { UndefinedValue() } else { ObjectValue(arr) });
        return true;
    }
    let obj = this.to_object();
    let obj_h = Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &obj };

    let Ok(c_key) = CString::new(key) else {
        let arr = mozjs::jsapi::NewArrayObject1(cx, 0);
        args.rval().set(if arr.is_null() { UndefinedValue() } else { ObjectValue(arr) });
        return true;
    };

    let mut has = false;
    JS_HasProperty(cx, obj_h, c_key.as_ptr(), &mut has);
    if !has {
        let arr = mozjs::jsapi::NewArrayObject1(cx, 0);
        args.rval().set(if arr.is_null() { UndefinedValue() } else { ObjectValue(arr) });
        return true;
    }

    let mut val = UndefinedValue();
    JS_GetProperty(cx, obj_h, c_key.as_ptr(), MutableHandle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &mut val });
    if !val.is_string() {
        let arr = mozjs::jsapi::NewArrayObject1(cx, 0);
        args.rval().set(if arr.is_null() { UndefinedValue() } else { ObjectValue(arr) });
        return true;
    }

    let val_str = crate::js_to_rust_string(cx, val);
    let parts: Vec<String> = val_str.split('\x01').map(|p| url_decode(p)).collect();
    let arr = mozjs::jsapi::NewArrayObject1(cx, parts.len());
    if arr.is_null() { args.rval().set(UndefinedValue()); return true; }
    for (i, part) in parts.iter().enumerate() {
        let Ok(c_part) = CString::new(part.as_str()) else { continue };
        let js_str = JS_NewStringCopyZ(cx, c_part.as_ptr());
        if js_str.is_null() { continue; }
        let str_val = StringValue(&*js_str);
        JS_DefineElement(cx, Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &arr },
            i as u32, Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &str_val }, JSPROP_ENUMERATE as u32);
    }
    args.rval().set(ObjectValue(arr));
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
            // Try parsing as relative URL (pathname-only like /foo/bar?baz=quux#frag)
            let (path_part, hash) = if let Some(pos) = input.find('#') {
                (&input[..pos], input[pos..].to_string())
            } else {
                (input.as_str(), String::new())
            };
            let (pathname, search) = if let Some(pos) = path_part.find('?') {
                (&path_part[..pos], path_part[pos..].to_string())
            } else {
                (path_part, String::new())
            };
            if pathname.starts_with('/') {
                let obj = mozjs_sys::jsapi::JS_NewPlainObject(cx);
                if obj.is_null() { args.rval().set(mozjs::jsval::NullValue()); return true; }
                let obj_h = Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &obj };
                let null_val = mozjs::jsval::NullValue();
                for (name, value) in [
                    ("href", input.as_str()),
                    ("path", format!("{}{}", pathname, search).as_str()),
                    ("pathname", pathname),
                    ("search", search.as_str()),
                    ("hash", hash.as_str()),
                ] {
                    let Ok(c_name) = CString::new(name) else { continue };
                    let js_str = JS_NewStringCopyN(cx, value.as_ptr() as *const ::std::os::raw::c_char, value.len());
                    if !js_str.is_null() {
                        let val = StringValue(&*js_str);
                        let val_h = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &val };
                        JS_DefineProperty(cx, obj_h, c_name.as_ptr(), val_h, JSPROP_ENUMERATE as u32);
                    }
                }
                for name in ["protocol", "host", "hostname", "port", "auth"] {
                    let Ok(c_name) = CString::new(name) else { continue };
                    let null_h = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &null_val };
                    JS_DefineProperty(cx, obj_h, c_name.as_ptr(), null_h, JSPROP_ENUMERATE as u32);
                }
                args.rval().set(ObjectValue(obj));
                return true;
            }
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
    let auth = if !state.username.is_empty() {
        if state.password.is_empty() { state.username.clone() } else { format!("{}:{}", state.username, state.password) }
    } else {
        String::new()
    };
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
        ("auth", auth.as_str()),
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
        let mut hostname_val = UndefinedValue();
        let mut port_val = UndefinedValue();
        let mut path_val = UndefinedValue();
        let mut pathname_val = UndefinedValue();
        let mut search_val = UndefinedValue();
        let mut hash_val = UndefinedValue();
        let mut auth_val = UndefinedValue();
        JS_GetProperty(cx, obj_h, c"protocol".as_ptr(), MutableHandle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &mut proto_val });
        JS_GetProperty(cx, obj_h, c"host".as_ptr(), MutableHandle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &mut host_val });
        JS_GetProperty(cx, obj_h, c"hostname".as_ptr(), MutableHandle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &mut hostname_val });
        JS_GetProperty(cx, obj_h, c"port".as_ptr(), MutableHandle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &mut port_val });
        JS_GetProperty(cx, obj_h, c"path".as_ptr(), MutableHandle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &mut path_val });
        JS_GetProperty(cx, obj_h, c"pathname".as_ptr(), MutableHandle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &mut pathname_val });
        JS_GetProperty(cx, obj_h, c"search".as_ptr(), MutableHandle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &mut search_val });
        JS_GetProperty(cx, obj_h, c"hash".as_ptr(), MutableHandle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &mut hash_val });
        JS_GetProperty(cx, obj_h, c"auth".as_ptr(), MutableHandle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &mut auth_val });

        let proto = if proto_val.is_string() { crate::js_to_rust_string(cx, proto_val) } else { "http:".to_string() };
        let host = if host_val.is_string() {
            crate::js_to_rust_string(cx, host_val)
        } else if hostname_val.is_string() {
            let hn = crate::js_to_rust_string(cx, hostname_val);
            if port_val.is_string() { format!("{}:{}", hn, crate::js_to_rust_string(cx, port_val)) } else { hn }
        } else {
            String::new()
        };
        let path = if path_val.is_string() {
            crate::js_to_rust_string(cx, path_val)
        } else {
            let pn = if pathname_val.is_string() { crate::js_to_rust_string(cx, pathname_val) } else { "/".to_string() };
            let s = if search_val.is_string() { crate::js_to_rust_string(cx, search_val) } else { String::new() };
            format!("{}{}", pn, s)
        };
        let hash = if hash_val.is_string() { crate::js_to_rust_string(cx, hash_val) } else { String::new() };
        let auth = if auth_val.is_string() { crate::js_to_rust_string(cx, auth_val) } else { String::new() };

        let formatted = if host.is_empty() {
            format!("{}//{}", proto, path)
        } else if auth.is_empty() {
            format!("{}//{}{}{}", proto, host, path, hash)
        } else {
            format!("{}//{}@{}{}{}", proto, auth, host, path, hash)
        };
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

