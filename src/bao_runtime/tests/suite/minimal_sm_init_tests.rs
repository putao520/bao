// Minimal SM init isolation test — no bun_runtime, no output, no shell.
// This narrows the crash to SM init alone vs our wrapper layers.
#[test]
fn minimal_sm_init_isolated() {
    crate::exit_isolation::dispatch_timeout(
        "minimal_sm_init_tests::minimal_sm_init_isolated",
        minimal_sm_init_isolated_body,
    );
}

fn minimal_sm_init_isolated_body() {
    // Layer 3 ONLY: the exact path the real tests use (for_test + globals + eval)
    // + bun_core::output::init_test (what the real test calls FIRST)
    bun_core::output::init_test();
    eprintln!("[min] init_test done");
    bun_runtime::install_exit_handler();
    eprintln!("[min] exit_handler done");
    bun_runtime::bun_api::init_process_start();
    eprintln!("[min] process_start done");
    eprintln!("[min] for_test start");
    let mut ctx = match bao_engine::context::JsContext::for_test() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("[min] for_test failed: {}", e.message);
            return;
        }
    };
    eprintln!("[min] for_test OK");
    ctx.set_global_setup(bun_runtime::globals::install_all);
    eprintln!("[min] globals set");
    // Simple eval to verify JS engine works
    let result = ctx.eval("1 + 1", "<min>");
    eprintln!("[min] eval result: {:?}", result);
    // require + spawn (the cp surface the real test exercises)
    let cp = ctx.eval("typeof require('child_process')", "<min>");
    eprintln!("[min] require cp: {:?}", cp);
    let spawn = ctx.eval(
        "var c = require('child_process').spawn('cmd.exe', ['/C', 'echo', 'hi']); typeof c.pid === 'number' ? 'ok' : 'bad'",
        "<min>",
    );
    eprintln!("[min] spawn: {:?}", spawn);
    eprintln!("[min] layer-3 done");
}

#[test]
fn minimal_sm_init_direct() {
    // Same as layer 1+2 but WITHOUT isolation re-exec — runs in-process.
    let engine = match mozjs::rust::JSEngine::init() {
        Ok(e) => e,
        Err(mozjs::rust::JSEngineError::AlreadyInitialized) => {
            eprintln!("[min-direct] AlreadyInitialized (TLS from prior test)");
            return;
        }
        Err(e) => {
            eprintln!("[min-direct] init failed: {:?}", e);
            return;
        }
    };
    eprintln!("[min-direct] JS_Init OK");
    let handle = engine.handle();
    let _rt = mozjs::rust::Runtime::new(handle);
    eprintln!("[min-direct] Runtime::new OK");
}
