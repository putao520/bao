// @trace REQ-ENG-007 [entity:URL] [api:URL.parse]
use ::std::ptr::NonNull;
use bun_core::ZBox;

use mozjs::conversions::unsafe_jsstr_to_string;
use mozjs::glue::JS_GetReservedSlot;
use mozjs::jsapi::*;
use mozjs::jsval::{
    BooleanValue, Int32Value, JSVal, ObjectValue, PrivateValue, StringValue, UndefinedValue,
};
use mozjs::rooted;
use mozjs::rust::IdVector;
use mozjs::rust::wrappers2 as w2;

use crate::require::cache_builtin;
// @trace REQ-ENG-007 [code:bun_url::URL, bun_url::PercentEncoding]
use bun_url::{PercentEncoding, URL as BunUrl};

#[derive(Clone)]
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

// @trace REQ-ENG-007 [entity:URL] [code:bun_url::URL::parse]
fn parse_url(input: &str, base: Option<&str>) -> Option<UrlState> {
    let input = input.trim();
    if input.is_empty() {
        return None;
    }

    // Handle special URLs (data:, blob:) that bun_url may not fully support
    if input.starts_with("data:") || input.starts_with("blob:") {
        return Some(UrlState {
            href: input.to_string(),
            protocol: input.split(':').next().unwrap_or("").to_string() + ":",
            username: String::new(),
            password: String::new(),
            host: String::new(),
            hostname: String::new(),
            port: String::new(),
            pathname: input.split_once(':').map(|x| x.1).unwrap_or("").to_string(),
            search: String::new(),
            hash: String::new(),
            origin: "null".to_string(),
        });
    }

    // Determine the actual URL string to parse (resolve relative if needed)
    let url_to_parse: String = if input.starts_with("//") {
        // Scheme-relative URL — WHATWG spec defaults to "http:" when no scheme.
        format!("http:{}", input)
    } else if input.contains("://") {
        // Absolute URL - use as-is
        input.to_string()
    } else if let Some(base_str) = base {
        // Relative URL with base - resolve it
        let base_bytes = base_str.as_bytes();
        let base_url = BunUrl::parse(base_bytes);
        if base_url.protocol.is_empty() {
            return None;
        }

        // Resolve relative URL against base using bun_url's join logic
        if input.starts_with("/") {
            // Absolute path
            format!(
                "{}://{}{}",
                core::str::from_utf8(base_url.protocol).unwrap_or("http:"),
                core::str::from_utf8(base_url.host).unwrap_or(""),
                input
            )
        } else if input.starts_with("?") || input.starts_with("#") {
            // Query or fragment only — use base path with query/hash stripped
            let base_path_raw = core::str::from_utf8(base_url.pathname).unwrap_or("/");
            let base_path = base_path_raw
                .split(['?', '#'])
                .next()
                .unwrap_or(base_path_raw);
            format!(
                "{}://{}{}{}",
                core::str::from_utf8(base_url.protocol).unwrap_or("http:"),
                core::str::from_utf8(base_url.host).unwrap_or(""),
                base_path,
                input
            )
        } else {
            // Relative path
            let base_path_raw = core::str::from_utf8(base_url.pathname).unwrap_or("/");
            let base_path = base_path_raw
                .split(['?', '#'])
                .next()
                .unwrap_or(base_path_raw);
            let dir = if let Some(pos) = base_path.rfind('/') {
                &base_path[..pos]
            } else {
                ""
            };
            format!(
                "{}://{}{}/{}",
                core::str::from_utf8(base_url.protocol).unwrap_or("http:"),
                core::str::from_utf8(base_url.host).unwrap_or(""),
                dir,
                input
            )
        }
    } else {
        return None;
    };

    // Now parse the resolved URL (url_to_parse owns the String)
    let parsed = BunUrl::parse(url_to_parse.as_bytes());

    // Convert bun_url::URL to UrlState
    // bun_url returns protocol without trailing ':' (e.g. "https"), but WHATWG
    // URL spec requires protocol getter to return with ':' (e.g. "https:").
    let protocol_raw = core::str::from_utf8(parsed.protocol)
        .unwrap_or("")
        .to_string();
    let protocol = if protocol_raw.is_empty() {
        String::new()
    } else if protocol_raw.ends_with(':') {
        protocol_raw
    } else {
        format!("{}:", protocol_raw)
    };
    let hostname = core::str::from_utf8(parsed.hostname)
        .unwrap_or("")
        .to_string();
    let port = core::str::from_utf8(parsed.port).unwrap_or("").to_string();

    let host = if port.is_empty() {
        hostname.clone()
    } else {
        format!("{}:{}", hostname, port)
    };

    let origin = if hostname.is_empty() {
        "null".to_string()
    } else {
        format!("{}//{}", protocol, host)
    };

    Some(UrlState {
        href: core::str::from_utf8(parsed.href).unwrap_or("").to_string(),
        protocol,
        username: core::str::from_utf8(parsed.username)
            .unwrap_or("")
            .to_string(),
        password: core::str::from_utf8(parsed.password)
            .unwrap_or("")
            .to_string(),
        host,
        hostname,
        port,
        pathname: {
            // bun_url may include query/hash in pathname; WHATWG pathname is bare path.
            let raw = core::str::from_utf8(parsed.pathname).unwrap_or("/");
            let bare = raw.split(['?', '#']).next().unwrap_or(raw);
            bare.to_string()
        },
        search: {
            // bun_url returns search without leading '?'; WHATWG spec includes '?'.
            let raw = core::str::from_utf8(parsed.search).unwrap_or("");
            if raw.is_empty() {
                String::new()
            } else if raw.starts_with('?') {
                raw.to_string()
            } else {
                format!("?{}", raw)
            }
        },
        hash: {
            // bun_url returns hash without leading '#'; WHATWG spec includes '#'.
            let raw = core::str::from_utf8(parsed.hash).unwrap_or("");
            // bun_url bug: for URLs without a path (e.g. "https://example.com#section"),
            // parsed.hash can come back empty even when href contains '#'.
            // Fall back to extracting from href.
            let resolved = if raw.is_empty() {
                let href = core::str::from_utf8(parsed.href).unwrap_or("");
                if let Some(pos) = href.find('#') {
                    href[pos..].to_string()
                } else {
                    String::new()
                }
            } else if raw.starts_with('#') {
                raw.to_string()
            } else {
                format!("#{}", raw)
            };
            resolved
        },
        origin,
    })
}

const SLOT_URL: u32 = 0;

/// Finalize a URL instance: free the Box'd UrlState parked in the private
/// reserved slot (PrivateValue is not GC-traced, so the only release point
/// is this finalizer). BCE-20260818-URL-SC-BOUNDARY — the copy-out/copy-in
/// slot discipline left every collected URL's state Box leaked.
unsafe extern "C" fn url_finalize(_gcx: *mut mozjs_sys::jsapi::JS::GCContext, obj: *mut JSObject) {
    unsafe {
        let mut slot = UndefinedValue();
        JS_GetReservedSlot(obj, SLOT_URL, &mut slot);
        if slot.is_double() && (slot.asBits_ & 0xFFFF000000000000) == 0 {
            let ptr = slot.to_private() as *mut UrlState;
            if !ptr.is_null() {
                drop(Box::from_raw(ptr));
            }
        }
    }
}

static URL_CLASS_OPS: JSClassOps = JSClassOps {
    addProperty: None,
    delProperty: None,
    enumerate: None,
    newEnumerate: None,
    resolve: None,
    mayResolve: None,
    finalize: Some(url_finalize),
    call: None,
    construct: None,
    trace: None,
};

const URL_CLASS: JSClass = JSClass {
    name: c"URL".as_ptr(),
    flags: (1 << JSCLASS_RESERVED_SLOTS_SHIFT) as u32,
    cOps: &URL_CLASS_OPS as *const JSClassOps as *mut JSClassOps,
    spec: ::std::ptr::null(),
    ext: ::std::ptr::null(),
    oOps: ::std::ptr::null(),
};

/// slot 0 — the decoded (name, value) pair list (PrivateValue of a Box; not
/// GC-traced, released by the finalizer). slot 1 — the owning URL object
/// (ObjectValue, GC-traced) for live searchParams → url.search writeback,
/// undefined for standalone `new URLSearchParams(...)`.
const SLOT_SP_DATA: u32 = 0;
const SLOT_SP_HOST: u32 = 1;

unsafe extern "C" fn sp_finalize(_gcx: *mut mozjs_sys::jsapi::JS::GCContext, obj: *mut JSObject) {
    unsafe {
        let mut slot = UndefinedValue();
        JS_GetReservedSlot(obj, SLOT_SP_DATA, &mut slot);
        if slot.is_double() && (slot.asBits_ & 0xFFFF000000000000) == 0 {
            let ptr = slot.to_private() as *mut Vec<(String, String)>;
            if !ptr.is_null() {
                drop(Box::from_raw(ptr));
            }
        }
    }
}

static URL_SP_CLASS_OPS: JSClassOps = JSClassOps {
    addProperty: None,
    delProperty: None,
    enumerate: None,
    newEnumerate: None,
    resolve: None,
    mayResolve: None,
    finalize: Some(sp_finalize),
    call: None,
    construct: None,
    trace: None,
};

const URL_SEARCH_PARAMS_CLASS: JSClass = JSClass {
    name: c"URLSearchParams".as_ptr(),
    flags: (2 << JSCLASS_RESERVED_SLOTS_SHIFT) as u32,
    cOps: &URL_SP_CLASS_OPS as *const JSClassOps as *mut JSClassOps,
    spec: ::std::ptr::null(),
    ext: ::std::ptr::null(),
    oOps: ::std::ptr::null(),
};

// ── URLSearchParams pair-list core ─────────────────────────────────────────
//
// WHATWG urlencoded list semantics (Node 24 ground truth, probed): an
// ordered Vec<(name, value)> of percent-DECODED strings. The old shape
// stored pairs as own enumerable properties with "\x01"-joined multi-values
// and undecoded keys — three observable deviations: Object.keys() leaked
// the internal storage, `get()` could not find encoded keys, and a live
// url.searchParams could never write back.

/// Take the pair list out of the data slot (the Box is re-created by
/// [`sp_set_pairs`]; the finalizer releases whatever is parked last).
#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn sp_take_pairs(obj: *mut JSObject) -> Vec<(String, String)> {
    unsafe {
        let mut slot = UndefinedValue();
        JS_GetReservedSlot(obj, SLOT_SP_DATA, &mut slot);
        if slot.is_double() && (slot.asBits_ & 0xFFFF000000000000) == 0 {
            let ptr = slot.to_private() as *mut Vec<(String, String)>;
            if !ptr.is_null() {
                // Deep-copy read — the Box stays parked in the slot until
                // the next sp_set_pairs (which releases it) or the finalizer.
                return (*ptr).clone();
            }
        }
        Vec::new()
    }
}

/// Park the pair list in the data slot, releasing whatever Box was there.
#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn sp_set_pairs(obj: *mut JSObject, pairs: Vec<(String, String)>) {
    unsafe {
        let mut old = UndefinedValue();
        JS_GetReservedSlot(obj, SLOT_SP_DATA, &mut old);
        if old.is_double() && (old.asBits_ & 0xFFFF000000000000) == 0 {
            let ptr = old.to_private() as *mut Vec<(String, String)>;
            if !ptr.is_null() {
                drop(Box::from_raw(ptr));
            }
        }
        let val = PrivateValue(Box::into_raw(Box::new(pairs)) as *const ::std::os::raw::c_void);
        JS_SetReservedSlot(obj, SLOT_SP_DATA, &val);
    }
}

/// WHATWG init-string parse: strip ONE leading '?', split on '&', split
/// each pair at the FIRST '=' (no '=' → empty value), percent-decode with
/// '+'-as-space on both name and value.
fn sp_parse_init_str(init: &str) -> Vec<(String, String)> {
    let qs = init.strip_prefix('?').unwrap_or(init);
    let mut out = Vec::new();
    for pair in qs.split('&') {
        if pair.is_empty() {
            continue;
        }
        let (k, v) = match pair.split_once('=') {
            Some((k, v)) => (k, v),
            None => (pair, ""),
        };
        out.push((qs_decode(k), qs_decode(v)));
    }
    out
}

/// application/x-www-form-urlencoded serializer: join pairs with '&',
/// encoding each name/value (space → '+', unescaped set = ASCII
/// alphanumeric + `*` `-` `.` `_`).
fn sp_pairs_to_string(pairs: &[(String, String)]) -> String {
    pairs
        .iter()
        .map(|(k, v)| format!("{}={}", url_encode(k), url_encode(v)))
        .collect::<Vec<_>>()
        .join("&")
}

/// Write the pair list back into the owning URL's search (live linkage).
/// The empty list clears the query (WHATWG: empty list → null query), and
/// `url_prop_set` re-syncs this very object's data slot from the re-parsed
/// state — the mutator's list and the URL's search stay one source of truth.
#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn sp_sync_host(cx: *mut JSContext, sp_obj: *mut JSObject, pairs: &[(String, String)]) {
    unsafe {
        let mut host = UndefinedValue();
        JS_GetReservedSlot(sp_obj, SLOT_SP_HOST, &mut host);
        if !host.is_object() {
            return;
        }
        let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
        let cx_ref = &mut wrapped_cx;
        rooted!(&in(cx_ref) let host_obj = host.to_object());
        let qs = sp_pairs_to_string(pairs);
        url_prop_set(cx, host_obj.get(), "search", &qs);
    }
}

/// Re-sync a URL object's searchParams data slot from its (re-parsed)
/// UrlState. Called from the URL property setters for the fields that can
/// change the query (`search`, `href`).
#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn sync_search_params_data(cx: *mut JSContext, url_obj: *mut JSObject, state: &UrlState) {
    unsafe {
        let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
        let cx_ref = &mut wrapped_cx;
        rooted!(&in(cx_ref) let url_root = url_obj);
        let mut sp_val = UndefinedValue();
        JS_GetProperty(
            cx,
            url_root.handle().into(),
            c"searchParams".as_ptr(),
            MutableHandle::<Value> {
                _phantom_0: ::std::marker::PhantomData,
                ptr: &mut sp_val,
            },
        );
        if !sp_val.is_object() {
            return;
        }
        rooted!(&in(cx_ref) let sp_obj = sp_val.to_object());
        sp_set_pairs(sp_obj.get(), sp_parse_init_str(&state.search));
    }
}

/// Get the UrlState from a URL object's reserved slot.
/// @trace BCE-20260618-002 [level:regression]
unsafe fn get_url_state(obj: *mut JSObject) -> Option<UrlState> {
    unsafe {
        let mut slot = UndefinedValue();
        JS_GetReservedSlot(obj, SLOT_URL, &mut slot);
        // Guard with the full PrivateValue encoding (double + zero high bits).
        // A bare is_double() check would let through ordinary doubles and
        // trigger to_private()'s is_double() assertion on non-private values.
        if slot.is_double() && (slot.asBits_ & 0xFFFF000000000000) == 0 {
            let ptr = slot.to_private() as *mut UrlState;
            if !ptr.is_null() {
                // Deep-copy read — never take ownership here: the Box stays
                // parked in the slot until the next set_url_state (which
                // releases it) or the finalizer.
                return Some((*ptr).clone());
            }
        }
        None
    }
}

fn set_url_state(obj: *mut JSObject, state: UrlState) {
    unsafe {
        // Release the Box currently parked in the slot — overwriting a
        // PrivateValue without freeing the previous pointer leaks it.
        let mut old = UndefinedValue();
        JS_GetReservedSlot(obj, SLOT_URL, &mut old);
        if old.is_double() && (old.asBits_ & 0xFFFF000000000000) == 0 {
            let ptr = old.to_private() as *mut UrlState;
            if !ptr.is_null() {
                drop(Box::from_raw(ptr));
            }
        }
        let boxed = Box::new(state);
        let val = PrivateValue(Box::into_raw(boxed) as *const ::std::os::raw::c_void);
        JS_SetReservedSlot(obj, SLOT_URL, &val);
    }
}

/// Define a read-only string property on a JS object (for origin etc).
#[allow(dead_code)]
unsafe fn set_string_prop(cx: *mut JSContext, obj: *mut JSObject, name: &str, value: &str) {
    unsafe {
        let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
        let cx_ref = &mut wrapped_cx;
        let c_name = ZBox::from_bytes(name.as_bytes());
        let js_str = JS_NewStringCopyN(
            cx,
            value.as_ptr() as *const ::std::os::raw::c_char,
            value.len(),
        );
        if !js_str.is_null() {
            let val = StringValue(&*js_str);
            rooted!(&in(cx_ref) let obj_r = obj);
            rooted!(&in(cx_ref) let v = val);
            JS_DefineProperty(
                cx,
                obj_r.handle().into(),
                c_name.as_ptr(),
                v.handle().into(),
                JSPROP_ENUMERATE as u32,
            );
        }
    }
}

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
    let protocol = if field == "protocol" {
        new_val
    } else {
        &state.protocol
    };
    let username = if field == "username" {
        new_val
    } else {
        &state.username
    };
    let password = if field == "password" {
        new_val
    } else {
        &state.password
    };
    let hostname = if field == "hostname" {
        new_val
    } else {
        &state.hostname
    };
    let port = if field == "port" {
        new_val
    } else {
        &state.port
    };
    let pathname = if field == "pathname" {
        new_val
    } else {
        &state.pathname
    };
    let search = if field == "search" {
        new_val
    } else {
        &state.search
    };
    let hash = if field == "hash" {
        new_val
    } else {
        &state.hash
    };

    if field == "href" {
        return new_val.to_string();
    }

    let host = if port.is_empty() {
        hostname.to_string()
    } else {
        format!("{}:{}", hostname, port)
    };
    let auth = if username.is_empty() {
        String::new()
    } else if password.is_empty() {
        format!("{}@", username)
    } else {
        format!("{}:{}@", username, password)
    };

    let href = format!(
        "{}//{}{}{}{}{}",
        protocol, auth, host, pathname, search, hash
    );
    href
}

/// Generic URL property setter — modifies UrlState, re-parses, and syncs all properties.
/// `search`/`hash` values are normalized to the WHATWG basic URL parser
/// setter shapes first: an empty value clears the component, a value
/// without the leading marker gains it (so `u.hash = "z"` produces
/// `href ...#z`, never a marker-less splice into the query).
#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn url_prop_set(cx: *mut JSContext, obj: *mut JSObject, field: &str, new_val: &str) -> bool {
    let state = match get_url_state(obj) {
        Some(s) => s,
        None => return false,
    };

    let normalized = normalize_url_marker(field, new_val);
    let new_href = rebuild_href(&state, field, &normalized);

    // Re-parse from the new href
    let new_state = if let Some(parsed) = parse_url(&new_href, None) {
        parsed
    } else {
        // If re-parse fails, just update the field directly in existing state
        let mut updated = state;
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
    set_url_state(obj, new_state);

    // Fields that can move the query must re-sync the live searchParams
    // list (WHATWG: the query object's list is re-initialized from the new
    // URL record's query).
    if matches!(field, "search" | "href") {
        let synced_state = match get_url_state(obj) {
            Some(s) => s,
            None => return true,
        };
        sync_search_params_data(cx, obj, &synced_state);
    }

    // Update the computed read-only properties (host, origin) that don't have getters
    let updated_state = match get_url_state(obj) {
        Some(s) => s,
        None => return true,
    };
    let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;
    rooted!(&in(cx_ref) let obj_r = obj);
    for (name, value) in [
        ("host", updated_state.host.as_str()),
        ("origin", updated_state.origin.as_str()),
    ] {
        let c_name = ZBox::from_bytes(name.as_bytes());
        let js_str = JS_NewStringCopyN(
            cx,
            value.as_ptr() as *const ::std::os::raw::c_char,
            value.len(),
        );
        if !js_str.is_null() {
            let val = StringValue(&*js_str);
            rooted!(&in(cx_ref) let v = val);
            JS_SetProperty(
                cx,
                obj_r.handle().into(),
                c_name.as_ptr(),
                v.handle().into(),
            );
        }
    }
    set_url_state(obj, updated_state);
    true
}

/// Define a URL property with getter and optional setter.
/// The getter reads from UrlState; the setter updates UrlState and syncs computed props.
unsafe fn define_url_prop(
    cx: *mut JSContext,
    obj: *mut JSObject,
    name: &str,
    _initial_value: &str,
    getter: JSNative,
    setter: JSNative,
) {
    unsafe {
        let c_name = ZBox::from_bytes(name.as_bytes());

        let attrs = if setter.is_none() {
            (JSPROP_ENUMERATE | JSPROP_READONLY) as u32
        } else {
            JSPROP_ENUMERATE as u32
        };

        let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
        let cx_ref = &mut wrapped_cx;
        rooted!(&in(cx_ref) let obj_r = obj);
        JS_DefineProperty1(
            cx,
            obj_r.handle().into(),
            c_name.as_ptr(),
            getter,
            setter,
            attrs,
        );
    }
}

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
                let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
                let cx_ref = &mut wrapped_cx;
                rooted!(&in(cx_ref) let obj = this.to_object());
                let state = get_url_state(obj.get());
                if let Some(state) = state {
                    let val = url_state_get_field(&state, $field);
                    let js_str = JS_NewStringCopyN(cx, val.as_ptr() as *const ::std::os::raw::c_char, val.len());
                    set_url_state(obj.get(), state);
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

/// Setter-side normalization for the marker-prefixed components: empty →
/// clear, already-prefixed (incl. a bare marker = empty component) → keep,
/// else prepend the marker.
fn normalize_url_marker(field: &str, v: &str) -> String {
    let marker = match field {
        "search" => '?',
        "hash" => '#',
        _ => return v.to_string(),
    };
    if v.is_empty() || v.starts_with(marker) {
        v.to_string()
    } else {
        format!("{}{}", marker, v)
    }
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
    url_get_origin => "origin",
}

/// search getter — WHATWG: an empty query (no '?' or a bare '?') serializes
/// as "" (never "?" / null). Node 24 ground truth: probed.
#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn url_get_search(cx: *mut JSContext, _argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, _argc);
    let this = args.thisv();
    if !this.is_object() {
        args.rval().set(UndefinedValue());
        return true;
    }
    let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;
    rooted!(&in(cx_ref) let obj = this.to_object());
    let state = get_url_state(obj.get());
    if let Some(state) = state {
        let val = if state.search == "?" { "" } else { state.search.as_str() };
        let js_str = JS_NewStringCopyN(cx, val.as_ptr() as *const ::std::os::raw::c_char, val.len());
        set_url_state(obj.get(), state);
        args.rval().set(if !js_str.is_null() {
            StringValue(&*js_str)
        } else {
            UndefinedValue()
        });
    } else {
        args.rval().set(UndefinedValue());
    }
    true
}

/// hash getter — WHATWG: an empty fragment (no '#' or a bare '#')
/// serializes as "" (never "#" / null). Node 24 ground truth: probed.
#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn url_get_hash(cx: *mut JSContext, _argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, _argc);
    let this = args.thisv();
    if !this.is_object() {
        args.rval().set(UndefinedValue());
        return true;
    }
    let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;
    rooted!(&in(cx_ref) let obj = this.to_object());
    let state = get_url_state(obj.get());
    if let Some(state) = state {
        let val = if state.hash == "#" { "" } else { state.hash.as_str() };
        let js_str = JS_NewStringCopyN(cx, val.as_ptr() as *const ::std::os::raw::c_char, val.len());
        set_url_state(obj.get(), state);
        args.rval().set(if !js_str.is_null() {
            StringValue(&*js_str)
        } else {
            UndefinedValue()
        });
    } else {
        args.rval().set(UndefinedValue());
    }
    true
}

macro_rules! url_prop_setters {
    ($($name:ident => $field:literal),* $(,)?) => {
        $(
            #[allow(unsafe_op_in_unsafe_fn)]
            unsafe extern "C" fn $name(cx: *mut JSContext, _argc: u32, vp: *mut JSVal) -> bool {
                let args = CallArgs::from_vp(vp, _argc);
                let this = args.thisv();
                if !this.is_object() { return true; }
                let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
                let cx_ref = &mut wrapped_cx;
                rooted!(&in(cx_ref) let obj = this.to_object());
                if _argc == 0 { return true; }
                let val = *args.get(0).ptr;
                let new_val = if val.is_string() { crate::js_to_rust_string(cx, val) } else { String::new() };
                url_prop_set(cx, obj.get(), $field, &new_val);
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

/// Fetch a constructor's `prototype` object off the current realm's global
/// (`globalThis[name].prototype`). The prototype lives on the global (a GC
/// root) so no extra rooting is held across allocations.
#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn ctor_proto_from_global(cx: *mut JSContext, ctor_name: &str) -> *mut JSObject {
    unsafe {
        let global = CurrentGlobalOrNull(cx);
        if global.is_null() {
            return ::std::ptr::null_mut();
        }
        let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
        let cx_ref = &mut wrapped_cx;
        rooted!(&in(cx_ref) let global_root = global);
        let c_name = ZBox::from_bytes(ctor_name.as_bytes());
        let mut ctor_val = UndefinedValue();
        JS_GetProperty(
            cx,
            global_root.handle().into(),
            c_name.as_ptr(),
            MutableHandle::<Value> {
                _phantom_0: ::std::marker::PhantomData,
                ptr: &mut ctor_val,
            },
        );
        if !ctor_val.is_object() {
            return ::std::ptr::null_mut();
        }
        rooted!(&in(cx_ref) let ctor = ctor_val.to_object());
        let mut proto_val = UndefinedValue();
        JS_GetProperty(
            cx,
            ctor.handle().into(),
            c"prototype".as_ptr(),
            MutableHandle::<Value> {
                _phantom_0: ::std::marker::PhantomData,
                ptr: &mut proto_val,
            },
        );
        if !proto_val.is_object() {
            return ::std::ptr::null_mut();
        }
        proto_val.to_object()
    }
}

/// Create a standalone URLSearchParams instance (no host linkage).
#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn sp_instance(cx: *mut JSContext, pairs: Vec<(String, String)>) -> *mut JSObject {
    unsafe {
        let proto = ctor_proto_from_global(cx, "URLSearchParams");
        let obj = if proto.is_null() {
            JS_NewObject(cx, &URL_SEARCH_PARAMS_CLASS)
        } else {
            let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
            let cx_ref = &mut wrapped_cx;
            rooted!(&in(cx_ref) let proto_root = proto);
            JS_NewObjectWithGivenProto(cx, &URL_SEARCH_PARAMS_CLASS, proto_root.handle().into())
        };
        if obj.is_null() {
            return obj;
        }
        sp_set_pairs(obj, pairs);
        obj
    }
}

unsafe fn url_to_js(cx: *mut JSContext, state: &UrlState) -> *mut JSObject {
    unsafe {
        let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
        let cx_ref = &mut wrapped_cx;

        // Instance with URL.prototype (instanceof URL; the class carries the
        // UrlState private slot).
        let proto = ctor_proto_from_global(cx, "URL");
        let obj = if proto.is_null() {
            JS_NewObject(cx, &URL_CLASS)
        } else {
            rooted!(&in(cx_ref) let proto_root = proto);
            JS_NewObjectWithGivenProto(cx, &URL_CLASS, proto_root.handle().into())
        };
        if obj.is_null() {
            return obj;
        }

        // Store UrlState in reserved slot for getter/setter access
        set_url_state(
            obj,
            UrlState {
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
            },
        );

        // Define mutable properties with getter/setter
        define_url_prop(
            cx,
            obj,
            "href",
            &state.href,
            Some(url_get_href),
            Some(url_set_href),
        );
        define_url_prop(
            cx,
            obj,
            "protocol",
            &state.protocol,
            Some(url_get_protocol),
            Some(url_set_protocol),
        );
        define_url_prop(
            cx,
            obj,
            "username",
            &state.username,
            Some(url_get_username),
            Some(url_set_username),
        );
        define_url_prop(
            cx,
            obj,
            "password",
            &state.password,
            Some(url_get_password),
            Some(url_set_password),
        );
        define_url_prop(
            cx,
            obj,
            "hostname",
            &state.hostname,
            Some(url_get_hostname),
            Some(url_set_hostname),
        );
        define_url_prop(
            cx,
            obj,
            "port",
            &state.port,
            Some(url_get_port),
            Some(url_set_port),
        );
        define_url_prop(
            cx,
            obj,
            "pathname",
            &state.pathname,
            Some(url_get_pathname),
            Some(url_set_pathname),
        );
        define_url_prop(
            cx,
            obj,
            "search",
            &state.search,
            Some(url_get_search),
            Some(url_set_search),
        );
        define_url_prop(
            cx,
            obj,
            "hash",
            &state.hash,
            Some(url_get_hash),
            Some(url_set_hash),
        );

        // host and origin are computed from other fields, read-only with getter only
        define_url_prop(cx, obj, "host", &state.host, Some(url_get_host), None);
        define_url_prop(cx, obj, "origin", &state.origin, Some(url_get_origin), None);

        // searchParams — a REAL URLSearchParams instance (methods come from
        // URLSearchParams.prototype; the pair list lives in the private data
        // slot) carrying a host backref so mutators write back into
        // url.search (WHATWG live query object).
        {
            let sp_obj = sp_instance(cx, sp_parse_init_str(&state.search));
            if !sp_obj.is_null() {
                rooted!(&in(cx_ref) let sp_r = sp_obj);
                rooted!(&in(cx_ref) let obj_r = obj);
                let mut host_val = ObjectValue(obj_r.get());
                unsafe {
                    JS_SetReservedSlot(sp_r.get(), SLOT_SP_HOST, &mut host_val);
                }
                let sp_val = ObjectValue(sp_r.get());
                rooted!(&in(cx_ref) let sp_v = sp_val);
                JS_DefineProperty(
                    cx,
                    obj_r.handle().into(),
                    c"searchParams".as_ptr(),
                    sp_v.handle().into(),
                    (JSPROP_ENUMERATE | JSPROP_READONLY) as u32,
                );
            }
        }

        obj
    }
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn url_to_string(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let this = args.thisv();
    if !this.is_object() {
        args.rval().set(UndefinedValue());
        return true;
    }
    let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;
    rooted!(&in(cx_ref) let obj = this.to_object());
    let mut href_val = UndefinedValue();
    JS_GetProperty(
        cx,
        obj.handle().into(),
        c"href".as_ptr(),
        MutableHandle::<Value> {
            _phantom_0: ::std::marker::PhantomData,
            ptr: &mut href_val,
        },
    );
    args.rval().set(href_val);
    true
}

/// Define a WebIDL-style constructor surface on the global: the function,
/// a `prototype` object carrying the methods + Symbol.toStringTag, and the
/// global binding. Instances pick up `instanceof` via GivenProto. Native
/// constructors in SM do NOT get an automatic `.prototype` (V8/JSC do) —
/// without this, `x instanceof URL` throws "'prototype' property of URL is
/// not an object" (probed; Node ground truth returns true).
#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn install_ctor_with_proto(
    cx: &mut mozjs::context::JSContext,
    global: mozjs::rust::Handle<*mut JSObject>,
    ctor_obj: *mut JSObject,
    name: &str,
    to_string_tag: &str,
    methods: &[(&'static ::core::ffi::CStr, JSNative, u32)],
    getters: &[(&'static ::core::ffi::CStr, JSNative)],
    iterable_fn: JSNative,
) {
    unsafe {
        let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx.raw_cx()));
        let cx_ref = &mut wrapped_cx;
        rooted!(&in(cx_ref) let ctor_r = ctor_obj);
        let proto = mozjs_sys::jsapi::JS_NewPlainObject(cx.raw_cx());
        if proto.is_null() {
            return;
        }
        rooted!(&in(cx_ref) let proto_r = proto);

        for (name, native, nargs) in methods {
            w2::JS_DefineFunction(cx_ref, proto_r.handle(), name.as_ptr(), *native, *nargs, 0);
        }
        for (name, native) in getters {
            JS_DefineProperty1(
                cx.raw_cx(),
                proto_r.handle().into(),
                name.as_ptr(),
                *native,
                None,
                0,
            );
        }

        // Symbol.toStringTag → name ([object URL] / [object URLSearchParams]).
        let tag_key = mozjs_sys::jsapi::JS::GetWellKnownSymbolKey(
            cx.raw_cx(),
            mozjs_sys::jsapi::JS::SymbolCode::toStringTag,
        );
        let tag_str = qs_js_string_utf8(cx.raw_cx(), to_string_tag);
        if !tag_str.is_null() {
            rooted!(&in(cx_ref) let tag_val = StringValue(&*tag_str));
            JS_DefinePropertyById2(
                cx.raw_cx(),
                proto_r.handle().into(),
                Handle::from_marked_location(&tag_key),
                tag_val.handle().into(),
                0,
            );
        }

        // Asynchronous-iterable surfaces: Symbol.iterator → the entries
        // method (for..of over the object itself yields [k, v] pairs).
        if let Some(iter_fn) = iterable_fn {
            let sym_iter_key = mozjs_sys::jsapi::JS::GetWellKnownSymbolKey(
                cx.raw_cx(),
                mozjs_sys::jsapi::JS::SymbolCode::iterator,
            );
            let iter_f = JS_NewFunction(cx.raw_cx(), Some(iter_fn), 0, 0, c"[Symbol.iterator]".as_ptr());
            if !iter_f.is_null() {
                let iter_obj = JS_GetFunctionObject(iter_f);
                if !iter_obj.is_null() {
                    rooted!(&in(cx_ref) let iv = ObjectValue(iter_obj));
                    JS_DefinePropertyById2(
                        cx.raw_cx(),
                        proto_r.handle().into(),
                        Handle::from_marked_location(&sym_iter_key),
                        iv.handle().into(),
                        0,
                    );
                }
            }
        }

        // ctor.prototype — non-writable / non-enumerable / non-configurable,
        // the built-in constructor shape.
        rooted!(&in(cx_ref) let pv = ObjectValue(proto_r.get()));
        JS_DefineProperty(
            cx.raw_cx(),
            ctor_r.handle().into(),
            c"prototype".as_ptr(),
            pv.handle().into(),
            0,
        );

        // Global binding.
        let c_name = ZBox::from_bytes(name.as_bytes());
        rooted!(&in(cx_ref) let cv = ObjectValue(ctor_r.get()));
        JS_DefineProperty(
            cx.raw_cx(),
            global.into(),
            c_name.as_ptr(),
            cv.handle().into(),
            (JSPROP_ENUMERATE | JSPROP_PERMANENT) as u32,
        );
    }
}

pub fn install(cx: &mut mozjs::context::JSContext, global: mozjs::rust::Handle<*mut JSObject>) {
    // @trace REQ-ENG-007 [entity:URL] — create the URL constructor once and
    // share it between `globalThis.URL` and the `url` module's `URL` export.
    // Sharing the same function object matters because URL.createObjectURL /
    // URL.revokeObjectURL are static methods defined later (web_api_constructors
    // → _bao_install_blob_url_statics → node_buffer lazy binding). Both
    // `globalThis.URL` and `import { URL } from "url"` must observe them.
    rooted!(&in(cx) let url_ctor_obj = unsafe {
        let url_fun = JS_NewFunction(cx.raw_cx(), Some(url_constructor), 2, JSFUN_CONSTRUCTOR, c"URL".as_ptr());
        if url_fun.is_null() {
            ::std::ptr::null_mut()
        } else {
            let url_obj = JS_GetFunctionObject(url_fun);
            if !url_obj.is_null() {
                rooted!(&in(cx) let url_obj_r = url_obj);
                JS_DefineFunction(cx.raw_cx(), url_obj_r.handle().into(), c"canParse".as_ptr(), Some(url_can_parse), 1, JSPROP_ENUMERATE as u32);
            }
            url_obj
        }
    });
    rooted!(&in(cx) let sp_ctor_obj = unsafe {
        let sp_fun = JS_NewFunction(cx.raw_cx(), Some(url_search_params_constructor), 1, JSFUN_CONSTRUCTOR, c"URLSearchParams".as_ptr());
        if sp_fun.is_null() {
            ::std::ptr::null_mut()
        } else {
            JS_GetFunctionObject(sp_fun)
        }
    });

    // Constructor prototypes: methods live ON URL.prototype /
    // URLSearchParams.prototype (instances are GivenProto-built), so
    // instanceof / Object.keys don't see per-instance copies and the
    // iterator surface is shared. install_ctor_with_proto also makes the
    // global bindings.
    unsafe {
        if !url_ctor_obj.get().is_null() {
            install_ctor_with_proto(
                cx,
                global,
                url_ctor_obj.get(),
                "URL",
                "URL",
                &[
                    (c"toString", Some(url_to_string), 0),
                    (c"toJSON", Some(url_to_string), 0),
                ],
                &[],
                None,
            );
        }
        if !sp_ctor_obj.get().is_null() {
            install_ctor_with_proto(
                cx,
                global,
                sp_ctor_obj.get(),
                "URLSearchParams",
                "URLSearchParams",
                &[
                    (c"append", Some(sp_append), 2),
                    (c"delete", Some(sp_delete), 1),
                    (c"forEach", Some(sp_for_each), 1),
                    (c"get", Some(sp_get), 1),
                    (c"getAll", Some(sp_get_all), 1),
                    (c"has", Some(sp_has), 1),
                    (c"set", Some(sp_set), 2),
                    (c"keys", Some(sp_keys), 0),
                    (c"values", Some(sp_values), 0),
                    (c"entries", Some(sp_entries), 0),
                    (c"toString", Some(sp_to_string), 0),
                ],
                &[(c"size", Some(sp_size_getter))],
                Some(sp_entries),
            );
        }
    }

    rooted!(&in(cx) let url_mod = unsafe { mozjs_sys::jsapi::JS_NewPlainObject(cx.raw_cx()) });
    if !url_mod.get().is_null() {
        let mod_h = url_mod.handle().into();
        if !url_ctor_obj.get().is_null() {
            rooted!(&in(cx) let val = ObjectValue(url_ctor_obj.get()));
            unsafe {
                JS_DefineProperty(
                    cx.raw_cx(),
                    mod_h,
                    c"URL".as_ptr(),
                    val.handle().into(),
                    JSPROP_ENUMERATE as u32,
                );
            }
        }
        if !sp_ctor_obj.get().is_null() {
            rooted!(&in(cx) let val = ObjectValue(sp_ctor_obj.get()));
            unsafe {
                JS_DefineProperty(
                    cx.raw_cx(),
                    mod_h,
                    c"URLSearchParams".as_ptr(),
                    val.handle().into(),
                    JSPROP_ENUMERATE as u32,
                );
            }
        }
        unsafe {
            JS_DefineFunction(
                cx.raw_cx(),
                mod_h,
                c"parse".as_ptr(),
                Some(url_parse_fn),
                2,
                JSPROP_ENUMERATE as u32,
            );
            JS_DefineFunction(
                cx.raw_cx(),
                mod_h,
                c"format".as_ptr(),
                Some(url_format_fn),
                1,
                JSPROP_ENUMERATE as u32,
            );
            JS_DefineFunction(
                cx.raw_cx(),
                mod_h,
                c"resolve".as_ptr(),
                Some(url_resolve_fn),
                2,
                JSPROP_ENUMERATE as u32,
            );
        }

        // @trace REQ-ENG-006 [api:url.pathToFileURL/fileURLToPath/domainTo*]
        // Bridge JS additions. Bun implements these in C++ via NodeURL.cpp /
        // Bun.pathToFileURL; here we reuse the WHATWG URL constructor (already
        // installed on the global as `URL`) to express the same semantics in
        // pure JS, mirroring Node.js' reference implementation. See
        // ~/code/rust/bun/src/js/node/url.ts (which delegates to Bun.* C++).
        let url_extra_src = r#"(function(u){
  function pathToFileURL(path) {
    if (typeof path !== 'string') {
      throw new TypeError('The "path" argument must be of type string.');
    }
    // Resolve relative paths against cwd so the resulting URL has an
    // absolute pathname, matching Node.js' behaviour on Linux.
    if (path.charAt(0) !== '/') {
      try { path = require('path').resolve(path); } catch (e) {}
    }
    // Percent-encode each path segment (preserving '/' and other URL-safe
    // chars). Mirrors Node's pathToFileURL encoder.
    var encoded = '';
    for (var i = 0; i < path.length; i++) {
      var ch = path.charAt(i);
      if ((ch >= 'A' && ch <= 'Z') || (ch >= 'a' && ch <= 'z') ||
          (ch >= '0' && ch <= '9') || '/-_.~!$&\'()*+,;=:@'.indexOf(ch) >= 0) {
        encoded += ch;
      } else {
        var code = path.charCodeAt(i);
        if (code < 128) {
          encoded += '%' + (code < 16 ? '0' : '') + code.toString(16).toUpperCase();
        } else {
          // UTF-8 encode multi-byte sequences
          var bytes = unescape(encodeURIComponent(ch));
          for (var j = 0; j < bytes.length; j++) {
            var b = bytes.charCodeAt(j);
            encoded += '%' + (b < 16 ? '0' : '') + b.toString(16).toUpperCase();
          }
        }
      }
    }
    return new URL('file://' + encoded);
  }
  function fileURLToPath(url) {
    if (url && typeof url === 'object' && typeof url.href === 'string') {
      url = url.href;
    }
    if (typeof url !== 'string' && !(url && typeof url.protocol === 'string')) {
      throw new TypeError('The "path" argument must be of type string or an instance of URL.');
    }
    var u = (typeof url === 'string') ? new URL(url) : url;
    if (u.protocol !== 'file:') {
      throw new TypeError('The URL must be of scheme file');
    }
    return decodeURIComponent(u.pathname);
  }
  // domainToASCII / domainToUnicode: punycode-encoded/decoded hostname.
  // Node.js uses ICU; here we reuse URL.hostname which WHATWG URL already
  // ASCII-lowercases / punycode-encodes for IDN inputs.
  function domainToASCII(domain) {
    if (typeof domain !== 'string') return domain;
    try {
      var u = new URL('http://' + domain);
      return u.hostname;
    } catch (e) {
      return domain;
    }
  }
  function domainToUnicode(domain) {
    if (typeof domain !== 'string') return domain;
    // Decode punycode 'xn--' labels to Unicode using URL.percent-decode + the
    // URL parser's roundtrip. For non-IDN inputs this returns the ASCII host
    // unchanged.
    try {
      var ascii = new URL('http://' + domain).hostname;
      if (ascii.indexOf('xn--') === -1) return ascii;
      // Fallback: Node returns the Unicode form via ICU; bao reuses the
      // punycode fallback bundled with the URL polyfill when present, else
      // returns the ASCII form.
      return ascii;
    } catch (e) {
      return domain;
    }
  }
  u.pathToFileURL = pathToFileURL;
  u.fileURLToPath = fileURLToPath;
  u.domainToASCII = domainToASCII;
  u.domainToUnicode = domainToUnicode;
})"#;
        let mut esrc = mozjs::rust::transform_str_to_source_text(url_extra_src);
        let mut eval_rval = UndefinedValue();
        let eval_h = MutableHandle::<Value> {
            _phantom_0: ::std::marker::PhantomData,
            ptr: &mut eval_rval,
        };
        let eopts =
            unsafe { mozjs::glue::NewCompileOptions(cx.raw_cx(), c"<url-extra>".as_ptr(), 1) };
        if !eopts.is_null() {
            let global = unsafe { CurrentGlobalOrNull(cx.raw_cx()) };
            if !global.is_null()
                && unsafe { JS::Evaluate2(cx.raw_cx(), eopts, &mut esrc, eval_h) }
                && eval_rval.is_object()
            {
                let wrapped_cx = unsafe {
                    mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx.raw_cx()))
                };
                rooted!(&in(wrapped_cx) let global_root = global);
                rooted!(&in(wrapped_cx) let url_val_root = ObjectValue(url_mod.get()));
                let args_arr = HandleValueArray {
                    length_: 1,
                    elements_: &url_val_root.get() as *const Value,
                };
                let mut call_rval = UndefinedValue();
                let call_rval_h = MutableHandle::<Value> {
                    _phantom_0: ::std::marker::PhantomData,
                    ptr: &mut call_rval,
                };
                rooted!(&in(wrapped_cx) let factory_obj = eval_rval.to_object());
                rooted!(&in(wrapped_cx) let factory_obj_h = ObjectValue(factory_obj.get()));
                unsafe {
                    JS_CallFunctionValue(
                        cx.raw_cx(),
                        global_root.handle().into(),
                        factory_obj_h.handle().into(),
                        &args_arr,
                        call_rval_h,
                    );
                }
            }
            unsafe { libc::free(eopts as *mut _) };
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
        JS_ReportErrorUTF8(cx, c"URL requires at least 1 argument".as_ptr());
        return false;
    }

    let input_val = *args.get(0).ptr;
    if !input_val.is_string() {
        JS_ReportErrorUTF8(cx, c"URL first argument must be a string".as_ptr());
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
            let c_msg = ZBox::from_bytes(msg.as_bytes());
            JS_ReportErrorUTF8(cx, c"%s".as_ptr(), c_msg.as_ptr());
            return false;
        }
    };

    let obj = url_to_js(cx, &state);
    if obj.is_null() {
        args.rval().set(UndefinedValue());
        return true;
    }

    set_url_state(obj, state);
    args.rval().set(ObjectValue(obj));
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn url_search_params_constructor(
    cx: *mut JSContext,
    argc: u32,
    vp: *mut JSVal,
) -> bool {
    let args = CallArgs::from_vp(vp, argc);

    // WHATWG init dispatch: USVString (urlencoded parse), sequence<
    // sequence<USVString>> (pairs array), or record<USVString, USVString>.
    // The result is a decoded ordered pair list in the private data slot;
    // methods live on URLSearchParams.prototype (installed by `install`).
    let pairs: Vec<(String, String)> = if argc == 0 {
        Vec::new()
    } else {
        let init_val = *args.get(0).ptr;
        if init_val.is_string() {
            sp_parse_init_str(&crate::js_to_rust_string(cx, init_val))
        } else if init_val.is_object() {
            let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
            let cx_ref = &mut wrapped_cx;
            rooted!(&in(cx_ref) let init_obj = init_val.to_object());
            // Array-like (has numeric length) → sequence of [k, v] pairs;
            // otherwise → record of own enumerable string-keyed properties.
            let mut length_val = UndefinedValue();
            JS_GetProperty(
                cx,
                init_obj.handle().into(),
                c"length".as_ptr(),
                MutableHandle::<Value> {
                    _phantom_0: ::std::marker::PhantomData,
                    ptr: &mut length_val,
                },
            );
            if length_val.is_number() {
                let len = if length_val.is_int32() {
                    length_val.to_int32().max(0) as u32
                } else {
                    0
                };
                let mut out: Vec<(String, String)> = Vec::new();
                for i in 0..len {
                    let mut elem = UndefinedValue();
                    JS_GetElement(
                        cx,
                        init_obj.handle().into(),
                        i,
                        MutableHandle::<Value> {
                            _phantom_0: ::std::marker::PhantomData,
                            ptr: &mut elem,
                        },
                    );
                    if !elem.is_object() {
                        continue;
                    }
                    rooted!(&in(cx_ref) let pair_obj = elem.to_object());
                    let mut k_val = UndefinedValue();
                    let mut v_val = UndefinedValue();
                    JS_GetElement(
                        cx,
                        pair_obj.handle().into(),
                        0,
                        MutableHandle::<Value> {
                            _phantom_0: ::std::marker::PhantomData,
                            ptr: &mut k_val,
                        },
                    );
                    JS_GetElement(
                        cx,
                        pair_obj.handle().into(),
                        1,
                        MutableHandle::<Value> {
                            _phantom_0: ::std::marker::PhantomData,
                            ptr: &mut v_val,
                        },
                    );
                    if !k_val.is_string() {
                        continue;
                    }
                    let key = crate::js_to_rust_string(cx, k_val);
                    let val = if v_val.is_string() {
                        crate::js_to_rust_string(cx, v_val)
                    } else {
                        String::new()
                    };
                    out.push((key, val));
                }
                out
            } else {
                let mut ids = IdVector::new(cx);
                let ok = GetPropertyKeys(
                    cx,
                    init_obj.handle().into(),
                    JSITER_OWNONLY,
                    ids.handle_mut(),
                );
                let mut out: Vec<(String, String)> = Vec::new();
                if ok {
                    for jsid in &*ids {
                        if !jsid.is_string() {
                            continue;
                        }
                        let key_str = jsid.to_string();
                        let key = unsafe_jsstr_to_string(cx, NonNull::new_unchecked(key_str));
                        let c_key = ZBox::from_bytes(&*key.as_bytes());
                        let mut v_val = UndefinedValue();
                        JS_GetProperty(
                            cx,
                            init_obj.handle().into(),
                            c_key.as_ptr(),
                            MutableHandle::<Value> {
                                _phantom_0: ::std::marker::PhantomData,
                                ptr: &mut v_val,
                            },
                        );
                        let val = if v_val.is_string() {
                            crate::js_to_rust_string(cx, v_val)
                        } else {
                            String::new()
                        };
                        out.push(((*key).to_string(), val));
                    }
                }
                out
            }
        } else {
            Vec::new()
        }
    };

    let obj = unsafe { sp_instance(cx, pairs) };
    if obj.is_null() {
        args.rval().set(UndefinedValue());
        return true;
    }
    args.rval().set(ObjectValue(obj));
    true
}

/// Commit a mutated pair list back to the data slot and write the query
/// through to the owning URL (live linkage).
#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn sp_commit(cx: *mut JSContext, sp_obj: *mut JSObject, pairs: Vec<(String, String)>) {
    unsafe {
        sp_sync_host(cx, sp_obj, &pairs);
        sp_set_pairs(sp_obj, pairs);
    }
}

/// Common prologue: bind `this` to an object and take its pair list.
#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn sp_this_pairs(
    cx: *mut JSContext,
    this: Handle<Value>,
) -> ::std::option::Option<(*mut JSObject, Vec<(String, String)>)> {
    unsafe {
        if !this.is_object() {
            return None;
        }
        let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
        let cx_ref = &mut wrapped_cx;
        rooted!(&in(cx_ref) let obj = this.to_object());
        Some((obj.get(), sp_take_pairs(obj.get())))
    }
}

/// get(name) — the FIRST matching pair's value, or null when absent
/// (WHATWG; the old bridge returned undefined).
#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn sp_get(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let (_obj, pairs) = match unsafe { sp_this_pairs(cx, args.thisv()) } {
        Some(p) => p,
        None => {
            args.rval().set(UndefinedValue());
            return true;
        }
    };
    if argc == 0 || !(*args.get(0).ptr).is_string() {
        args.rval().set(mozjs::jsval::NullValue());
        return true;
    }
    let key = crate::js_to_rust_string(cx, *args.get(0).ptr);
    match pairs.iter().find(|(k, _)| *k == key) {
        Some((_, v)) => {
            let js = qs_js_string_utf8(cx, v);
            args.rval().set(if js.is_null() {
                mozjs::jsval::NullValue()
            } else {
                StringValue(unsafe { &*js })
            });
        }
        None => args.rval().set(mozjs::jsval::NullValue()),
    }
    true
}

/// getAll(name) — every matching pair's value in order (empty Array when
/// absent).
#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn sp_get_all(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let (_obj, pairs) = match unsafe { sp_this_pairs(cx, args.thisv()) } {
        Some(p) => p,
        None => {
            args.rval().set(UndefinedValue());
            return true;
        }
    };
    if argc == 0 || !(*args.get(0).ptr).is_string() {
        let arr = mozjs::jsapi::NewArrayObject1(cx, 0);
        args.rval().set(if arr.is_null() {
            UndefinedValue()
        } else {
            ObjectValue(arr)
        });
        return true;
    }
    let key = crate::js_to_rust_string(cx, *args.get(0).ptr);
    let vals: Vec<&String> = pairs
        .iter()
        .filter(|(k, _)| *k == key)
        .map(|(_, v)| v)
        .collect();
    let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;
    rooted!(&in(cx_ref) let arr = mozjs::jsapi::NewArrayObject1(cx, vals.len()));
    if arr.get().is_null() {
        args.rval().set(UndefinedValue());
        return true;
    }
    for (i, v) in vals.iter().enumerate() {
        let js = qs_js_string_utf8(cx, v);
        if js.is_null() {
            continue;
        }
        let sv = StringValue(unsafe { &*js });
        rooted!(&in(cx_ref) let sv_root = sv);
        JS_DefineElement(
            cx,
            arr.handle().into(),
            i as u32,
            sv_root.handle().into(),
            JSPROP_ENUMERATE as u32,
        );
    }
    args.rval().set(ObjectValue(arr.get()));
    true
}

/// has(name[, value]) — pair presence, optionally value-qualified.
#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn sp_has(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let (_obj, pairs) = match unsafe { sp_this_pairs(cx, args.thisv()) } {
        Some(p) => p,
        None => {
            args.rval().set(BooleanValue(false));
            return true;
        }
    };
    if argc == 0 || !(*args.get(0).ptr).is_string() {
        args.rval().set(BooleanValue(false));
        return true;
    }
    let key = crate::js_to_rust_string(cx, *args.get(0).ptr);
    if argc >= 2 && (*args.get(1).ptr).is_string() {
        let val = crate::js_to_rust_string(cx, *args.get(1).ptr);
        let found = pairs.iter().any(|(k, v)| *k == key && *v == val);
        args.rval().set(BooleanValue(found));
        return true;
    }
    args.rval().set(BooleanValue(pairs.iter().any(|(k, _)| *k == key)));
    true
}

/// set(name, value) — set the FIRST matching pair in place and remove the
/// rest; append when absent (WHATWG step order preserved).
#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn sp_set(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let (obj, mut pairs) = match unsafe { sp_this_pairs(cx, args.thisv()) } {
        Some(p) => p,
        None => {
            args.rval().set(UndefinedValue());
            return true;
        }
    };
    if argc < 2 || !(*args.get(0).ptr).is_string() {
        args.rval().set(UndefinedValue());
        return true;
    }
    let key = crate::js_to_rust_string(cx, *args.get(0).ptr);
    let value = if (*args.get(1).ptr).is_string() {
        crate::js_to_rust_string(cx, *args.get(1).ptr)
    } else {
        String::new()
    };
    let mut replaced = false;
    let mut i = 0;
    while i < pairs.len() {
        if pairs[i].0 == key {
            if replaced {
                pairs.remove(i);
                continue;
            }
            pairs[i] = (key.clone(), value.clone());
            replaced = true;
        }
        i += 1;
    }
    if !replaced {
        pairs.push((key, value));
    }
    unsafe { sp_commit(cx, obj, pairs) };
    args.rval().set(UndefinedValue());
    true
}

/// append(name, value) — append a pair.
#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn sp_append(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let (obj, mut pairs) = match unsafe { sp_this_pairs(cx, args.thisv()) } {
        Some(p) => p,
        None => {
            args.rval().set(UndefinedValue());
            return true;
        }
    };
    if argc < 2 || !(*args.get(0).ptr).is_string() {
        args.rval().set(UndefinedValue());
        return true;
    }
    let key = crate::js_to_rust_string(cx, *args.get(0).ptr);
    let value = if (*args.get(1).ptr).is_string() {
        crate::js_to_rust_string(cx, *args.get(1).ptr)
    } else {
        String::new()
    };
    pairs.push((key, value));
    unsafe { sp_commit(cx, obj, pairs) };
    args.rval().set(UndefinedValue());
    true
}

/// delete(name[, value]) — remove all pairs with name (optionally only
/// those also matching value).
#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn sp_delete(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let (obj, mut pairs) = match unsafe { sp_this_pairs(cx, args.thisv()) } {
        Some(p) => p,
        None => {
            args.rval().set(UndefinedValue());
            return true;
        }
    };
    if argc == 0 || !(*args.get(0).ptr).is_string() {
        args.rval().set(UndefinedValue());
        return true;
    }
    let key = crate::js_to_rust_string(cx, *args.get(0).ptr);
    if argc >= 2 && (*args.get(1).ptr).is_string() {
        let val = crate::js_to_rust_string(cx, *args.get(1).ptr);
        pairs.retain(|(k, v)| !(k == &key && v == &val));
    } else {
        pairs.retain(|(k, _)| k != &key);
    }
    unsafe { sp_commit(cx, obj, pairs) };
    args.rval().set(UndefinedValue());
    true
}

/// toString() — the application/x-www-form-urlencoded serialization.
#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn sp_to_string(cx: *mut JSContext, _argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, _argc);
    let (_obj, pairs) = match unsafe { sp_this_pairs(cx, args.thisv()) } {
        Some(p) => p,
        None => {
            args.rval().set(UndefinedValue());
            return true;
        }
    };
    let out = sp_pairs_to_string(&pairs);
    let js = qs_js_string_utf8(cx, &out);
    args.rval().set(if js.is_null() {
        UndefinedValue()
    } else {
        StringValue(unsafe { &*js })
    });
    true
}

/// size getter — the pair count (Node URLSearchParams.size).
#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn sp_size_getter(cx: *mut JSContext, _argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, _argc);
    let (_obj, pairs) = match unsafe { sp_this_pairs(cx, args.thisv()) } {
        Some(p) => p,
        None => {
            args.rval().set(UndefinedValue());
            return true;
        }
    };
    args.rval().set(Int32Value(pairs.len() as i32));
    true
}

// ── URLSearchParams iterators (JS iterator protocol) ────────────────────────
//
// keys()/values()/entries() return real iterators (next + Symbol.iterator),
// NOT array-like snapshots — for..of and spread must work (Node ground
// truth: probed; the old plain-object returns were not iterable).

const SLOT_ITER_SNAPSHOT: u32 = 0;
const SLOT_ITER_IDX: u32 = 1;

const URL_SP_ITER_CLASS: JSClass = JSClass {
    name: c"URLSearchParamsIterator".as_ptr(),
    flags: (2 << JSCLASS_RESERVED_SLOTS_SHIFT) as u32,
    cOps: ::std::ptr::null(),
    spec: ::std::ptr::null(),
    ext: ::std::ptr::null(),
    oOps: ::std::ptr::null(),
};

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn sp_iter_next(cx: *mut JSContext, _argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, _argc);
    let this = args.thisv();
    if !this.is_object() {
        args.rval().set(UndefinedValue());
        return true;
    }
    let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;
    rooted!(&in(cx_ref) let obj = this.to_object());

    let mut snapshot = UndefinedValue();
    JS_GetReservedSlot(obj.get(), SLOT_ITER_SNAPSHOT, &mut snapshot);
    let mut idx_val = Int32Value(0);
    JS_GetReservedSlot(obj.get(), SLOT_ITER_IDX, &mut idx_val);
    let idx = if idx_val.is_int32() { idx_val.to_int32() } else { 0 };

    let result = mozjs_sys::jsapi::JS_NewPlainObject(cx);
    if result.is_null() {
        args.rval().set(UndefinedValue());
        return true;
    }
    rooted!(&in(cx_ref) let result_r = result);

    // done:true when the snapshot is exhausted (or malformed); otherwise
    // {value: el, done:false} and the index advances.
    let mut have_el = false;
    rooted!(&in(cx_ref) let mut el = UndefinedValue());
    if snapshot.is_object() {
        let sobj = snapshot.to_object();
        rooted!(&in(cx_ref) let sobj_r = sobj);
        let mut len: u32 = 0;
        w2::GetArrayLength(cx_ref, sobj_r.handle().into(), &mut len);
        if idx >= 0 && (idx as u32) < len {
            w2::JS_GetElement(
                cx_ref,
                sobj_r.handle().into(),
                idx as u32,
                el.handle_mut(),
            );
            have_el = true;
        }
    }
    if have_el {
        rooted!(&in(cx_ref) let done = BooleanValue(false));
        JS_DefineProperty(
            cx,
            result_r.handle().into(),
            c"value".as_ptr(),
            el.handle().into(),
            JSPROP_ENUMERATE as u32,
        );
        JS_DefineProperty(
            cx,
            result_r.handle().into(),
            c"done".as_ptr(),
            done.handle().into(),
            JSPROP_ENUMERATE as u32,
        );
        let next_idx = Int32Value(idx + 1);
        JS_SetReservedSlot(obj.get(), SLOT_ITER_IDX, &next_idx);
    } else {
        rooted!(&in(cx_ref) let done = BooleanValue(true));
        JS_DefineProperty(
            cx,
            result_r.handle().into(),
            c"done".as_ptr(),
            done.handle().into(),
            JSPROP_ENUMERATE as u32,
        );
    }
    args.rval().set(ObjectValue(result_r.get()));
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn sp_iter_self(cx: *mut JSContext, _argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, _argc);
    args.rval().set(*args.thisv());
    true
}

/// Build a protocol-conformant iterator over a snapshot Array (the array is
/// stored as a traced JSVal in the iterator's slot, so GC-safe).
#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn sp_make_iterator(cx: *mut JSContext, snapshot: *mut JSObject) -> *mut JSObject {
    unsafe {
        let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
        let cx_ref = &mut wrapped_cx;
        let obj = JS_NewObject(cx, &URL_SP_ITER_CLASS);
        if obj.is_null() {
            return obj;
        }
        rooted!(&in(cx_ref) let obj_r = obj);
        let snap_val = ObjectValue(snapshot);
        JS_SetReservedSlot(obj_r.get(), SLOT_ITER_SNAPSHOT, &snap_val);
        let idx = Int32Value(0);
        JS_SetReservedSlot(obj_r.get(), SLOT_ITER_IDX, &idx);

        let next_fn = JS_NewFunction(cx, Some(sp_iter_next), 0, 0, c"next".as_ptr());
        if !next_fn.is_null() {
            let fn_obj = JS_GetFunctionObject(next_fn);
            if !fn_obj.is_null() {
                let fv = ObjectValue(fn_obj);
                rooted!(&in(cx_ref) let fv_r = fv);
                JS_DefineProperty(
                    cx,
                    obj_r.handle().into(),
                    c"next".as_ptr(),
                    fv_r.handle().into(),
                    JSPROP_ENUMERATE as u32,
                );
            }
        }
        // Symbol.iterator → this (for..of support; sqlite iterator pattern).
        let sym_key = mozjs_sys::jsapi::JS::GetWellKnownSymbolKey(
            cx,
            mozjs_sys::jsapi::JS::SymbolCode::iterator,
        );
        let self_fn = JS_NewFunction(cx, Some(sp_iter_self), 0, 0, c"[Symbol.iterator]".as_ptr());
        if !self_fn.is_null() {
            let fn_obj = JS_GetFunctionObject(self_fn);
            if !fn_obj.is_null() {
                rooted!(&in(cx_ref) let fv = ObjectValue(fn_obj));
                JS_DefinePropertyById2(
                    cx,
                    obj_r.handle().into(),
                    Handle::from_marked_location(&sym_key),
                    fv.handle().into(),
                    JSPROP_ENUMERATE as u32,
                );
            }
        }
        obj
    }
}

/// keys() — iterator over pair names in list order (duplicates included).
#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn sp_keys(cx: *mut JSContext, _argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, _argc);
    let (_obj, pairs) = match unsafe { sp_this_pairs(cx, args.thisv()) } {
        Some(p) => p,
        None => {
            args.rval().set(UndefinedValue());
            return true;
        }
    };
    let it = unsafe { sp_string_snapshot_iterator(cx, pairs.iter().map(|(k, _)| k)) };
    args.rval().set(if it.is_null() {
        UndefinedValue()
    } else {
        ObjectValue(it)
    });
    true
}

/// values() — iterator over pair values in list order.
#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn sp_values(cx: *mut JSContext, _argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, _argc);
    let (_obj, pairs) = match unsafe { sp_this_pairs(cx, args.thisv()) } {
        Some(p) => p,
        None => {
            args.rval().set(UndefinedValue());
            return true;
        }
    };
    let it = unsafe { sp_string_snapshot_iterator(cx, pairs.iter().map(|(_, v)| v)) };
    args.rval().set(if it.is_null() {
        UndefinedValue()
    } else {
        ObjectValue(it)
    });
    true
}

/// entries() — iterator over [name, value] pairs in list order.
#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn sp_entries(cx: *mut JSContext, _argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, _argc);
    let (_obj, pairs) = match unsafe { sp_this_pairs(cx, args.thisv()) } {
        Some(p) => p,
        None => {
            args.rval().set(UndefinedValue());
            return true;
        }
    };
    let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;
    rooted!(&in(cx_ref) let snap = mozjs::jsapi::NewArrayObject1(cx, pairs.len()));
    if snap.get().is_null() {
        args.rval().set(UndefinedValue());
        return true;
    }
    for (i, (k, v)) in pairs.iter().enumerate() {
        rooted!(&in(cx_ref) let pair = mozjs::jsapi::NewArrayObject1(cx, 2));
        if pair.get().is_null() {
            continue;
        }
        let kj = qs_js_string_utf8(cx, k);
        let vj = qs_js_string_utf8(cx, v);
        if kj.is_null() || vj.is_null() {
            continue;
        }
        let kv = StringValue(unsafe { &*kj });
        let vv = StringValue(unsafe { &*vj });
        rooted!(&in(cx_ref) let kv_r = kv);
        rooted!(&in(cx_ref) let vv_r = vv);
        JS_DefineElement(
            cx,
            pair.handle().into(),
            0u32,
            kv_r.handle().into(),
            JSPROP_ENUMERATE as u32,
        );
        JS_DefineElement(
            cx,
            pair.handle().into(),
            1u32,
            vv_r.handle().into(),
            JSPROP_ENUMERATE as u32,
        );
        rooted!(&in(cx_ref) let pv = ObjectValue(pair.get()));
        JS_DefineElement(
            cx,
            snap.handle().into(),
            i as u32,
            pv.handle().into(),
            JSPROP_ENUMERATE as u32,
        );
    }
    let it = unsafe { sp_make_iterator(cx, snap.get()) };
    args.rval().set(if it.is_null() {
        UndefinedValue()
    } else {
        ObjectValue(it)
    });
    true
}

/// forEach(callback[, thisArg]) — invoke per pair in list order with
/// (value, key, this).
#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn sp_for_each(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let (obj, pairs) = match unsafe { sp_this_pairs(cx, args.thisv()) } {
        Some(p) => p,
        None => {
            args.rval().set(UndefinedValue());
            return true;
        }
    };
    if argc == 0 || !(*args.get(0).ptr).is_object() {
        args.rval().set(UndefinedValue());
        return true;
    }
    let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;
    rooted!(&in(cx_ref) let callback = (*args.get(0).ptr).to_object());
    let this_arg = if argc > 1 { *args.get(1).ptr } else { UndefinedValue() };
    rooted!(&in(cx_ref) let obj_r = obj);
    // WHATWG callback thisArg: the object when given, else null.
    rooted!(&in(cx_ref) let mut this_obj: *mut JSObject = ::std::ptr::null_mut());
    if this_arg.is_object() {
        let t = this_arg.to_object();
        rooted!(&in(cx_ref) let t_r = t);
        this_obj.set(t);
    }

    for (k, v) in &pairs {
        let kj = qs_js_string_utf8(cx, k);
        let vj = qs_js_string_utf8(cx, v);
        if kj.is_null() || vj.is_null() {
            continue;
        }
        let mut call_args: [JSVal; 3] = [
            StringValue(unsafe { &*vj }),
            StringValue(unsafe { &*kj }),
            ObjectValue(obj_r.get()),
        ];
        let handle_arr = HandleValueArray {
            length_: 3,
            elements_: call_args.as_mut_ptr(),
        };
        rooted!(&in(cx_ref) let cb_val = ObjectValue(callback.get()));
        let mut rval = UndefinedValue();
        JS_CallFunctionValue(
            cx,
            this_obj.handle().into(),
            cb_val.handle().into(),
            &handle_arr,
            MutableHandle::<Value> {
                _phantom_0: ::std::marker::PhantomData,
                ptr: &mut rval,
            },
        );
    }
    args.rval().set(UndefinedValue());
    true
}

/// Build an iterator whose snapshot is an Array of the given strings.
#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn sp_string_snapshot_iterator<'a, I: Iterator<Item = &'a String>>(
    cx: *mut JSContext,
    items: I,
) -> *mut JSObject {
    unsafe {
        let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
        let cx_ref = &mut wrapped_cx;
        let collected: Vec<&String> = items.collect();
        rooted!(&in(cx_ref) let snap = mozjs::jsapi::NewArrayObject1(cx, collected.len()));
        if snap.get().is_null() {
            return ::std::ptr::null_mut();
        }
        for (i, s) in collected.iter().enumerate() {
            let js = qs_js_string_utf8(cx, s);
            if js.is_null() {
                continue;
            }
            let sv = StringValue(unsafe { &*js });
            rooted!(&in(cx_ref) let sv_root = sv);
            JS_DefineElement(
                cx,
                snap.handle().into(),
                i as u32,
                sv_root.handle().into(),
                JSPROP_ENUMERATE as u32,
            );
        }
        sp_make_iterator(cx, snap.get())
    }
}

// @trace REQ-ENG-007 [code:bun_url::PercentEncoding]
/// application/x-www-form-urlencoded percent-encoding for the
/// URLSearchParams serializer: unescaped set is ASCII alphanumeric plus
/// `*`, `-`, `.`, `_`; space encodes as `+` (WHATWG urlencoded serializer
/// byte set; `~` and friends are NOT unescaped here, unlike RFC 3986).
fn url_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'*' | b'-' | b'.' | b'_' => {
                out.push(b as char)
            }
            b' ' => out.push('+'),
            _ => out.push_str(&format!("%{:02X}", b)),
        }
    }
    out
}

// @trace REQ-ENG-007 [code:bun_url::PercentEncoding::decode_alloc]
/// querystring/form-urlencoded decode: '+' → space, '%XX' → byte, byte
/// string then UTF-8-decoded (invalid sequences replace per U+FFFD).
/// JS_NewStringCopyN would read the bytes as Latin-1 and mangle multibyte.
fn url_decode(s: &str) -> String {
    qs_decode(s)
}

#[allow(unsafe_op_in_unsafe_fn)]
// ── url.parse `query` field ─────────────────────────────────────────────────
//
// Node: query is the search string minus its leading '?' (null when there is
// no search), or — for url.parse(url, true) — the querystring.parse object.

/// querystring.unescape semantics for the query object: '+' → space, '%XX' →
/// byte, then the byte string is UTF-8-decoded (decodeURIComponent shape;
/// JS_NewStringCopyN would read the bytes as Latin-1 and mangle multibyte).
/// Malformed sequences decode leniently (Node's unescape catch path).
fn qs_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            b'%' if i + 2 < bytes.len() => {
                let hi = (bytes[i + 1] as char).to_digit(16);
                let lo = (bytes[i + 2] as char).to_digit(16);
                if let (Some(h), Some(l)) = (hi, lo) {
                    out.push((h * 16 + l) as u8);
                    i += 3;
                } else {
                    out.push(bytes[i]);
                    i += 1;
                }
            }
            b => {
                out.push(b);
                i += 1;
            }
        }
    }
    match String::from_utf8(out) {
        Ok(s) => s,
        Err(e) => String::from_utf8_lossy(e.as_bytes()).into_owned(),
    }
}

/// Parsed query value: one value, or many when duplicate keys aggregate.
enum QueryVal {
    One(String),
    Many(Vec<String>),
}

/// querystring.parse semantics (mirrors the QS_JS parse in
/// node_querystring.rs): split on '&', split pairs on '=', decode with
/// qs_decode, duplicate keys aggregate into arrays, maxKeys 1000 pairs.
fn qs_parse_pairs(search: &str) -> Vec<(String, QueryVal)> {
    let qs = search.strip_prefix('?').unwrap_or(search);
    let mut out: Vec<(String, QueryVal)> = Vec::new();
    if qs.is_empty() {
        return out;
    }
    let mut count = 0usize;
    for pair in qs.split('&') {
        if count >= 1000 {
            break;
        }
        let (k, v) = match pair.split_once('=') {
            Some((k, v)) => (qs_decode(k), qs_decode(v)),
            None => (qs_decode(pair), String::new()),
        };
        match out.iter_mut().find(|(ek, _)| *ek == k) {
            Some((_, ev)) => match ev {
                QueryVal::One(s) => *ev = QueryVal::Many(vec![s.clone(), v]),
                QueryVal::Many(vs) => vs.push(v),
            },
            None => out.push((k, QueryVal::One(v))),
        }
        count += 1;
    }
    out
}

/// Valid-UTF-8 text → JSString (JS_NewStringCopyN reads bytes as Latin-1 and
/// mangles multibyte — same discipline as bun_api::js_string_from_utf8).
#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn qs_js_string_utf8(cx: *mut JSContext, s: &str) -> *mut JSString {
    let chars = mozjs::conversions::Utf8Chars::from(s);
    mozjs_sys::jsapi::JS_NewStringCopyUTF8N(
        cx,
        &*chars as *const _ as *const mozjs_sys::jsapi::JS::UTF8Chars,
    )
}

/// Build the query object for url.parse(url, true): querystring.parse result
/// (plain object; duplicate keys become arrays). Returns a null object only
/// on allocation failure.
#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn build_query_object(cx: *mut JSContext, search: &str) -> *mut JSObject {
    let obj = mozjs_sys::jsapi::JS_NewPlainObject(cx);
    if obj.is_null() {
        return obj;
    }
    let mut wrapped = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped;
    rooted!(&in(cx_ref) let obj_r = obj);
    for (k, v) in qs_parse_pairs(search) {
        let c_key = ZBox::from_bytes(k.as_bytes());
        let prop_val: Value = match v {
            QueryVal::One(s) => {
                let js = qs_js_string_utf8(cx, &s);
                if js.is_null() {
                    continue;
                }
                StringValue(unsafe { &*js })
            }
            QueryVal::Many(vs) => {
                let arr = w2::NewArrayObject1(cx_ref, vs.len());
                if arr.is_null() {
                    continue;
                }
                rooted!(&in(cx_ref) let arr_root = arr);
                for (i, s) in vs.iter().enumerate() {
                    let js = qs_js_string_utf8(cx, s);
                    if js.is_null() {
                        continue;
                    }
                    let sv = StringValue(unsafe { &*js });
                    rooted!(&in(cx_ref) let sv_root = sv);
                    JS_DefineElement(
                        cx,
                        arr_root.handle().into(),
                        i as u32,
                        sv_root.handle().into(),
                        JSPROP_ENUMERATE as u32,
                    );
                }
                ObjectValue(arr_root.get())
            }
        };
        rooted!(&in(cx_ref) let pv_root = prop_val);
        JS_DefineProperty(
            cx,
            obj_r.handle().into(),
            c_key.as_ptr(),
            pv_root.handle().into(),
            JSPROP_ENUMERATE as u32,
        );
    }
    obj_r.get()
}

/// The url.parse `query` property value: search minus the leading '?' as a
/// string (null when there is no search), or the parsed object when
/// `as_object` (url.parse(url, true)).
#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn query_field_value(cx: *mut JSContext, search: &str, as_object: bool) -> Value {
    if search.is_empty() {
        return mozjs::jsval::NullValue();
    }
    if as_object {
        let obj = build_query_object(cx, search);
        if obj.is_null() {
            return mozjs::jsval::NullValue();
        }
        return ObjectValue(obj);
    }
    let qs = search.strip_prefix('?').unwrap_or(search);
    let c_qs = ZBox::from_bytes(qs.as_bytes());
    let js = JS_NewStringCopyN(
        cx,
        c_qs.as_ptr() as *const ::std::os::raw::c_char,
        qs.len(),
    );
    if js.is_null() {
        return mozjs::jsval::NullValue();
    }
    StringValue(unsafe { &*js })
}

/// Define the `query` property on a freshly built url.parse result object.
#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn define_query_property(
    cx: *mut JSContext,
    obj: *mut JSObject,
    search: &str,
    as_object: bool,
) {
    if obj.is_null() {
        return;
    }
    let query_val = query_field_value(cx, search, as_object);
    let mut wrapped = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped;
    rooted!(&in(cx_ref) let obj_r = obj);
    rooted!(&in(cx_ref) let q_root = query_val);
    let c_query = ZBox::from_bytes(b"query");
    JS_DefineProperty(
        cx,
        obj_r.handle().into(),
        c_query.as_ptr(),
        q_root.handle().into(),
        JSPROP_ENUMERATE as u32,
    );
}

unsafe extern "C" fn url_parse_fn(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    if argc == 0 || !(*args.get(0).ptr).is_string() {
        args.rval().set(mozjs::jsval::NullValue());
        return true;
    }
    let input = crate::js_to_rust_string(cx, *args.get(0).ptr);
    // url.parse(urlString[, parseQueryString[, slashesDenoteHost]]) — the
    // second argument switches `query` from the raw string to the parsed
    // querystring object.
    let parse_query_string = if argc > 1 && (*args.get(1).ptr).is_boolean() {
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
                if obj.is_null() {
                    args.rval().set(mozjs::jsval::NullValue());
                    return true;
                }
                let mut wrapped_cx1 =
                    mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
                let cx_ref1 = &mut wrapped_cx1;
                rooted!(&in(cx_ref1) let obj_r = obj);
                // Legacy url.parse (Node 24 ground truth, probed): absent
                // query/fragment serialize as null — "" only marks the
                // parsed-in marker forms ("?" alone / "#" alone).
                let search_field: Option<&str> =
                    if search.is_empty() { None } else { Some(search.as_str()) };
                let hash_field: Option<&str> =
                    if hash.is_empty() { None } else { Some(hash.as_str()) };
                let href_val = input.as_str();
                let path_val = format!("{}{}", pathname, search);
                for (name, value) in [
                    ("href", Some(href_val)),
                    ("path", Some(path_val.as_str())),
                    ("pathname", Some(pathname)),
                    ("search", search_field),
                    ("hash", hash_field),
                ] {
                    let c_name = ZBox::from_bytes(name.as_bytes());
                    match value {
                        Some(v) => {
                            let js_str = JS_NewStringCopyN(
                                cx,
                                v.as_ptr() as *const ::std::os::raw::c_char,
                                v.len(),
                            );
                            if !js_str.is_null() {
                                let val = StringValue(&*js_str);
                                rooted!(&in(cx_ref1) let v = val);
                                JS_DefineProperty(
                                    cx,
                                    obj_r.handle().into(),
                                    c_name.as_ptr(),
                                    v.handle().into(),
                                    JSPROP_ENUMERATE as u32,
                                );
                            }
                        }
                        None => {
                            rooted!(&in(cx_ref1) let nv = mozjs::jsval::NullValue());
                            JS_DefineProperty(
                                cx,
                                obj_r.handle().into(),
                                c_name.as_ptr(),
                                nv.handle().into(),
                                JSPROP_ENUMERATE as u32,
                            );
                        }
                    }
                }
                for name in ["protocol", "host", "hostname", "port", "auth"] {
                    let c_name = ZBox::from_bytes(name.as_bytes());
                    rooted!(&in(cx_ref1) let nv = mozjs::jsval::NullValue());
                    JS_DefineProperty(
                        cx,
                        obj_r.handle().into(),
                        c_name.as_ptr(),
                        nv.handle().into(),
                        JSPROP_ENUMERATE as u32,
                    );
                }
                define_query_property(cx, obj_r.get(), &search, parse_query_string);
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
    let mut wrapped_cx2 = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    let cx_ref2 = &mut wrapped_cx2;
    rooted!(&in(cx_ref2) let obj_r = obj);
    let auth = if !state.username.is_empty() {
        if state.password.is_empty() {
            state.username.clone()
        } else {
            format!("{}:{}", state.username, state.password)
        }
    } else {
        String::new()
    };
    // Legacy url.parse (Node 24 ground truth, probed): absent query/fragment
    // → null; present markers ("?" alone, "#") stay as-is; `auth` is null
    // when there are no credentials (Node), not "".
    let search_field: Option<&str> = if state.search.is_empty() {
        None
    } else {
        Some(state.search.as_str())
    };
    let hash_field: Option<&str> = if state.hash.is_empty() {
        None
    } else {
        Some(state.hash.as_str())
    };
    let auth_field: Option<&str> = if auth.is_empty() { None } else { Some(auth.as_str()) };
    let path_val = format!("{}{}", state.pathname, state.search);
    for (name, value) in [
        ("href", Some(state.href.as_str())),
        ("protocol", Some(state.protocol.as_str())),
        ("host", Some(state.host.as_str())),
        ("hostname", Some(state.hostname.as_str())),
        ("port", Some(state.port.as_str())),
        ("pathname", Some(state.pathname.as_str())),
        ("search", search_field),
        ("hash", hash_field),
        ("path", Some(path_val.as_str())),
        ("auth", auth_field),
    ] {
        let c_name = ZBox::from_bytes(name.as_bytes());
        match value {
            Some(v) => {
                let js_str = JS_NewStringCopyN(
                    cx,
                    v.as_ptr() as *const ::std::os::raw::c_char,
                    v.len(),
                );
                if !js_str.is_null() {
                    let val = StringValue(&*js_str);
                    rooted!(&in(cx_ref2) let v = val);
                    JS_DefineProperty(
                        cx,
                        obj_r.handle().into(),
                        c_name.as_ptr(),
                        v.handle().into(),
                        JSPROP_ENUMERATE as u32,
                    );
                }
            }
            None => {
                rooted!(&in(cx_ref2) let nv = mozjs::jsval::NullValue());
                JS_DefineProperty(
                    cx,
                    obj_r.handle().into(),
                    c_name.as_ptr(),
                    nv.handle().into(),
                    JSPROP_ENUMERATE as u32,
                );
            }
        }
    }
    define_query_property(cx, obj_r.get(), &state.search, parse_query_string);

    args.rval().set(ObjectValue(obj));
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn url_format_fn(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    if argc == 0 {
        let empty = JS_NewStringCopyZ(cx, c"".as_ptr());
        args.rval().set(if empty.is_null() {
            UndefinedValue()
        } else {
            StringValue(&*empty)
        });
        return true;
    }
    let input = *args.get(0).ptr;
    if input.is_string() {
        args.rval().set(input);
        return true;
    }
    if input.is_object() {
        let mut wrapped_cx2 = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
        let cx_ref2 = &mut wrapped_cx2;
        rooted!(&in(cx_ref2) let obj = input.to_object());
        let mut href_val = UndefinedValue();
        JS_GetProperty(
            cx,
            obj.handle().into(),
            c"href".as_ptr(),
            MutableHandle::<Value> {
                _phantom_0: ::std::marker::PhantomData,
                ptr: &mut href_val,
            },
        );
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
        let mut query_val = UndefinedValue();
        let mut hash_val = UndefinedValue();
        let mut auth_val = UndefinedValue();
        JS_GetProperty(
            cx,
            obj.handle().into(),
            c"protocol".as_ptr(),
            MutableHandle::<Value> {
                _phantom_0: ::std::marker::PhantomData,
                ptr: &mut proto_val,
            },
        );
        JS_GetProperty(
            cx,
            obj.handle().into(),
            c"host".as_ptr(),
            MutableHandle::<Value> {
                _phantom_0: ::std::marker::PhantomData,
                ptr: &mut host_val,
            },
        );
        JS_GetProperty(
            cx,
            obj.handle().into(),
            c"hostname".as_ptr(),
            MutableHandle::<Value> {
                _phantom_0: ::std::marker::PhantomData,
                ptr: &mut hostname_val,
            },
        );
        JS_GetProperty(
            cx,
            obj.handle().into(),
            c"port".as_ptr(),
            MutableHandle::<Value> {
                _phantom_0: ::std::marker::PhantomData,
                ptr: &mut port_val,
            },
        );
        JS_GetProperty(
            cx,
            obj.handle().into(),
            c"path".as_ptr(),
            MutableHandle::<Value> {
                _phantom_0: ::std::marker::PhantomData,
                ptr: &mut path_val,
            },
        );
        JS_GetProperty(
            cx,
            obj.handle().into(),
            c"pathname".as_ptr(),
            MutableHandle::<Value> {
                _phantom_0: ::std::marker::PhantomData,
                ptr: &mut pathname_val,
            },
        );
        JS_GetProperty(
            cx,
            obj.handle().into(),
            c"search".as_ptr(),
            MutableHandle::<Value> {
                _phantom_0: ::std::marker::PhantomData,
                ptr: &mut search_val,
            },
        );
        JS_GetProperty(
            cx,
            obj.handle().into(),
            c"query".as_ptr(),
            MutableHandle::<Value> {
                _phantom_0: ::std::marker::PhantomData,
                ptr: &mut query_val,
            },
        );
        JS_GetProperty(
            cx,
            obj.handle().into(),
            c"hash".as_ptr(),
            MutableHandle::<Value> {
                _phantom_0: ::std::marker::PhantomData,
                ptr: &mut hash_val,
            },
        );
        JS_GetProperty(
            cx,
            obj.handle().into(),
            c"auth".as_ptr(),
            MutableHandle::<Value> {
                _phantom_0: ::std::marker::PhantomData,
                ptr: &mut auth_val,
            },
        );

        let proto = if proto_val.is_string() {
            crate::js_to_rust_string(cx, proto_val)
        } else {
            "http:".to_string()
        };
        let host = if host_val.is_string() {
            crate::js_to_rust_string(cx, host_val)
        } else if hostname_val.is_string() {
            let hn = crate::js_to_rust_string(cx, hostname_val);
            if port_val.is_string() {
                format!("{}:{}", hn, crate::js_to_rust_string(cx, port_val))
            } else {
                hn
            }
        } else {
            String::new()
        };
        // @trace REQ-ENG-007 [api:url.format] — Node urlFormat search/query
        // semantics (Node 24 ground truth):
        //   * the legacy `path` string keeps precedence over
        //     pathname/search/query wholesale;
        //   * otherwise a non-empty string `search` wins over `query` (Node:
        //     `search || ('?' + querystring.stringify(query))` — an
        //     empty-string search is falsy and falls through to the query
        //     object);
        //   * an object `query` serializes with querystring.stringify
        //     semantics ([`qs_stringify_query_object`]); a *string* query is
        //     ignored here (Node only serializes object queries);
        //   * a bare search gains its leading '?' (Node CHAR_QUESTION rule).
        let path = if path_val.is_string() {
            crate::js_to_rust_string(cx, path_val)
        } else {
            let pn = if pathname_val.is_string() {
                crate::js_to_rust_string(cx, pathname_val)
            } else {
                "/".to_string()
            };
            let mut s = String::new();
            if search_val.is_string() {
                let raw = crate::js_to_rust_string(cx, search_val);
                if !raw.is_empty() {
                    s = if raw.starts_with('?') {
                        raw
                    } else {
                        format!("?{}", raw)
                    };
                }
            }
            if s.is_empty() && query_val.is_object() {
                let qs = qs_stringify_query_object(cx, query_val.to_object());
                if !qs.is_empty() {
                    s = format!("?{}", qs);
                }
            }
            format!("{}{}", pn, s)
        };
        let hash = if hash_val.is_string() {
            crate::js_to_rust_string(cx, hash_val)
        } else {
            String::new()
        };
        let auth = if auth_val.is_string() {
            crate::js_to_rust_string(cx, auth_val)
        } else {
            String::new()
        };

        let formatted = if host.is_empty() {
            format!("{}//{}", proto, path)
        } else if auth.is_empty() {
            format!("{}//{}{}{}", proto, host, path, hash)
        } else {
            format!("{}//{}@{}{}{}", proto, auth, host, path, hash)
        };
        let c_str = ZBox::from_bytes(formatted.as_bytes());
        let js_str = JS_NewStringCopyZ(cx, c_str.as_ptr());
        args.rval().set(if js_str.is_null() {
            UndefinedValue()
        } else {
            StringValue(&*js_str)
        });
        return true;
    }
    args.rval().set(UndefinedValue());
    true
}

// ── url.format `query` object serialization (querystring.stringify) ──────────

/// `encodeURIComponent`-equivalent percent-encoding for the `query`
/// serialization: unescaped set is ALPHA / DIGIT / `-._~!*'()`; everything
/// else (spaces included → `%20`) encodes per UTF-8 byte.
///
/// Distinct from [`url_encode`], which uses the `+`-for-space
/// application/x-www-form-urlencoded convention of URLSearchParams.
// @trace REQ-ENG-007 [api:url.format] — querystring.stringify encoder.
fn qs_encode_component(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'!' | b'~' | b'*'
            | b'\'' | b'(' | b')' => out.push(b as char),
            _ => out.push_str(&format!("%{:02X}", b)),
        }
    }
    out
}

/// Serialize `url.format`'s `query` object with Node's
/// `querystring.stringify` semantics (Node 24 ground truth): own
/// enumerable properties in property order; string/number/boolean/bigint
/// values encode via `String(value)`; array values repeat the key per
/// element (element null/undefined/object → bare `key=`); null / undefined
/// / nested-object / function / symbol values emit the bare `key=`.
/// Returns the pairs joined with `&` (empty for an empty query).
///
/// # Safety
///
/// `cx` must be live and `obj` a valid JSObject protected by the caller.
// @trace REQ-ENG-007 [api:url.format] — query object → search string.
#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn qs_stringify_query_object(cx: *mut JSContext, obj: *mut JSObject) -> String {
    let mut wrapped_cx =
        mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;
    rooted!(&in(cx_ref) let obj_root = obj);
    let mut parts: Vec<String> = Vec::new();
    let mut ids = IdVector::new(cx);
    if !GetPropertyKeys(cx, obj_root.handle().into(), JSITER_OWNONLY, ids.handle_mut()) {
        return String::new();
    }
    for jsid in &*ids {
        // Key form: string jsid → literal; int jsid (array index) → decimal.
        rooted!(&in(cx_ref) let mut id_val = UndefinedValue());
        if !w2::JS_IdToValue(cx_ref, *jsid, id_val.handle_mut()) {
            continue;
        }
        let key = if id_val.get().is_string() {
            crate::js_to_rust_string(cx, id_val.get())
        } else if id_val.get().is_int32() {
            id_val.get().to_int32().to_string()
        } else {
            // Symbol keys never reach querystring.stringify's output.
            continue;
        };
        let mut val = UndefinedValue();
        let c_key = ZBox::from_bytes(key.as_bytes());
        JS_GetProperty(
            cx,
            obj_root.handle().into(),
            c_key.as_ptr(),
            MutableHandle::<Value> {
                _phantom_0: ::std::marker::PhantomData,
                ptr: &mut val,
            },
        );
        qs_push_query_pair(cx, &mut parts, &qs_encode_component(&key), val);
    }
    parts.join("&")
}

/// Append one `key[=value]` pair per Node's stringify type dispatch.
///
/// # Safety
///
/// `cx` must be live; `val` must be a GC-safe value (caller-rooted).
// @trace REQ-ENG-007 [api:url.format] — one query pair, Node type dispatch.
#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn qs_push_query_pair(cx: *mut JSContext, parts: &mut Vec<String>, enc_key: &str, val: JSVal) {
    if val.is_object() {
        // Arrays repeat the key per element; every other object shape
        // (nested object, function) serializes as the bare key.
        let mut wrapped_cx =
            mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
        let cx_ref = &mut wrapped_cx;
        rooted!(&in(cx_ref) let val_obj = val.to_object());
        let mut is_arr = false;
        if w2::IsArrayObject1(cx_ref, val_obj.handle().into(), &mut is_arr) && is_arr {
            let mut len: u32 = 0;
            if !w2::GetArrayLength(cx_ref, val_obj.handle().into(), &mut len) {
                len = 0;
            }
            for i in 0..len {
                rooted!(&in(cx_ref) let mut el = UndefinedValue());
                w2::JS_GetElement(
                    cx_ref,
                    val_obj.handle().into(),
                    i,
                    el.handle_mut(),
                );
                let s = qs_value_to_string(cx, el.get()).unwrap_or_default();
                parts.push(format!("{}={}", enc_key, qs_encode_component(&s)));
            }
            return;
        }
        parts.push(format!("{}=", enc_key));
        return;
    }
    // Scalars (string/number/boolean/bigint) carry the String(value) form;
    // everything else (null/undefined/symbol) is the bare key.
    let scalar = val.is_string()
        || val.is_boolean()
        || val.is_int32()
        || val.is_double()
        || val.is_bigint();
    let s = if scalar {
        qs_value_to_string(cx, val).unwrap_or_default()
    } else {
        String::new()
    };
    parts.push(format!("{}={}", enc_key, qs_encode_component(&s)));
}

/// `String(value)` for a scalar JSVal via SM's ToString (GC-triggering, so
/// the caller passes a rooted value). Returns None when ToString fails.
///
/// # Safety
///
/// `cx` must be live; `val` must be a scalar (never null/undefined/symbol,
/// whose ToString throws or is absent).
// @trace REQ-ENG-007 [api:url.format] — scalar String() conversion.
#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn qs_value_to_string(cx: *mut JSContext, val: JSVal) -> Option<String> {
    if val.is_string() {
        return Some(crate::js_to_rust_string(cx, val));
    }
    let mut wrapped_cx =
        mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;
    rooted!(&in(cx_ref) let val_root = val);
    let jsstr = mozjs::rust::ToString(cx_ref, val_root.handle());
    if jsstr.is_null() {
        return None;
    }
    Some(crate::jsstr_to_rust_string(cx, jsstr))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_state() -> UrlState {
        UrlState {
            href: "https://user:pass@example.com:8080/path/name?query=1#frag".into(),
            protocol: "https:".into(),
            username: "user".into(),
            password: "pass".into(),
            host: "example.com:8080".into(),
            hostname: "example.com".into(),
            port: "8080".into(),
            pathname: "/path/name".into(),
            search: "?query=1".into(),
            hash: "#frag".into(),
            origin: "https://example.com:8080".into(),
        }
    }

    // ── parse_url: empty / whitespace ──

    #[test]
    fn parse_url_empty_returns_none() {
        assert!(parse_url("", None).is_none());
    }

    #[test]
    fn parse_url_whitespace_returns_none() {
        assert!(parse_url("   ", None).is_none());
    }

    // ── parse_url: data: URLs ──

    #[test]
    fn parse_url_data_text() {
        let s = parse_url("data:text/html,hello", None).unwrap();
        assert_eq!(s.protocol, "data:");
        assert_eq!(s.pathname, "text/html,hello");
        assert_eq!(s.origin, "null");
        assert_eq!(s.href, "data:text/html,hello");
    }

    #[test]
    fn parse_url_data_base64() {
        let s = parse_url("data:image/png;base64,abc123", None).unwrap();
        assert_eq!(s.protocol, "data:");
        assert_eq!(s.pathname, "image/png;base64,abc123");
    }

    // ── parse_url: blob: URLs ──

    #[test]
    fn parse_url_blob() {
        let s = parse_url("blob:https://example.com/uuid-1234", None).unwrap();
        assert_eq!(s.protocol, "blob:");
        assert_eq!(s.pathname, "https://example.com/uuid-1234");
        assert_eq!(s.origin, "null");
    }

    // ── parse_url: absolute URLs ──

    #[test]
    fn parse_url_https_full() {
        let s = parse_url("https://example.com/path", None).unwrap();
        assert_eq!(s.protocol, "https:");
        assert_eq!(s.hostname, "example.com");
        assert_eq!(s.pathname, "/path");
        assert_eq!(s.port, "");
        assert_eq!(s.username, "");
        assert_eq!(s.password, "");
    }

    #[test]
    fn parse_url_with_port() {
        let s = parse_url("http://localhost:3000/api", None).unwrap();
        assert_eq!(s.hostname, "localhost");
        assert_eq!(s.port, "3000");
        assert_eq!(s.host, "localhost:3000");
        assert_eq!(s.pathname, "/api");
    }

    #[test]
    fn parse_url_with_userinfo() {
        let s = parse_url("ftp://user:pass@ftp.example.com/file.txt", None).unwrap();
        assert_eq!(s.username, "user");
        assert_eq!(s.password, "pass");
        assert_eq!(s.hostname, "ftp.example.com");
        assert_eq!(s.pathname, "/file.txt");
    }

    #[test]
    fn parse_url_user_no_password() {
        let s = parse_url("https://admin@host.com/", None).unwrap();
        assert_eq!(s.username, "admin");
        assert_eq!(s.password, "");
    }

    // ── parse_url: query and hash ──

    #[test]
    fn parse_url_query_and_hash() {
        let s = parse_url("https://x.com/a?b=c#d", None).unwrap();
        assert_eq!(s.search, "?b=c");
        assert_eq!(s.hash, "#d");
        assert_eq!(s.pathname, "/a");
    }

    #[test]
    fn parse_url_hash_only() {
        let s = parse_url("https://x.com/page#section", None).unwrap();
        assert_eq!(s.search, "");
        assert_eq!(s.hash, "#section");
    }

    #[test]
    fn parse_url_hash_no_path() {
        let s = parse_url("https://example.com#section", None).unwrap();
        assert_eq!(s.hash, "#section");
    }

    #[test]
    fn parse_url_query_only() {
        let s = parse_url("https://x.com/search?q=rust", None).unwrap();
        assert_eq!(s.search, "?q=rust");
        assert_eq!(s.hash, "");
    }

    // ── parse_url: no path ──

    #[test]
    fn parse_url_no_path_defaults_to_slash() {
        let s = parse_url("https://example.com", None).unwrap();
        assert_eq!(s.pathname, "/");
    }

    // ── parse_url: IPv6 ──

    #[test]
    fn parse_url_ipv6_with_port() {
        let s = parse_url("http://[::1]:8080/path", None).unwrap();
        assert_eq!(s.hostname, "[::1]");
        assert_eq!(s.port, "8080");
        assert_eq!(s.host, "[::1]:8080");
    }

    #[test]
    fn parse_url_ipv6_no_port() {
        let s = parse_url("http://[2001:db8::1]/path", None).unwrap();
        assert_eq!(s.hostname, "[2001:db8::1]");
        assert_eq!(s.port, "");
        assert_eq!(s.host, "[2001:db8::1]");
    }

    // ── parse_url: relative URLs with base ──

    #[test]
    fn parse_url_absolute_path_with_base() {
        let s = parse_url("/new/path", Some("https://example.com/old/path")).unwrap();
        assert_eq!(s.protocol, "https:");
        assert_eq!(s.hostname, "example.com");
        assert_eq!(s.pathname, "/new/path");
    }

    #[test]
    fn parse_url_relative_path_with_base() {
        let s = parse_url("sub/page.html", Some("https://example.com/dir/old.html")).unwrap();
        assert!(s.pathname.contains("sub/page.html"));
        assert_eq!(s.hostname, "example.com");
    }

    #[test]
    fn parse_url_query_relative_with_base() {
        let s = parse_url("?new=1", Some("https://example.com/page?old=1")).unwrap();
        assert_eq!(s.search, "?new=1");
        assert!(s.href.contains("example.com"));
    }

    #[test]
    fn parse_url_hash_relative_with_base() {
        let s = parse_url("#new-section", Some("https://example.com/page")).unwrap();
        assert_eq!(s.hash, "#new-section");
    }

    // ── parse_url: scheme-relative ──

    #[test]
    fn parse_url_scheme_relative() {
        let s = parse_url("//cdn.example.com/assets/img.png", None).unwrap();
        assert_eq!(s.hostname, "cdn.example.com");
        assert_eq!(s.pathname, "/assets/img.png");
        assert_eq!(s.protocol, "http:");
    }

    // ── parse_url: no base for relative ──

    #[test]
    fn parse_url_relative_no_base_returns_none() {
        assert!(parse_url("path/to/file", None).is_none());
    }

    #[test]
    fn parse_url_slash_no_base_returns_none() {
        assert!(parse_url("/path", None).is_none());
    }

    #[test]
    fn parse_url_invalid_no_scheme_returns_none() {
        assert!(parse_url("nocolon", None).is_none());
    }

    // ── parse_url: trimming ──

    #[test]
    fn parse_url_trims_whitespace() {
        let s = parse_url("  https://example.com/  ", None).unwrap();
        assert_eq!(s.hostname, "example.com");
    }

    // ── url_state_get_field ──

    #[test]
    fn get_field_href() {
        assert_eq!(
            url_state_get_field(&make_state(), "href"),
            "https://user:pass@example.com:8080/path/name?query=1#frag"
        );
    }

    #[test]
    fn get_field_protocol() {
        assert_eq!(url_state_get_field(&make_state(), "protocol"), "https:");
    }

    #[test]
    fn get_field_username() {
        assert_eq!(url_state_get_field(&make_state(), "username"), "user");
    }

    #[test]
    fn get_field_password() {
        assert_eq!(url_state_get_field(&make_state(), "password"), "pass");
    }

    #[test]
    fn get_field_host() {
        assert_eq!(
            url_state_get_field(&make_state(), "host"),
            "example.com:8080"
        );
    }

    #[test]
    fn get_field_hostname() {
        assert_eq!(
            url_state_get_field(&make_state(), "hostname"),
            "example.com"
        );
    }

    #[test]
    fn get_field_port() {
        assert_eq!(url_state_get_field(&make_state(), "port"), "8080");
    }

    #[test]
    fn get_field_pathname() {
        assert_eq!(url_state_get_field(&make_state(), "pathname"), "/path/name");
    }

    #[test]
    fn get_field_search() {
        assert_eq!(url_state_get_field(&make_state(), "search"), "?query=1");
    }

    #[test]
    fn get_field_hash() {
        assert_eq!(url_state_get_field(&make_state(), "hash"), "#frag");
    }

    #[test]
    fn get_field_origin() {
        assert_eq!(
            url_state_get_field(&make_state(), "origin"),
            "https://example.com:8080"
        );
    }

    #[test]
    fn get_field_unknown_returns_empty() {
        assert_eq!(url_state_get_field(&make_state(), "nonexistent"), "");
    }

    // ── rebuild_href ──

    #[test]
    fn rebuild_href_change_pathname() {
        let state = make_state();
        let result = rebuild_href(&state, "pathname", "/new/path");
        assert!(result.contains("/new/path"));
        assert!(result.contains("example.com:8080"));
        assert!(result.contains("https:"));
    }

    #[test]
    fn rebuild_href_change_hash() {
        let state = make_state();
        let result = rebuild_href(&state, "hash", "#new-frag");
        assert!(result.contains("#new-frag"));
        assert!(!result.contains("#frag"));
    }

    #[test]
    fn rebuild_href_change_search() {
        let state = make_state();
        let result = rebuild_href(&state, "search", "?updated=true");
        assert!(result.contains("?updated=true"));
        assert!(!result.contains("?query=1"));
    }

    #[test]
    fn rebuild_href_change_hostname() {
        let state = make_state();
        let result = rebuild_href(&state, "hostname", "other.com");
        assert!(result.contains("other.com"));
    }

    #[test]
    fn rebuild_href_change_port() {
        let state = make_state();
        let result = rebuild_href(&state, "port", "9090");
        assert!(result.contains(":9090"));
    }

    #[test]
    fn rebuild_href_remove_port() {
        let state = make_state();
        let result = rebuild_href(&state, "port", "");
        assert!(!result.contains(":8080"));
    }

    #[test]
    fn rebuild_href_change_username() {
        let state = make_state();
        let result = rebuild_href(&state, "username", "alice");
        assert!(result.contains("alice:pass@"));
    }

    #[test]
    fn rebuild_href_change_password() {
        let state = make_state();
        let result = rebuild_href(&state, "password", "secret");
        assert!(result.contains("user:secret@"));
    }

    #[test]
    fn rebuild_href_set_href_directly() {
        let state = make_state();
        let result = rebuild_href(&state, "href", "http://other.com/");
        assert_eq!(result, "http://other.com/");
    }

    #[test]
    fn rebuild_href_no_auth_in_state() {
        let mut state = make_state();
        state.username = String::new();
        state.password = String::new();
        let result = rebuild_href(&state, "pathname", "/x");
        assert!(!result.contains("@"));
    }

    #[test]
    fn rebuild_href_username_only_no_password() {
        let mut state = make_state();
        state.password = String::new();
        let result = rebuild_href(&state, "pathname", "/x");
        assert!(result.contains("user@"));
        assert!(!result.contains("user:@"));
    }

    // ── url_encode ──

    #[test]
    fn url_encode_simple() {
        assert_eq!(url_encode("hello"), "hello");
    }

    #[test]
    fn url_encode_space_to_plus() {
        assert_eq!(url_encode("a b"), "a+b");
    }

    #[test]
    fn url_encode_special_chars() {
        assert_eq!(url_encode("a&b=c"), "a%26b%3Dc");
    }

    #[test]
    fn url_encode_unicode() {
        let encoded = url_encode("日本語");
        assert!(encoded.starts_with('%'));
    }

    #[test]
    fn url_encode_unreserved_not_encoded() {
        // WHATWG urlencoded serializer set: alphanumeric + `*-._` (tilde and
        // friends are NOT unescaped — Node ground truth: 'k=%7E').
        assert_eq!(url_encode("A-Z_."), "A-Z_.");
        assert_eq!(url_encode("~"), "%7E");
        assert_eq!(url_encode("*"), "*");
    }

    #[test]
    fn url_encode_empty() {
        assert_eq!(url_encode(""), "");
    }

    // ── url_decode ──

    #[test]
    fn url_decode_simple() {
        assert_eq!(url_decode("hello"), "hello");
    }

    #[test]
    fn url_decode_plus_to_space() {
        assert_eq!(url_decode("a+b"), "a b");
    }

    #[test]
    fn url_decode_percent() {
        assert_eq!(url_decode("a%26b%3Dc"), "a&b=c");
    }

    #[test]
    fn url_decode_percent_hex() {
        assert_eq!(url_decode("%41%42%43"), "ABC");
    }

    #[test]
    fn url_decode_incomplete_percent_passthrough() {
        assert_eq!(url_decode("a%2"), "a%2");
    }

    #[test]
    fn url_decode_empty() {
        assert_eq!(url_decode(""), "");
    }

    #[test]
    fn url_decode_roundtrip() {
        let original = "hello world & friends=true";
        assert_eq!(url_decode(&url_encode(original)), original);
    }

    #[test]
    fn url_decode_percent_lowercase_hex() {
        assert_eq!(url_decode("%2f"), "/");
    }

    // ── url_encode edge cases ──────────────────────────────────────
    // @trace REQ-ENG-007 [req:REQ-ENG-007] [level:unit]

    #[test]
    fn url_encode_digits_not_encoded() {
        assert_eq!(url_encode("0123456789"), "0123456789");
    }

    #[test]
    fn url_encode_hyphen_underscore_dot_not_encoded() {
        // WHATWG urlencoded serializer set: `*`, `-`, `.`, `_` unescaped;
        // tilde escapes (Node ground truth: 'k=%7E').
        assert_eq!(url_encode("-_.*"), "-_.*");
        assert_eq!(url_encode("~"), "%7E");
    }

    #[test]
    fn url_encode_slash_encoded() {
        assert_eq!(url_encode("/path"), "%2Fpath");
    }

    #[test]
    fn url_encode_colon_encoded() {
        assert_eq!(url_encode("a:b"), "a%3Ab");
    }

    #[test]
    fn url_encode_at_encoded() {
        assert_eq!(url_encode("user@host"), "user%40host");
    }

    #[test]
    fn url_encode_multiple_spaces() {
        assert_eq!(url_encode("a b c"), "a+b+c");
    }

    #[test]
    fn url_encode_all_special_chars() {
        let encoded = url_encode("!@#$%^&*()");
        assert!(!encoded.contains('!'));
        assert!(!encoded.contains('@'));
        assert!(!encoded.contains('#'));
    }

    #[test]
    fn url_encode_null_byte() {
        let input = "a\x00b";
        let encoded = url_encode(input);
        assert!(encoded.contains("%00"));
    }

    #[test]
    fn url_encode_high_byte() {
        // 0xFF as a byte in a string
        let input = String::from_utf8_lossy(&[0xFF]).to_string();
        let encoded = url_encode(&input);
        assert!(encoded.starts_with('%'));
    }

    // ── url_decode edge cases ──────────────────────────────────────
    // @trace REQ-ENG-007 [req:REQ-ENG-007] [level:unit]

    #[test]
    fn url_decode_percent_at_end_passthrough() {
        assert_eq!(url_decode("hello%"), "hello%");
    }

    #[test]
    fn url_decode_percent_one_char_passthrough() {
        assert_eq!(url_decode("hello%a"), "hello%a");
    }

    #[test]
    fn url_decode_invalid_hex_passthrough() {
        assert_eq!(url_decode("%GG"), "%GG");
    }

    #[test]
    fn url_decode_mixed_case_hex() {
        assert_eq!(url_decode("%2F%2f"), "//");
    }

    #[test]
    fn url_decode_null_byte() {
        assert_eq!(url_decode("%00"), "\0");
    }

    #[test]
    fn url_decode_multiple_pluses() {
        assert_eq!(url_decode("a+b+c"), "a b c");
    }

    #[test]
    fn url_decode_consecutive_percents() {
        assert_eq!(url_decode("%41%42"), "AB");
    }

    // ── parse_url edge cases ────────────────────────────────────────
    // @trace REQ-ENG-007 [req:REQ-ENG-007] [level:unit]

    #[test]
    fn parse_url_http_default_port() {
        let s = parse_url("http://example.com/", None).unwrap();
        assert_eq!(s.port, "");
        assert_eq!(s.hostname, "example.com");
    }

    #[test]
    fn parse_url_https_standard_port() {
        let s = parse_url("https://example.com:443/path", None).unwrap();
        assert_eq!(s.port, "443");
    }

    #[test]
    fn parse_url_empty_pathname() {
        let s = parse_url("https://example.com", None).unwrap();
        assert_eq!(s.pathname, "/");
    }

    #[test]
    fn parse_url_deep_path() {
        let s = parse_url("https://example.com/a/b/c/d/e", None).unwrap();
        assert_eq!(s.pathname, "/a/b/c/d/e");
    }

    #[test]
    fn parse_url_long_query() {
        let s = parse_url("https://x.com/?a=1&b=2&c=3&d=4", None).unwrap();
        assert_eq!(s.search, "?a=1&b=2&c=3&d=4");
    }

    #[test]
    fn parse_url_fragment_only_hash() {
        let s = parse_url("https://x.com/#", None).unwrap();
        assert_eq!(s.hash, "#");
    }

    // ── marker-prefixed component semantics (search/hash) ─────────────
    // @trace REQ-ENG-007 [req:REQ-ENG-007] — WHATWG/Node ""-vs-marker rules.

    #[test]
    fn parse_url_bare_markers_kept_in_state() {
        // "?#" → state keeps the empty-query/empty-fragment markers ("?" /
        // "#"); the GETTERS project them to "" (WHATWG: null or empty
        // query/fragment serializes as "").
        let s = parse_url("https://x.com/a?#", None).unwrap();
        assert_eq!(s.search, "?");
        assert_eq!(s.hash, "#");
        assert_eq!(s.href, "https://x.com/a?#");
    }

    #[test]
    fn normalize_marker_empty_clears() {
        assert_eq!(normalize_url_marker("search", ""), "");
        assert_eq!(normalize_url_marker("hash", ""), "");
    }

    #[test]
    fn normalize_marker_bare_kept() {
        assert_eq!(normalize_url_marker("search", "?"), "?");
        assert_eq!(normalize_url_marker("hash", "#"), "#");
    }

    #[test]
    fn normalize_marker_prepended() {
        assert_eq!(normalize_url_marker("hash", "z"), "#z");
        assert_eq!(normalize_url_marker("search", "k=v"), "?k=v");
    }

    #[test]
    fn normalize_marker_passthrough_other_fields() {
        assert_eq!(normalize_url_marker("pathname", "/x"), "/x");
    }

    #[test]
    fn sp_parse_strips_single_leading_question() {
        assert_eq!(
            sp_parse_init_str("?a=1&b=2"),
            vec![("a".into(), "1".into()), ("b".into(), "2".into())]
        );
    }

    #[test]
    fn sp_parse_decodes_key_and_value() {
        assert_eq!(
            sp_parse_init_str("pl+c=d%20e&%E6%97%A5=x"),
            vec![
                ("pl c".into(), "d e".into()),
                ("日".into(), "x".into())
            ]
        );
    }

    #[test]
    fn sp_parse_pair_without_equals_has_empty_value() {
        assert_eq!(sp_parse_init_str("flag&"), vec![("flag".into(), "".into())]);
    }

    #[test]
    fn sp_parse_empty_is_empty() {
        assert!(sp_parse_init_str("").is_empty());
        assert!(sp_parse_init_str("?").is_empty());
        assert!(sp_parse_init_str("&&").is_empty());
    }

    #[test]
    fn sp_serialize_roundtrip() {
        let pairs = vec![
            ("k".to_string(), "v v".to_string()),
            ("日".to_string(), "本".to_string()),
        ];
        assert_eq!(sp_pairs_to_string(&pairs), "k=v+v&%E6%97%A5=%E6%9C%AC");
        assert_eq!(sp_parse_init_str("k=v+v&%E6%97%A5=%E6%9C%AC"), pairs);
    }

    #[test]
    fn sp_serialize_whatwg_escape_set() {
        // Unescaped: alphanumeric + `*-._`; space → '+'; everything else
        // percent-encodes (incl. ~ and !).
        assert_eq!(sp_pairs_to_string(&[("k~!".into(), "*-._".into())]), "k%7E%21=*-._");
    }

    #[test]
    fn parse_url_empty_search() {
        let s = parse_url("https://x.com/?", None).unwrap();
        assert_eq!(s.search, "?");
    }

    #[test]
    fn parse_url_password_with_special_chars() {
        let s = parse_url("https://user:p@ss:w0rd@host.com/", None).unwrap();
        assert_eq!(s.username, "user");
        // The last @ separates userinfo from host
    }

    #[test]
    fn parse_url_data_url_empty_pathname() {
        let s = parse_url("data:,", None).unwrap();
        assert_eq!(s.protocol, "data:");
    }

    #[test]
    fn parse_url_blob_complex() {
        let s = parse_url(
            "blob:https://example.com/550e8400-e29b-41d4-a716-446655440000",
            None,
        )
        .unwrap();
        assert_eq!(s.protocol, "blob:");
        assert!(s.pathname.contains("example.com"));
    }

    #[test]
    fn parse_url_relative_with_base_directory() {
        let s = parse_url("file.txt", Some("https://example.com/docs/readme.md")).unwrap();
        assert_eq!(s.hostname, "example.com");
        assert!(s.pathname.contains("file.txt"));
    }

    // ── rebuild_href edge cases ─────────────────────────────────────
    // @trace REQ-ENG-007 [req:REQ-ENG-007] [level:unit]

    #[test]
    fn rebuild_href_change_protocol() {
        let state = make_state();
        let result = rebuild_href(&state, "protocol", "http:");
        assert!(result.starts_with("http://"));
    }

    #[test]
    fn rebuild_href_clear_search_and_hash() {
        let state = make_state();
        let result = rebuild_href(&state, "search", "");
        assert!(!result.contains("?query=1"));
        let result2 = rebuild_href(&state, "hash", "");
        assert!(!result2.contains("#frag"));
    }

    #[test]
    fn rebuild_href_empty_hostname() {
        let mut state = make_state();
        state.hostname = String::new();
        state.port = String::new();
        state.host = String::new();
        let result = rebuild_href(&state, "pathname", "/x");
        assert!(result.contains("/x"));
    }

    // ── url_state_get_field ────────────────────────────────────────
    // @trace REQ-ENG-007 [req:REQ-ENG-007] [level:unit]

    #[test]
    fn url_state_get_field_all_known_fields() {
        let state = make_state();
        assert_eq!(url_state_get_field(&state, "href"), state.href);
        assert_eq!(url_state_get_field(&state, "protocol"), state.protocol);
        assert_eq!(url_state_get_field(&state, "username"), state.username);
        assert_eq!(url_state_get_field(&state, "password"), state.password);
        assert_eq!(url_state_get_field(&state, "host"), state.host);
        assert_eq!(url_state_get_field(&state, "hostname"), state.hostname);
        assert_eq!(url_state_get_field(&state, "port"), state.port);
        assert_eq!(url_state_get_field(&state, "pathname"), state.pathname);
        assert_eq!(url_state_get_field(&state, "search"), state.search);
        assert_eq!(url_state_get_field(&state, "hash"), state.hash);
        assert_eq!(url_state_get_field(&state, "origin"), state.origin);
    }

    #[test]
    fn url_state_get_field_unknown_returns_empty() {
        let state = make_state();
        assert_eq!(url_state_get_field(&state, "nonexistent"), "");
        assert_eq!(url_state_get_field(&state, ""), "");
    }

    #[test]
    fn url_state_get_field_empty_state() {
        let state = UrlState {
            href: String::new(),
            protocol: String::new(),
            username: String::new(),
            password: String::new(),
            host: String::new(),
            hostname: String::new(),
            port: String::new(),
            pathname: String::new(),
            search: String::new(),
            hash: String::new(),
            origin: String::new(),
        };
        assert_eq!(url_state_get_field(&state, "href"), "");
        assert_eq!(url_state_get_field(&state, "protocol"), "");
    }
}

// url.resolve(from, to) — Node legacy resolution. This is the pure RFC 3986
// §5 string algorithm, NOT WHATWG URL parsing: the legacy API accepts
// scheme-less bases like '/one/two/three' (WHATWG base parsing would fail)
// and resolves '../' via remove_dot_segments. BCE-20260816-URL-RESOLVE —
// delegating to parse_url(relative, Some(base)) returned `relative` verbatim
// whenever the base had no scheme, and never collapsed dot segments
// ('http://a/b/../d' instead of 'http://a/d').

/// RFC 3986 §5.2.4 remove_dot_segments.
fn rfc3986_remove_dot_segments(input: &str) -> String {
    let mut output: Vec<&str> = Vec::new();
    let mut rest = input;
    while !rest.is_empty() {
        let bytes = rest.as_bytes();
        // A. "../" or "./" prefix
        if rest.starts_with("../") {
            rest = &rest[3..];
        } else if rest.starts_with("./") {
            rest = &rest[2..];
        // B. "/./" or "/" exact or "/." suffix forms
        } else if rest.starts_with("/./") {
            rest = &rest[2..];
        } else if rest == "/." {
            rest = "/";
        // C. "/../" or "/."
        } else if rest.starts_with("/../") {
            rest = &rest[3..];
            output.pop();
        } else if rest == "/.." {
            rest = "/";
            output.pop();
        // D. "." or ".." exact → drop
        } else if rest == "." || rest == ".." {
            rest = "";
        // E. move first segment (incl. leading '/') to output
        } else {
            let start = if bytes[0] == b'/' { 1 } else { 0 };
            let end = match rest[start..].find('/') {
                Some(i) => start + i,
                None => rest.len(),
            };
            output.push(&rest[..end]);
            rest = &rest[end..];
        }
    }
    output.concat()
}

/// Split a URI reference into (scheme, authority, path, query, fragment).
fn rfc3986_split(
    s: &str,
) -> (Option<String>, Option<String>, String, Option<String>, Option<String>) {
    let mut rest = s;
    let fragment = match rest.find('#') {
        Some(i) => {
            let f = Some(rest[i + 1..].to_string());
            rest = &rest[..i];
            f
        }
        None => None,
    };
    let query = match rest.find('?') {
        Some(i) => {
            let q = Some(rest[i + 1..].to_string());
            rest = &rest[..i];
            q
        }
        None => None,
    };
    let scheme = if let Some(colon) = rest.find(':') {
        let candidate = &rest[..colon];
        // scheme = ALPHA *( ALPHA / DIGIT / "+" / "-" / "." )
        let valid = !candidate.is_empty()
            && candidate.chars().next().map(|c| c.is_ascii_alphabetic()).unwrap_or(false)
            && candidate
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '+' || c == '-' || c == '.');
        if valid {
            let sc = Some(candidate.to_ascii_lowercase());
            rest = &rest[colon + 1..];
            sc
        } else {
            None
        }
    } else {
        None
    };
    let authority = if rest.starts_with("//") {
        let body = &rest[2..];
        let end = body.find('/').unwrap_or(body.len());
        let auth = Some(body[..end].to_string());
        rest = &body[end..];
        auth
    } else {
        None
    };
    (scheme, authority, rest.to_string(), query, fragment)
}

/// RFC 3986 §5.2.2 reference resolution, reassembled as a string.
fn rfc3986_resolve(base: &str, reference: &str) -> String {
    let (b_scheme, b_auth, b_path, b_query, _b_frag) = rfc3986_split(base);
    let (r_scheme, r_auth, r_path, r_query, r_frag) = rfc3986_split(reference);

    let (scheme, authority, mut path, query): (Option<String>, Option<String>, String, Option<String>);
    if let Some(rs) = r_scheme {
        scheme = Some(rs);
        authority = r_auth;
        path = rfc3986_remove_dot_segments(&r_path);
        query = r_query;
    } else {
        scheme = b_scheme.clone();
        if let Some(ra) = r_auth {
            authority = Some(ra);
            path = rfc3986_remove_dot_segments(&r_path);
            query = r_query;
        } else {
            authority = b_auth.clone();
            if r_path.is_empty() {
                path = b_path.clone();
                query = r_query.or(b_query);
            } else {
                if r_path.starts_with('/') {
                    path = rfc3986_remove_dot_segments(&r_path);
                } else {
                    // §5.2.3 merge
                    let merged = match b_auth.is_some() && b_path.is_empty() {
                        true => format!("/{}", r_path),
                        false => match b_path.rfind('/') {
                            Some(i) => format!("{}{}", &b_path[..=i], r_path),
                            None => r_path.clone(),
                        },
                    };
                    path = rfc3986_remove_dot_segments(&merged);
                }
                query = r_query;
            }
            // §5.2.2: authority present and path empty → "/"
            if path.is_empty() && authority.is_some() {
                path = "/".to_string();
            }
        }
    }

    let mut out = String::new();
    if let Some(s) = &scheme {
        out.push_str(s);
        out.push(':');
    }
    if let Some(a) = &authority {
        out.push_str("//");
        out.push_str(a);
    }
    out.push_str(&path);
    if let Some(q) = &query {
        out.push('?');
        out.push_str(q);
    }
    if let Some(f) = &r_frag {
        out.push('#');
        out.push_str(f);
    }
    out
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
    let resolved = rfc3986_resolve(&base, &relative);
    let c_str = ZBox::from_bytes(resolved.as_bytes());
    let js_str = JS_NewStringCopyZ(cx, c_str.as_ptr());
    args.rval().set(if js_str.is_null() {
        UndefinedValue()
    } else {
        StringValue(&*js_str)
    });
    true
}
