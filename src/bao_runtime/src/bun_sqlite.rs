// @trace REQ-ENG-008 [entity:SqliteDatabase] [api:GET /api/sqlite-bridge]
// bun:sqlite SpiderMonkey bridge — Database + Statement classes.
//
// Architecture: Native pointers stored in JS object reserved slot 0
// via PrivateValue. Uses JS_InitClass for proper constructor/prototype chain.

use ::std::cell::RefCell;
use bun_core::ZBox;
use ::std::ptr::NonNull;
use ::std::result::Result;

use mozjs::glue::JS_GetReservedSlot;
use mozjs::jsapi::*;
use mozjs::jsval::{
    JSVal, ObjectValue, UndefinedValue, PrivateValue, NullValue, BooleanValue, DoubleValue,
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
}

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
    JSFunctionSpec::ZERO,
];

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
    let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    rooted!(&in(wrapped_cx) let obj_root = thisv.to_object());
    let mut slot = UndefinedValue();
    JS_GetReservedSlot(obj_root.get(), SLOT_DB, &mut slot);
    // @trace BCE-20260618-002 — guard non-private doubles before to_private().
    if !val_is_private(&slot) {
        return None;
    }
    let ptr = slot.to_private() as *mut SqliteDatabase;
    if ptr.is_null() {
        None
    } else {
        Some(ptr)
    }
}

unsafe fn get_stmt_ptr(cx: *mut JSContext, thisv: Handle<Value>) -> Option<*mut SqliteStatement> {
    if !thisv.is_object() {
        return None;
    }
    let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    rooted!(&in(wrapped_cx) let obj_root = thisv.to_object());
    let mut slot = UndefinedValue();
    JS_GetReservedSlot(obj_root.get(), SLOT_STMT, &mut slot);
    // @trace BCE-20260618-002 — guard non-private doubles before to_private().
    if !val_is_private(&slot) {
        return None;
    }
    let ptr = slot.to_private() as *mut SqliteStatement;
    if ptr.is_null() {
        None
    } else {
        Some(ptr)
    }
}

// ── Helper: convert rusqlite Value to JSVal ──

unsafe fn sqlite_value_to_jsval(cx: *mut JSContext, val: rusqlite::types::Value) -> JSVal {
    match val {
        rusqlite::types::Value::Null => NullValue(),
        rusqlite::types::Value::Integer(n) => DoubleValue(n as f64),
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
        rusqlite::types::Value::Blob(_b) => NullValue(),
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

// ── Database constructor ──

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn database_constructor(
    cx: *mut JSContext,
    argc: u32,
    vp: *mut JSVal,
) -> bool {
    let args = CallArgs::from_vp(vp, argc);

    let this = JS_NewObjectForConstructor(cx, &DATABASE_CLASS, &args);
    if this.is_null() {
        JS_ClearPendingException(cx);
        let this_val = args.thisv();
        if this_val.is_object() {
            let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
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
unsafe extern "C" fn database_exec(
    cx: *mut JSContext,
    argc: u32,
    vp: *mut JSVal,
) -> bool {
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
unsafe extern "C" fn database_run(
    cx: *mut JSContext,
    argc: u32,
    vp: *mut JSVal,
) -> bool {
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
    match db.run(&sql) {
        Ok(result) => {
            let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
            let cx_ref = &mut wrapped_cx;
            rooted!(&in(cx_ref) let obj = w2::JS_NewPlainObject(cx_ref));
            if obj.get().is_null() {
                args.rval().set(NullValue());
                return true;
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
            true
        }
        Err(e) => {
            let msg = ZBox::from_bytes(e.as_bytes());
            JS_ReportErrorUTF8(cx, c"%s".as_ptr(), msg.as_ptr());
            false
        }
    }
}

// ── Database.close() ──

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn database_close(
    cx: *mut JSContext,
    _argc: u32,
    vp: *mut JSVal,
) -> bool {
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
unsafe extern "C" fn database_query(
    cx: *mut JSContext,
    argc: u32,
    vp: *mut JSVal,
) -> bool {
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

            let mut row_idx: u32 = 0;
            match stmt.query([]) {
                Ok(mut rows_iter) => {
                    loop {
                        match rows_iter.next() {
                            Ok(Some(row)) => {
                                let row_obj =
                                    row_to_js_object(cx, &row, &col_names, cx_ref);
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
                }
                Err(e) => {
                    let msg = ZBox::from_vec(e.to_string().into_bytes());
                    JS_ReportErrorUTF8(cx, c"%s".as_ptr(), msg.as_ptr());
                    return false;
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
unsafe extern "C" fn database_prepare(
    cx: *mut JSContext,
    argc: u32,
    vp: *mut JSVal,
) -> bool {
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
unsafe extern "C" fn statement_run(
    cx: *mut JSContext,
    _argc: u32,
    vp: *mut JSVal,
) -> bool {
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

    let changes_before = conn.changes();
    match stmt.execute([]) {
        Ok(_) => {
            let changes = conn.changes() - changes_before;
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
unsafe extern "C" fn statement_get(
    cx: *mut JSContext,
    _argc: u32,
    vp: *mut JSVal,
) -> bool {
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

    let mut rows = match stmt.query([]) {
        Ok(r) => r,
        Err(e) => {
            let msg = ZBox::from_vec(e.to_string().into_bytes());
            JS_ReportErrorUTF8(cx, c"%s".as_ptr(), msg.as_ptr());
            return false;
        }
    };

    match rows.next() {
        Ok(Some(row)) => {
            let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
            let cx_ref = &mut wrapped_cx;
            let row_obj = row_to_js_object(cx, &row, &col_names, cx_ref);
            if row_obj.is_null() {
                args.rval().set(NullValue());
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
unsafe extern "C" fn statement_all(
    cx: *mut JSContext,
    _argc: u32,
    vp: *mut JSVal,
) -> bool {
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

    let mut rows = match stmt.query([]) {
        Ok(r) => r,
        Err(e) => {
            let msg = ZBox::from_vec(e.to_string().into_bytes());
            JS_ReportErrorUTF8(cx, c"%s".as_ptr(), msg.as_ptr());
            return false;
        }
    };

    let mut row_idx: u32 = 0;
    loop {
        match rows.next() {
            Ok(Some(row)) => {
                let row_obj = row_to_js_object(cx, &row, &col_names, cx_ref);
                if row_obj.is_null() {
                    break;
                }
                rooted!(&in(cx_ref) let row_val = ObjectValue(row_obj));
                w2::JS_SetElement(cx_ref, result_arr.handle().into(), row_idx, row_val.handle().into());
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
        db.exec("CREATE TABLE t (id INTEGER PRIMARY KEY, name TEXT)").unwrap();
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
        let rows: Vec<(i64, String)> = stmt.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
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
