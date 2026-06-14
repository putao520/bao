// @trace REQ-ENG-006 [api:GET /api/bun-compat]
// bun: built-in module registration.
//
// Registers bun:sqlite (Database constructor via bun_sqlite module)
// and bun:ffi (dlopen/FfiLibrary via bun_ffi module).

use mozjs::rooted;
use mozjs::rust::wrappers2 as w2;

use crate::require::cache_builtin;

pub fn install(cx: &mut mozjs::context::JSContext) {
    crate::bun_sqlite::install(cx);
    crate::bun_ffi::install(cx);
    install_bun_wrap(cx);
}

fn install_bun_wrap(cx: &mut mozjs::context::JSContext) {
    rooted!(&in(cx) let obj = unsafe { w2::JS_NewPlainObject(cx) });
    if obj.get().is_null() {
        return;
    }

    cache_builtin(cx, "bun:wrap", obj.get());
}
