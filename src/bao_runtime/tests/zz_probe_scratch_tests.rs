// TEMP probe — deleted after diagnosis.
use bao_engine::context::JsContext;
use bao_engine::value::JsValue;

fn eval_string(ctx: &mut JsContext, source: &str) -> String {
    match ctx.eval(source, "<probe>") {
        Ok(JsValue::String(s)) => s,
        Ok(JsValue::Number(n)) => format!("{}", n),
        Ok(JsValue::Bool(b)) => if b { "true" } else { "false" }.to_string(),
        Ok(JsValue::Null) => "null".to_string(),
        Ok(JsValue::Undefined) => "undefined".to_string(),
        Ok(JsValue::Object(_)) => "[object]".to_string(),
        Err(e) => format!("ERROR:{}", e.message),
    }
}

fn setup_ctx() -> JsContext {
    bun_runtime::install_exit_handler();
    bun_runtime::bun_api::init_process_start();
    let mut ctx = JsContext::for_test().expect("JsContext");
    ctx.set_global_setup(bun_runtime::globals::install_all);
    ctx
}

#[test]
fn probe_js_function_forms() {
    let mut ctx = setup_ctx();
    // Form A: string 'js_function' in dlopen args (known passing form)
    let a = eval_string(
        &mut ctx,
        r#"
        var ffi = require('bun:ffi');
        try {
          var lib = ffi.dlopen('/usr/lib/x86_64-linux-gnu/libc.so.6', {
            qsort: { args: ['ptr', 'usize', 'usize', 'js_function'], returns: 'void' }
          });
          'OK-string-form';
        } catch (e) { 'ERR:' + e.message; }
    "#,
    );
    assert_eq!(a, "OK-string-form", "string form: {}", a);

    // Form B: ffi.types.js_function token object in dlopen args
    let b = eval_string(
        &mut ctx,
        r#"
        try {
          var lib2 = ffi.dlopen('/usr/lib/x86_64-linux-gnu/libc.so.6', {
            qsort: { args: ['ptr', 'usize', 'usize', ffi.types.js_function], returns: 'void' }
          });
          'OK-token-form';
        } catch (e) { 'ERR:' + e.message; }
    "#,
    );
    assert_eq!(b, "OK-token-form", "token form: {}", b);

    // Form C: 'callback' alias string in dlopen args
    let c = eval_string(
        &mut ctx,
        r#"
        try {
          var lib3 = ffi.dlopen('/usr/lib/x86_64-linux-gnu/libc.so.6', {
            qsort: { args: ['ptr', 'usize', 'usize', 'callback'], returns: 'void' }
          });
          'OK-alias-form';
        } catch (e) { 'ERR:' + e.message; }
    "#,
    );
    assert_eq!(c, "OK-alias-form", "alias form: {}", c);

    // Form D: js_function as RETURNS type in dlopen descriptor
    let d = eval_string(
        &mut ctx,
        r#"
        try {
          var lib4 = ffi.dlopen('/usr/lib/x86_64-linux-gnu/libc.so.6', {
            qsort: { args: ['ptr', 'usize', 'usize', 'ptr'], returns: 'js_function' }
          });
          'OK-ret-form';
        } catch (e) { 'ERR:' + e.message; }
    "#,
    );
    assert_eq!(d, "OK-ret-form", "returns form: {}", d);

    // Form E: token in callback() args list
    let e = eval_string(
        &mut ctx,
        r#"
        try {
          var cb = ffi.callback([ffi.types.ptr, ffi.types.ptr], 'i32', function (a, b) { return 0; });
          'OK-cb-token:' + typeof cb;
        } catch (e2) { 'ERR:' + e2.message; }
    "#,
    );
    assert!(e.starts_with("OK-cb-token"), "callback token form: {}", e);
}
