// @trace TEST-ENG-OS [req:REQ-ENG-007] [level:integration]

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
fn test_os_deep() {
    bao_runtime::install_exit_handler();
    bao_runtime::bun_api::init_process_start();
    let mut ctx = JsContext::new().expect("JsContext");
    ctx.set_global_setup(bao_runtime::globals::install_all);

    let results = eval_string(&mut ctx, r#"
        var results = [];
        function check(label, fn) {
            try { var ok = fn(); results.push(label + (ok ? " PASS" : " FAIL")); }
            catch(e) { results.push(label + " ERR:" + (e.message || e).substring(0, 50)); }
        }

        var os = require('os');

        // === os module shape ===
        check("os_is_object", function() { return typeof os === 'object' && os !== null; });

        // === os.hostname ===
        check("hostname_exists", function() { return typeof os.hostname === 'function'; });
        check("hostname_type", function() { return typeof os.hostname() === 'string'; });
        check("hostname_nonempty", function() { return os.hostname().length > 0; });

        // === os.type ===
        check("type_exists", function() { return typeof os.type === 'function'; });
        check("type_value", function() { var t = os.type(); return t === 'Linux' || t === 'Darwin' || t === 'Windows_NT'; });

        // === os.platform ===
        check("platform_exists", function() { return typeof os.platform === 'function'; });
        check("platform_value", function() {
            var p = os.platform();
            return p === 'linux' || p === 'darwin' || p === 'win32' || p === 'freebsd' || p === 'openbsd';
        });

        // === os.arch ===
        check("arch_exists", function() { return typeof os.arch === 'function'; });
        check("arch_value", function() {
            var a = os.arch();
            return a === 'x64' || a === 'arm64' || a === 'x86' || a === 'arm';
        });

        // === os.release ===
        check("release_exists", function() { return typeof os.release === 'function'; });
        check("release_type", function() { return typeof os.release() === 'string'; });

        // === os.version ===
        check("version_exists", function() { return typeof os.version === 'function'; });
        check("version_type", function() { return typeof os.version() === 'string'; });

        // === os.totalmem ===
        check("totalmem_exists", function() { return typeof os.totalmem === 'function'; });
        check("totalmem_type", function() { return typeof os.totalmem() === 'number'; });
        check("totalmem_positive", function() { return os.totalmem() > 0; });

        // === os.freemem ===
        check("freemem_exists", function() { return typeof os.freemem === 'function'; });
        check("freemem_type", function() { return typeof os.freemem() === 'number'; });
        check("freemem_positive", function() { return os.freemem() >= 0; });

        // === os.uptime ===
        check("uptime_exists", function() { return typeof os.uptime === 'function'; });
        check("uptime_type", function() { return typeof os.uptime() === 'number'; });
        check("uptime_positive", function() { return os.uptime() > 0; });

        // === os.cpus ===
        check("cpus_exists", function() { return typeof os.cpus === 'function'; });
        check("cpus_array", function() { return Array.isArray(os.cpus()); });
        check("cpus_nonempty", function() { return os.cpus().length > 0; });
        check("cpus_model", function() { return typeof os.cpus()[0].model === 'string'; });
        check("cpus_speed", function() { return typeof os.cpus()[0].speed === 'number'; });

        // === os.networkInterfaces ===
        check("networkInterfaces_exists", function() { return typeof os.networkInterfaces === 'function'; });
        check("networkInterfaces_object", function() { return typeof os.networkInterfaces() === 'object'; });

        // === os.homedir ===
        check("homedir_exists", function() { return typeof os.homedir === 'function'; });
        check("homedir_type", function() { return typeof os.homedir() === 'string'; });
        check("homedir_nonempty", function() { return os.homedir().length > 0; });

        // === os.tmpdir ===
        check("tmpdir_exists", function() { return typeof os.tmpdir === 'function'; });
        check("tmpdir_type", function() { return typeof os.tmpdir() === 'string'; });
        check("tmpdir_nonempty", function() { return os.tmpdir().length > 0; });

        // === os.EOL ===
        check("EOL_type", function() { return typeof os.EOL === 'string'; });
        check("EOL_value", function() { return os.EOL === '\n' || os.EOL === '\r\n'; });

        // === os.constants ===
        check("constants_exists", function() { return typeof os.constants === 'object'; });

        // === os.endianness ===
        check("endianness_exists", function() { return typeof os.endianness === 'function'; });
        check("endianness_value", function() { var e = os.endianness(); return e === 'LE' || e === 'BE'; });

        // === os.loadavg ===
        check("loadavg_exists", function() { return typeof os.loadavg === 'function'; });
        check("loadavg_array", function() { return Array.isArray(os.loadavg()) && os.loadavg().length === 3; });

        // === os.userInfo ===
        check("userInfo_exists", function() { return typeof os.userInfo === 'function'; });
        check("userInfo_object", function() { return typeof os.userInfo() === 'object' && os.userInfo() !== null; });
        check("userInfo_username", function() { return typeof os.userInfo().username === 'string'; });
        check("userInfo_homedir", function() { return typeof os.userInfo().homedir === 'string'; });
        check("userInfo_shell", function() { return typeof os.userInfo().shell === 'string' || os.userInfo().shell === null; });

        // === os.devNull ===
        check("devNull_type", function() { return typeof os.devNull === 'string'; });

        results.join("|")
    "#);

    let mut all_passed = true;
    for item in results.split('|') {
        if !item.contains(" PASS") {
            eprintln!("  FAIL: {}", item);
            all_passed = false;
        }
    }
    assert!(all_passed, "All os deep tests should pass. Results: {}", results);

    std::mem::forget(ctx);
}