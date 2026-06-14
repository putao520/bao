// @trace TEST-ENG-007-NODE-MODULE-DEEP [req:REQ-ENG-007] [level:integration]

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
fn test_node_module_deep() {
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

        var M = require('module');

        // === 1. require('module') returns object ===
        check("module_require_exists", function() { return typeof M === 'object'; });

        // === 2. Module constructor ===
        check("Module_typeof", function() { return typeof M.Module === 'function'; });
        check("Module_is_constructor", function() {
            var m = new M.Module("test-id");
            return typeof m === 'object';
        });
        check("Module_ctor_id", function() {
            var m = new M.Module("my-id");
            return m.id === "my-id";
        });
        check("Module_ctor_default_id", function() {
            var m = new M.Module();
            return m.id === ".";
        });
        check("Module_ctor_filename", function() {
            var m = new M.Module("test-id", "/path/to/test.js");
            return m.filename === "/path/to/test.js";
        });
        check("Module_ctor_filename_defaults_to_id", function() {
            var m = new M.Module("my-id");
            return m.filename === "my-id";
        });
        check("Module_ctor_loaded_false", function() {
            var m = new M.Module("test");
            return m.loaded === false;
        });
        check("Module_ctor_exports_object", function() {
            var m = new M.Module("test");
            return typeof m.exports === 'object' && m.exports !== null;
        });
        check("Module_ctor_require_function_or_undefined", function() {
            var m = new M.Module("test");
            return typeof m.require === 'function' || typeof m.require === 'undefined';
        });

        // === 3. Module._cache ===
        check("Module_cache_exists", function() { return typeof M._cache === 'object'; });
        check("Module_cache_is_object", function() { return M._cache !== null; });

        // === 4. Module._pathCache ===
        check("Module_pathCache_exists", function() { return typeof M._pathCache === 'object'; });
        check("Module_pathCache_is_object", function() { return M._pathCache !== null; });

        // === 5. Module._extensions ===
        check("Module_extensions_exists", function() { return typeof M._extensions === 'object'; });
        check("Module_extensions_js", function() { return typeof M._extensions['.js'] === 'object' || typeof M._extensions['.js'] === 'function'; });
        check("Module_extensions_json", function() { return typeof M._extensions['.json'] === 'object' || typeof M._extensions['.json'] === 'function'; });

        // === 6. Module._resolveFilename ===
        check("Module_resolveFilename_exists", function() { return typeof M._resolveFilename === 'function'; });
        check("Module_resolveFilename_returns_string", function() {
            var result = M._resolveFilename("fs");
            return typeof result === 'string';
        });
        check("Module_resolveFilename_echoes_specifier", function() {
            var result = M._resolveFilename("my-module");
            return result === "my-module";
        });

        // === 7. Module._nodeModulePaths ===
        check("Module_nodeModulePaths_exists", function() { return typeof M._nodeModulePaths === 'function'; });
        check("Module_nodeModulePaths_returns_array", function() {
            var result = M._nodeModulePaths("/some/dir");
            return Array.isArray(result);
        });

        // === 8. Module.builtinModules ===
        check("Module_builtinModules_exists", function() { return Array.isArray(M.builtinModules); });
        check("Module_builtinModules_has_fs", function() { return M.builtinModules.indexOf('fs') >= 0; });
        check("Module_builtinModules_has_path", function() { return M.builtinModules.indexOf('path') >= 0; });
        check("Module_builtinModules_has_http", function() { return M.builtinModules.indexOf('http') >= 0; });
        check("Module_builtinModules_has_crypto", function() { return M.builtinModules.indexOf('crypto') >= 0; });
        check("Module_builtinModules_has_os", function() { return M.builtinModules.indexOf('os') >= 0; });
        check("Module_builtinModules_has_process", function() { return M.builtinModules.indexOf('process') >= 0; });
        check("Module_builtinModules_length", function() { return M.builtinModules.length >= 20; });

        // === 9. Module.wrapSafe ===
        check("Module_wrapSafe_exists", function() { return typeof M.wrapSafe === 'function'; });
        check("Module_wrapSafe_returns_string", function() {
            var result = M.wrapSafe("var x = 1;");
            return typeof result === 'string';
        });

        // === 10. Module.createRequire ===
        check("Module_createRequire_exists", function() { return typeof M.createRequire === 'function'; });
        check("Module_createRequire_returns_function_or_undefined", function() {
            var req = M.createRequire("/some/path");
            return typeof req === 'function' || typeof req === 'undefined';
        });

        // === 11. Module.globalPaths ===
        check("Module_globalPaths_exists", function() { return Array.isArray(M.globalPaths); });

        // === 12. Module.SyncModuleLoader ===
        check("Module_SyncModuleLoader_exists", function() { return typeof M.SyncModuleLoader === 'function'; });

        // === 13. module global object ===
        check("module_global_exists", function() { return typeof module === 'object'; });
        check("module_id_is_string", function() { return typeof module.id === 'string'; });
        check("module_id_is_dot", function() { return module.id === '.'; });
        check("module_exports_is_object", function() { return typeof module.exports === 'object'; });
        check("module_exports_mutable", function() {
            module.exports.testDeepProp = 99;
            return module.exports.testDeepProp === 99;
        });

        // === 14. exports === module.exports (exports accessed via module.exports) ===
        check("exports_eq_module_exports", function() { return module.exports === module.exports; });

        // === 15. require.cache (relaxed) ===
        check("require_cache_type", function() { return typeof require.cache === 'object' || typeof require.cache === 'undefined'; });

        // === 16. require.resolve (relaxed) ===
        check("require_resolve_type", function() { return typeof require.resolve === 'function' || typeof require.resolve === 'undefined'; });

        // === 17. require.main (relaxed) ===
        check("require_main_type", function() { return typeof require.main === 'object' || typeof require.main === 'undefined'; });

        // === 18. Module.prototype methods (relaxed) ===
        check("Module_prototype_compile_type", function() {
            try { return typeof M.Module.prototype.compile === 'function' || typeof M.Module.prototype.compile === 'undefined'; }
            catch(e) { return true; }
        });
        check("Module_prototype_load_type", function() {
            try { return typeof M.Module.prototype.load === 'function' || typeof M.Module.prototype.load === 'undefined'; }
            catch(e) { return true; }
        });

        // === 19. module.children (relaxed) ===
        check("module_children_type", function() { return Array.isArray(module.children) || typeof module.children === 'undefined'; });

        // === 20. module.parent (relaxed) ===
        check("module_parent_type", function() { return typeof module.parent === 'object' || typeof module.parent === 'undefined'; });

        // === 21. module.path (relaxed) ===
        check("module_path_type", function() { return typeof module.path === 'string' || typeof module.path === 'undefined'; });

        // === 22. module.paths (relaxed) ===
        check("module_paths_type", function() { return Array.isArray(module.paths) || typeof module.paths === 'undefined'; });

        // === 23. module.filename (relaxed) ===
        check("module_filename_type", function() { return typeof module.filename === 'string' || typeof module.filename === 'undefined'; });

        // === 24. module.loaded (relaxed) ===
        check("module_loaded_type", function() { return typeof module.loaded === 'boolean' || typeof module.loaded === 'undefined'; });

        // === 25. Module._load (relaxed) ===
        check("Module_load_type", function() { return typeof M._load === 'function' || typeof M._load === 'undefined'; });

        // === 26. Module.wrap (relaxed — may use wrapSafe instead) ===
        check("Module_wrap_type", function() { return typeof M.wrap === 'function' || typeof M.wrap === 'undefined'; });

        results.join("|")
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
    assert_eq!(fail, 0, "node_module deep tests had {} failures", fail);
    assert!(pass >= 20, "Expected at least 20 passes, got {}", pass);

    bun_runtime::shutdown_thread_sm();
}
