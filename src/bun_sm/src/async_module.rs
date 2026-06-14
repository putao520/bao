// @trace REQ-ENG-001
//! Async module — top-level await / dynamic import state tracking.
//!
//! Integrates with SM's module evaluation pipeline to track fulfillment state.
//! Uses gc_store for GC-safe callback storage.

use ::std::sync::atomic::{AtomicU8, Ordering};

const STATE_PENDING: u8 = 0;
const STATE_FULFILLED: u8 = 1;
const STATE_REJECTED: u8 = 2;

pub struct AsyncModule {
    module: *mut mozjs::jsapi::JSObject,
    state: AtomicU8,
}

impl AsyncModule {
    pub fn new(module: *mut mozjs::jsapi::JSObject) -> Self {
        AsyncModule { module, state: AtomicU8::new(STATE_PENDING) }
    }

    pub fn new_pending() -> Self {
        AsyncModule { module: std::ptr::null_mut(), state: AtomicU8::new(STATE_PENDING) }
    }

    pub fn is_fulfilled(&self) -> bool {
        self.state.load(Ordering::Acquire) == STATE_FULFILLED
    }

    pub fn is_rejected(&self) -> bool {
        self.state.load(Ordering::Acquire) == STATE_REJECTED
    }

    pub fn is_pending(&self) -> bool {
        self.state.load(Ordering::Acquire) == STATE_PENDING
    }

    pub fn set_fulfilled(&self) {
        self.state.store(STATE_FULFILLED, Ordering::Release);
    }

    pub fn set_rejected(&self) {
        self.state.store(STATE_REJECTED, Ordering::Release);
    }

    pub fn module_object(&self) -> *mut mozjs::jsapi::JSObject {
        self.module
    }

    pub fn result(&self) -> crate::JSValue {
        if self.module.is_null() {
            crate::JSValue::UNDEFINED
        } else {
            crate::JSValue::UNDEFINED
        }
    }
}

impl Default for AsyncModule {
    fn default() -> Self {
        Self::new_pending()
    }
}

pub struct Queue {
    wake_callback: Option<*mut mozjs::jsapi::JSObject>,
    error_callback: Option<*mut mozjs::jsapi::JSObject>,
}

impl Queue {
    pub fn new() -> Self {
        Queue { wake_callback: None, error_callback: None }
    }

    pub fn set_wake_handler(&mut self, callback: *mut mozjs::jsapi::JSObject) {
        self.wake_callback = if callback.is_null() { None } else { Some(callback) };
    }

    pub fn set_error_handler(&mut self, callback: *mut mozjs::jsapi::JSObject) {
        self.error_callback = if callback.is_null() { None } else { Some(callback) };
    }

    pub unsafe fn on_wake_handler(cx: *mut mozjs::jsapi::JSContext, module: *mut mozjs::jsapi::JSObject) {
        if module.is_null() { return; }
        let global = unsafe { mozjs::jsapi::CurrentGlobalOrNull(cx) };
        if global.is_null() { return; }
        let key = format!("__async_wake_{:p}", module);
        if let Some(cb) = crate::gc::gc_store::get(cx, &key) {
            let cb_val = mozjs::jsval::ObjectValue(cb);
            let module_val = mozjs::jsval::ObjectValue(module);
            let args = [module_val];
            let call_args = mozjs::jsapi::HandleValueArray {
                length_: args.len(),
                elements_: args.as_ptr(),
            };
            let global_h = mozjs::jsapi::Handle::<*mut mozjs::jsapi::JSObject> {
                _phantom_0: std::marker::PhantomData,
                ptr: &global,
            };
            let cb_h = mozjs::jsapi::Handle::<mozjs::jsapi::Value> {
                _phantom_0: std::marker::PhantomData,
                ptr: &cb_val,
            };
            let mut rval = mozjs::jsval::UndefinedValue();
            let rval_h = mozjs::jsapi::MutableHandle::<mozjs::jsapi::Value> {
                _phantom_0: std::marker::PhantomData,
                ptr: &mut rval,
            };
            unsafe { mozjs::jsapi::JS_CallFunctionValue(cx, global_h, cb_h, &call_args, rval_h); }
        }
    }

    pub unsafe fn on_dependency_error(cx: *mut mozjs::jsapi::JSContext, module: *mut mozjs::jsapi::JSObject, error: *mut mozjs::jsapi::JSObject) {
        if module.is_null() { return; }
        let key = format!("__async_err_{:p}", module);
        if let Some(cb) = crate::gc::gc_store::get(cx, &key) {
            let cb_val = mozjs::jsval::ObjectValue(cb);
            let module_val = mozjs::jsval::ObjectValue(module);
            let error_val = if error.is_null() { mozjs::jsval::UndefinedValue() } else { mozjs::jsval::ObjectValue(error) };
            let args = [module_val, error_val];
            let call_args = mozjs::jsapi::HandleValueArray {
                length_: args.len(),
                elements_: args.as_ptr(),
            };
            let global = mozjs::jsapi::CurrentGlobalOrNull(cx);
            if global.is_null() { return; }
            let global_h = mozjs::jsapi::Handle::<*mut mozjs::jsapi::JSObject> {
                _phantom_0: std::marker::PhantomData,
                ptr: &global,
            };
            let cb_h = mozjs::jsapi::Handle::<mozjs::jsapi::Value> {
                _phantom_0: std::marker::PhantomData,
                ptr: &cb_val,
            };
            let mut rval = mozjs::jsval::UndefinedValue();
            let rval_h = mozjs::jsapi::MutableHandle::<mozjs::jsapi::Value> {
                _phantom_0: std::marker::PhantomData,
                ptr: &mut rval,
            };
            unsafe { mozjs::jsapi::JS_CallFunctionValue(cx, global_h, cb_h, &call_args, rval_h); }
        }
    }
}

impl Default for Queue {
    fn default() -> Self {
        Self::new()
    }
}

pub struct InitOpts {
    pub module: *mut mozjs::jsapi::JSObject,
    pub is_top_level_await: bool,
}

impl Default for InitOpts {
    fn default() -> Self {
        InitOpts { module: std::ptr::null_mut(), is_top_level_await: false }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn async_module_state() {
        let m = AsyncModule::new_pending();
        assert!(m.is_pending());
        assert!(!m.is_fulfilled());
        assert!(!m.is_rejected());

        m.set_fulfilled();
        assert!(m.is_fulfilled());
        assert!(!m.is_pending());

        let m2 = AsyncModule::new_pending();
        m2.set_rejected();
        assert!(m2.is_rejected());
    }

    #[test]
    fn queue_new() {
        let q = Queue::new();
        assert!(q.wake_callback.is_none());
        assert!(q.error_callback.is_none());
    }
}
