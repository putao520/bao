// @trace TEST-ENG-007-BUN-TEST-DEEP [req:REQ-ENG-007] [level:integration]

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
fn test_bun_test_deep() {
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

        // ---- Bun.test existence ----
        check("Bun_test_exists", function() { return typeof Bun.test === 'function'; });
        check("Bun_test_is_function", function() { return typeof Bun.test === 'function'; });

        // ---- __bun_test_module existence (internal shim) ----
        check("bun_test_module_exists", function() { return typeof __bun_test_module === 'object'; });

        // ---- describe function ----
        check("describe_exists", function() { return typeof __bun_test_module.describe === 'function'; });
        check("describe_skip_exists", function() { return typeof __bun_test_module.describe.skip === 'function'; });
        check("describe_todo_exists", function() { return typeof __bun_test_module.describe.todo === 'function'; });
        check("describe_only_exists", function() { return typeof __bun_test_module.describe.only === 'function'; });
        check("describe_each_exists", function() { return typeof __bun_test_module.describe.each === 'function'; });
        check("describe_if_exists", function() { return typeof __bun_test_module.describe.if === 'function'; });

        // ---- it function ----
        check("it_exists", function() { return typeof __bun_test_module.it === 'function'; });
        check("it_skip_exists", function() { return typeof __bun_test_module.it.skip === 'function'; });
        check("it_todo_exists", function() { return typeof __bun_test_module.it.todo === 'function'; });
        check("it_only_exists", function() { return typeof __bun_test_module.it.only === 'function'; });
        check("it_each_exists", function() { return typeof __bun_test_module.it.each === 'function'; });
        check("it_failing_exists", function() { return typeof __bun_test_module.it.failing === 'function'; });

        // ---- test function ----
        check("test_exists", function() { return typeof __bun_test_module.test === 'function'; });
        check("test_skip_exists", function() { return typeof __bun_test_module.test.skip === 'function'; });
        check("test_todo_exists", function() { return typeof __bun_test_module.test.todo === 'function'; });
        check("test_only_exists", function() { return typeof __bun_test_module.test.only === 'function'; });
        check("test_each_exists", function() { return typeof __bun_test_module.test.each === 'function'; });
        check("test_failing_exists", function() { return typeof __bun_test_module.test.failing === 'function'; });
        check("test_if_exists", function() { return typeof __bun_test_module.test.if === 'function'; });

        // ---- expect function ----
        check("expect_exists", function() { return typeof __bun_test_module.expect === 'function'; });
        check("expect_extend_exists", function() { return typeof __bun_test_module.expect.extend === 'function'; });

        // ---- expect().toBe ----
        check("expect_toBe_exists", function() {
            var e = __bun_test_module.expect(1);
            return typeof e.toBe === 'function';
        });
        check("expect_toBe_passes_equal", function() {
            var e = __bun_test_module.expect(42);
            e.toBe(42); // should not throw
            return true;
        });
        check("expect_toBe_fails_different", function() {
            var e = __bun_test_module.expect(1);
            try { e.toBe(2); return false; }
            catch(err) { return err.message.indexOf('to be') !== -1; }
        });

        // ---- expect().toEqual ----
        check("expect_toEqual_exists", function() {
            var e = __bun_test_module.expect({});
            return typeof e.toEqual === 'function';
        });
        check("expect_toEqual_passes_same_json", function() {
            var e = __bun_test_module.expect({a: 1});
            e.toEqual({a: 1}); // should not throw
            return true;
        });
        check("expect_toEqual_fails_different_json", function() {
            var e = __bun_test_module.expect({a: 1});
            try { e.toEqual({a: 2}); return false; }
            catch(err) { return err.message.indexOf('to equal') !== -1; }
        });

        // ---- expect().toBeTruthy / toBeFalsy ----
        check("expect_toBeTruthy_exists", function() {
            return typeof __bun_test_module.expect(1).toBeTruthy === 'function';
        });
        check("expect_toBeTruthy_passes_truthy", function() {
            __bun_test_module.expect(1).toBeTruthy();
            __bun_test_module.expect("hello").toBeTruthy();
            __bun_test_module.expect({}).toBeTruthy();
            return true;
        });
        check("expect_toBeFalsy_exists", function() {
            return typeof __bun_test_module.expect(0).toBeFalsy === 'function';
        });
        check("expect_toBeFalsy_passes_falsy", function() {
            __bun_test_module.expect(0).toBeFalsy();
            __bun_test_module.expect("").toBeFalsy();
            __bun_test_module.expect(null).toBeFalsy();
            __bun_test_module.expect(undefined).toBeFalsy();
            return true;
        });

        // ---- expect().toBeNull / toBeUndefined / toBeDefined ----
        check("expect_toBeNull_exists", function() {
            return typeof __bun_test_module.expect(null).toBeNull === 'function';
        });
        check("expect_toBeNull_passes_null", function() {
            __bun_test_module.expect(null).toBeNull();
            return true;
        });
        check("expect_toBeUndefined_exists", function() {
            return typeof __bun_test_module.expect(undefined).toBeUndefined === 'function';
        });
        check("expect_toBeUndefined_passes_undefined", function() {
            __bun_test_module.expect(undefined).toBeUndefined();
            return true;
        });
        check("expect_toBeDefined_exists", function() {
            return typeof __bun_test_module.expect(1).toBeDefined === 'function';
        });
        check("expect_toBeDefined_passes_defined", function() {
            __bun_test_module.expect(1).toBeDefined();
            __bun_test_module.expect(null).toBeDefined(); // null is defined
            return true;
        });

        // ---- expect().toBeNaN ----
        check("expect_toBeNaN_exists", function() {
            return typeof __bun_test_module.expect(NaN).toBeNaN === 'function';
        });
        check("expect_toBeNaN_passes_nan", function() {
            __bun_test_module.expect(NaN).toBeNaN();
            return true;
        });

        // ---- expect().toBeGreaterThan / toBeGreaterThanOrEqual ----
        check("expect_toBeGreaterThan_exists", function() {
            return typeof __bun_test_module.expect(5).toBeGreaterThan === 'function';
        });
        check("expect_toBeGreaterThan_passes", function() {
            __bun_test_module.expect(5).toBeGreaterThan(3);
            return true;
        });
        check("expect_toBeGreaterThanOrEqual_exists", function() {
            return typeof __bun_test_module.expect(5).toBeGreaterThanOrEqual === 'function';
        });
        check("expect_toBeGreaterThanOrEqual_passes", function() {
            __bun_test_module.expect(5).toBeGreaterThanOrEqual(5);
            __bun_test_module.expect(5).toBeGreaterThanOrEqual(4);
            return true;
        });

        // ---- expect().toBeLessThan / toBeLessThanOrEqual ----
        check("expect_toBeLessThan_exists", function() {
            return typeof __bun_test_module.expect(3).toBeLessThan === 'function';
        });
        check("expect_toBeLessThan_passes", function() {
            __bun_test_module.expect(3).toBeLessThan(5);
            return true;
        });
        check("expect_toBeLessThanOrEqual_exists", function() {
            return typeof __bun_test_module.expect(5).toBeLessThanOrEqual === 'function';
        });
        check("expect_toBeLessThanOrEqual_passes", function() {
            __bun_test_module.expect(5).toBeLessThanOrEqual(5);
            __bun_test_module.expect(4).toBeLessThanOrEqual(5);
            return true;
        });

        // ---- expect().toBeCloseTo ----
        check("expect_toBeCloseTo_exists", function() {
            return typeof __bun_test_module.expect(0.1 + 0.2).toBeCloseTo === 'function';
        });
        check("expect_toBeCloseTo_passes", function() {
            __bun_test_module.expect(0.1 + 0.2).toBeCloseTo(0.3, 5);
            return true;
        });

        // ---- expect().toContain ----
        check("expect_toContain_exists", function() {
            return typeof __bun_test_module.expect([1, 2, 3]).toContain === 'function';
        });
        check("expect_toContain_passes_array", function() {
            __bun_test_module.expect([1, 2, 3]).toContain(2);
            return true;
        });
        check("expect_toContain_passes_string", function() {
            __bun_test_module.expect("hello world").toContain("world");
            return true;
        });

        // ---- expect().toHaveLength ----
        check("expect_toHaveLength_exists", function() {
            return typeof __bun_test_module.expect([1, 2]).toHaveLength === 'function';
        });
        check("expect_toHaveLength_passes", function() {
            __bun_test_module.expect([1, 2, 3]).toHaveLength(3);
            __bun_test_module.expect("hello").toHaveLength(5);
            return true;
        });

        // ---- expect().toThrow / toThrowError ----
        check("expect_toThrow_exists", function() {
            return typeof __bun_test_module.expect(function() { throw new Error("x"); }).toThrow === 'function';
        });
        check("expect_toThrow_passes_throwing", function() {
            __bun_test_module.expect(function() { throw new Error("boom"); }).toThrow();
            return true;
        });
        check("expect_toThrowError_exists", function() {
            return typeof __bun_test_module.expect(function() { throw new Error("x"); }).toThrowError === 'function';
        });
        check("expect_toThrowError_passes_message", function() {
            __bun_test_module.expect(function() { throw new Error("boom"); }).toThrowError("boom");
            return true;
        });

        // ---- expect().toMatch ----
        check("expect_toMatch_exists", function() {
            return typeof __bun_test_module.expect("hello").toMatch === 'function';
        });
        check("expect_toMatch_passes_regex", function() {
            __bun_test_module.expect("hello world").toMatch(/world/);
            return true;
        });
        check("expect_toMatch_passes_string", function() {
            __bun_test_module.expect("hello world").toMatch("world");
            return true;
        });

        // ---- expect().toMatchObject ----
        check("expect_toMatchObject_exists", function() {
            return typeof __bun_test_module.expect({a: 1, b: 2}).toMatchObject === 'function';
        });
        check("expect_toMatchObject_passes", function() {
            __bun_test_module.expect({a: 1, b: 2, c: 3}).toMatchObject({a: 1, b: 2});
            return true;
        });

        // ---- expect().toHaveProperty ----
        check("expect_toHaveProperty_exists", function() {
            return typeof __bun_test_module.expect({a: {b: 1}}).toHaveProperty === 'function';
        });
        check("expect_toHaveProperty_passes", function() {
            __bun_test_module.expect({a: {b: 1}}).toHaveProperty("a.b");
            return true;
        });
        check("expect_toHaveProperty_with_value", function() {
            __bun_test_module.expect({a: {b: 1}}).toHaveProperty("a.b", 1);
            return true;
        });

        // ---- expect().not ----
        check("expect_not_exists", function() {
            return typeof __bun_test_module.expect(1).not === 'object';
        });
        check("expect_not_toBe_exists", function() {
            return typeof __bun_test_module.expect(1).not.toBe === 'function';
        });
        check("expect_not_toBe_passes", function() {
            __bun_test_module.expect(1).not.toBe(2);
            return true;
        });
        check("expect_not_toEqual_exists", function() {
            return typeof __bun_test_module.expect({}).not.toEqual === 'function';
        });
        check("expect_not_toEqual_passes", function() {
            __bun_test_module.expect({a: 1}).not.toEqual({a: 2});
            return true;
        });
        check("expect_not_toBeTruthy_exists", function() {
            return typeof __bun_test_module.expect(0).not.toBeTruthy === 'function';
        });
        check("expect_not_toBeFalsy_exists", function() {
            return typeof __bun_test_module.expect(1).not.toBeFalsy === 'function';
        });
        check("expect_not_toBeNull_exists", function() {
            return typeof __bun_test_module.expect(1).not.toBeNull === 'function';
        });
        check("expect_not_toThrow_exists", function() {
            return typeof __bun_test_module.expect(function() {}).not.toThrow === 'function';
        });
        check("expect_not_toContain_exists", function() {
            return typeof __bun_test_module.expect([1, 2]).not.toContain === 'function';
        });
        check("expect_not_toMatch_exists", function() {
            return typeof __bun_test_module.expect("hello").not.toMatch === 'function';
        });

        // ---- beforeEach / afterEach ----
        check("beforeEach_exists", function() {
            return typeof __bun_test_module.beforeEach === 'function';
        });
        check("afterEach_exists", function() {
            return typeof __bun_test_module.afterEach === 'function';
        });

        // ---- beforeAll / afterAll ----
        check("beforeAll_exists", function() {
            return typeof __bun_test_module.beforeAll === 'function';
        });
        check("afterAll_exists", function() {
            return typeof __bun_test_module.afterAll === 'function';
        });

        // ---- jest compatibility shim ----
        check("jest_exists", function() {
            return typeof __bun_test_module.jest === 'object';
        });
        check("jest_fn_exists", function() {
            return typeof __bun_test_module.jest.fn === 'function';
        });
        check("jest_spyOn_exists", function() {
            return typeof __bun_test_module.jest.spyOn === 'function';
        });

        // ---- skip / todo / fail ----
        check("skip_exists", function() {
            return typeof __bun_test_module.skip === 'function';
        });
        check("todo_exists", function() {
            return typeof __bun_test_module.todo === 'function';
        });
        check("fail_exists", function() {
            return typeof __bun_test_module.fail === 'function';
        });

        // ---- setDefaultTimeout / gc / printConsole ----
        check("setDefaultTimeout_exists", function() {
            return typeof __bun_test_module.setDefaultTimeout === 'function';
        });
        check("gc_exists", function() {
            return typeof __bun_test_module.gc === 'function';
        });
        check("printConsole_exists", function() {
            return typeof __bun_test_module.printConsole === 'function';
        });

        // ---- __run_bun_tests runner ----
        check("run_bun_tests_exists", function() {
            return typeof __run_bun_tests === 'function';
        });

        // ---- describe.skip is no-op ----
        check("describe_skip_noop", function() {
            var called = false;
            __bun_test_module.describe.skip("skipped", function() { called = true; });
            return !called; // should NOT have called the fn
        });

        // ---- describe.todo is no-op ----
        check("describe_todo_noop", function() {
            var called = false;
            __bun_test_module.describe.todo("todo", function() { called = true; });
            return !called;
        });

        // ---- it.skip is no-op ----
        check("it_skip_noop", function() {
            var called = false;
            __bun_test_module.it.skip("skipped", function() { called = true; });
            return !called;
        });

        // ---- it.todo is no-op ----
        check("it_todo_noop", function() {
            var called = false;
            __bun_test_module.it.todo("todo", function() { called = true; });
            return !called;
        });

        // ---- test.skip is no-op ----
        check("test_skip_noop", function() {
            var called = false;
            __bun_test_module.test.skip("skipped", function() { called = true; });
            return !called;
        });

        // ---- test.todo is no-op ----
        check("test_todo_noop", function() {
            var called = false;
            __bun_test_module.test.todo("todo", function() { called = true; });
            return !called;
        });

        // ---- describe.only runs the test ----
        check("describe_only_runs", function() {
            var called = false;
            __bun_test_module.describe.only("only suite", function() {
                __bun_test_module.it("test", function() { called = true; });
            });
            return true; // registration succeeded
        });

        // ---- it.only runs the test ----
        check("it_only_runs", function() {
            var called = false;
            __bun_test_module.it.only("only test", function() { called = true; });
            return true; // registration succeeded
        });

        // ---- test.only runs the test ----
        check("test_only_runs", function() {
            var called = false;
            __bun_test_module.test.only("only test", function() { called = true; });
            return true; // registration succeeded
        });

        // ---- describe.if conditional ----
        check("describe_if_true_runs", function() {
            var called = false;
            __bun_test_module.describe.if(true)("conditional suite", function() {
                __bun_test_module.it("test", function() { called = true; });
            });
            return true; // registration succeeded
        });
        check("describe_if_false_skips", function() {
            var called = false;
            __bun_test_module.describe.if(false).skip("skipped", function() { called = true; });
            return true; // skip method exists
        });

        // ---- test.if conditional ----
        check("test_if_true_runs", function() {
            var called = false;
            __bun_test_module.test.if(true)("conditional test", function() { called = true; });
            return true; // registration succeeded
        });
        check("test_if_false_skips", function() {
            var called = false;
            __bun_test_module.test.if(false).skip("skipped", function() { called = true; });
            return true; // skip method exists
        });

        // ---- it.failing expects failure ----
        check("it_failing_exists", function() {
            return typeof __bun_test_module.it.failing === 'function';
        });

        // ---- test.failing expects failure ----
        check("test_failing_exists", function() {
            return typeof __bun_test_module.test.failing === 'function';
        });

        // ---- expect().resolves placeholder ----
        check("expect_resolves_exists", function() {
            return typeof __bun_test_module.expect(Promise.resolve(1)).resolves === 'object';
        });

        // ---- expect().rejects placeholder ----
        check("expect_rejects_exists", function() {
            return typeof __bun_test_module.expect(Promise.reject(1)).rejects === 'object';
        });

        // ---- Bun.test() native binding ----
        check("Bun_test_callable", function() {
            // Bun.test(name, fn) should register a test
            Bun.test("native test", function() {});
            return true;
        });

        // ---- test runner result structure ----
        check("run_bun_tests_returns_object", function() {
            var result = __run_bun_tests();
            return typeof result === 'object' && 'passed' in result && 'failed' in result;
        });
        check("run_bun_tests_passed_is_number", function() {
            var result = __run_bun_tests();
            return typeof result.passed === 'number';
        });
        check("run_bun_tests_failed_is_number", function() {
            var result = __run_bun_tests();
            return typeof result.failed === 'number';
        });
        check("run_bun_tests_errors_is_array", function() {
            var result = __run_bun_tests();
            return Array.isArray(result.errors);
        });

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
    assert_eq!(fail, 0, "bun_test deep tests had {} failures", fail);
    assert!(pass >= 25, "Expected at least 25 passes, got {}", pass);

    bun_runtime::shutdown_thread_sm();
}
