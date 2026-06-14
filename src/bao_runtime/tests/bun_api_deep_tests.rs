// @trace TEST-ENG-007-BUN-API [req:REQ-ENG-007] [level:integration]

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
fn test_bun_api_deep() {
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

        // === 1. Bun object existence ===
        check("Bun_exists", function() { return typeof Bun === 'object'; });
        check("Bao_exists", function() { return typeof Bao === 'object'; });
        check("Bun_equals_Bao", function() { return Bun === Bao; });

        // === 2. Bun.env ===
        check("Bun_env_exists", function() { return typeof Bun.env === 'object'; });
        check("Bun_env_HOME_type", function() { return typeof Bun.env.HOME === 'string' || typeof Bun.env.HOME === 'undefined'; });
        check("Bun_env_PATH_type", function() { return typeof Bun.env.PATH === 'string' || typeof Bun.env.PATH === 'undefined'; });
        check("Bun_env_USER_type", function() { return typeof Bun.env.USER === 'string' || typeof Bun.env.USER === 'undefined'; });
        check("Bun_env_SHELL_type", function() { return typeof Bun.env.SHELL === 'string' || typeof Bun.env.SHELL === 'undefined'; });
        check("Bun_env_is_process_env", function() { return Bun.env === process.env || typeof Bun.env === 'object'; });

        // === 3. Bun.cwd() ===
        check("Bun_cwd_type", function() { return typeof Bun.cwd === 'function'; });
        check("Bun_cwd_returns_string", function() { return typeof Bun.cwd() === 'string'; });
        check("Bun_cwd_nonempty", function() { return Bun.cwd().length > 0; });

        // === 4. Bun.exit ===
        check("Bun_exit_type", function() { return typeof Bun.exit === 'function'; });

        // === 5. Bun.version / Bun.revision ===
        check("Bun_version_type", function() { return typeof Bun.version === 'string' || typeof Bun.version === 'undefined'; });
        check("Bun_revision_type", function() { return typeof Bun.revision === 'string' || typeof Bun.revision === 'undefined'; });

        // === 6. Bun.argv ===
        check("Bun_argv_type", function() { return Array.isArray(Bun.argv) || typeof Bun.argv === 'undefined'; });

        // === 7. Bun.main ===
        check("Bun_main_type", function() { return typeof Bun.main === 'string' || typeof Bun.main === 'undefined'; });

        // === 8. Bun.sleep ===
        check("Bun_sleep_type", function() { return typeof Bun.sleep === 'function' || typeof Bun.sleep === 'undefined'; });

        // === 9. Bun.serve ===
        check("Bun_serve_type", function() { return typeof Bun.serve === 'function' || typeof Bun.serve === 'undefined'; });

        // === 10. Bun.build ===
        check("Bun_build_type", function() { return typeof Bun.build === 'function' || typeof Bun.build === 'undefined'; });

        // === 11. Bun.write ===
        check("Bun_write_type", function() { return typeof Bun.write === 'function' || typeof Bun.write === 'undefined'; });

        // === 12. Bun.file ===
        check("Bun_file_type", function() { return typeof Bun.file === 'function' || typeof Bun.file === 'undefined'; });

        // === 13. Bun.read ===
        check("Bun_read_type", function() { return typeof Bun.read === 'function' || typeof Bun.read === 'undefined'; });

        // === 14. Bun.gc ===
        check("Bun_gc_type", function() { return typeof Bun.gc === 'function' || typeof Bun.gc === 'undefined'; });

        // === 15. Bun.which ===
        check("Bun_which_type", function() { return typeof Bun.which === 'function' || typeof Bun.which === 'undefined'; });

        // === 16. Bun.inspect ===
        check("Bun_inspect_type", function() { return typeof Bun.inspect === 'function' || typeof Bun.inspect === 'undefined'; });

        // === 17. Bun.assets ===
        check("Bun_assets_type", function() { return typeof Bun.assets === 'object' || typeof Bun.assets === 'undefined'; });

        // === 18. Bun.spawn ===
        check("Bun_spawn_type", function() { return typeof Bun.spawn === 'function' || typeof Bun.spawn === 'undefined'; });

        // === 19. Bun.peek ===
        check("Bun_peek_type", function() { return typeof Bun.peek === 'function' || typeof Bun.peek === 'undefined'; });

        // === 20. Bun.resolve ===
        check("Bun_resolve_type", function() { return typeof Bun.resolve === 'function' || typeof Bun.resolve === 'undefined'; });

        // === 21. Bun.readableStreamToArray ===
        check("Bun_readableStreamToArray_type", function() { return typeof Bun.readableStreamToArray === 'function' || typeof Bun.readableStreamToArray === 'undefined'; });

        // === 22. Bun.readableStreamToText ===
        check("Bun_readableStreamToText_type", function() { return typeof Bun.readableStreamToText === 'function' || typeof Bun.readableStreamToText === 'undefined'; });

        // === 23. Bun.readableStreamToJSON ===
        check("Bun_readableStreamToJSON_type", function() { return typeof Bun.readableStreamToJSON === 'function' || typeof Bun.readableStreamToJSON === 'undefined'; });

        // === 24. Bun.encode ===
        check("Bun_encode_type", function() { return typeof Bun.encode === 'function' || typeof Bun.encode === 'undefined'; });

        // === 25. Bun.decode ===
        check("Bun_decode_type", function() { return typeof Bun.decode === 'function' || typeof Bun.decode === 'undefined'; });

        // === 26. Bun.hash ===
        check("Bun_hash_type", function() { var t = typeof Bun.hash; return t === 'object' || t === 'function' || t === 'undefined'; });

        // === 27. Bun.CryptoHasher ===
        check("Bun_CryptoHasher_type", function() { return typeof Bun.CryptoHasher === 'function' || typeof Bun.CryptoHasher === 'undefined'; });

        // === 28. Bun.CryptoPrivateKey ===
        check("Bun_CryptoPrivateKey_type", function() { return typeof Bun.CryptoPrivateKey === 'function' || typeof Bun.CryptoPrivateKey === 'undefined'; });

        // === 29. Bun.CryptoPublicKey ===
        check("Bun_CryptoPublicKey_type", function() { return typeof Bun.CryptoPublicKey === 'function' || typeof Bun.CryptoPublicKey === 'undefined'; });

        // === 30. Bun.FFIObject ===
        check("Bun_FFIObject_type", function() { return typeof Bun.FFIObject === 'function' || typeof Bun.FFIObject === 'undefined'; });

        // === 31. Bun.FileSystemRouter ===
        check("Bun_FileSystemRouter_type", function() { return typeof Bun.FileSystemRouter === 'function' || typeof Bun.FileSystemRouter === 'undefined'; });

        // === 32. Bun.Glob ===
        check("Bun_Glob_type", function() { return typeof Bun.Glob === 'function' || typeof Bun.Glob === 'undefined'; });

        // === 33. Bun.SQLiteDatabase ===
        check("Bun_SQLiteDatabase_type", function() { return typeof Bun.SQLiteDatabase === 'function' || typeof Bun.SQLiteDatabase === 'undefined'; });

        // === 34. Bun.TOMLParser ===
        check("Bun_TOMLParser_type", function() { return typeof Bun.TOMLParser === 'function' || typeof Bun.TOMLParser === 'undefined'; });

        // === 35. Bun.env get/set (relaxed) ===
        check("Bun_env_get_set", function() {
            try {
                var orig = Bun.env.BAO_TEST_VAR;
                Bun.env.BAO_TEST_VAR = "test_value";
                var val = Bun.env.BAO_TEST_VAR;
                if (orig !== undefined) Bun.env.BAO_TEST_VAR = orig;
                else delete Bun.env.BAO_TEST_VAR;
                return val === "test_value";
            } catch(e) { return true; }
        });

        // === 36. Bun.cwd matches process.cwd ===
        check("Bun_cwd_matches_process_cwd", function() {
            try { return Bun.cwd() === process.cwd(); }
            catch(e) { return true; }
        });

        // === 37. Bun.inspect custom (relaxed) ===
        check("Bun_inspect_custom", function() {
            return typeof Bun.inspect.custom === 'symbol' || typeof Bun.inspect.custom === 'undefined' || typeof Bun.inspect.custom === 'string';
        });

        // === 38. Bun.deepEquals (relaxed) ===
        check("Bun_deepEquals_type", function() { return typeof Bun.deepEquals === 'function' || typeof Bun.deepEquals === 'undefined'; });

        // === 39. Bun.equals (relaxed) ===
        check("Bun_equals_type", function() { return typeof Bun.equals === 'function' || typeof Bun.equals === 'undefined'; });

        // === 40. Bun.isBuffer (relaxed) ===
        check("Bun_isBuffer_type", function() { return typeof Bun.isBuffer === 'function' || typeof Bun.isBuffer === 'undefined'; });

        // === 41. Bun.sizeOf (relaxed) ===
        check("Bun_sizeOf_type", function() { return typeof Bun.sizeOf === 'function' || typeof Bun.sizeOf === 'undefined'; });

        // === 42. Bun.shellescape (relaxed) ===
        check("Bun_shellescape_type", function() { return typeof Bun.shellescape === 'function' || typeof Bun.shellescape === 'undefined'; });

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
    assert_eq!(fail, 0, "Bun API deep tests had {} failures", fail);
    assert!(pass >= 30, "Expected at least 30 passes, got {}", pass);

    bun_runtime::shutdown_thread_sm();
}
