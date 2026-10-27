// @trace REQ-LIB-004 [criterion:5] — `Bao.run(path, options)` script-level
// sandbox: JS `{permission}` options parser + panic-safe guard scope.
//
// ── DEFAULT-DENY vs per-Page DEFAULT-ALLOW (deliberate contract difference) ──
//
// The per-Page face (bao_browser page_pool → PermissionCheck at
// page_pool.rs:296-313) defaults every missing key to ALLOW
// (`env.unwrap_or(true)`, bridge `None` = allow all): a Page is created by
// the embedder through a typed Rust config struct, so an absent key means the
// embedder made an informed choice not to restrict that class.
//
// `Bao.run` takes its permission object from JS at call time, i.e. from the
// very script (or whatever tricked it) being sandboxed. A script-level sandbox
// is an explicit minimal-privilege grant: a missing key must default to DENY
// (empty allow-list / `false`), never to allow. The bridge's `None`-slot
// (allow-all) is therefore reachable here only through an explicit `true` on
// the corresponding key; the absent-key default maps to `Some(vec![])` (empty
// allow-list = deny all), which the bridge checks already treat as total deny.
//
// ── Guard restore semantics (why a stack, not save/read) ──
//
// permission_bridge exposes only `set_permission` (no reader). That is sound
// here because of a structural invariant: the Bun/Bao global (and therefore
// `Bao.run`) is installed exclusively by `install_node_apis` on privileged
// CLI/engine realms — "These are NOT installed on browser page globals"
// (globals.rs install_node_apis) — while the only other `set_permission`
// writers are per-Page script-thread callbacks drained on page ScriptThreads
// (page_pool.rs), where the Bun global is absent. So on every thread where
// `Bao.run` can execute, the thread-local guard state IS the script-level
// stack state: restoring the stack's previous entry (or `None` when the stack
// empties) restores the true previous value, including `None`.

use ::std::cell::RefCell;

use mozjs::jsapi::*;
use mozjs::jsval::{UndefinedValue, JSVal};
use mozjs::rooted;
use mozjs::rust::wrappers2 as w2;

use crate::permission_bridge::PermissionCheck;

// ─── Parse failure carrier ─────────────────────────────────────────────────

/// A `{permission}` object that failed validation. Surfaced by the caller as a
/// TypeError with `code` defined (node shape; house pattern:
/// node_fs throw_fs_coded_error / bun_inspect_api throw_invalid_arg_type).
pub struct PermissionParseError {
    pub code: &'static str,
    pub message: String,
    /// `true` when a JS exception is already pending (a getter or proxy trap
    /// threw while the permission object was read, or the property walk
    /// itself failed). The caller must propagate the pending exception
    /// (return `false` without reporting) instead of reporting a second
    /// error over it.
    pub exception_pending: bool,
}

impl PermissionParseError {
    fn pending() -> Self {
        PermissionParseError {
            code: "",
            message: ::std::string::String::new(),
            exception_pending: true,
        }
    }

    fn arg_type(prop: &str, expected: &str, received: String) -> Self {
        PermissionParseError {
            code: "ERR_INVALID_ARG_TYPE",
            message: ::std::format!(
                "The \"{}\" argument must be of type {}. Received {}",
                prop,
                expected,
                received
            ),
            exception_pending: false,
        }
    }

    fn arg_value(message: String) -> Self {
        PermissionParseError {
            code: "ERR_INVALID_ARG_VALUE",
            message,
            exception_pending: false,
        }
    }
}

/// node v24 `Received ...` fragment of ERR_INVALID_ARG_TYPE messages
/// (oracle-verbatim shapes, mirror of node_fs received_fs_desc: undefined /
/// null / `type boolean (true)` / `type number (42)` / `type string ('abc')`
/// / `function ` / `an instance of Array|Object`). Never invokes user code
/// (no toString on object values — validation must be side-effect free).
#[allow(unsafe_op_in_unsafe_fn)]
pub(crate) unsafe fn received_descriptor(cx: *mut JSContext, v: JSVal) -> ::std::string::String {
    if v.is_undefined() {
        "undefined".to_string()
    } else if v.is_null() {
        "null".to_string()
    } else if v.is_boolean() {
        ::std::format!("type boolean ({})", v.to_boolean())
    } else if v.is_int32() {
        ::std::format!("type number ({})", v.to_int32())
    } else if v.is_double() {
        ::std::format!("type number ({})", v.to_double())
    } else if v.is_string() {
        let s = v.to_string();
        if s.is_null() {
            "type string".to_string()
        } else {
            ::std::format!("type string ({:?})", crate::jsstr_to_rust_string(cx, s))
        }
    } else if v.is_object() {
        rooted!(in(cx) let obj = v.to_object());
        if unsafe { mozjs_sys::jsapi::js::IsFunctionObject(obj.get()) } {
            return "function ".to_string();
        }
        let mut is_arr = false;
        rooted!(in(cx) let v_root = v);
        let ok = IsArrayObject(cx, v_root.handle().into(), &mut is_arr);
        if ok && is_arr {
            "an instance of Array".to_string()
        } else {
            "an instance of Object".to_string()
        }
    } else {
        "an unexpected value".to_string()
    }
}

// ─── Parsed option values ───────────────────────────────────────────────────

/// A parsed read/write/net key. `AllowAll` maps to the bridge's `None`
/// (allow-all) slot; `AllowList` maps to `Some(entries)` — the empty list is
/// the absent-key default (deny all), never `None`.
#[derive(Debug, Clone, PartialEq)]
enum ListPermission {
    AllowAll,
    AllowList(::std::vec::Vec<::std::string::String>),
}

impl ListPermission {
    fn deny_all() -> Self {
        ListPermission::AllowList(::std::vec::Vec::new())
    }

    fn into_bridge_slot(self) -> ::std::option::Option<::std::vec::Vec<::std::string::String>> {
        match self {
            ListPermission::AllowAll => ::std::option::Option::None,
            ListPermission::AllowList(entries) => ::std::option::Option::Some(entries),
        }
    }
}

/// Recognized `options.permission` keys. `sys` is deliberately NOT here (see
/// `unknown_key_error`).
const KNOWN_PERMISSION_KEYS: [&str; 5] = ["read", "write", "net", "env", "run"];

fn unknown_key_error(key: &str) -> PermissionParseError {
    let sys_note = if key == "sys" {
        " The \"sys\" class is enforced only on browser pages (per-Page permission); the script-level sandbox has no sys enforcement bridge."
    } else {
        ""
    };
    PermissionParseError::arg_value(::std::format!(
        "The \"options.permission\" object has an unknown property \"{}\". Recognized keys: read, write, net, env, run.{}",
        key,
        sys_note
    ))
}

/// Everything the walk collected for the five known keys. Absent-key defaults
/// live here: deny-all lists and `None` bools (surfaced as `false`).
#[derive(Default)]
struct ParsedPermission {
    read: ::std::option::Option<ListPermission>,
    write: ::std::option::Option<ListPermission>,
    net: ::std::option::Option<ListPermission>,
    env: ::std::option::Option<bool>,
    run: ::std::option::Option<bool>,
}

/// Parse `options.permission` (already known to be present and neither
/// undefined nor null) into a [`PermissionCheck`]. Fail-closed: every shape
/// violation surfaces as an error — nothing is silently skipped or coerced.
///
/// # Safety
/// `cx` must be a live JSContext on the current thread; `permission_val` must
/// be protected from GC for the duration of the call.
#[allow(unsafe_op_in_unsafe_fn)]
pub unsafe fn parse_permission_object(
    cx: *mut JSContext,
    permission_val: JSVal,
) -> ::std::result::Result<PermissionCheck, PermissionParseError> {
    if !permission_val.is_object() {
        return ::std::result::Result::Err(PermissionParseError::arg_type(
            "options.permission",
            "object",
            received_descriptor(cx, permission_val),
        ));
    }
    rooted!(in(cx) let perm_obj = permission_val.to_object());

    // A plain object is required: an Array would enumerate zero string keys
    // (indices are skipped by the walk) and silently parse as deny-all; a
    // function would do the same. Both are type errors, not empty configs.
    {
        rooted!(in(cx) let v_root = permission_val);
        let mut is_arr = false;
        if !IsArrayObject(cx, v_root.handle().into(), &mut is_arr) {
            return ::std::result::Result::Err(PermissionParseError::pending());
        }
        if is_arr || unsafe { mozjs_sys::jsapi::js::IsFunctionObject(perm_obj.get()) } {
            return ::std::result::Result::Err(PermissionParseError::arg_type(
                "options.permission",
                "object",
                received_descriptor(cx, permission_val),
            ));
        }
    }

    let mut parsed = ParsedPermission::default();
    let mut unknown_key: ::std::option::Option<::std::string::String> =
        ::std::option::Option::None;
    let mut first_error: ::std::option::Option<PermissionParseError> =
        ::std::option::Option::None;

    let walk_ok = crate::for_each_string_property(cx, perm_obj.handle().into(), |name, _id, value, got| {
        // First unknown key wins; its record is independent of the value
        // fetch (a throwing getter on an unknown key does not make the key
        // known — the key-rejection error supersedes the getter's exception,
        // which reporting our error over the pending one drops deliberately:
        // the actionable failure is the unknown key itself).
        if !KNOWN_PERMISSION_KEYS.contains(&name) {
            if unknown_key.is_none() {
                unknown_key = ::std::option::Option::Some(name.to_string());
            }
            return;
        }
        // Known key with a throwing getter: the value is unavailable and
        // guessing a default would silently shift the grant — fail closed by
        // propagating the pending exception.
        if !got {
            if first_error.is_none() {
                first_error = ::std::option::Option::Some(PermissionParseError::pending());
            }
            return;
        }
        // SAFETY: `cx` is the live context threaded through the walk; `value`
        // is kept rooted by the walk helper for the callback's duration
        // (for_each_string_property contract). The explicit unsafe block is
        // required: a closure body does not inherit the enclosing unsafe
        // fn's context.
        let outcome = unsafe {
            match name {
                "read" => parse_list_key(cx, value, "read")
                    .map(|v| parsed.read = ::std::option::Option::Some(v)),
                "write" => parse_list_key(cx, value, "write")
                    .map(|v| parsed.write = ::std::option::Option::Some(v)),
                "net" => parse_list_key(cx, value, "net")
                    .map(|v| parsed.net = ::std::option::Option::Some(v)),
                "env" => parse_bool_key(cx, value, "env").map(|v| parsed.env = v),
                "run" => parse_bool_key(cx, value, "run").map(|v| parsed.run = v),
                _ => ::std::result::Result::Ok(()),
            }
        };
        if let ::std::result::Result::Err(err) = outcome {
            if first_error.is_none() {
                first_error = ::std::option::Option::Some(err);
            }
        }
    });

    if let ::std::option::Option::Some(err) = first_error {
        return ::std::result::Result::Err(err);
    }
    if !walk_ok {
        // GetPropertyKeys failure (e.g. a proxy ownKeys trap threw): a JS
        // exception is pending — propagate it instead of reporting a second
        // error over it.
        return ::std::result::Result::Err(PermissionParseError::pending());
    }
    if let ::std::option::Option::Some(key) = unknown_key {
        return ::std::result::Result::Err(unknown_key_error(&key));
    }

    // Absent-key defaults materialize here: deny-all lists (`Some(vec![])`,
    // NOT the bridge's `None` = allow slot) and `false` bools.
    ::std::result::Result::Ok(PermissionCheck {
        read_paths: parsed
            .read
            .unwrap_or_else(ListPermission::deny_all)
            .into_bridge_slot(),
        write_paths: parsed
            .write
            .unwrap_or_else(ListPermission::deny_all)
            .into_bridge_slot(),
        net_hosts: parsed
            .net
            .unwrap_or_else(ListPermission::deny_all)
            .into_bridge_slot(),
        env_allowed: parsed.env.unwrap_or(false),
        run_allowed: parsed.run.unwrap_or(false),
    })
}

/// Parse one read/write/net value: `true` → allow all (bridge `None`), `false`
/// or absent → deny all, array of non-empty strings → explicit allow-list.
/// Anything else is an ERR_INVALID_ARG_TYPE failure.
#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn parse_list_key(
    cx: *mut JSContext,
    value: JSVal,
    key: &str,
) -> ::std::result::Result<ListPermission, PermissionParseError> {
    if value.is_undefined() {
        // Absent key AND explicit `{read: undefined}` both land here: the
        // default is DENY ALL (script-level minimal-privilege contract).
        return ::std::result::Result::Ok(ListPermission::deny_all());
    }
    if value.is_boolean() {
        return ::std::result::Result::Ok(if value.to_boolean() {
            ListPermission::AllowAll
        } else {
            ListPermission::deny_all()
        });
    }
    let prop = ::std::format!("options.permission.{}", key);
    if !value.is_object() {
        return ::std::result::Result::Err(PermissionParseError::arg_type(
            &prop,
            "boolean or an array of strings",
            received_descriptor(cx, value),
        ));
    }

    rooted!(in(cx) let v_root = value);
    let mut is_arr = false;
    if !IsArrayObject(cx, v_root.handle().into(), &mut is_arr) {
        return ::std::result::Result::Err(PermissionParseError::pending());
    }
    if !is_arr {
        return ::std::result::Result::Err(PermissionParseError::arg_type(
            &prop,
            "boolean or an array of strings",
            received_descriptor(cx, value),
        ));
    }

    rooted!(in(cx) let arr = value.to_object());
    let mut wcx = mozjs::context::JSContext::from_ptr(::std::ptr::NonNull::new_unchecked(cx));
    let mut len: u32 = 0;
    if !w2::GetArrayLength(&mut wcx, arr.handle().into(), &mut len) {
        return ::std::result::Result::Err(PermissionParseError::pending());
    }

    let mut entries = ::std::vec::Vec::with_capacity(len as usize);
    for i in 0..len {
        rooted!(in(cx) let mut elem = UndefinedValue());
        if !JS_GetElement(cx, arr.handle().into(), i, elem.handle_mut().into()) {
            return ::std::result::Result::Err(PermissionParseError::pending());
        }
        let elem_val = elem.get();
        if !elem_val.is_string() {
            return ::std::result::Result::Err(PermissionParseError::arg_type(
                &::std::format!("options.permission.{}[{}]", key, i),
                "string",
                received_descriptor(cx, elem_val),
            ));
        }
        let entry = crate::js_to_rust_string(cx, elem_val);
        if entry.is_empty() {
            // `check_fs_read`/`check_net` match by prefix/equality: an empty
            // string entry would match EVERY target and silently void the
            // sandbox the caller asked for. Reject instead of honoring it.
            return ::std::result::Result::Err(PermissionParseError::arg_value(::std::format!(
                "The \"options.permission.{}[{}]\" argument must be a non-empty string: an empty allow-list entry would match every target and silently void the sandbox.",
                key,
                i
            )));
        }
        entries.push(entry);
    }
    ::std::result::Result::Ok(ListPermission::AllowList(entries))
}

/// Parse one env/run value: absent or explicit `undefined` → `None` (surfaced
/// as `false` = deny), boolean → itself, anything else → ERR_INVALID_ARG_TYPE.
#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn parse_bool_key(
    cx: *mut JSContext,
    value: JSVal,
    key: &str,
) -> ::std::result::Result<::std::option::Option<bool>, PermissionParseError> {
    if value.is_undefined() {
        return ::std::result::Result::Ok(::std::option::Option::None);
    }
    if value.is_boolean() {
        return ::std::result::Result::Ok(::std::option::Option::Some(value.to_boolean()));
    }
    ::std::result::Result::Err(PermissionParseError::arg_type(
        &::std::format!("options.permission.{}", key),
        "boolean",
        received_descriptor(cx, value),
    ))
}

// ─── Panic-safe guard scope ─────────────────────────────────────────────────

thread_local! {
    /// Script-level guard stack (see module header for the thread-state
    /// invariant that makes this the authoritative guard state on every
    /// thread where `Bao.run` is reachable).
    static SCRIPT_GUARD_STACK: RefCell<::std::vec::Vec<PermissionCheck>> =
        const { RefCell::new(::std::vec::Vec::new()) };
}

/// Installs a script-level [`PermissionCheck`] on the current thread for the
/// lifetime of the returned scope. Restore runs in `Drop` — on the ok path,
/// every error return, AND on unwind (panic-safe; never a bare set-back).
///
/// Nesting: an inner `Bao.run`'s scope restores the outer scope's check; the
/// outermost scope restores `None` (the base state on `Bao.run`-reachable
/// threads — see the module-header invariant).
pub struct ScriptPermissionScope {
    _unconstructible: (),
}

impl ScriptPermissionScope {
    pub fn install(check: PermissionCheck) -> Self {
        SCRIPT_GUARD_STACK.with(|stack| {
            stack.borrow_mut().push(check.clone());
        });
        crate::permission_bridge::set_permission(::std::option::Option::Some(check));
        ScriptPermissionScope {
            _unconstructible: (),
        }
    }
}

impl ::std::ops::Drop for ScriptPermissionScope {
    fn drop(&mut self) {
        let restore = SCRIPT_GUARD_STACK.with(|stack| {
            let mut slots = stack.borrow_mut();
            slots.pop();
            slots.last().cloned()
        });
        crate::permission_bridge::set_permission(restore);
    }
}
