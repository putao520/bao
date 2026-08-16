// @trace REQ-ENG-008 [entity:SqliteDatabase] [api:GET /api/sqlite-bridge]
// bun:sqlite SpiderMonkey bridge — Database + Statement classes.
//
// Architecture: Native pointers stored in JS object reserved slot 0
// via PrivateValue. Uses JS_InitClass for proper constructor/prototype chain.

use ::std::cell::RefCell;
use ::std::ptr::NonNull;
use ::std::result::Result;
use bun_core::ZBox;

use mozjs::glue::JS_GetReservedSlot;
use mozjs::jsapi::*;
use mozjs::jsval::{
    BooleanValue, DoubleValue, JSVal, NullValue, ObjectValue, PrivateValue, StringValue,
    UndefinedValue,
};
use mozjs::rooted;
use mozjs::rust::wrappers2 as w2;

use rusqlite::Connection;

use crate::require::cache_builtin;

// ── Reserved slot indices ──

const SLOT_DB: u32 = 0;
const SLOT_STMT: u32 = 0;

// ── JSClass definitions ──

static DATABASE_CLASS: JSClass = JSClass {
    name: c"Database".as_ptr(),
    flags: (1u32 << JSCLASS_RESERVED_SLOTS_SHIFT),
    cOps: ::std::ptr::null(),
    spec: ::std::ptr::null(),
    ext: ::std::ptr::null(),
    oOps: ::std::ptr::null(),
};

static STATEMENT_CLASS: JSClass = JSClass {
    name: c"Statement".as_ptr(),
    flags: (1u32 << JSCLASS_RESERVED_SLOTS_SHIFT),
    cOps: ::std::ptr::null(),
    spec: ::std::ptr::null(),
    ext: ::std::ptr::null(),
    oOps: ::std::ptr::null(),
};

// ── Rust data types ──

/// Opaque handle wrapping a rusqlite::Connection.
pub struct SqliteDatabase {
    conn: RefCell<Option<Connection>>,
}

impl SqliteDatabase {
    pub fn new(path: &str) -> Result<Self, String> {
        let conn = if path == ":memory:" {
            Connection::open_in_memory().map_err(|e| e.to_string())?
        } else {
            Connection::open(path).map_err(|e| e.to_string())?
        };
        Ok(Self {
            conn: RefCell::new(Some(conn)),
        })
    }

    pub fn exec(&self, sql: &str) -> Result<(), String> {
        let borrow = self.conn.borrow();
        let conn = borrow.as_ref().ok_or("Database is closed")?;
        conn.execute_batch(sql).map_err(|e| e.to_string())
    }

    pub fn run(&self, sql: &str) -> Result<RunResult, String> {
        let borrow = self.conn.borrow();
        let conn = borrow.as_ref().ok_or("Database is closed")?;
        conn.execute_batch(sql).map_err(|e| e.to_string())?;
        Ok(RunResult {
            changes: conn.changes(),
            last_insert_rowid: conn.last_insert_rowid(),
        })
    }

    pub fn close(&self) -> Result<(), String> {
        let mut borrow = self.conn.borrow_mut();
        borrow.take().ok_or("Database already closed")?;
        Ok(())
    }

    pub fn is_closed(&self) -> bool {
        self.conn.borrow().is_none()
    }

    pub fn in_transaction(&self) -> bool {
        let borrow = self.conn.borrow();
        match borrow.as_ref() {
            Some(conn) => !conn.is_autocommit(),
            None => false,
        }
    }

    /// Serialize the whole database to bytes — a real SQLite snapshot via
    /// `VACUUM INTO` (SQLite's own consistent-snapshot machinery), read back
    /// from the temp target and the temp file removed. The produced bytes are
    /// a valid, standalone database file.
    pub fn serialize_bytes(&self) -> Result<Vec<u8>, String> {
        use ::std::sync::atomic::{AtomicU64, Ordering};
        static SERIALIZE_COUNTER: AtomicU64 = AtomicU64::new(0);
        let n = SERIALIZE_COUNTER.fetch_add(1, Ordering::Relaxed);
        let path = ::std::env::temp_dir().join(format!(
            "bao-sqlite-ser-{}-{}.db",
            ::std::process::id(),
            n
        ));
        let path_str = path.to_string_lossy().to_string();
        // VACUUM INTO requires the target NOT to exist — the counter+pid name
        // guarantees that for a fresh path.
        {
            let borrow = self.conn.borrow();
            let conn = borrow.as_ref().ok_or("Database is closed")?;
            conn.execute("VACUUM INTO ?1", [&path_str])
                .map_err(|e| e.to_string())?;
        }
        let bytes = ::std::fs::read(&path).map_err(|e| e.to_string())?;
        let _ = ::std::fs::remove_file(&path);
        Ok(bytes)
    }

    /// Backup (snapshot) the database to `path` via `VACUUM INTO` — SQLite's
    /// official online-backup equivalent producing a consistent standalone
    /// database file. Fails when the target already exists (SQLite contract).
    pub fn backup_to_path(&self, path: &str) -> Result<(), String> {
        let borrow = self.conn.borrow();
        let conn = borrow.as_ref().ok_or("Database is closed")?;
        if ::std::path::Path::new(path).exists() {
            return Err(format!(
                "backup target already exists: {} (SQLite VACUUM INTO requires a new file)",
                path
            ));
        }
        conn.execute("VACUUM INTO ?1", [path])
            .map_err(|e| e.to_string())?;
        Ok(())
    }
}

/// Live row-cursor for `Statement#iterate()`. The `Rows` borrow is transmuted
/// to 'static (same lifetime discipline as `SqliteStatement`: the parent
/// statement must outlive the iterator — the raw stmt pointer pins it).
pub struct SqliteIterator {
    rows: RefCell<Option<rusqlite::Rows<'static>>>,
    col_names: Vec<String>,
}

/// Savepoint name counter for nested transactions (Bun maps nested
/// transactions onto SAVEPOINT/RELEASE automatically).
static SAVEPOINT_COUNTER: ::std::sync::atomic::AtomicU64 = ::std::sync::atomic::AtomicU64::new(0);

/// Result of a .run() call.
pub struct RunResult {
    pub changes: u64,
    pub last_insert_rowid: i64,
}

/// Opaque handle wrapping a rusqlite::Statement.
pub struct SqliteStatement {
    stmt: RefCell<Option<rusqlite::Statement<'static>>>,
    db_ptr: *mut SqliteDatabase,
}

// Safety: SqliteStatement holds a raw pointer to SqliteDatabase.
// The Database JS object must outlive the Statement JS object.
unsafe impl Send for SqliteStatement {}

// ── JS method tables ──

const DATABASE_METHODS: &[JSFunctionSpec] = &[
    JSFunctionSpec {
        name: JSPropertySpec_Name {
            string_: c"exec".as_ptr(),
        },
        call: JSNativeWrapper {
            op: Some(database_exec),
            info: ::std::ptr::null_mut(),
        },
        nargs: 1,
        flags: JSPROP_ENUMERATE as u16,
        selfHostedName: ::std::ptr::null_mut(),
    },
    JSFunctionSpec {
        name: JSPropertySpec_Name {
            string_: c"run".as_ptr(),
        },
        call: JSNativeWrapper {
            op: Some(database_run),
            info: ::std::ptr::null_mut(),
        },
        nargs: 1,
        flags: JSPROP_ENUMERATE as u16,
        selfHostedName: ::std::ptr::null_mut(),
    },
    JSFunctionSpec {
        name: JSPropertySpec_Name {
            string_: c"close".as_ptr(),
        },
        call: JSNativeWrapper {
            op: Some(database_close),
            info: ::std::ptr::null_mut(),
        },
        nargs: 0,
        flags: JSPROP_ENUMERATE as u16,
        selfHostedName: ::std::ptr::null_mut(),
    },
    JSFunctionSpec {
        name: JSPropertySpec_Name {
            string_: c"query".as_ptr(),
        },
        call: JSNativeWrapper {
            op: Some(database_query),
            info: ::std::ptr::null_mut(),
        },
        nargs: 2,
        flags: JSPROP_ENUMERATE as u16,
        selfHostedName: ::std::ptr::null_mut(),
    },
    JSFunctionSpec {
        name: JSPropertySpec_Name {
            string_: c"prepare".as_ptr(),
        },
        call: JSNativeWrapper {
            op: Some(database_prepare),
            info: ::std::ptr::null_mut(),
        },
        nargs: 1,
        flags: JSPROP_ENUMERATE as u16,
        selfHostedName: ::std::ptr::null_mut(),
    },
    JSFunctionSpec {
        name: JSPropertySpec_Name {
            string_: c"transaction".as_ptr(),
        },
        call: JSNativeWrapper {
            op: Some(database_transaction),
            info: ::std::ptr::null_mut(),
        },
        nargs: 1,
        flags: JSPROP_ENUMERATE as u16,
        selfHostedName: ::std::ptr::null_mut(),
    },
    JSFunctionSpec {
        name: JSPropertySpec_Name {
            string_: c"serialize".as_ptr(),
        },
        call: JSNativeWrapper {
            op: Some(database_serialize),
            info: ::std::ptr::null_mut(),
        },
        nargs: 0,
        flags: JSPROP_ENUMERATE as u16,
        selfHostedName: ::std::ptr::null_mut(),
    },
    JSFunctionSpec {
        name: JSPropertySpec_Name {
            string_: c"backup".as_ptr(),
        },
        call: JSNativeWrapper {
            op: Some(database_backup),
            info: ::std::ptr::null_mut(),
        },
        nargs: 1,
        flags: JSPROP_ENUMERATE as u16,
        selfHostedName: ::std::ptr::null_mut(),
    },
    JSFunctionSpec::ZERO,
];

#[allow(dead_code)]
const STATEMENT_METHODS: &[JSFunctionSpec] = &[
    JSFunctionSpec {
        name: JSPropertySpec_Name {
            string_: c"run".as_ptr(),
        },
        call: JSNativeWrapper {
            op: Some(statement_run),
            info: ::std::ptr::null_mut(),
        },
        nargs: 0,
        flags: JSPROP_ENUMERATE as u16,
        selfHostedName: ::std::ptr::null_mut(),
    },
    JSFunctionSpec {
        name: JSPropertySpec_Name {
            string_: c"get".as_ptr(),
        },
        call: JSNativeWrapper {
            op: Some(statement_get),
            info: ::std::ptr::null_mut(),
        },
        nargs: 0,
        flags: JSPROP_ENUMERATE as u16,
        selfHostedName: ::std::ptr::null_mut(),
    },
    JSFunctionSpec {
        name: JSPropertySpec_Name {
            string_: c"all".as_ptr(),
        },
        call: JSNativeWrapper {
            op: Some(statement_all),
            info: ::std::ptr::null_mut(),
        },
        nargs: 0,
        flags: JSPROP_ENUMERATE as u16,
        selfHostedName: ::std::ptr::null_mut(),
    },
    JSFunctionSpec::ZERO,
];

// ── Helper: extract native pointer from reserved slot 0 ──

/// Check if a JSVal is a PrivateValue (double with zero high bits).
/// @trace BCE-20260618-002 [level:regression]
#[inline]
fn val_is_private(v: &JSVal) -> bool {
    v.is_double() && (v.asBits_ & 0xFFFF000000000000) == 0
}

unsafe fn get_db_ptr(cx: *mut JSContext, thisv: Handle<Value>) -> Option<*mut SqliteDatabase> {
    if !thisv.is_object() {
        return None;
    }
    let wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    rooted!(&in(wrapped_cx) let obj_root = thisv.to_object());
    let mut slot = UndefinedValue();
    JS_GetReservedSlot(obj_root.get(), SLOT_DB, &mut slot);
    // @trace BCE-20260618-002 — guard non-private doubles before to_private().
    if !val_is_private(&slot) {
        return None;
    }
    let ptr = slot.to_private() as *mut SqliteDatabase;
    if ptr.is_null() { None } else { Some(ptr) }
}

unsafe fn get_stmt_ptr(cx: *mut JSContext, thisv: Handle<Value>) -> Option<*mut SqliteStatement> {
    if !thisv.is_object() {
        return None;
    }
    let wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    rooted!(&in(wrapped_cx) let obj_root = thisv.to_object());
    let mut slot = UndefinedValue();
    JS_GetReservedSlot(obj_root.get(), SLOT_STMT, &mut slot);
    // @trace BCE-20260618-002 — guard non-private doubles before to_private().
    if !val_is_private(&slot) {
        return None;
    }
    let ptr = slot.to_private() as *mut SqliteStatement;
    if ptr.is_null() { None } else { Some(ptr) }
}

// ── Helper: convert rusqlite Value to JSVal ──

unsafe fn sqlite_value_to_jsval(cx: *mut JSContext, val: rusqlite::types::Value) -> JSVal {
    match val {
        rusqlite::types::Value::Null => NullValue(),
        rusqlite::types::Value::Integer(n) => {
            // Keep small integers exact as JS numbers (Int32); fall back to
            // double for i64 values outside the int32 range.
            if n >= i32::MIN as i64 && n <= i32::MAX as i64 {
                mozjs::jsval::Int32Value(n as i32)
            } else {
                DoubleValue(n as f64)
            }
        }
        rusqlite::types::Value::Real(f) => DoubleValue(f),
        rusqlite::types::Value::Text(s) => {
            let c_str = ZBox::from_bytes(s.as_bytes());
            let js_str = JS_NewStringCopyZ(cx, c_str.as_ptr());
            if js_str.is_null() {
                NullValue()
            } else {
                mozjs::jsval::StringValue(&*js_str)
            }
        }
        // BCE (v-surface P0-2): BLOB columns previously mapped to JS null —
        // silent data loss. Return the bytes as a Buffer (Uint8Array).
        rusqlite::types::Value::Blob(b) => {
            let buf = crate::globals::create_buffer_object(cx, &b);
            if buf.is_null() {
                NullValue()
            } else {
                ObjectValue(buf)
            }
        }
    }
}

// ── Helper: build a JS row object from a rusqlite Row ──

unsafe fn row_to_js_object(
    cx: *mut JSContext,
    row: &rusqlite::Row,
    col_names: &[String],
    cx_ref: &mut mozjs::context::JSContext,
) -> *mut JSObject {
    rooted!(&in(cx_ref) let row_obj = w2::JS_NewPlainObject(cx_ref));
    if row_obj.get().is_null() {
        return ::std::ptr::null_mut();
    }
    for (col_idx, col_name) in col_names.iter().enumerate() {
        let val: rusqlite::Result<rusqlite::types::Value> = row.get(col_idx);
        let js_val = match val {
            Ok(v) => sqlite_value_to_jsval(cx, v),
            Err(_) => NullValue(),
        };
        rooted!(&in(cx_ref) let rv = js_val);
        let c_name = ZBox::from_bytes(col_name.as_bytes());
        JS_DefineProperty(
            cx,
            row_obj.handle().into(),
            c_name.as_ptr(),
            rv.handle().into(),
            (JSPROP_ENUMERATE | JSPROP_PERMANENT) as u32,
        );
    }
    row_obj.get()
}

// ── Parameter binding (bun:sqlite bridge) ──────────────────────────────────
//
// BCE (v-surface P0-2): statement run/get/all and database query/run called
// `stmt.execute([])` / `stmt.query([])` — the JS arguments were never
// forwarded, so EVERY parameterized query failed with "Wrong number of
// parameters: got 0". These helpers implement the bun:sqlite binding
// conventions on top of rusqlite's raw-bind API:
//   stmt.run('a', 1)          → variadic positional
//   stmt.run(['a', 1])        → positional array (single Array arg)
//   stmt.run({ $name: 'a' })  → named params (implicit prefix inference)
// Value mapping: null/undefined → NULL, bool → 0/1, number → INTEGER/REAL,
// string → TEXT, Buffer/TypedArray/number[] → BLOB, BigInt → INTEGER.

/// Convert a single JS value into a rusqlite column value.
#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn js_val_to_sql_value(cx: *mut JSContext, val: JSVal) -> Result<rusqlite::types::Value, String> {
    if val.is_null() || val.is_undefined() {
        Ok(rusqlite::types::Value::Null)
    } else if val.is_boolean() {
        Ok(rusqlite::types::Value::Integer(if val.to_boolean() { 1 } else { 0 }))
    } else if val.is_int32() {
        Ok(rusqlite::types::Value::Integer(val.to_int32() as i64))
    } else if val.is_double() {
        let d = val.to_double();
        // Integral doubles within the exact-i64 range bind as INTEGER
        // (SQLite type affinity: `WHERE x = 3` must match a bound 3.0).
        if d.fract() == 0.0 && d.abs() <= 9_223_372_036_854_775_807.0 {
            Ok(rusqlite::types::Value::Integer(d as i64))
        } else {
            Ok(rusqlite::types::Value::Real(d))
        }
    } else if val.is_string() {
        Ok(rusqlite::types::Value::Text(
            crate::js_to_rust_string(cx, val),
        ))
    } else if val.is_bigint() {
        // BigInt → INTEGER via decimal string (values beyond i64 rejected).
        let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
        let cx_ref = &mut wrapped_cx;
        rooted!(&in(cx_ref) let v_root = val);
        let jsstr = mozjs::rust::ToString(cx_ref, v_root.handle());
        if jsstr.is_null() {
            return Err("BigInt parameter conversion failed".to_string());
        }
        let str_val = mozjs::jsval::StringValue(&*jsstr);
        let s = crate::js_to_rust_string(cx, str_val);
        s.trim()
            .parse::<i64>()
            .map(rusqlite::types::Value::Integer)
            .map_err(|_| format!("BigInt value out of range for INTEGER: {}", s))
    } else if val.is_object() {
        Ok(rusqlite::types::Value::Blob(crate_buffer_bytes(cx, val)))
    } else {
        Err("unsupported parameter value type".to_string())
    }
}

/// Extract Buffer/TypedArray/plain-number-array bytes (same coercion as
/// node_crypto::extract_buffer_bytes, kept local for module cohesion).
#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn crate_buffer_bytes(cx: *mut JSContext, val: JSVal) -> Vec<u8> {
    if !val.is_object() {
        return Vec::new();
    }
    let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;
    rooted!(&in(cx_ref) let obj_root = val.to_object());
    let mut length: usize = 0;
    let mut is_shared = false;
    let mut data_ptr: *mut u8 = ::std::ptr::null_mut();
    let u8_unwrapped = mozjs_sys::jsapi::JS_GetObjectAsUint8Array(
        obj_root.get(),
        &mut length,
        &mut is_shared,
        &mut data_ptr,
    );
    if !u8_unwrapped.is_null() && !data_ptr.is_null() && length > 0 {
        return ::std::slice::from_raw_parts(data_ptr, length).to_vec();
    }
    let mut view_length: usize = 0;
    let mut view_shared = false;
    let mut view_data: *mut u8 = ::std::ptr::null_mut();
    let view_unwrapped = mozjs_sys::jsapi::JS_GetObjectAsArrayBufferView(
        obj_root.get(),
        &mut view_length,
        &mut view_shared,
        &mut view_data,
    );
    if !view_unwrapped.is_null() && !view_data.is_null() && view_length > 0 {
        return ::std::slice::from_raw_parts(view_data, view_length).to_vec();
    }
    // Plain number[] fallback.
    let mut len_val = UndefinedValue();
    JS_GetProperty(
        cx,
        obj_root.handle().into(),
        c"length".as_ptr(),
        MutableHandle::<Value> {
            _phantom_0: ::std::marker::PhantomData,
            ptr: &mut len_val,
        },
    );
    let len = if len_val.is_int32() {
        len_val.to_int32() as usize
    } else {
        return Vec::new();
    };
    let mut bytes = Vec::with_capacity(len);
    for i in 0u32..len as u32 {
        let mut byte_val = UndefinedValue();
        JS_GetElement(
            cx,
            obj_root.handle().into(),
            i,
            MutableHandle::<Value> {
                _phantom_0: ::std::marker::PhantomData,
                ptr: &mut byte_val,
            },
        );
        bytes.push(if byte_val.is_int32() {
            byte_val.to_int32() as u8
        } else {
            0
        });
    }
    bytes
}

/// True when the value is a Buffer/TypedArray/DataView (binary payload, binds
/// as a single BLOB positional parameter rather than named params).
#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn is_binary_object(cx: *mut JSContext, val: JSVal) -> bool {
    if !val.is_object() {
        return false;
    }
    let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;
    rooted!(&in(cx_ref) let obj_root = val.to_object());
    let mut length: usize = 0;
    let mut is_shared = false;
    let mut data_ptr: *mut u8 = ::std::ptr::null_mut();
    let u8_unwrapped = mozjs_sys::jsapi::JS_GetObjectAsUint8Array(
        obj_root.get(),
        &mut length,
        &mut is_shared,
        &mut data_ptr,
    );
    if !u8_unwrapped.is_null() {
        return true;
    }
    let mut view_length: usize = 0;
    let mut view_shared = false;
    let mut view_data: *mut u8 = ::std::ptr::null_mut();
    let view_unwrapped = mozjs_sys::jsapi::JS_GetObjectAsArrayBufferView(
        obj_root.get(),
        &mut view_length,
        &mut view_shared,
        &mut view_data,
    );
    !view_unwrapped.is_null()
}

/// Read a property off a JS object, returning Undefined when absent.
#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn js_get_prop_val(cx: *mut JSContext, obj: *mut JSObject, name: &str) -> JSVal {
    let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;
    rooted!(&in(cx_ref) let obj_root = obj);
    let c_name = ZBox::from_bytes(name.as_bytes());
    let mut v = UndefinedValue();
    JS_GetProperty(
        cx,
        obj_root.handle().into(),
        c_name.as_ptr(),
        MutableHandle::<Value> {
            _phantom_0: ::std::marker::PhantomData,
            ptr: &mut v,
        },
    );
    v
}

/// Bind JS call arguments to a prepared statement (bun:sqlite conventions).
/// `start` is the first CallArgs index that carries a parameter (0 for
/// Statement.run/get/all, 1 for Database one-shot query/run where args[0] is
/// the SQL string). `end` is exclusive.
#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn bind_stmt_args(
    cx: *mut JSContext,
    stmt: &mut rusqlite::Statement<'_>,
    args: &CallArgs,
    start: u32,
    end: u32,
) -> Result<(), String> {
    let argc = end.saturating_sub(start);
    if argc == 0 {
        return Ok(());
    }
    let first = *args.get(start).ptr;
    let needed = stmt.parameter_count();

    // Single Array argument → positional binding from its elements.
    if argc == 1 && first.is_object() && !is_binary_object(cx, first) {
        let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
        let cx_ref = &mut wrapped_cx;
        rooted!(&in(cx_ref) let arr = first.to_object());
        let mut is_arr = false;
        if IsArrayObject1(cx, arr.handle().into(), &mut is_arr) && is_arr {
            let mut len: u32 = 0;
            if !w2::GetArrayLength(cx_ref, arr.handle().into(), &mut len) {
                return Err("failed to read parameter array length".to_string());
            }
            if (len as usize) < needed {
                return Err(format!(
                    "Wrong number of parameters: got {}, needed {}",
                    len, needed
                ));
            }
            for i in 0..len {
                let mut elem = UndefinedValue();
                JS_GetElement(
                    cx,
                    arr.handle().into(),
                    i,
                    MutableHandle::<Value> {
                        _phantom_0: ::std::marker::PhantomData,
                        ptr: &mut elem,
                    },
                );
                let v = js_val_to_sql_value(cx, elem)
                    .map_err(|e| format!("parameter {} invalid: {}", i + 1, e))?;
                stmt
                    .raw_bind_parameter((i as usize) + 1, &v)
                    .map_err(|e| e.to_string())?;
            }
            return Ok(());
        }
    }

    // Single plain-object argument → named parameters. Bun infers the sigil:
    // `{ name: v }` binds `$name`/`@name`/`:name`.
    if argc == 1 && first.is_object() && !is_binary_object(cx, first) {
        let obj = first.to_object();
        let count = stmt.parameter_count();
        for idx in 1..=count {
            let name = match stmt.parameter_name(idx) {
                Some(n) => n.to_string(),
                None => {
                    return Err(
                        "cannot mix named and positional parameters in one query"
                            .to_string(),
                    )
                }
            };
            // Exact ("$x") first, then sigil-less ("x").
            let mut v = js_get_prop_val(cx, obj, &name);
            if v.is_undefined() {
                let stripped = name
                    .strip_prefix('$')
                    .or_else(|| name.strip_prefix('@'))
                    .or_else(|| name.strip_prefix(':'))
                    .unwrap_or(&name);
                v = js_get_prop_val(cx, obj, stripped);
            }
            if v.is_undefined() {
                return Err(format!("Missing named parameter \"{}\"", name));
            }
            let sql_val = js_val_to_sql_value(cx, v)
                .map_err(|e| format!("parameter {} invalid: {}", name, e))?;
            stmt
                .raw_bind_parameter(idx, &sql_val)
                .map_err(|e| e.to_string())?;
        }
        return Ok(());
    }

    // Variadic positional (also the path for a single binary object arg).
    if (argc as usize) < needed {
        return Err(format!(
            "Wrong number of parameters: got {}, needed {}",
            argc, needed
        ));
    }
    for i in start..end {
        let v = js_val_to_sql_value(cx, *args.get(i).ptr)
            .map_err(|e| format!("parameter {} invalid: {}", i - start + 1, e))?;
        stmt
            .raw_bind_parameter((i - start + 1) as usize, &v)
            .map_err(|e| e.to_string())?;
    }
    Ok(())
}

// ── Database constructor ──

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn database_constructor(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);

    let this = JS_NewObjectForConstructor(cx, &DATABASE_CLASS, &args);
    if this.is_null() {
        JS_ClearPendingException(cx);
        let this_val = args.thisv();
        if this_val.is_object() {
            let wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
            rooted!(&in(wrapped_cx) let this_root = this_val.to_object());
            args.rval().set(ObjectValue(this_root.get()));
        } else {
            args.rval().set(UndefinedValue());
        }
        return true;
    }

    let path = if argc >= 1 {
        let path_val = *args.get(0).ptr;
        if path_val.is_string() {
            crate::js_to_rust_string(cx, path_val)
        } else {
            ":memory:".to_string()
        }
    } else {
        ":memory:".to_string()
    };

    match SqliteDatabase::new(&path) {
        Ok(db) => {
            let db_ptr = Box::into_raw(Box::new(db)) as *const ::std::os::raw::c_void;
            let val = PrivateValue(db_ptr);
            JS_SetReservedSlot(this, SLOT_DB, &val);
            args.rval().set(ObjectValue(this));
            true
        }
        Err(e) => {
            let msg = ZBox::from_bytes(e.as_bytes());
            JS_ReportErrorUTF8(cx, c"%s".as_ptr(), msg.as_ptr());
            false
        }
    }
}

// ── Database.exec(sql) ──

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn database_exec(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let thisv = args.thisv();

    let db_ptr = match get_db_ptr(cx, thisv) {
        Some(p) => p,
        None => {
            let msg = ZBox::from_bytes("Database.exec: invalid Database object".as_bytes());
            JS_ReportErrorUTF8(cx, c"%s".as_ptr(), msg.as_ptr());
            return false;
        }
    };

    let sql = if argc >= 1 {
        crate::js_to_rust_string(cx, *args.get(0).ptr)
    } else {
        String::new()
    };

    let db = &*db_ptr;
    match db.exec(&sql) {
        Ok(()) => {
            args.rval().set(UndefinedValue());
            true
        }
        Err(e) => {
            let msg = ZBox::from_bytes(e.as_bytes());
            JS_ReportErrorUTF8(cx, c"%s".as_ptr(), msg.as_ptr());
            false
        }
    }
}

// ── Database.run(sql) → { changes, lastInsertRowid } ──

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn database_run(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let thisv = args.thisv();

    let db_ptr = match get_db_ptr(cx, thisv) {
        Some(p) => p,
        None => {
            let msg = ZBox::from_bytes("Database.run: invalid Database object".as_bytes());
            JS_ReportErrorUTF8(cx, c"%s".as_ptr(), msg.as_ptr());
            return false;
        }
    };

    let sql = if argc >= 1 {
        crate::js_to_rust_string(cx, *args.get(0).ptr)
    } else {
        String::new()
    };

    let db = &*db_ptr;
    // BCE (v-surface P0-2): db.run(sql, ...params) previously funneled into
    // execute_batch and dropped the parameters. With parameters present,
    // prepare + bind + execute the single statement (bun:sqlite contract);
    // the bare single-argument form keeps execute_batch semantics (multi-
    // statement scripts).
    if argc > 1 {
        let borrow = db.conn.borrow();
        let conn = match borrow.as_ref() {
            Some(c) => c,
            None => {
                let msg = ZBox::from_bytes("Database is closed".as_bytes());
                JS_ReportErrorUTF8(cx, c"%s".as_ptr(), msg.as_ptr());
                return false;
            }
        };
        let mut stmt = match conn.prepare(&sql) {
            Ok(s) => s,
            Err(e) => {
                let msg = ZBox::from_vec(e.to_string().into_bytes());
                JS_ReportErrorUTF8(cx, c"%s".as_ptr(), msg.as_ptr());
                return false;
            }
        };
        // bind_stmt_args is unsafe due to JS API use.
        if let Err(e) = unsafe { bind_stmt_args(cx, &mut stmt, &args, 1, argc) } {
            let msg = ZBox::from_vec(e.into_bytes());
            JS_ReportErrorUTF8(cx, c"%s".as_ptr(), msg.as_ptr());
            return false;
        }
        match stmt.raw_execute() {
            Ok(changed) => {
                let result = RunResult {
                    changes: changed as u64,
                    last_insert_rowid: conn.last_insert_rowid(),
                };
                report_run_result(cx, &args, &result);
                true
            }
            Err(e) => {
                let msg = ZBox::from_vec(e.to_string().into_bytes());
                JS_ReportErrorUTF8(cx, c"%s".as_ptr(), msg.as_ptr());
                false
            }
        }
    } else {
        match db.run(&sql) {
            Ok(result) => {
                report_run_result(cx, &args, &result);
                true
            }
            Err(e) => {
                let msg = ZBox::from_bytes(e.as_bytes());
                JS_ReportErrorUTF8(cx, c"%s".as_ptr(), msg.as_ptr());
                false
            }
        }
    }
}

/// Build the `{ changes, lastInsertRowid }` result object for run().
#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn report_run_result(cx: *mut JSContext, args: &CallArgs, result: &RunResult) {
    let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;
    rooted!(&in(cx_ref) let obj = w2::JS_NewPlainObject(cx_ref));
    if obj.get().is_null() {
        args.rval().set(NullValue());
        return;
    }
    rooted!(&in(cx_ref) let changes_val = DoubleValue(result.changes as f64));
    JS_DefineProperty(
        cx,
        obj.handle().into(),
        c"changes".as_ptr(),
        changes_val.handle().into(),
        (JSPROP_ENUMERATE | JSPROP_PERMANENT) as u32,
    );
    rooted!(&in(cx_ref) let rowid_val = DoubleValue(result.last_insert_rowid as f64));
    JS_DefineProperty(
        cx,
        obj.handle().into(),
        c"lastInsertRowid".as_ptr(),
        rowid_val.handle().into(),
        (JSPROP_ENUMERATE | JSPROP_PERMANENT) as u32,
    );
    args.rval().set(ObjectValue(obj.get()));
}

// ── Database.close() ──

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn database_close(cx: *mut JSContext, _argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, _argc);
    let thisv = args.thisv();

    let db_ptr = match get_db_ptr(cx, thisv) {
        Some(p) => p,
        None => {
            let msg = ZBox::from_bytes("Database.close: invalid Database object".as_bytes());
            JS_ReportErrorUTF8(cx, c"%s".as_ptr(), msg.as_ptr());
            return false;
        }
    };

    let db = &*db_ptr;
    match db.close() {
        Ok(()) => {
            args.rval().set(UndefinedValue());
            true
        }
        Err(e) => {
            let msg = ZBox::from_bytes(e.as_bytes());
            JS_ReportErrorUTF8(cx, c"%s".as_ptr(), msg.as_ptr());
            false
        }
    }
}

// ── Database.query(sql, params?) → row[] ──

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn database_query(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let thisv = args.thisv();

    let db_ptr = match get_db_ptr(cx, thisv) {
        Some(p) => p,
        None => {
            let msg = ZBox::from_bytes("Database.query: invalid Database object".as_bytes());
            JS_ReportErrorUTF8(cx, c"%s".as_ptr(), msg.as_ptr());
            return false;
        }
    };

    let sql = if argc >= 1 {
        crate::js_to_rust_string(cx, *args.get(0).ptr)
    } else {
        String::new()
    };

    let db = &*db_ptr;
    let conn = match db.conn.borrow() {
        c if c.is_some() => c,
        _ => {
            let msg = ZBox::from_bytes("Database is closed".as_bytes());
            JS_ReportErrorUTF8(cx, c"%s".as_ptr(), msg.as_ptr());
            return false;
        }
    };
    let conn = conn.as_ref().unwrap();

    match conn.prepare(&sql) {
        Ok(mut stmt) => {
            let col_count = stmt.column_count();
            let col_names: Vec<String> = (0..col_count)
                .map(|i| stmt.column_name(i).unwrap_or("unknown").to_string())
                .collect();

            let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
            let cx_ref = &mut wrapped_cx;
            rooted!(&in(cx_ref) let result_arr = w2::NewArrayObject1(cx_ref, 0));
            if result_arr.get().is_null() {
                args.rval().set(NullValue());
                return true;
            }

            // BCE (v-surface P0-2): forward db.query(sql, ...params) args.
            if let Err(e) = bind_stmt_args(cx, &mut stmt, &args, 1, argc) {
                let msg = ZBox::from_vec(e.into_bytes());
                JS_ReportErrorUTF8(cx, c"%s".as_ptr(), msg.as_ptr());
                return false;
            }

            let mut row_idx: u32 = 0;
            let mut rows_iter = stmt.raw_query();
            loop {
                match rows_iter.next() {
                    Ok(Some(row)) => {
                        let row_obj = row_to_js_object(cx, &row, &col_names, cx_ref);
                        if row_obj.is_null() {
                            break;
                        }
                        rooted!(&in(cx_ref) let row_val = ObjectValue(row_obj));
                        w2::JS_SetElement(
                            cx_ref,
                            result_arr.handle().into(),
                            row_idx,
                            row_val.handle().into(),
                        );
                        row_idx += 1;
                    }
                    Ok(None) => break,
                    Err(e) => {
                        let msg = ZBox::from_vec(e.to_string().into_bytes());
                        JS_ReportErrorUTF8(cx, c"%s".as_ptr(), msg.as_ptr());
                        return false;
                    }
                }
            }

            args.rval().set(ObjectValue(result_arr.get()));
            true
        }
        Err(e) => {
            let msg = ZBox::from_vec(e.to_string().into_bytes());
            JS_ReportErrorUTF8(cx, c"%s".as_ptr(), msg.as_ptr());
            false
        }
    }
}

// ── Database.prepare(sql) → Statement ──

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn database_prepare(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let thisv = args.thisv();

    let db_ptr = match get_db_ptr(cx, thisv) {
        Some(p) => p,
        None => {
            let msg = ZBox::from_bytes("Database.prepare: invalid Database object".as_bytes());
            JS_ReportErrorUTF8(cx, c"%s".as_ptr(), msg.as_ptr());
            return false;
        }
    };

    let sql = if argc >= 1 {
        crate::js_to_rust_string(cx, *args.get(0).ptr)
    } else {
        let msg = ZBox::from_bytes("prepare: SQL argument required".as_bytes());
        JS_ReportErrorUTF8(cx, c"%s".as_ptr(), msg.as_ptr());
        return false;
    };

    let db = &*db_ptr;
    let conn = match db.conn.borrow() {
        c if c.is_some() => c,
        _ => {
            let msg = ZBox::from_bytes("Database is closed".as_bytes());
            JS_ReportErrorUTF8(cx, c"%s".as_ptr(), msg.as_ptr());
            return false;
        }
    };
    let conn = conn.as_ref().unwrap();

    match conn.prepare(&sql) {
        Ok(stmt) => {
            // Transmute lifetime: the Statement borrows Connection, but we
            // manage the lifetime manually — the DB must outlive the Statement.
            let stmt_static: rusqlite::Statement<'static> =
                ::std::mem::transmute::<_, rusqlite::Statement<'static>>(stmt);
            let sqlite_stmt = SqliteStatement {
                stmt: RefCell::new(Some(stmt_static)),
                db_ptr,
            };
            let stmt_ptr = Box::into_raw(Box::new(sqlite_stmt)) as *const ::std::os::raw::c_void;

            let obj = JS_NewObject(cx, &STATEMENT_CLASS);
            if obj.is_null() {
                args.rval().set(NullValue());
                return true;
            }
            let val = PrivateValue(stmt_ptr);
            JS_SetReservedSlot(obj, SLOT_STMT, &val);

            // Root the object and define instance methods using rooted handle
            let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
            let cx_ref = &mut wrapped_cx;
            rooted!(&in(cx_ref) let obj_r = obj);
            JS_DefineFunction(
                cx,
                obj_r.handle().into(),
                c"run".as_ptr(),
                Some(statement_run),
                0,
                JSPROP_ENUMERATE as u32,
            );
            JS_DefineFunction(
                cx,
                obj_r.handle().into(),
                c"get".as_ptr(),
                Some(statement_get),
                0,
                JSPROP_ENUMERATE as u32,
            );
            JS_DefineFunction(
                cx,
                obj_r.handle().into(),
                c"all".as_ptr(),
                Some(statement_all),
                0,
                JSPROP_ENUMERATE as u32,
            );
            JS_DefineFunction(
                cx,
                obj_r.handle().into(),
                c"iterate".as_ptr(),
                Some(statement_iterate),
                0,
                JSPROP_ENUMERATE as u32,
            );

            args.rval().set(ObjectValue(obj_r.get()));
            true
        }
        Err(e) => {
            let msg = ZBox::from_vec(e.to_string().into_bytes());
            JS_ReportErrorUTF8(cx, c"%s".as_ptr(), msg.as_ptr());
            false
        }
    }
}

// ── Statement.run() → { changes, lastInsertRowid } ──

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn statement_run(cx: *mut JSContext, _argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, _argc);
    let thisv = args.thisv();

    let stmt_ptr = match get_stmt_ptr(cx, thisv) {
        Some(p) => p,
        None => {
            let msg = ZBox::from_bytes("Statement.run: invalid Statement object".as_bytes());
            JS_ReportErrorUTF8(cx, c"%s".as_ptr(), msg.as_ptr());
            return false;
        }
    };

    let sqlite_stmt = &*stmt_ptr;
    let db = &*sqlite_stmt.db_ptr;
    let borrow = db.conn.borrow();
    let conn = match borrow.as_ref() {
        Some(c) => c,
        None => {
            let msg = ZBox::from_bytes("Database is closed".as_bytes());
            JS_ReportErrorUTF8(cx, c"%s".as_ptr(), msg.as_ptr());
            return false;
        }
    };

    let mut stmt_borrow = sqlite_stmt.stmt.borrow_mut();
    let stmt = match stmt_borrow.as_mut() {
        Some(s) => s,
        None => {
            let msg = ZBox::from_bytes("Statement is finalized".as_bytes());
            JS_ReportErrorUTF8(cx, c"%s".as_ptr(), msg.as_ptr());
            return false;
        }
    };

    // BCE (v-surface P0-2): forward the JS arguments to the bound statement.
    if let Err(e) = bind_stmt_args(cx, stmt, &args, 0, _argc) {
        let msg = ZBox::from_vec(e.into_bytes());
        JS_ReportErrorUTF8(cx, c"%s".as_ptr(), msg.as_ptr());
        return false;
    }

    // raw_execute returns the rows changed by THIS statement — do NOT delta
    // conn.changes() (that counter holds the MOST RECENT statement's count,
    // so before/after subtraction zeroes out for consecutive statements).
    match stmt.raw_execute() {
        Ok(changed) => {
            let changes = changed as u64;
            let last_insert_rowid = conn.last_insert_rowid();

            let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
            let cx_ref = &mut wrapped_cx;
            rooted!(&in(cx_ref) let obj = w2::JS_NewPlainObject(cx_ref));
            if obj.get().is_null() {
                args.rval().set(NullValue());
                return true;
            }
            rooted!(&in(cx_ref) let changes_val = DoubleValue(changes as f64));
            JS_DefineProperty(
                cx,
                obj.handle().into(),
                c"changes".as_ptr(),
                changes_val.handle().into(),
                (JSPROP_ENUMERATE | JSPROP_PERMANENT) as u32,
            );
            rooted!(&in(cx_ref) let rowid_val = DoubleValue(last_insert_rowid as f64));
            JS_DefineProperty(
                cx,
                obj.handle().into(),
                c"lastInsertRowid".as_ptr(),
                rowid_val.handle().into(),
                (JSPROP_ENUMERATE | JSPROP_PERMANENT) as u32,
            );
            args.rval().set(ObjectValue(obj.get()));
            true
        }
        Err(e) => {
            let msg = ZBox::from_vec(e.to_string().into_bytes());
            JS_ReportErrorUTF8(cx, c"%s".as_ptr(), msg.as_ptr());
            false
        }
    }
}

// ── Statement.get() → row | null ──

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn statement_get(cx: *mut JSContext, _argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, _argc);
    let thisv = args.thisv();

    let stmt_ptr = match get_stmt_ptr(cx, thisv) {
        Some(p) => p,
        None => {
            let msg = ZBox::from_bytes("Statement.get: invalid Statement object".as_bytes());
            JS_ReportErrorUTF8(cx, c"%s".as_ptr(), msg.as_ptr());
            return false;
        }
    };

    let sqlite_stmt = &*stmt_ptr;
    let mut stmt_borrow = sqlite_stmt.stmt.borrow_mut();
    let stmt = match stmt_borrow.as_mut() {
        Some(s) => s,
        None => {
            let msg = ZBox::from_bytes("Statement is finalized".as_bytes());
            JS_ReportErrorUTF8(cx, c"%s".as_ptr(), msg.as_ptr());
            return false;
        }
    };

    let col_count = stmt.column_count();
    let col_names: Vec<String> = (0..col_count)
        .map(|i| stmt.column_name(i).unwrap_or("unknown").to_string())
        .collect();

    // BCE (v-surface P0-2): forward the JS arguments to the bound statement.
    if let Err(e) = bind_stmt_args(cx, stmt, &args, 0, _argc) {
        let msg = ZBox::from_vec(e.into_bytes());
        JS_ReportErrorUTF8(cx, c"%s".as_ptr(), msg.as_ptr());
        return false;
    }

    let mut rows = stmt.raw_query();

    match rows.next() {
        Ok(Some(row)) => {
            let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
            let cx_ref = &mut wrapped_cx;
            let row_obj = row_to_js_object(cx, &row, &col_names, cx_ref);
            if row_obj.is_null() {
                args.rval().set(UndefinedValue());
            } else {
                args.rval().set(ObjectValue(row_obj));
            }
            true
        }
        Ok(None) => {
            args.rval().set(NullValue());
            true
        }
        Err(e) => {
            let msg = ZBox::from_vec(e.to_string().into_bytes());
            JS_ReportErrorUTF8(cx, c"%s".as_ptr(), msg.as_ptr());
            false
        }
    }
}

// ── Statement.all() → row[] ──

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn statement_all(cx: *mut JSContext, _argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, _argc);
    let thisv = args.thisv();

    let stmt_ptr = match get_stmt_ptr(cx, thisv) {
        Some(p) => p,
        None => {
            let msg = ZBox::from_bytes("Statement.all: invalid Statement object".as_bytes());
            JS_ReportErrorUTF8(cx, c"%s".as_ptr(), msg.as_ptr());
            return false;
        }
    };

    let sqlite_stmt = &*stmt_ptr;
    let mut stmt_borrow = sqlite_stmt.stmt.borrow_mut();
    let stmt = match stmt_borrow.as_mut() {
        Some(s) => s,
        None => {
            let msg = ZBox::from_bytes("Statement is finalized".as_bytes());
            JS_ReportErrorUTF8(cx, c"%s".as_ptr(), msg.as_ptr());
            return false;
        }
    };

    let col_count = stmt.column_count();
    let col_names: Vec<String> = (0..col_count)
        .map(|i| stmt.column_name(i).unwrap_or("unknown").to_string())
        .collect();

    let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;
    rooted!(&in(cx_ref) let result_arr = w2::NewArrayObject1(cx_ref, 0));
    if result_arr.get().is_null() {
        args.rval().set(NullValue());
        return true;
    }

    // BCE (v-surface P0-2): forward the JS arguments to the bound statement.
    if let Err(e) = bind_stmt_args(cx, stmt, &args, 0, _argc) {
        let msg = ZBox::from_vec(e.into_bytes());
        JS_ReportErrorUTF8(cx, c"%s".as_ptr(), msg.as_ptr());
        return false;
    }

    let mut rows = stmt.raw_query();

    let mut row_idx: u32 = 0;
    loop {
        match rows.next() {
            Ok(Some(row)) => {
                let row_obj = row_to_js_object(cx, &row, &col_names, cx_ref);
                if row_obj.is_null() {
                    break;
                }
                rooted!(&in(cx_ref) let row_val = ObjectValue(row_obj));
                w2::JS_SetElement(
                    cx_ref,
                    result_arr.handle().into(),
                    row_idx,
                    row_val.handle().into(),
                );
                row_idx += 1;
            }
            Ok(None) => break,
            Err(e) => {
                let msg = ZBox::from_vec(e.to_string().into_bytes());
                JS_ReportErrorUTF8(cx, c"%s".as_ptr(), msg.as_ptr());
                return false;
            }
        }
    }

    args.rval().set(ObjectValue(result_arr.get()));
    true
}

// ── inTransaction getter (defined on Database.prototype) ──

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn database_in_transaction(
    cx: *mut JSContext,
    argc: u32,
    vp: *mut JSVal,
) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let thisv = args.thisv();

    let db_ptr = match get_db_ptr(cx, thisv) {
        Some(p) => p,
        None => {
            args.rval().set(BooleanValue(false));
            return true;
        }
    };

    let db = &*db_ptr;
    args.rval().set(BooleanValue(db.in_transaction()));
    true
}

// ── Database.transaction(fn) → wrapped transaction function ────────────────
//
// Bun semantics: `const tx = db.transaction(fn); tx(args...)` runs fn inside
// BEGIN/COMMIT (ROLLBACK on throw, exception propagates, return value
// forwarded). Nested calls (tx invoked while another transaction is open on
// the same connection) map onto SAVEPOINT/RELEASE. The returned function
// carries `.deferred` / `.immediate` / `.exclusive` variants selecting the
// BEGIN mode (the plain call is DEFERRED, matching SQLite's default).
//
// Trampoline wiring: the wrapper is a native JSFunction with hidden
// properties — `_dbPtr` (PrivateValue → SqliteDatabase), `_trxFn` (the user
// function), `_beginMode` ("DEFERRED"|"IMMEDIATE"|"EXCLUSIVE").

/// Read one of the trampoline's hidden properties off its callee function.
#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn trampoline_prop(cx: *mut JSContext, fn_obj: *mut JSObject, name: &str) -> JSVal {
    let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;
    rooted!(&in(cx_ref) let fn_root = fn_obj);
    js_get_prop_val(cx, fn_root.get(), name)
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn database_transaction(
    cx: *mut JSContext,
    argc: u32,
    vp: *mut JSVal,
) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let thisv = args.thisv();

    let db_ptr = match get_db_ptr(cx, thisv) {
        Some(p) => p,
        None => {
            let msg = ZBox::from_bytes("Database.transaction: invalid Database object".as_bytes());
            JS_ReportErrorUTF8(cx, c"%s".as_ptr(), msg.as_ptr());
            return false;
        }
    };
    if argc < 1 || !(*args.get(0).ptr).is_object() {
        let msg = ZBox::from_bytes("transaction requires a function argument".as_bytes());
        JS_ReportErrorUTF8(cx, c"%s".as_ptr(), msg.as_ptr());
        return false;
    }
    let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;
    rooted!(&in(cx_ref) let fn_arg = (*args.get(0).ptr).to_object());
    if !JS_ObjectIsFunction(fn_arg.get()) {
        let msg = ZBox::from_bytes("transaction requires a function argument".as_bytes());
        JS_ReportErrorUTF8(cx, c"%s".as_ptr(), msg.as_ptr());
        return false;
    }

    // Build the wrapper + its mode variants. Each variant is the same native
    // trampoline; the mode rides along as a hidden property.
    let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;
    let build_variant = |cx: *mut JSContext,
                         mode: Option<&'static str>|
     -> Option<*mut JSObject> {
        let f = JS_NewFunction(cx, Some(transaction_trampoline), 0, 0, c"transaction".as_ptr());
        if f.is_null() {
            return None;
        }
        let fobj = JS_GetFunctionObject(f);
        if fobj.is_null() {
            return None;
        }
        let mut wc = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
        let cr = &mut wc;
        rooted!(&in(cr) let fo = fobj);
        let fo_h = fo.handle().into();
        rooted!(&in(cr) let db_v = PrivateValue(db_ptr as *const ::std::os::raw::c_void));
        if !JS_DefineProperty(
            cx,
            fo_h,
            c"_dbPtr".as_ptr(),
            db_v.handle().into(),
            0,
        ) {
            return None;
        }
        rooted!(&in(cr) let fn_v = ObjectValue(fn_arg.get()));
        if !JS_DefineProperty(cx, fo_h, c"_trxFn".as_ptr(), fn_v.handle().into(), 0) {
            return None;
        }
        if let Some(m) = mode {
            let c_m = ZBox::from_bytes(m.as_bytes());
            let m_js = JS_NewStringCopyZ(cx, c_m.as_ptr());
            if m_js.is_null() {
                return None;
            }
            rooted!(&in(cr) let mv = mozjs::jsval::StringValue(&*m_js));
            if !JS_DefineProperty(cx, fo_h, c"_beginMode".as_ptr(), mv.handle().into(), 0) {
                return None;
            }
        }
        Some(fo.get())
    };

    let Some(wrapper) = build_variant(cx, None) else {
        let msg = ZBox::from_bytes("transaction: failed to create wrapper".as_bytes());
        JS_ReportErrorUTF8(cx, c"%s".as_ptr(), msg.as_ptr());
        return false;
    };
    rooted!(&in(cx_ref) let wrapper_r = wrapper);
    let wrapper_h = wrapper_r.handle().into();

    for (prop, mode) in [
        ("deferred", "DEFERRED"),
        ("immediate", "IMMEDIATE"),
        ("exclusive", "EXCLUSIVE"),
    ] {
        if let Some(variant) = build_variant(cx, Some(mode)) {
            rooted!(&in(cx_ref) let vv = ObjectValue(variant));
            let c_prop = ZBox::from_bytes(prop.as_bytes());
            JS_DefineProperty(
                cx,
                wrapper_h,
                c_prop.as_ptr(),
                vv.handle().into(),
                JSPROP_ENUMERATE as u32,
            );
        }
    }

    args.rval().set(ObjectValue(wrapper_r.get()));
    true
}

/// The transaction wrapper body: BEGIN/SAVEPOINT → call user fn with the
/// caller's args → COMMIT/RELEASE (ROLLBACK on error or JS exception).
#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn transaction_trampoline(
    cx: *mut JSContext,
    argc: u32,
    vp: *mut JSVal,
) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let callee = args.calleev();
    if !callee.is_object() {
        let msg = ZBox::from_bytes("transaction: invalid callee".as_bytes());
        JS_ReportErrorUTF8(cx, c"%s".as_ptr(), msg.as_ptr());
        return false;
    }
    let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;
    rooted!(&in(cx_ref) let callee_obj = callee.to_object());

    let db_ptr_val = trampoline_prop(cx, callee_obj.get(), "_dbPtr");
    if !val_is_private(&db_ptr_val) {
        let msg = ZBox::from_bytes("transaction: invalid Database object".as_bytes());
        JS_ReportErrorUTF8(cx, c"%s".as_ptr(), msg.as_ptr());
        return false;
    }
    let db_ptr = db_ptr_val.to_private() as *mut SqliteDatabase;
    let db = &*db_ptr;

    let trxfn_val = trampoline_prop(cx, callee_obj.get(), "_trxFn");
    if !trxfn_val.is_object() {
        let msg = ZBox::from_bytes("transaction: wrapped function is gone".as_bytes());
        JS_ReportErrorUTF8(cx, c"%s".as_ptr(), msg.as_ptr());
        return false;
    }
    let mode_val = trampoline_prop(cx, callee_obj.get(), "_beginMode");
    let mode = if mode_val.is_string() {
        crate::js_to_rust_string(cx, mode_val)
    } else {
        "DEFERRED".to_string()
    };
    if !matches!(mode.as_str(), "DEFERRED" | "IMMEDIATE" | "EXCLUSIVE") {
        let msg = ZBox::from_bytes("transaction: unknown BEGIN mode".as_bytes());
        JS_ReportErrorUTF8(cx, c"%s".as_ptr(), msg.as_ptr());
        return false;
    }

    // Open the transaction scope. Borrows are scoped: none may straddle the
    // JS call (the user fn touches the same connection).
    let outer = !db.in_transaction();
    let savepoint = if outer {
        None
    } else {
        let n = SAVEPOINT_COUNTER.fetch_add(1, ::std::sync::atomic::Ordering::Relaxed);
        Some(format!("bao_sp_{}", n))
    };
    let begin_sql = match (&savepoint, mode.as_str()) {
        (Some(sp), _) => format!("SAVEPOINT {}", sp),
        (None, "DEFERRED") => "BEGIN".to_string(),
        (None, m) => format!("BEGIN {}", m),
    };
    {
        let borrow = db.conn.borrow();
        let Some(conn) = borrow.as_ref() else {
            let msg = ZBox::from_bytes("Database is closed".as_bytes());
            JS_ReportErrorUTF8(cx, c"%s".as_ptr(), msg.as_ptr());
            return false;
        };
        if let Err(e) = conn.execute_batch(&begin_sql) {
            let msg = ZBox::from_vec(e.to_string().into_bytes());
            JS_ReportErrorUTF8(cx, c"%s".as_ptr(), msg.as_ptr());
            return false;
        }
    }

    // Call the user function with the wrapper's incoming args.
    rooted!(&in(cx_ref) let fn_root = trxfn_val.to_object());
    rooted!(&in(cx_ref) let fn_val = ObjectValue(fn_root.get()));
    let mut fwd_args: Vec<JSVal> = (0..argc).map(|i| *args.get(i).ptr).collect();
    let call_args = HandleValueArray {
        length_: fwd_args.len(),
        elements_: fwd_args.as_mut_ptr(),
    };
    rooted!(&in(cx_ref) let undef_this = ::std::ptr::null_mut::<JSObject>());
    let mut rval = UndefinedValue();
    let called = JS_CallFunctionValue(
        cx,
        undef_this.handle().into(),
        fn_val.handle().into(),
        &call_args,
        MutableHandle::<Value> {
            _phantom_0: ::std::marker::PhantomData,
            ptr: &mut rval,
        },
    );

    if !called {
        // JS exception pending → roll the scope back, propagate the exception.
        let rollback_sql = match &savepoint {
            Some(sp) => format!("ROLLBACK TO {}; RELEASE {}", sp, sp),
            None => "ROLLBACK".to_string(),
        };
        let borrow = db.conn.borrow();
        if let Some(conn) = borrow.as_ref() {
            let _ = conn.execute_batch(&rollback_sql);
        }
        return false;
    }

    let commit_sql = match &savepoint {
        Some(sp) => format!("RELEASE {}", sp),
        None => "COMMIT".to_string(),
    };
    {
        let borrow = db.conn.borrow();
        let Some(conn) = borrow.as_ref() else {
            let msg = ZBox::from_bytes("Database is closed".as_bytes());
            JS_ReportErrorUTF8(cx, c"%s".as_ptr(), msg.as_ptr());
            return false;
        };
        if let Err(e) = conn.execute_batch(&commit_sql) {
            let msg = ZBox::from_vec(e.to_string().into_bytes());
            JS_ReportErrorUTF8(cx, c"%s".as_ptr(), msg.as_ptr());
            return false;
        }
    }

    rooted!(&in(cx_ref) let rval_root = rval);
    args.rval().set(*rval_root.handle());
    true
}

// ── Database.serialize() → Buffer (real VACUUM INTO snapshot) ──────────────

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn database_serialize(
    cx: *mut JSContext,
    _argc: u32,
    vp: *mut JSVal,
) -> bool {
    let args = CallArgs::from_vp(vp, _argc);
    let thisv = args.thisv();

    let db_ptr = match get_db_ptr(cx, thisv) {
        Some(p) => p,
        None => {
            let msg = ZBox::from_bytes("Database.serialize: invalid Database object".as_bytes());
            JS_ReportErrorUTF8(cx, c"%s".as_ptr(), msg.as_ptr());
            return false;
        }
    };

    let db = &*db_ptr;
    match db.serialize_bytes() {
        Ok(bytes) => {
            let buf = crate::globals::create_buffer_object(cx, &bytes);
            if buf.is_null() {
                args.rval().set(NullValue());
            } else {
                args.rval().set(ObjectValue(buf));
            }
            true
        }
        Err(e) => {
            let msg = ZBox::from_vec(e.into_bytes());
            JS_ReportErrorUTF8(cx, c"%s".as_ptr(), msg.as_ptr());
            false
        }
    }
}

// ── Database.backup(path) → Promise<string> (consistent snapshot file) ─────

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn database_backup(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let thisv = args.thisv();

    let db_ptr = match get_db_ptr(cx, thisv) {
        Some(p) => p,
        None => {
            let msg = ZBox::from_bytes("Database.backup: invalid Database object".as_bytes());
            JS_ReportErrorUTF8(cx, c"%s".as_ptr(), msg.as_ptr());
            return false;
        }
    };
    if argc < 1 || !(*args.get(0).ptr).is_string() {
        let msg = ZBox::from_bytes("backup requires a destination path string".as_bytes());
        JS_ReportErrorUTF8(cx, c"%s".as_ptr(), msg.as_ptr());
        return false;
    }
    let path = crate::js_to_rust_string(cx, *args.get(0).ptr);

    // Bun contract: Database.backup(path) → Promise<string>, resolving with
    // the destination path (the VACUUM INTO snapshot was written there).
    // Runtime failures reject the promise (callers await .then/.catch) —
    // argument validation above stays a synchronous throw.
    let mut wrapped = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped;
    rooted!(&in(cx_ref) let promise = JS::NewPromiseObject(cx, HandleObject::null()));
    if promise.get().is_null() {
        JS_ClearPendingException(cx);
        args.rval().set(UndefinedValue());
        return true;
    }

    let db = &*db_ptr;
    let settled = match db.backup_to_path(&path) {
        Ok(()) => {
            let c_path = ZBox::from_bytes(path.as_bytes());
            let js = JS_NewStringCopyZ(cx, c_path.as_ptr());
            if js.is_null() {
                rooted!(&in(cx_ref) let uv = UndefinedValue());
                JS::RejectPromise(cx, promise.handle().into(), uv.handle().into())
            } else {
                rooted!(&in(cx_ref) let pv = StringValue(&*js));
                JS::ResolvePromise(cx, promise.handle().into(), pv.handle().into())
            }
        }
        Err(e) => {
            // Build the reject reason as an Error value without leaving it
            // pending (harvest pattern from bun_api::make_coded_error_value).
            let msg = ZBox::from_vec(e.into_bytes());
            JS_ReportErrorUTF8(cx, c"%s".as_ptr(), msg.as_ptr());
            let mut exn = UndefinedValue();
            let exn_h = MutableHandle::<Value> {
                _phantom_0: ::std::marker::PhantomData,
                ptr: &mut exn,
            };
            if !JS_GetPendingException(cx, exn_h) || !exn.is_object() {
                JS_ClearPendingException(cx);
                rooted!(&in(cx_ref) let uv = UndefinedValue());
                JS::RejectPromise(cx, promise.handle().into(), uv.handle().into())
            } else {
                JS_ClearPendingException(cx);
                rooted!(&in(cx_ref) let ev = exn);
                JS::RejectPromise(cx, promise.handle().into(), ev.handle().into())
            }
        }
    };
    let _ = settled;
    args.rval().set(ObjectValue(promise.get()));
    true
}

// ── Statement.iterate(...params?) → row iterator ───────────────────────────
//
// Bun contract: `for (const row of stmt.iterate(params...))` — each call
// starts a FRESH iteration (raw_query resets the statement), next() returns
// the row object and undefined at exhaustion, and the object is itself its
// own Symbol.iterator.

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn statement_iterate(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let thisv = args.thisv();

    let stmt_ptr = match get_stmt_ptr(cx, thisv) {
        Some(p) => p,
        None => {
            let msg = ZBox::from_bytes("Statement.iterate: invalid Statement object".as_bytes());
            JS_ReportErrorUTF8(cx, c"%s".as_ptr(), msg.as_ptr());
            return false;
        }
    };

    let sqlite_stmt = &*stmt_ptr;
    let col_names;
    let rows;
    {
        let mut stmt_borrow = sqlite_stmt.stmt.borrow_mut();
        let stmt = match stmt_borrow.as_mut() {
            Some(s) => s,
            None => {
                let msg = ZBox::from_bytes("Statement is finalized".as_bytes());
                JS_ReportErrorUTF8(cx, c"%s".as_ptr(), msg.as_ptr());
                return false;
            }
        };

        col_names = (0..stmt.column_count())
            .map(|i| stmt.column_name(i).unwrap_or("unknown").to_string())
            .collect::<Vec<_>>();

        if let Err(e) = bind_stmt_args(cx, stmt, &args, 0, argc) {
            let msg = ZBox::from_vec(e.into_bytes());
            JS_ReportErrorUTF8(cx, c"%s".as_ptr(), msg.as_ptr());
            return false;
        }

        let r = stmt.raw_query();
        // SAFETY: same lifetime discipline as SqliteStatement — the parent
        // statement (pinned via stmt_ptr) outlives this cursor; the RefCell
        // borrows are dropped before any JS re-enters.
        rows = ::std::mem::transmute::<_, rusqlite::Rows<'static>>(r);
    }

    let iter = Box::new(SqliteIterator {
        rows: RefCell::new(Some(rows)),
        col_names,
    });
    let iter_ptr = Box::into_raw(iter) as *const ::std::os::raw::c_void;

    let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;
    rooted!(&in(cx_ref) let obj = w2::JS_NewPlainObject(cx_ref));
    if obj.get().is_null() {
        drop(Box::from_raw(iter_ptr as *mut SqliteIterator));
        args.rval().set(NullValue());
        return true;
    }
    let obj_h = obj.handle().into();
    rooted!(&in(cx_ref) let iter_v = PrivateValue(iter_ptr));
    JS_DefineProperty(cx, obj_h, c"_iterPtr".as_ptr(), iter_v.handle().into(), 0);
    JS_DefineFunction(
        cx,
        obj_h,
        c"next".as_ptr(),
        Some(iterator_next),
        0,
        JSPROP_ENUMERATE as u32,
    );
    // Symbol.iterator → this (for..of support).
    let sym_key = mozjs_sys::jsapi::JS::GetWellKnownSymbolKey(
        cx,
        mozjs_sys::jsapi::JS::SymbolCode::iterator,
    );
    let fn_js = JS_NewFunction(cx, Some(iterator_self), 0, 0, c"[Symbol.iterator]".as_ptr());
    if !fn_js.is_null() {
        let fn_obj = JS_GetFunctionObject(fn_js);
        if !fn_obj.is_null() {
            rooted!(&in(cx_ref) let fv = ObjectValue(fn_obj));
            JS_DefinePropertyById2(
                cx,
                obj_h,
                Handle::from_marked_location(&sym_key),
                fv.handle().into(),
                JSPROP_ENUMERATE as u32,
            );
        }
    }

    args.rval().set(ObjectValue(obj.get()));
    true
}

/// Symbol.iterator body — returns `this`.
#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn iterator_self(cx: *mut JSContext, _argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, _argc);
    let this = *args.thisv();
    args.rval().set(this);
    true
}

/// Iterator next() — the JS iterator protocol result: `{value: row, done:
/// false}` per row, `{value: undefined, done: true}` at exhaustion (for..of
/// contract).
#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn iterator_next(cx: *mut JSContext, _argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, _argc);
    let thisv = args.thisv();
    if !thisv.is_object() {
        let msg = ZBox::from_bytes("iterator.next: invalid iterator object".as_bytes());
        JS_ReportErrorUTF8(cx, c"%s".as_ptr(), msg.as_ptr());
        return false;
    }
    let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;
    rooted!(&in(cx_ref) let this_obj = thisv.to_object());
    let mut iter_val = UndefinedValue();
    JS_GetProperty(
        cx,
        this_obj.handle().into(),
        c"_iterPtr".as_ptr(),
        MutableHandle::<Value> {
            _phantom_0: ::std::marker::PhantomData,
            ptr: &mut iter_val,
        },
    );
    if !val_is_private(&iter_val) {
        let msg = ZBox::from_bytes("iterator.next: invalid iterator object".as_bytes());
        JS_ReportErrorUTF8(cx, c"%s".as_ptr(), msg.as_ptr());
        return false;
    }
    let iter_ptr = iter_val.to_private() as *mut SqliteIterator;
    let iter = &*iter_ptr;

    // Row borrows Rows, so the row object is built INSIDE the borrow scope.
    enum StepOutcome {
        Row(*mut JSObject),
        Done,
        Err(String),
    }
    let outcome = {
        let mut rows_borrow = iter.rows.borrow_mut();
        match rows_borrow.as_mut() {
            Some(rows) => match rows.next() {
                Ok(Some(row)) => {
                    StepOutcome::Row(row_to_js_object(cx, &row, &iter.col_names, cx_ref))
                }
                Ok(None) => StepOutcome::Done,
                Err(e) => StepOutcome::Err(e.to_string()),
            },
            None => StepOutcome::Done,
        }
    };

    rooted!(&in(cx_ref) let result_obj = w2::JS_NewPlainObject(cx_ref));
    if result_obj.get().is_null() {
        args.rval().set(UndefinedValue());
        return true;
    }
    let rh = result_obj.handle().into();
    match outcome {
        StepOutcome::Row(row_obj) => {
            rooted!(&in(cx_ref) let dv = BooleanValue(false));
            JS_DefineProperty(cx, rh, c"done".as_ptr(), dv.handle().into(), JSPROP_ENUMERATE as u32);
            if !row_obj.is_null() {
                rooted!(&in(cx_ref) let rv = ObjectValue(row_obj));
                JS_DefineProperty(cx, rh, c"value".as_ptr(), rv.handle().into(), JSPROP_ENUMERATE as u32);
            }
            args.rval().set(ObjectValue(result_obj.get()));
            true
        }
        StepOutcome::Done => {
            rooted!(&in(cx_ref) let dv = BooleanValue(true));
            JS_DefineProperty(cx, rh, c"done".as_ptr(), dv.handle().into(), JSPROP_ENUMERATE as u32);
            rooted!(&in(cx_ref) let uv = UndefinedValue());
            JS_DefineProperty(cx, rh, c"value".as_ptr(), uv.handle().into(), JSPROP_ENUMERATE as u32);
            args.rval().set(ObjectValue(result_obj.get()));
            true
        }
        StepOutcome::Err(e) => {
            let msg = ZBox::from_vec(e.into_bytes());
            JS_ReportErrorUTF8(cx, c"%s".as_ptr(), msg.as_ptr());
            false
        }
    }
}

// ── Module installation ──

/// Register the Database constructor on the bun:sqlite module object.
pub fn register_database_constructor(
    cx: &mut mozjs::context::JSContext,
    module_obj: *mut JSObject,
) {
    unsafe {
        rooted!(&in(cx) let global = CurrentGlobalOrNull(cx.raw_cx()));
        if global.get().is_null() {
            return;
        }

        rooted!(&in(cx) let null_proto = ::std::ptr::null_mut::<JSObject>());
        let proto = w2::JS_InitClass(
            cx,
            global.handle(),
            &DATABASE_CLASS,
            null_proto.handle(),
            c"Database".as_ptr(),
            Some(database_constructor),
            1,
            ::std::ptr::null(),
            DATABASE_METHODS.as_ptr(),
            ::std::ptr::null(),
            ::std::ptr::null(),
        );

        if proto.is_null() {
            return;
        }

        // Add inTransaction getter on the prototype
        rooted!(&in(cx) let proto_h = proto);
        w2::JS_DefineProperty1(
            cx,
            proto_h.handle(),
            c"inTransaction".as_ptr(),
            Some(database_in_transaction),
            None,
            (JSPROP_ENUMERATE | JSPROP_READONLY | JSPROP_PERMANENT) as u32,
        );
        rooted!(&in(cx) let ctor = JS_GetConstructor(cx.raw_cx(), proto_h.handle().into()));
        if ctor.get().is_null() {
            return;
        }

        let ctor_val = ObjectValue(ctor.get());
        rooted!(&in(cx) let cv = ctor_val);
        rooted!(&in(cx) let module_obj_r = module_obj);
        JS_DefineProperty(
            cx.raw_cx(),
            module_obj_r.handle().into(),
            c"Database".as_ptr(),
            cv.handle().into(),
            (JSPROP_ENUMERATE | JSPROP_PERMANENT) as u32,
        );
    }
}

/// Install bun:sqlite module with real Database constructor.
pub fn install(cx: &mut mozjs::context::JSContext) {
    rooted!(&in(cx) let obj = unsafe { w2::JS_NewPlainObject(cx) });
    if obj.get().is_null() {
        return;
    }

    register_database_constructor(cx, obj.get());

    cache_builtin(cx, "bun:sqlite", obj.get());
}

// ── Unit tests ──

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sqlite_database_open_memory() {
        let db = SqliteDatabase::new(":memory:").unwrap();
        assert!(!db.is_closed());
    }

    #[test]
    fn test_sqlite_database_exec_and_close() {
        let db = SqliteDatabase::new(":memory:").unwrap();
        db.exec("CREATE TABLE test (id INTEGER PRIMARY KEY, name TEXT)")
            .unwrap();
        db.exec("INSERT INTO test VALUES (1, 'hello')").unwrap();
        db.close().unwrap();
        assert!(db.is_closed());
    }

    #[test]
    fn test_sqlite_database_close_twice_errors() {
        let db = SqliteDatabase::new(":memory:").unwrap();
        db.close().unwrap();
        assert!(db.close().is_err());
    }

    #[test]
    fn test_sqlite_database_exec_after_close_errors() {
        let db = SqliteDatabase::new(":memory:").unwrap();
        db.close().unwrap();
        assert!(db.exec("SELECT 1").is_err());
    }

    #[test]
    fn test_sqlite_database_invalid_path_errors() {
        assert!(SqliteDatabase::new("/nonexistent/path/db.sqlite").is_err());
    }

    #[test]
    fn test_sqlite_database_in_transaction() {
        let db = SqliteDatabase::new(":memory:").unwrap();
        assert!(!db.in_transaction()); // autocommit = true
        db.exec("BEGIN").unwrap();
        assert!(db.in_transaction());
        db.exec("COMMIT").unwrap();
        assert!(!db.in_transaction());
    }

    #[test]
    fn test_sqlite_database_run() {
        let db = SqliteDatabase::new(":memory:").unwrap();
        db.exec("CREATE TABLE t (id INTEGER PRIMARY KEY, name TEXT)")
            .unwrap();
        let result = db.run("INSERT INTO t VALUES (1, 'hello')").unwrap();
        assert_eq!(result.changes, 1);
        assert!(result.last_insert_rowid > 0);
    }

    #[test]
    fn test_sqlite_database_query_roundtrip() {
        let db = SqliteDatabase::new(":memory:").unwrap();
        db.exec("CREATE TABLE t (id INTEGER, val TEXT)").unwrap();
        db.exec("INSERT INTO t VALUES (1, 'a')").unwrap();
        db.exec("INSERT INTO t VALUES (2, 'b')").unwrap();

        let borrow = db.conn.borrow();
        let conn = borrow.as_ref().unwrap();
        let mut stmt = conn.prepare("SELECT id, val FROM t ORDER BY id").unwrap();
        let rows: Vec<(i64, String)> = stmt
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
            .unwrap()
            .filter_map(|r| r.ok())
            .collect();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].0, 1);
        assert_eq!(rows[0].1, "a");
        assert_eq!(rows[1].0, 2);
        assert_eq!(rows[1].1, "b");
    }
}
