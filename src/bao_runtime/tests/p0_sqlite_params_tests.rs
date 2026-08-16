// @trace TEST-ENG-008-SQLITE [req:REQ-ENG-008] [level:integration]
// P0 vector-level regression tests for the bun:sqlite parameter-binding
// silent-fake (v-surface audit): statement run/get/all and database query/run
// called stmt.execute([])/stmt.query([]) — JS arguments were never forwarded,
// so every parameterized query failed with "got 0, needed N".
//
// Coverage: all three bun:sqlite binding forms (variadic / array / named with
// implicit sigil), NULL/blob/bool/float values, prepared-statement reuse, the
// one-shot db.query/db.run forms, explicit binding errors, and an independent
// cross-check of the persisted rows through a direct rusqlite connection
// (same SQLite C library the sqlite3 CLI uses — external-truth comparison).

use bao_engine::context::JsContext;
use bao_engine::value::JsValue;

fn eval_string(ctx: &mut JsContext, source: &str) -> String {
    match ctx.eval(source, "<test>") {
        Ok(JsValue::String(s)) => s,
        Ok(JsValue::Number(n)) => format!("{}", n),
        Ok(JsValue::Bool(b)) => if b { "true" } else { "false" }.to_string(),
        Ok(JsValue::Null) => "null".to_string(),
        Ok(JsValue::Undefined) => "undefined".to_string(),
        Ok(JsValue::Object(_)) => "[object]".to_string(),
        Ok(_) => "[other]".to_string(),
        Err(e) => format!("ERROR:{}", e.message),
    }
}

#[test]
fn test_bun_sqlite_parameter_binding_forms() {
    bun_runtime::install_exit_handler();
    bun_runtime::bun_api::init_process_start();
    let mut ctx = JsContext::for_test().expect("Failed to create JSContext");
    ctx.set_global_setup(bun_runtime::globals::install_all);

    let db_path = std::env::temp_dir().join("bao_p0_sqlite_params_test.db");
    let db_path_str = db_path.to_str().unwrap().to_string();
    let _ = std::fs::remove_file(&db_path);

    let results = eval_string(
        &mut ctx,
        &format!(
            r#"
        var {{ Database }} = require('bun:sqlite');
        var out = [];
        function check(name, fn) {{
            try {{ out.push(name + ':' + (fn() ? 'PASS' : 'FAIL')); }}
            catch (e) {{ out.push(name + ':ERROR:' + (e.message || e)); }}
        }}

        var db = new Database({db_path:?});
        db.exec('CREATE TABLE t (id INTEGER PRIMARY KEY, name TEXT, score REAL, flags INTEGER, data BLOB)');

        // ── Positional: variadic form ──
        var ins = db.prepare('INSERT INTO t (name, score, flags, data) VALUES (?, ?, ?, ?)');
        check('variadic_run', function () {{
            var r = ins.run('alice', 91.5, 1, Buffer.from([1,2,3]));
            return r.changes === 1 && typeof r.lastInsertRowid === 'number';
        }});
        // ── Positional: array form ──
        check('array_run', function () {{
            var r = ins.run(['bob', 80.25, 0, null]);
            return r.changes === 1;
        }});

        // ── Named: implicit sigil inference (the object key without its
        // sigil binds the $-prefixed SQL parameter) ──
        check('named_implicit_get', function () {{
            var row = db.prepare('SELECT name, score FROM t WHERE name = $name').get({{ name: 'alice' }});
            return row && row.name === 'alice' && row.score === 91.5;
        }});
        // ── Named: explicit sigils $x / :x / @x ──
        check('named_explicit_get', function () {{
            var a = db.prepare('SELECT id FROM t WHERE name = $tgt').get({{ $tgt: 'alice' }});
            var b = db.prepare('SELECT id FROM t WHERE name = :tgt').get({{ ':tgt': 'bob' }});
            var c = db.prepare('SELECT id FROM t WHERE name = @tgt').get({{ '@tgt': 'alice' }});
            return a && b && c && a.id === c.id;
        }});
        // ── Named: run with bound values ──
        check('named_run', function () {{
            var r = db.prepare('UPDATE t SET score = score + :bonus WHERE name = :who')
                .run({{ ':bonus': 0.5, ':who': 'bob' }});
            return r.changes === 1;
        }});

        // ── Value fidelity ──
        check('null_value', function () {{
            var row = db.prepare('SELECT data FROM t WHERE name = ?').get('bob');
            return row && row.data === null;
        }});
        check('blob_roundtrip', function () {{
            var row = db.prepare('SELECT data FROM t WHERE name = ?').get('alice');
            if (!row || !row.data) return false;
            var b = row.data;
            return (Buffer.isBuffer(b) || b instanceof Uint8Array)
                && b.length === 3 && b[0] === 1 && b[1] === 2 && b[2] === 3;
        }});
        check('bool_binds_as_int', function () {{
            db.prepare('INSERT INTO t (name, flags) VALUES (?, ?)').run('boolrow', true);
            var row = db.prepare('SELECT flags, typeof(flags) AS ty FROM t WHERE name = ?').get('boolrow');
            return row && row.flags === 1 && row.ty === 'integer';
        }});
        check('float_stays_real', function () {{
            var row = db.prepare('SELECT score, typeof(score) AS ty FROM t WHERE name = ?').get('bob');
            return row && row.score === 80.75 && row.ty === 'real';
        }});

        // ── Prepared statement reuse: rebind different values each call ──
        check('prepared_reuse', function () {{
            var byName = db.prepare('SELECT id FROM t WHERE name = ?');
            var a = byName.get('alice');
            var b = byName.get('bob');
            var a2 = byName.get('alice');
            return a && b && a.id !== b.id && a.id === a2.id;
        }});

        // ── get() with no params / all() ──
        check('get_no_params', function () {{
            var row = db.prepare('SELECT COUNT(*) AS c FROM t').get();
            return row && row.c === 3;
        }});
        check('all_positional', function () {{
            var rows = db.prepare('SELECT name FROM t WHERE score > ? ORDER BY id').all(50);
            return rows.length === 2 && rows[0].name === 'alice' && rows[1].name === 'bob';
        }});

        // ── One-shot db.query / db.run with parameters ──
        check('db_query_params', function () {{
            var rows = db.query('SELECT name FROM t WHERE name = ?', ['alice']);
            return rows.length === 1 && rows[0].name === 'alice';
        }});
        check('db_run_params', function () {{
            var r = db.run('UPDATE t SET flags = ? WHERE name = ?', 7, 'alice');
            return r.changes === 1;
        }});
        check('db_run_change_visible', function () {{
            var row = db.query('SELECT flags FROM t WHERE name = ?', ['alice'])[0];
            return row.flags === 7;
        }});

        // ── Error surfaces (explicit, not silent) ──
        check('too_few_params_throws', function () {{
            try {{ db.prepare('SELECT ? + ?').get(1); return false; }}
            catch (e) {{ return /parameter/i.test(e.message || ''); }}
        }});
        check('missing_named_param_throws', function () {{
            try {{ db.prepare('SELECT $a').get({{ }}); return false; }}
            catch (e) {{ return true; }}
        }});
        check('bad_sql_throws', function () {{
            try {{ db.prepare('SELECT * FROM nosuchtable').all(); return false; }}
            catch (e) {{ return /nosuchtable/i.test(e.message || ''); }}
        }});

        try {{ db.close(); }} catch (e) {{}}
        out.join('|')
        "#,
            db_path = db_path_str,
        ),
    );

    let mut all_passed = true;
    for item in results.split('|') {
        if !item.contains(":PASS") {
            eprintln!("  FAIL: {}", item);
            all_passed = false;
        }
    }
    assert!(
        all_passed,
        "bun:sqlite parameter binding tests should pass. Results: {}",
        results
    );

    // ── Independent cross-check: read the same file through a direct
    // rusqlite connection (the SQLite C library the sqlite3 CLI uses) and
    // verify the JS-written rows persist byte-exact.
    let conn = rusqlite::Connection::open(&db_path).expect("open db for cross-check");
    let (name, score, flags, blob): (String, f64, i64, Option<Vec<u8>>) = conn
        .query_row(
            "SELECT name, score, flags, data FROM t WHERE name = 'alice'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .expect("query alice row");
    assert_eq!(name, "alice");
    assert_eq!(score, 91.5);
    assert_eq!(flags, 7);
    assert_eq!(blob.as_deref(), Some(&[1u8, 2, 3][..]));

    let count: i64 = conn
        .query_row("SELECT COUNT(*) FROM t", [], |row| row.get(0))
        .expect("count rows");
    assert_eq!(count, 3, "alice + bob + boolrow");

    let _ = std::fs::remove_file(&db_path);
    bun_runtime::shutdown_thread_sm();
}

#[test]
fn test_bun_sqlite_in_memory_all_forms() {
    bun_runtime::install_exit_handler();
    bun_runtime::bun_api::init_process_start();
    let mut ctx = JsContext::for_test().expect("Failed to create JSContext");
    ctx.set_global_setup(bun_runtime::globals::install_all);

    let results = eval_string(
        &mut ctx,
        r#"
        var { Database } = require('bun:sqlite');
        var out = [];
        function check(name, fn) {
            try { out.push(name + ':' + (fn() ? 'PASS' : 'FAIL')); }
            catch (e) { out.push(name + ':ERROR:' + (e.message || e)); }
        }
        var db = new Database(':memory:');
        db.exec('CREATE TABLE kv (k TEXT PRIMARY KEY, v INTEGER)');
        var put = db.prepare('INSERT OR REPLACE INTO kv (k, v) VALUES (?, ?)');
        put.run('a', 1); put.run('b', 2); put.run('c', 3);
        check('variadic', function () {
            return db.prepare('SELECT v FROM kv WHERE k = ?').get('b').v === 2;
        });
        check('array', function () {
            return db.prepare('SELECT v FROM kv WHERE k = ?').get(['c']).v === 3;
        });
        check('named_implicit', function () {
            return db.prepare('SELECT v FROM kv WHERE k = $k').get({ k: 'a' }).v === 1;
        });
        check('integer_exact', function () {
            var row = db.prepare('SELECT typeof(v) AS ty FROM kv WHERE k = ?').get('a');
            return row.ty === 'integer';
        });
        check('big_int_binding', function () {
            // 2^32 — outside i32, exactly representable as a JS double.
            db.prepare('INSERT INTO kv (k, v) VALUES (?, ?)').run('big', 4294967296n);
            return db.prepare('SELECT v FROM kv WHERE k = ?').get('big').v === 4294967296;
        });
        try { db.close(); } catch (e) {}
        out.join('|')
        "#,
    );

    let mut all_passed = true;
    for item in results.split('|') {
        if !item.contains(":PASS") {
            eprintln!("  FAIL: {}", item);
            all_passed = false;
        }
    }
    assert!(
        all_passed,
        "in-memory sqlite param tests should pass. Results: {}",
        results
    );
    bun_runtime::shutdown_thread_sm();
}
