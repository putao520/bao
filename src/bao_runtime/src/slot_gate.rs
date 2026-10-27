// @trace REQ-ENG-004
//! Central reserved-slot pointer-carrier helpers (BCE-20261025-FFI-SLOT-GATE).
//!
//! Shared by every runtime module that stamps a raw uWS / user-data pointer
//! into a JS object: bun_listen, bun_udp, node_http, node_http2, bun_api, ...
//! Extracted from the bun_udp `slot_gate` / `udp_slot_ptr` precedent so the
//! class-gate + reserved-slot read shape has one source of truth
//! (REQ-ARCH-001 duplicate convergence).

use mozjs::glue::JS_GetReservedSlot;
use mozjs::jsapi::*;
use mozjs::jsval::{PrivateValue, UndefinedValue};
use mozjs::rooted;
use mozjs::rust::wrappers2 as w2;
use ::std::ptr::NonNull;

/// Pure-boolean class gate that must run BEFORE any reserved-slot access on an
/// object whose `this` can be foreign (BCE-20261025-FFI-SLOT-GATE class, same
/// fix as node_url `sp_take_pairs`): `JS_GetReservedSlot` on a foreign class
/// reads that object's fixed-slot area, where stale nursery-recycled bits pass
/// the private-encoding / `is_object` checks and yield garbage pointers
/// (`Box::from_raw` = free of an arbitrary address; `&*ptr` = wild deref).
/// `JS_InstanceOf` with a null `CallArgs*` is a pure class-pointer comparison —
/// it reports nothing and runs no user code, so the caller's own fallback stays
/// the single behavior surface.
#[allow(unsafe_op_in_unsafe_fn)]
pub(crate) unsafe fn slot_gate(
    cx: *mut JSContext,
    obj: *mut JSObject,
    clasp: *const JSClass,
) -> bool {
    unsafe {
        let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
        rooted!(&in(wrapped_cx) let obj_root = obj);
        w2::JS_InstanceOf(
            &mut wrapped_cx,
            obj_root.handle(),
            clasp,
            ::std::ptr::null_mut(),
        )
    }
}

/// Raw pointer read out of a class-gated reserved slot. Returns null for any
/// foreign-class receiver (proxies included — their class is the proxy class)
/// or a cleared/undefined slot, so stale foreign bits never reach
/// `to_private()`.
#[allow(unsafe_op_in_unsafe_fn)]
pub(crate) unsafe fn slot_ptr(
    cx: *mut JSContext,
    obj_h: mozjs::rust::Handle<*mut JSObject>,
    clasp: *const JSClass,
    slot: u32,
) -> *mut ::std::os::raw::c_void {
    unsafe {
        if !slot_gate(cx, *obj_h, clasp) {
            return ::std::ptr::null_mut();
        }
        let mut sv = UndefinedValue();
        JS_GetReservedSlot(*obj_h, slot, &mut sv);
        if sv.is_double() && (sv.asBits_ & 0xFFFF000000000000) == 0 {
            sv.to_private() as *mut ::std::os::raw::c_void
        } else {
            ::std::ptr::null_mut()
        }
    }
}

/// Store a raw pointer into an engine-private reserved slot.
pub(crate) unsafe fn set_slot(
    obj: *mut JSObject,
    slot: u32,
    ptr: *const ::std::os::raw::c_void,
) {
    let v = PrivateValue(ptr);
    unsafe { JS_SetReservedSlot(obj, slot, &v) };
}
