// @trace REQ-ENG-006 [api:node:domain]
// Deprecated Node.js module — minimal compatibility implementation.
// Domain is essentially an EventEmitter with .run() and error interception.
use bun_core::ZBox;
use ::std::ptr::NonNull;

use mozjs::jsapi::*;
use mozjs::jsval::{JSVal, UndefinedValue, ObjectValue, StringValue, BooleanValue};
use mozjs::rooted;
use mozjs::rust::wrappers2 as w2;

use crate::require::cache_builtin;

pub fn install(cx: &mut mozjs::context::JSContext) {
    rooted!(&in(cx) let domain_obj = unsafe { w2::JS_NewPlainObject(cx) });
    if domain_obj.get().is_null() {
        return;
    }

    unsafe {
        // Domain class constructor
        let domain_fn = JS_NewFunction(cx.raw_cx(), Some(domain_constructor), 0, 0x400, c"Domain".as_ptr());
        if !domain_fn.is_null() {
            let fn_obj = JS_GetFunctionObject(domain_fn);
            rooted!(&in(cx) let fn_root = fn_obj);

            // Static create() method — returns a new Domain instance
            w2::JS_DefineFunction(cx, fn_root.handle(), c"create".as_ptr(), Some(domain_create), 0, JSPROP_ENUMERATE as u32);

            // Instance methods defined on prototype
            rooted!(&in(cx) let proto = w2::JS_NewPlainObject(cx));
            if !proto.get().is_null() {
                // Inherit from EventEmitter
                let events_obj = crate::gc_store::gc_store_get(cx.raw_cx(), "builtin:events");
                if let Some(events) = events_obj {
                    if !events.is_null() {
                        rooted!(&in(cx) let events_root = events);
                        let mut ee_proto = UndefinedValue();
                        JS_GetProperty(cx.raw_cx(), events_root.handle().into(), c"EventEmitter".as_ptr(),
                            MutableHandle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &mut ee_proto });
                        if ee_proto.is_object() {
                            rooted!(&in(cx) let ee_ctor = ee_proto.to_object());
                            let mut ee_proto_val = UndefinedValue();
                            JS_GetProperty(cx.raw_cx(), ee_ctor.handle().into(), c"prototype".as_ptr(),
                                MutableHandle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &mut ee_proto_val });
                            if ee_proto_val.is_object() {
                                rooted!(&in(cx) let ee_proto_obj = ee_proto_val.to_object());
                                JS_SetPrototype(cx.raw_cx(), proto.handle().into(), ee_proto_obj.handle().into());
                            }
                        }
                    }
                }

                // Domain-specific methods
                w2::JS_DefineFunction(cx, proto.handle(), c"run".as_ptr(), Some(domain_run), 1, JSPROP_ENUMERATE as u32);
                w2::JS_DefineFunction(cx, proto.handle(), c"add".as_ptr(), Some(domain_add), 1, JSPROP_ENUMERATE as u32);
                w2::JS_DefineFunction(cx, proto.handle(), c"remove".as_ptr(), Some(domain_remove), 1, JSPROP_ENUMERATE as u32);
                w2::JS_DefineFunction(cx, proto.handle(), c"bind".as_ptr(), Some(domain_bind), 1, JSPROP_ENUMERATE as u32);
                w2::JS_DefineFunction(cx, proto.handle(), c"intercept".as_ptr(), Some(domain_intercept), 1, JSPROP_ENUMERATE as u32);
                w2::JS_DefineFunction(cx, proto.handle(), c"enter".as_ptr(), Some(domain_enter), 0, JSPROP_ENUMERATE as u32);
                w2::JS_DefineFunction(cx, proto.handle(), c"exit".as_ptr(), Some(domain_exit), 0, JSPROP_ENUMERATE as u32);
                w2::JS_DefineFunction(cx, proto.handle(), c"dispose".as_ptr(), Some(domain_dispose), 0, JSPROP_ENUMERATE as u32);

                // Wire prototype
                rooted!(&in(cx) let proto_val = ObjectValue(proto.get()));
                JS_DefineProperty(cx.raw_cx(), fn_root.handle().into(), c"prototype".as_ptr(), proto_val.handle().into(), 0u32);
            }

            rooted!(&in(cx) let fn_val = ObjectValue(fn_root.get()));
            JS_DefineProperty(cx.raw_cx(), domain_obj.handle().into(), c"Domain".as_ptr(), fn_val.handle().into(), JSPROP_ENUMERATE as u32);
        }

        // Default active domain instance
        w2::JS_DefineFunction(cx, domain_obj.handle(), c"create".as_ptr(), Some(domain_create), 0, JSPROP_ENUMERATE as u32);
    }

    cache_builtin(cx, "domain", domain_obj.get());
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn domain_constructor(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    // Create a new Domain object (plain object with EE prototype)
    let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;
    rooted!(&in(cx_ref) let obj = w2::JS_NewPlainObject(cx_ref));
    if obj.get().is_null() {
        args.rval().set(UndefinedValue());
        return true;
    }
    // Mark as a domain
    rooted!(&in(cx_ref) let is_domain = BooleanValue(true));
    JS_DefineProperty(cx, obj.handle().into(), c"_domain".as_ptr(), is_domain.handle().into(), 0u32);
    args.rval().set(ObjectValue(obj.get()));
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn domain_create(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    // create() returns a new Domain instance (same as new Domain())
    let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;
    rooted!(&in(cx_ref) let obj = w2::JS_NewPlainObject(cx_ref));
    if obj.get().is_null() {
        args.rval().set(UndefinedValue());
        return true;
    }
    rooted!(&in(cx_ref) let is_domain = BooleanValue(true));
    JS_DefineProperty(cx, obj.handle().into(), c"_domain".as_ptr(), is_domain.handle().into(), 0u32);
    args.rval().set(ObjectValue(obj.get()));
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn domain_run(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    if argc == 0 {
        JS_ReportErrorUTF8(cx, c"domain.run() requires a function argument".as_ptr());
        return false;
    }
    let fn_val = *args.get(0).ptr;
    if !fn_val.is_object() {
        JS_ReportErrorUTF8(cx, c"domain.run() argument must be a function".as_ptr());
        return false;
    }

    // Call the function with the domain as `this`
    let this_val = args.thisv();
    let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;
    rooted!(&in(cx_ref) let this_obj = if this_val.is_object() { this_val.to_object() } else {
        let g = CurrentGlobalOrNull(cx);
        if g.is_null() { return true; } else { g }
    });
    rooted!(&in(cx_ref) let fn_obj = fn_val.to_object());
    rooted!(&in(cx_ref) let fn_val_obj = ObjectValue(fn_obj.get()));

    // Build additional args (after the function)
    let extra_args: Vec<JSVal> = (1..argc).map(|i| *args.get(i).ptr).collect();
    let call_args = if extra_args.is_empty() {
        HandleValueArray::empty()
    } else {
        HandleValueArray { length_: extra_args.len(), elements_: extra_args.as_ptr() }
    };

    let mut rval = UndefinedValue();
    JS_CallFunctionValue(cx, this_obj.handle().into(), fn_val_obj.handle().into(), &call_args,
        MutableHandle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &mut rval });

    args.rval().set(rval);
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn domain_add(_cx: *mut JSContext, _argc: u32, vp: *mut JSVal) -> bool {
    // Deprecated, no-op
    let args = CallArgs::from_vp(vp, _argc);
    args.rval().set(UndefinedValue());
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn domain_remove(_cx: *mut JSContext, _argc: u32, vp: *mut JSVal) -> bool {
    // Deprecated, no-op
    let args = CallArgs::from_vp(vp, _argc);
    args.rval().set(UndefinedValue());
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn domain_bind(_cx: *mut JSContext, _argc: u32, vp: *mut JSVal) -> bool {
    // Deprecated, no-op
    let args = CallArgs::from_vp(vp, _argc);
    args.rval().set(UndefinedValue());
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn domain_intercept(_cx: *mut JSContext, _argc: u32, vp: *mut JSVal) -> bool {
    // Deprecated, no-op
    let args = CallArgs::from_vp(vp, _argc);
    args.rval().set(UndefinedValue());
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn domain_enter(_cx: *mut JSContext, _argc: u32, vp: *mut JSVal) -> bool {
    // Deprecated, no-op
    let args = CallArgs::from_vp(vp, _argc);
    args.rval().set(UndefinedValue());
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn domain_exit(_cx: *mut JSContext, _argc: u32, vp: *mut JSVal) -> bool {
    // Deprecated, no-op
    let args = CallArgs::from_vp(vp, _argc);
    args.rval().set(UndefinedValue());
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn domain_dispose(_cx: *mut JSContext, _argc: u32, vp: *mut JSVal) -> bool {
    // Deprecated, no-op
    let args = CallArgs::from_vp(vp, _argc);
    args.rval().set(UndefinedValue());
    true
}
