// @trace TEST-LIB-4 [req:REQ-LIB-004] [level:integration]
// REQ-LIB-004 criterion 5 discriminating battery — `Bao.run(path, options?)`
// per-script permission sandbox (E48v2 delivery, behavior face).
//
// Truth sources (source-anchored, not inferred):
//   - registration      bun_api.rs:604-614 (Bun.run; `Bao` alias :65-71 is the
//                       SAME object → Bao.run === Bun.run)
//   - bao_run native    bun_api.rs:11619-11770: options parse (11648-11717,
//                       fail-closed) → script read under the OUTER guard
//                       (11719-11732, nested-run semantics) → scope install
//                       AFTER load (11734-11738) → Evaluate2 completion value
//                       passthrough (11760-11769)
//   - options parser    permission_options.rs:191-305: absent-key defaults
//                       materialize deny-all/false (287-304); unknown key →
//                       ERR_INVALID_ARG_VALUE (159-170 + 283-285); list-key
//                       type errors → ERR_INVALID_ARG_TYPE with the node v24
//                       `Received ...` descriptor (95-129, 311-385)
//   - guard bridge      permission_bridge.rs:21-38: string-prefix allow list,
//                       denial message verbatim "Permission denied: read on
//                       <path>"; stack install/Drop-restore 425-450
//   - in-script gate    node_fs.rs fs_read_file_sync :2498 (readFileSync
//                       consults the same bridge check)
//
// Method: real engine (common::setup_ctx → install_all) + real script files
// in per-test tempdirs (fs_watch_events scratch idiom). No test doubles —
// every group drives the real parser/guard/evaluator through Bao.run. Error
// assertions run on the in-script `e.message` of JS_ReportErrorUTF8-created
// Errors, which is exactly the reported text (suite precedent: c12
// "THREW:Unknown encoding: ucs-2be" exact match); coded props (code/name/
// path) are asserted alongside so a message-only coincidence cannot pass.

use bao_engine::context::JsContext;
#[path = "common/mod.rs"]
mod common;

use common::eval_string_full as eval;

fn make_ctx() -> JsContext {
    common::setup_ctx()
}

/// Per-process unique scratch dir (fs_watch_events idiom): two invocations of
/// this binary must not share or delete each other's fixtures.
fn scratch_dir(name: &str) -> std::path::PathBuf {
    std::env::temp_dir().join(format!("bao_run_perm_{}_{}", name, std::process::id()))
}

fn write_file(path: &std::path::Path, contents: &str) -> std::path::PathBuf {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).expect("create tmp parent dir");
    }
    std::fs::write(path, contents).expect("write tmp fixture file");
    path.to_path_buf()
}

/// Embed an absolute path into a JS double-quoted literal.
fn js_path(p: &std::path::Path) -> String {
    p.to_string_lossy()
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
}

fn cleanup(dir: &std::path::Path) {
    let _ = std::fs::remove_dir_all(dir);
}

// ─── Group 1: no options — baseline passthrough (criterion ①) ──────────────

#[test]
fn lib4_run_no_options_baseline_passthrough() {
    let dir = scratch_dir("g1_baseline");
    let _ = std::fs::remove_dir_all(&dir);
    let script = write_file(&dir.join("ok.js"), "\"baorun-ok-\" + (40 + 2)");

    let mut ctx = make_ctx();
    let report = eval(
        &mut ctx,
        &format!(
            "typeof Bao.run + \"|\" + Bao.run(\"{}\") + \"|\" + (Bao.run === Bun.run)",
            js_path(&script)
        ),
    );

    assert_eq!(
        report, "function|baorun-ok-42|true",
        "no-options Bao.run must execute the script and pass the completion value through \
         unchanged (criterion ① baseline, guard untouched); Bao and Bun share one object so \
         Bao.run IS Bun.run"
    );

    cleanup(&dir);
}

// ─── Group 2: granted directory read succeeds ───────────────────────────────

#[test]
fn lib4_run_granted_read_dir_allows_listed_prefix() {
    let dir = scratch_dir("g2_grant");
    let _ = std::fs::remove_dir_all(&dir);
    let data = write_file(&dir.join("data").join("secret.txt"), "granted-content");
    let script = write_file(
        &dir.join("reader.js"),
        &format!(
            "var fs = require('fs');\nfs.readFileSync(\"{}\", \"utf8\")",
            js_path(&data)
        ),
    );

    let mut ctx = make_ctx();
    let report = eval(
        &mut ctx,
        &format!(
            r#"Bao.run("{}", {{permission: {{read: ["{}"]}}}})"#,
            js_path(&script),
            js_path(&dir)
        ),
    );

    assert_eq!(
        report, "granted-content",
        "a read grant on the containing directory must let the sandboxed script read files \
         under that prefix (bridge string-prefix match, permission_bridge.rs:27-30)"
    );

    cleanup(&dir);
}

// ─── Group 3: read outside the grant list is denied ─────────────────────────

#[test]
fn lib4_run_unlisted_read_denied_with_bridge_message() {
    let dir = scratch_dir("g3_deny");
    let _ = std::fs::remove_dir_all(&dir);
    let granted_dir = dir.join("granted");
    let _granted = write_file(&granted_dir.join("ok.txt"), "g");
    let target = write_file(&dir.join("target").join("secret.txt"), "s");
    let script = write_file(
        &dir.join("reader.js"),
        &format!(
            "var out;\ntry {{ require('fs').readFileSync(\"{t}\", \"utf8\"); out = 'READ-ALLOWED'; }}\ncatch (e) {{ out = 'DENIED:' + e.message + '|code=' + e.code; }}\nout",
            t = js_path(&target)
        ),
    );

    let mut ctx = make_ctx();
    let report = eval(
        &mut ctx,
        &format!(
            r#"Bao.run("{}", {{permission: {{read: ["{}"]}}}})"#,
            js_path(&script),
            js_path(&granted_dir)
        ),
    );

    assert_eq!(
        report,
        format!(
            "DENIED:Permission denied: read on {}|code=undefined",
            target.to_string_lossy()
        ),
        "a read outside the grant list must be denied with the verbatim bridge denial message \
         and NO errno code — the bridge-denial shape (permission_bridge.rs:33), distinct from \
         a coded fs error like ENOENT"
    );

    cleanup(&dir);
}

// ─── Group 4: absent keys default to deny-all ───────────────────────────────

#[test]
fn lib4_run_empty_permission_object_default_denies_read() {
    let dir = scratch_dir("g4_default_deny");
    let _ = std::fs::remove_dir_all(&dir);
    let data = write_file(&dir.join("data.txt"), "d");
    let script = write_file(
        &dir.join("reader.js"),
        &format!(
            "var out;\ntry {{ require('fs').readFileSync(\"{t}\", \"utf8\"); out = 'READ-ALLOWED'; }}\ncatch (e) {{ out = 'DENIED:' + e.message; }}\nout",
            t = js_path(&data)
        ),
    );

    let mut ctx = make_ctx();
    let report = eval(
        &mut ctx,
        &format!(r#"Bao.run("{}", {{permission: {{}}}})"#, js_path(&script)),
    );

    assert_eq!(
        report,
        format!(
            "DENIED:Permission denied: read on {}",
            data.to_string_lossy()
        ),
        "absent keys must default to DENY (script-level minimal privilege, \
         permission_options.rs:287-304): an existing, otherwise-readable file is denied under \
         {{permission:{{}}}} while the script itself still executes (report returned, not a \
         Bao.run-level failure)"
    );

    cleanup(&dir);
}

// ─── Group 5: options parse failures fail closed ────────────────────────────

#[test]
fn lib4_run_permission_parse_failures_fail_closed() {
    let dir = scratch_dir("g5_parse");
    let _ = std::fs::remove_dir_all(&dir);
    let script = write_file(&dir.join("noop.js"), "\"noop\"");

    let mut ctx = make_ctx();
    let report = eval(
        &mut ctx,
        &format!(
            "var out = [];\ntry {{ Bao.run(\"{s}\", {{permission: {{read: 'x'}}}}); out.push('ACCEPTED'); }}\ncatch (e) {{ out.push(e.name + ':' + e.code + ':' + e.message); }}\ntry {{ Bao.run(\"{s}\", {{permission: {{sys: []}}}}); out.push('ACCEPTED'); }}\ncatch (e) {{ out.push(e.name + ':' + e.code + ':' + e.message); }}\nout.join('|')",
            s = js_path(&script)
        ),
    );

    // Expected strings are oracle-verbatim from permission_options.rs
    // (arg_type :67-78 + parse_list_key :328-334; unknown_key_error :159-170)
    // and throw_run_coded_type_error's name/code props (bun_api.rs:11522).
    // concat! avoids string-continuation whitespace traps.
    let expected_read_type_err = concat!(
        "TypeError:ERR_INVALID_ARG_TYPE:",
        "The \"options.permission.read\" argument must be of type boolean or an array of ",
        "strings. Received type string (\"x\")"
    );
    let expected_unknown_key_err = concat!(
        "TypeError:ERR_INVALID_ARG_VALUE:",
        "The \"options.permission\" object has an unknown property \"sys\". ",
        "Recognized keys: read, write, net, env, run. ",
        "The \"sys\" class is enforced only on browser pages (per-Page permission); ",
        "the script-level sandbox has no sys enforcement bridge."
    );

    assert_eq!(
        report,
        format!("{}|{}", expected_read_type_err, expected_unknown_key_err),
        "non-array read value → ERR_INVALID_ARG_TYPE TypeError with the node Received \
         descriptor (parse_list_key :329-348); the sys key is rejected outright \
         ERR_INVALID_ARG_VALUE (unknown_key_error :159-170) — neither may be silently coerced \
         into a grant"
    );

    cleanup(&dir);
}

// ─── Group 6: missing script file → coded ENOENT ────────────────────────────

#[test]
fn lib4_run_missing_script_enoent_coded() {
    let dir = scratch_dir("g6_enoent");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("create tmp dir");
    let missing = dir.join("no_such_script.js");

    let mut ctx = make_ctx();
    let report = eval(
        &mut ctx,
        &format!(
            "var out;\ntry {{ Bao.run(\"{p}\"); out = 'ACCEPTED'; }}\ncatch (e) {{ out = e.code + '|' + e.path + '|' + e.message; }}\nout",
            p = js_path(&missing)
        ),
    );

    let prefix = format!(
        "ENOENT|{}|Bao.run '{}': ",
        missing.display(),
        missing.display()
    );
    assert!(
        report.starts_with(&prefix),
        "a missing script must throw with code=ENOENT AND the path property set \
         (throw_run_fs_error bun_api.rs:11566-11616); got: {}",
        report
    );
    assert!(
        report.contains("No such file"),
        "ENOENT message must carry the OS not-found text; got: {}",
        report
    );

    cleanup(&dir);
}

// ─── Group 7: guard restores the true base state after the scope ────────────

#[test]
fn lib4_run_guard_restores_base_state_after_scope() {
    let dir = scratch_dir("g7_restore");
    let _ = std::fs::remove_dir_all(&dir);
    let allowed = write_file(&dir.join("sandboxed").join("allowed.txt"), "allowed-content");
    let outside = write_file(&dir.join("outside.txt"), "outside-content");
    let script = write_file(
        &dir.join("reader.js"),
        &format!(
            "var out;\ntry {{ require('fs').readFileSync(\"{o}\", \"utf8\"); out = 'OUTSIDE-ALLOWED'; }}\ncatch (e) {{ out = 'OUTSIDE-DENIED'; }}\nrequire('fs').readFileSync(\"{a}\", \"utf8\") + '|' + out",
            o = js_path(&outside),
            a = js_path(&allowed)
        ),
    );

    let mut ctx = make_ctx();
    let report = eval(
        &mut ctx,
        &format!(
            "var r = Bao.run(\"{s}\", {{permission: {{read: [\"{d}\"]}}}});\nr + '|after=' + require('fs').readFileSync(\"{o}\", \"utf8\")",
            s = js_path(&script),
            d = js_path(&dir.join("sandboxed")),
            o = js_path(&outside)
        ),
    );

    assert_eq!(
        report, "allowed-content|OUTSIDE-DENIED|after=outside-content",
        "during the sandbox the ungranted read is denied; after Bao.run returns the SAME read \
         in the SAME context succeeds — the guard stack emptied back to the true base state \
         (ScriptPermissionScope Drop restore, permission_options.rs:441-450), not a leftover \
         or cleared-to-allow-all guard"
    );

    cleanup(&dir);
}

// ─── Group 8: nested Bao.run — inner loads under the OUTER guard ────────────

#[test]
fn lib4_run_nested_inner_runs_under_outer_guard() {
    let dir = scratch_dir("g8_nested");
    let _ = std::fs::remove_dir_all(&dir);
    // OUTSIDE the granted directory: the outer permission grants `read` on
    // `dir` only, so this file must stay denied for the whole nested run.
    // (First draft placed it inside `dir` — the grant covered it and the
    // DENIED expectation could never hold.)
    let outside = write_file(
        &std::env::temp_dir().join(format!("bao_run_perm_g8_outside_{}.txt", std::process::id())),
        "outside-content",
    );
    let inner = write_file(&dir.join("inner.js"), "\"inner-ran-\" + (6 * 7)");
    let outer = write_file(
        &dir.join("outer.js"),
        &format!(
            "var v = Bao.run(\"{i}\");\nvar out;\ntry {{ require('fs').readFileSync(\"{o}\", \"utf8\"); out = 'OUTSIDE-ALLOWED'; }}\ncatch (e) {{ out = 'OUTSIDE-DENIED'; }}\n'inner=' + v + '|outside=' + out",
            i = js_path(&inner),
            o = js_path(&outside)
        ),
    );

    let mut ctx = make_ctx();
    let report = eval(
        &mut ctx,
        &format!(
            r#"Bao.run("{}", {{permission: {{read: ["{}"]}}}})"#,
            js_path(&outer),
            js_path(&dir)
        ),
    );

    assert_eq!(
        report, "inner=inner-ran-42|outside=OUTSIDE-DENIED",
        "nested semantics (E48v2 ruling 3, bun_api.rs:11649-11655): the inner script's file \
         read is gated by the OUTER guard (grant covers it → loads and runs, completion value \
         flows back through the nested frame), the inner no-options run installs nothing, and \
         the outer guard stays authoritative for the whole nested execution (ungranted read \
         still denied inside outer.js after the inner run returned)"
    );

    cleanup(&dir);
}
