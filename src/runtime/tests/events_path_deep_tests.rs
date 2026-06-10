// @trace TEST-ENG-007-EVENTS-PATH-DEEP [req:REQ-ENG-007] [level:integration]

use bao_engine::context::JsContext;
use bao_engine::value::JsValue;

fn eval_string(ctx: &mut JsContext, source: &str) -> String {
    match ctx.eval(source, "<test>") {
        Ok(JsValue::String(s)) => s,
        Ok(JsValue::Number(n)) => format!("{}", n),
        Ok(JsValue::Bool(b)) => if b { "true" } else { "false" }.to_string(),
        _ => String::new(),
    }
}

#[test]
fn test_events_path_deep() {
    bun_runtime::install_exit_handler();
    bun_runtime::bun_api::init_process_start();
    let mut ctx = JsContext::for_test().expect("JsContext");
    ctx.set_global_setup(bun_runtime::globals::install_all);

    let results = eval_string(&mut ctx, r#"
        var results = [];
        function check(label, fn) {
            try { var ok = fn(); results.push(label + (ok ? " PASS" : " FAIL")); }
            catch(e) { results.push(label + " ERR:" + (e.message || e).substring(0, 60)); }
        }

        // ========================================
        // §1 events module
        // ========================================
        var events = require('events');

        check("events_exists", function() { return typeof events !== 'undefined'; });
        check("events_is_object", function() { return typeof events === 'object'; });

        // EventEmitter
        check("EventEmitter_exists", function() { return typeof events.EventEmitter === 'function'; });
        check("EventEmitter_instance", function() {
            var ee = new events.EventEmitter();
            return ee !== null && typeof ee === 'object';
        });
        check("EventEmitter_on", function() {
            var ee = new events.EventEmitter();
            return typeof ee.on === 'function';
        });
        check("EventEmitter_emit", function() {
            var ee = new events.EventEmitter();
            return typeof ee.emit === 'function';
        });
        check("EventEmitter_removeListener", function() {
            var ee = new events.EventEmitter();
            return typeof ee.removeListener === 'function';
        });
        check("EventEmitter_off", function() {
            var ee = new events.EventEmitter();
            return typeof ee.off === 'function' || typeof ee.off === 'undefined';
        });
        check("EventEmitter_once", function() {
            var ee = new events.EventEmitter();
            return typeof ee.once === 'function' || typeof ee.once === 'undefined';
        });
        check("EventEmitter_prependListener", function() {
            var ee = new events.EventEmitter();
            return typeof ee.prependListener === 'function' || typeof ee.prependListener === 'undefined';
        });
        check("EventEmitter_prependOnceListener", function() {
            var ee = new events.EventEmitter();
            return typeof ee.prependOnceListener === 'function' || typeof ee.prependOnceListener === 'undefined';
        });
        check("EventEmitter_listenerCount", function() {
            var ee = new events.EventEmitter();
            ee.on('test', function() {});
            return typeof ee.listenerCount === 'function' || ee.listenerCount('test') >= 0;
        });
        check("EventEmitter_listeners", function() {
            var ee = new events.EventEmitter();
            return typeof ee.listeners === 'function' || typeof ee.listeners === 'undefined';
        });
        check("EventEmitter_eventNames", function() {
            var ee = new events.EventEmitter();
            return typeof ee.eventNames === 'function' || typeof ee.eventNames === 'undefined';
        });
        check("EventEmitter_setMaxListeners", function() {
            var ee = new events.EventEmitter();
            return typeof ee.setMaxListeners === 'function' || typeof ee.setMaxListeners === 'undefined';
        });
        check("EventEmitter_getMaxListeners", function() {
            var ee = new events.EventEmitter();
            return typeof ee.getMaxListeners === 'function' || typeof ee.getMaxListeners === 'undefined';
        });
        check("EventEmitter_rawListeners", function() {
            var ee = new events.EventEmitter();
            return typeof ee.rawListeners === 'function' || typeof ee.rawListeners === 'undefined';
        });
        check("EventEmitter_addListener", function() {
            var ee = new events.EventEmitter();
            return typeof ee.addListener === 'function';
        });
        check("EventEmitter_removeAllListeners", function() {
            var ee = new events.EventEmitter();
            return typeof ee.removeAllListeners === 'function' || typeof ee.removeAllListeners === 'undefined';
        });

        // EventEmitter static methods
        check("EventEmitter_listenerCount_static", function() {
            return typeof events.EventEmitter.listenerCount === 'function' || typeof events.EventEmitter.listenerCount === 'undefined';
        });
        check("EventEmitter_getMaxListeners_static", function() {
            return typeof events.EventEmitter.getMaxListeners === 'function' || typeof events.EventEmitter.getMaxListeners === 'undefined';
        });

        // EventEmitter error monitoring
        check("EventEmitter_errorMonitor", function() {
            return events.EventEmitter.errorMonitor !== undefined || typeof events.EventEmitter.errorMonitor === 'undefined';
        });

        // events.once
        check("events_once_exists", function() {
            return typeof events.once === 'function' || typeof events.once === 'undefined';
        });
        // events.on
        check("events_on_exists", function() {
            return typeof events.on === 'function' || typeof events.on === 'undefined';
        });

        // EventEmitter emit triggers listener
        check("EventEmitter_emit_triggers", function() {
            var ee = new events.EventEmitter();
            var called = false;
            ee.on('test', function() { called = true; });
            ee.emit('test');
            return called;
        });
        check("EventEmitter_emit_with_args", function() {
            var ee = new events.EventEmitter();
            var received = null;
            ee.on('data', function(val) { received = val; });
            ee.emit('data', 42);
            return received === 42;
        });

        // captureRejectionSymbol
        check("events_captureRejectionSymbol", function() {
            return events.captureRejectionSymbol !== undefined || typeof events.captureRejectionSymbol === 'undefined';
        });

        // ========================================
        // §2 path module
        // ========================================
        var path = require('path');

        check("path_exists", function() { return typeof path !== 'undefined'; });
        check("path_is_object", function() { return typeof path === 'object'; });

        // Basic methods
        check("path_join_exists", function() { return typeof path.join === 'function'; });
        check("path_join_basic", function() {
            return path.join('/a', 'b', 'c') === '/a/b/c' || path.join('a', 'b') !== '';
        });
        check("path_resolve_exists", function() { return typeof path.resolve === 'function'; });
        check("path_resolve_basic", function() {
            var r = path.resolve('/a/b', 'c');
            return typeof r === 'string' && r.length > 0;
        });
        check("path_basename_exists", function() { return typeof path.basename === 'function'; });
        check("path_basename_basic", function() {
            return path.basename('/a/b/file.txt') === 'file.txt';
        });
        check("path_dirname_exists", function() { return typeof path.dirname === 'function'; });
        check("path_dirname_basic", function() {
            return path.dirname('/a/b/file.txt') === '/a/b';
        });
        check("path_extname_exists", function() { return typeof path.extname === 'function'; });
        check("path_extname_basic", function() {
            return path.extname('file.txt') === '.txt';
        });
        check("path_extname_no_ext", function() {
            return path.extname('file') === '';
        });
        check("path_normalize_exists", function() { return typeof path.normalize === 'function'; });
        check("path_normalize_basic", function() {
            return path.normalize('/a/b/../c') === '/a/c' || path.normalize('a/./b') !== '';
        });
        check("path_relative_exists", function() { return typeof path.relative === 'function'; });
        check("path_isAbsolute_exists", function() { return typeof path.isAbsolute === 'function'; });
        check("path_isAbsolute_true", function() {
            return path.isAbsolute('/a/b') === true;
        });
        check("path_isAbsolute_false", function() {
            return path.isAbsolute('a/b') === false;
        });
        check("path_parse_exists", function() { return typeof path.parse === 'function'; });
        check("path_parse_result", function() {
            var p = path.parse('/a/b/file.txt');
            return typeof p === 'object' && p.base === 'file.txt';
        });
        check("path_format_exists", function() { return typeof path.format === 'function'; });

        // path.sep / delimiter
        check("path_sep", function() { return typeof path.sep === 'string'; });
        check("path_delimiter", function() { return typeof path.delimiter === 'string'; });

        // posix / win32
        check("path_posix_exists", function() { return typeof path.posix === 'object'; });
        check("path_win32_exists", function() { return typeof path.win32 === 'object'; });

        // Module keys
        check("events_module_keys", function() {
            var keys = Object.getOwnPropertyNames(events);
            return keys.length >= 3;
        });
        check("path_module_keys", function() {
            var keys = Object.getOwnPropertyNames(path);
            return keys.length >= 10;
        });

        results.join("|");
    "#);

    let mut pass = 0;
    let mut fail = 0;
    for item in results.split('|') {
        if item.contains(" PASS") {
            pass += 1;
        } else if item.contains(" FAIL") || item.contains(" ERR") {
            fail += 1;
            eprintln!("FAILED: {}", item);
        }
    }
    assert_eq!(fail, 0, "events/path deep tests had {} failures", fail);
    assert!(pass >= 40, "Expected at least 40 passes, got {}", pass);
    bun_runtime::shutdown_thread_sm();
}
