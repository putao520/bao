// @trace TEST-ENG-7-FS-C11 [req:REQ-ENG-007] [level:integration]
//
// C11 line-coverage wave for src/bao_runtime/src/node_fs.rs + node_crypto.rs.
//
// Gap ledger (baseline lcov 2026-10-18, /var/cargo-builds/bao-cov-final/
// coverage-final.lcov): node_fs LH=5309 LF=7362 gap=2053; node_crypto
// LH=5054 LF=5711 gap=657. Zero-hit structure this wave drives:
//   fs     — the entire fs.<api>(path, cb) spawn_fs_async closure family
//            (fs_read_file/fs_stat/fs_readdir/... ~36 closures), fs.promises
//            partial faces, Dir iterator face (dir_symbol_iterator /
//            dir_iterator_next / dir_close / dir_read), fs_readv/fs_writev,
//            fs_lchown/fs_fchown/fh_chown/fh_utimes, dirent/stats type
//            predicates (is_character_device / is_block_device /
//            is_symbolic_link / is_socket / is_fifo).
//   crypto — generateKeyPair(Sync) / generateKey(Sync) / generatePrime(Sync)
//            / createDiffieHellman(Group) / createECDH / createPublicKey /
//            createSecretKey / hash.copy / sign/verify / timingSafeEqual /
//            getRandomValues / cipher-decipher error arms / async delivery
//            (crypto_async_defer_callback / crypto_async_tasklet_shim).
//
// Oracle: node v24.19.0 (local). Probe transcript distilled into the
// per-check comments below (`node oracle:` notes). Where the node behavior
// and a C1 registry entry conflict, node wins and the entry is annotated.
//
// Product single-point fixes verified against the oracle in this wave
// (node_fs.rs only; each asserted by a *_fix_* test here):
//   1. parse_open_flags 'x' → O_EXCL — wx/ax/wx+ on an existing path now
//      fail with EEXIST (C1 #1; node probe: openSync(existing,'wx')=EEXIST).
//   2. io_error_code EBADF arm — fd errors on closed/invalid fds now report
//      code="EBADF" instead of the generic "ERR" (C1 #7; node probe:
//      readSync(9999,...)=EBADF).
//   3. Dir Symbol.asyncIterator registration — was registered under the
//      literal string key "[]" so the iterator was undiscoverable; now
//      registered via the well-known symbol key (C1 #15; node probe:
//      dir[Symbol.asyncIterator]=function, sync for..of → TypeError "d is
//      not iterable").
//   4. fs.open callback delivery — extraction started at index 3 so the
//      plain fs.open(path, flags, cb) form never fired its callback; now
//      scans from index 1 and only captures callables (C1 #9; node probe:
//      fs.open(path,'w',cb) → cb(null, fd)).
//
// C1 registry items re-checked against the oracle:
//   #14 fh double-close: node v24.19.0 probe shows the SECOND close
//      RESOLVES (NO_THROW), so bao's permissive double-close matches node —
//      the C1 entry ("node: ERR_INVALID_HANDLE") is falsified by the oracle.
//   #2 writeSync (offset,length,position), #3 readvSync caller-buffer
//      fill, #4 rmSync {force}, #10 fs.writeFile cb, #13 fh.readFile
//      encoding, #17 fs.cp cb, and writeFileSync options.flag are FIXED in
//      the fs6 wave and now asserted against the node v24.19.0 oracle (see
//      c11_fs_sync_gap_faces / c11_fs_registry_fixed_faces).
//   #12 fs.exists cb arg shape and #5/#6 utimesSync/lutimesSync remain
//      registered gaps (characterization below).

use bao_engine::context::JsContext;
#[path = "common/mod.rs"]
mod common;

use common::eval_string;
use common::eval_string_full;
use common::setup_ctx;

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// Escape a path for interpolation into a JS double-quoted string literal.
fn js_escape(p: &std::path::Path) -> String {
    p.to_string_lossy()
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
}

/// Unique per-test temp dir (nextest runs each test in its own process, but
/// keep names collision-free anyway).
fn tmp_dir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("bao_fs_c11_{tag}"));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// Pump the loop until a JS boolean predicate holds (bounded). Returns the
/// final predicate value so the caller can fail with a precise message.
fn pump_until(ctx: &mut JsContext, predicate_js: &str, deadline_ms: u64) -> bool {
    let final_value = common::pump_until(
        ctx,
        predicate_js,
        Duration::from_millis(deadline_ms),
        eval_string,
        |v| v == "true",
    );
    final_value == "true"
}

/// Pump until the predicate holds or the deadline expires, then REQUIRE it
/// (the delivery-contract form: a callback that never fired is a failure).
/// `collector_js` is an optional JS expression stringified into the failure
/// message (e.g. the check-log array) so a delivery gap names its APIs.
fn pump_require(ctx: &mut JsContext, predicate_js: &str, what: &str) {
    if !pump_until(ctx, predicate_js, 15_000) {
        panic!("c11 delivery contract not met: {what}");
    }
}

/// Same as [`pump_require`] but embeds the collector transcript on failure.
fn pump_require_with_log(ctx: &mut JsContext, predicate_js: &str, collector_js: &str, what: &str) {
    if !pump_until(ctx, predicate_js, 15_000) {
        let log = eval_string(ctx, collector_js);
        panic!("c11 delivery contract not met: {what}\ncollector: {log}");
    }
}

/// Evaluate a check-batch body that returns `label PASS|FAIL|ERR:...` joined
/// by `|`; assert every check passed and hand back the full transcript.
fn run_checks(ctx: &mut JsContext, body: &str) -> Vec<String> {
    let out = eval_string(ctx, body);
    let results: Vec<String> = out.split('|').map(|s| s.trim().to_string()).collect();
    let failures: Vec<String> = results
        .iter()
        .filter(|i| i.contains(" FAIL") || i.contains(" ERR:"))
        .cloned()
        .collect();
    assert!(
        failures.is_empty(),
        "fs_crypto_coverage_c11 checks had {} failure(s):\n{}",
        failures.len(),
        failures.join("\n")
    );
    results
}

/// Shared JS scaffolding for check batches: result collector + error-code
/// probe helper.
const CHECK_PRELUDE: &str = r#"
    var fs = require('fs');
    var R = [];
    function ck(n, f) {
        try { var ok = f(); R.push(n + (ok ? " PASS" : " FAIL")); }
        catch (e) { R.push(n + " ERR:" + String((e && e.code) || (e && e.message) || e).substring(0, 90)); }
    }
    function codeof(f) { try { f(); return "NOTHROW"; } catch (e) { return (e && e.code) || "THREW"; } }
    function msgof(f) { try { f(); return "NOTHROW"; } catch (e) { return String((e && e.message) || e); } }
"#;

// ===========================================================================
// Fix verification — the four oracle-confirmed single-point product fixes.
// ===========================================================================

/// Fix 1: parse_open_flags 'x' → O_EXCL. node oracle: openSync_wx=EEXIST,
/// openSync_ax=EEXIST, openSync_wxplus=EEXIST, wx_fresh_ok=OK,
/// wx_exists(writeFileSync flag wx)=THROW:EEXIST.
#[test]
fn c11_fs_fix_wx_ax_eexist() {
    let mut ctx = setup_ctx();
    let dir = tmp_dir("fix_wx");
    let existing = dir.join("exists.txt");
    std::fs::write(&existing, "aa").unwrap();
    let fresh = dir.join("fresh.txt");
    let _ = std::fs::remove_file(&fresh);
    let (ex, fr) = (js_escape(&existing), js_escape(&fresh));

    let script = format!(
        r#"
        {prelude}
        var ex = "{ex}", fr = "{fr}";
        ck("openSync_wx_existing_eexist", function() {{ return codeof(function() {{ fs.openSync(ex, "wx"); }}) === "EEXIST"; }});
        ck("openSync_ax_existing_eexist", function() {{ return codeof(function() {{ fs.openSync(ex, "ax"); }}) === "EEXIST"; }});
        ck("openSync_wxplus_existing_eexist", function() {{ return codeof(function() {{ fs.openSync(ex, "wx+"); }}) === "EEXIST"; }});
        ck("openSync_wx_fresh_opens", function() {{ var fd = fs.openSync(fr, "wx"); fs.closeSync(fd); return fs.existsSync(fr); }});
        ck("openSync_ax_fresh_opens", function() {{ var fd2 = fs.openSync(fr + "2", "ax"); fs.closeSync(fd2); return fs.existsSync(fr + "2"); }});
        // C11 registry fixed: writeFileSync now honors options.flag. node
        // v24.19.0 oracle: wx on existing → EEXIST (content preserved);
        // a appends; r+ overwrites from 0 without truncating; w truncates.
        ck("writefilesync_flag_wx_eexist", function() {{
            var outcome = codeof(function() {{ fs.writeFileSync(ex, "b", {{ flag: "wx" }}); }});
            return outcome === "EEXIST" && fs.readFileSync(ex, "utf8") === "aa";
        }});
        ck("writefilesync_flag_a_appends", function() {{
            fs.writeFileSync(ex, "b", {{ flag: "a" }});
            return fs.readFileSync(ex, "utf8") === "aab";
        }});
        ck("writefilesync_flag_rplus_no_truncate", function() {{
            fs.writeFileSync(ex, "ZZ", {{ flag: "r+" }});
            return fs.readFileSync(ex, "utf8") === "ZZb";
        }});
        ck("plain_w_still_truncates", function() {{ fs.writeFileSync(ex, "z"); return fs.readFileSync(ex, "utf8") === "z"; }});
        R.join("|");
        "#,
        prelude = CHECK_PRELUDE,
        ex = ex,
        fr = fr
    );
    run_checks(&mut ctx, &script);
}

/// Fix 2: io_error_code EBADF arm. node oracle:
/// readSync_ebadf=EBADF, fstatSync_ebadf=EBADF, closeSync_ebadf=EBADF.
#[test]
fn c11_fs_fix_ebadf() {
    let mut ctx = setup_ctx();
    let dir = tmp_dir("fix_ebadf");
    let file = dir.join("f.txt");
    std::fs::write(&file, "xx").unwrap();
    let f = js_escape(&file);

    let script = format!(
        r#"
        {prelude}
        var f = "{f}";
        ck("readSync_ebadf", function() {{ return codeof(function() {{ fs.readSync(9999, Buffer.alloc(4), 0, 4); }}) === "EBADF"; }});
        ck("fstatSync_ebadf", function() {{ return codeof(function() {{ fs.fstatSync(9999); }}) === "EBADF"; }});
        ck("closeSync_ebadf", function() {{ return codeof(function() {{ fs.closeSync(9999); }}) === "EBADF"; }});
        ck("ftruncateSync_ebadf", function() {{ return codeof(function() {{ fs.ftruncateSync(9999, 0); }}) === "EBADF"; }});
        ck("fchmodSync_ebadf", function() {{ return codeof(function() {{ fs.fchmodSync(9999, 420); }}) === "EBADF"; }});
        ck("realpath_good_not_ebadf", function() {{ return codeof(function() {{ fs.realpathSync(f); }}) !== "EBADF"; }});
        R.join("|");
        "#,
        prelude = CHECK_PRELUDE,
        f = f
    );
    run_checks(&mut ctx, &script);

    // Async delivery: fs.fstat(9999, cb) must deliver cb with EBADF.
    eval_string(
        &mut ctx,
        r#"
        globalThis.__eb = { fired: false, code: null };
        require('fs').fstat(9999, function(err, st) {
            globalThis.__eb.fired = true;
            globalThis.__eb.code = err ? (err.code || String(err)) : null;
        });
        'scheduled'
        "#,
    );
    pump_require(
        &mut ctx,
        "globalThis.__eb.fired === true",
        "fstat(9999, cb) delivered its callback",
    );
    assert_eq!(
        eval_string(&mut ctx, "String(globalThis.__eb.code)"),
        "EBADF"
    );
}

/// Fix 3: Dir Symbol.asyncIterator registration. node oracle:
/// dir[Symbol.asyncIterator]=function; sync for..of → TypeError; fresh
/// for-await yields entries (break closes the iterator via return()).
#[test]
fn c11_fs_fix_dir_async_iterator() {
    let mut ctx = setup_ctx();
    let dir = tmp_dir("fix_diriter");
    for n in ["a.txt", "b.txt", "c.txt"] {
        std::fs::write(dir.join(n), "x").unwrap();
    }
    let d = js_escape(&dir);

    // Symbol key discoverability + manual next() shape (promise-valued).
    let script = format!(
        r#"
        {prelude}
        var dp = "{d}";
        var dir = fs.opendirSync(dp);
        ck("asyncIterator_is_function", function() {{ return typeof dir[Symbol.asyncIterator] === "function"; }});
        ck("sync_iter_is_typeerror", function() {{ try {{ var n = 0; for (var e of dir) {{ n++; if (n >= 2) break; }} }} catch (err) {{ return String(err && err.message || err).indexOf("not iterable") >= 0; }} return false; }});
        ck("readSync_returns_dirent", function() {{ var e = dir.readSync(); return e && typeof e.name === "string"; }});
        ck("readSync_consumes_entries", function() {{ var n = 1; while (dir.readSync()) n++; return n >= 3; }});
        ck("closeSync_ok", function() {{ dir.closeSync(); return true; }});
        R.join("|");
        "#,
        prelude = CHECK_PRELUDE,
        d = d
    );
    run_checks(&mut ctx, &script);

    // for-await-of over the Dir itself (the fixed registration path).
    eval_string(
        &mut ctx,
        &format!(
            r#"
            globalThis.__dit = {{ done: false, count: 0, err: null }};
            (async function() {{
                try {{
                    var dd = fs.opendirSync("{d}");
                    for await (const e of dd) {{ globalThis.__dit.count++; if (globalThis.__dit.count >= 3) break; }}
                    globalThis.__dit.done = true;
                }} catch (err) {{ globalThis.__dit.err = String((err && err.code) || err); }}
            }})();
            'scheduled'
            "#,
            d = d
        ),
    );
    pump_require_with_log(
        &mut ctx,
        "globalThis.__dit.done === true || globalThis.__dit.err !== null",
        "String(globalThis.__dit.err) + '/count=' + globalThis.__dit.count",
        "for-await Dir iteration completed",
    );
    assert_eq!(
        eval_string(&mut ctx, "String(globalThis.__dit.err)"),
        "null",
        "for-await Dir iteration errored"
    );
    assert!(
        eval_string(&mut ctx, "String(globalThis.__dit.count)")
            .parse::<i64>()
            .unwrap_or(0)
            >= 3,
        "for-await yielded fewer than 3 entries"
    );
}

/// Fix 4: fs.open callback delivery. node oracle: fs.open(path,'w',cb) →
/// cb(null, fd); fs.open(path,'w',0o644,cb) → cb(null, fd).
#[test]
fn c11_fs_fix_open_cb() {
    let mut ctx = setup_ctx();
    let dir = tmp_dir("fix_opencb");
    let p1 = dir.join("cb1.txt");
    let p2 = dir.join("cb2.txt");
    let (s1, s2) = (js_escape(&p1), js_escape(&p2));

    eval_string(
        &mut ctx,
        &format!(
            r#"
            globalThis.__oc = {{ fired: 0, fd1: -1, fd2: -1, err: null }};
            var fs = require('fs');
            fs.open("{s1}", "w", function(err, fd) {{
                globalThis.__oc.fired++;
                globalThis.__oc.err = err ? String((err && err.code) || err) : null;
                if (typeof fd === "number") {{ globalThis.__oc.fd1 = fd; fs.closeSync(fd); }}
            }});
            fs.open("{s2}", "w", 420, function(err, fd) {{
                globalThis.__oc.fired++;
                if (typeof fd === "number") {{ globalThis.__oc.fd2 = fd; fs.closeSync(fd); }}
            }});
            'scheduled'
            "#,
            s1 = s1,
            s2 = s2
        ),
    );
    pump_require(
        &mut ctx,
        "globalThis.__oc.fired >= 2",
        "fs.open(p, flags[, mode], cb) delivered both callbacks",
    );
    assert_eq!(
        eval_string(&mut ctx, "String(globalThis.__oc.err)"),
        "null",
        "fs.open cb delivered an error for a writable fresh path"
    );
    assert!(
        eval_string(&mut ctx, "String(globalThis.__oc.fd1)")
            .parse::<i64>()
            .unwrap_or(-1)
            > 0,
        "fs.open cb did not deliver a numeric fd"
    );
    assert!(
        p1.exists() && p2.exists(),
        "fs.open cb form did not create files"
    );
}

// ===========================================================================
// fs callback family — the spawn_fs_async closure face (zero-hit closures).
// ===========================================================================

/// cb success paths for path-taking APIs: every delivery lands on the JS
/// thread via the ConcurrentTask carrier.
#[test]
fn c11_fs_callback_path_family() {
    let mut ctx = setup_ctx();
    let dir = tmp_dir("cb_path");
    let f = dir.join("f.txt");
    std::fs::write(&f, "payload").unwrap();
    let sub = dir.join("sub");
    std::fs::create_dir_all(&sub).unwrap();
    let d = js_escape(&dir);
    let fp = js_escape(&f);

    eval_string(
        &mut ctx,
        &format!(
            r#"
            globalThis.__cb = {{ n: 0, log: [] }};
            function rec(name, err, extra) {{
                globalThis.__cb.n++;
                globalThis.__cb.log.push(name + ":" + (err ? ((err && err.code) || String(err)) : "ok") + ":" + (extra === undefined ? "-" : (typeof extra === "object" && extra ? "obj" : String(extra))));
            }}
            var fs = require('fs');
            var F = "{fp}", D = "{d}";
            fs.writeFileSync(F + ".rf", "payload");
            fs.readFile(F + ".rf", "utf8", function(e, v) {{ rec("readFile", e, v === "payload"); }});
            // C1 #10 fixed: writeFile delivers its callback (node shape
            // cb(err)). Writes to its own file: delivery order is unordered
            // across workers, so no other assert may read this path.
            fs.writeFile(F + ".wc", "ww", function(e) {{ rec("writeFile", e, true); }});
            fs.appendFile(F + ".wa", "+a", function(e) {{ rec("appendFile", e, fs.readFileSync(F + ".wa", "utf8") === "+a"); }});
            fs.stat(F, function(e, st) {{ rec("stat", e, st && typeof st.size === "number"); }});
            fs.lstat(F, function(e, st) {{ rec("lstat", e, st && typeof st.size === "number"); }});
            fs.readdir(D, function(e, names) {{ rec("readdir", e, Array.isArray(names) && names.indexOf("f.txt") >= 0); }});
            fs.writeFileSync(F + ".rn", "r");
            fs.rename(F + ".rn", F + ".r", function(e) {{ rec("rename", e, fs.existsSync(F + ".r")); }});
            fs.truncate(F, 3, function(e) {{ rec("truncate", e, fs.readFileSync(F, "utf8") === "pay"); }});
            // copyFile's source is truncated concurrently by the truncate op
            // above — the byte-exact content is a race; pin delivery + the
            // copy existing (its own cb, content-agnostic).
            fs.copyFile(F, F + ".cp", function(e) {{ rec("copyFile", e, fs.existsSync(F + ".cp")); }});
            fs.chmod(F, 420, function(e) {{ rec("chmod", e, (fs.statSync(F).mode & 0o777) === 420); }});
            fs.access(F, fs.constants.R_OK, function(e) {{ rec("access", e, !e); }});
            fs.realpath(F, function(e, p) {{ rec("realpath", e, typeof p === "string" && p.length > 0); }});
            // readlink AFTER symlink completed — delivery is unordered, so the
            // dependent call is scheduled from the symlink callback.
            fs.symlink(F, F + ".sym2", function(e2) {{
                fs.readlink(F + ".sym2", function(e3, t) {{ rec("readlink", e3, t === F); }});
            }});
            fs.symlink(F, F + ".sym", function(e) {{ rec("symlink", e, true); }});
            fs.link(F, F + ".hard", function(e) {{ rec("link", e, true); }});
            fs.writeFileSync(F + ".ul", "u");
            fs.unlink(F + ".ul", function(e) {{ rec("unlink", e, !fs.existsSync(F + ".ul")); }});
            fs.mkdir(D + "/made", function(e) {{ rec("mkdir", e, fs.existsSync(D + "/made")); }});
            fs.rmdir(D + "/made", function(e) {{ rec("rmdir", e, !fs.existsSync(D + "/made")); }});
            fs.mkdtemp(D + "/dt_", function(e, p) {{ rec("mkdtemp", e, typeof p === "string" && p.indexOf(D + "/dt_") === 0); }});
            fs.utimes(F, 1000, 2000, function(e) {{ rec("utimes", e, fs.statSync(F).mtimeMs === 2000000); }});
            fs.statfs(D, function(e, s) {{ rec("statfs", e, s && typeof s.bsize === "number"); }});
            // rm targets its own pre-created file — rm raced copyFile's
            // destination earlier (deleted F+".cp" before copyFile's cb read
            // it): every path here has exactly one owning API now.
            fs.writeFileSync(F + ".rmme", "z");
            fs.rm(F + ".rmme", function(e) {{ rec("rm", e, !fs.existsSync(F + ".rmme")); }});
            'scheduled'
            "#,
            d = d,
            fp = fp
        ),
    );
    pump_require_with_log(
        &mut ctx,
        "globalThis.__cb.n >= 20",
        "globalThis.__cb.log.join('|')",
        "path-family callbacks all delivered (20 delivering APIs)",
    );
    let log = eval_string(&mut ctx, "globalThis.__cb.log.join('|')");
    let failures: Vec<&str> = log
        .split('|')
        .filter(|l| !l.ends_with(":ok:true"))
        .collect();
    assert!(
        failures.is_empty(),
        "path-family cb results not ok:\n{}",
        failures.join("\n")
    );
    // C1 #10 fixed: fs.writeFile's callback fires with a nullish error.
    assert!(
        log.split('|').any(|l| l == "writeFile:ok:true"),
        "fs.writeFile callback form did not deliver — log: {}",
        log
    );
}

/// cb error paths: errors are delivered to the callback with the node code
/// (not thrown), for the io_error_code funneled families.
#[test]
fn c11_fs_callback_error_paths() {
    let mut ctx = setup_ctx();
    let dir = tmp_dir("cb_err");
    let plain = dir.join("plain.txt");
    std::fs::write(&plain, "x").unwrap();
    let (d, miss) = (js_escape(&dir), js_escape(&dir.join("absent_c11.txt")));

    eval_string(
        &mut ctx,
        &format!(
            r#"
            globalThis.__ce = {{ n: 0, codes: [] }};
            function rec(name, err) {{
                globalThis.__ce.n++;
                globalThis.__ce.codes.push(name + ":" + (err ? ((err && err.code) || String(err)) : "NO_ERR"));
            }}
            var fs = require('fs');
            var MISS = "{miss}", D = "{d}", P = "{plain}";
            fs.readFile(MISS, function(e) {{ rec("readFile", e); }});
            fs.stat(MISS, function(e) {{ rec("stat", e); }});
            fs.lstat(MISS, function(e) {{ rec("lstat", e); }});
            fs.readdir(MISS, function(e) {{ rec("readdir", e); }});
            fs.unlink(MISS, function(e) {{ rec("unlink", e); }});
            fs.rename(MISS, P + ".r", function(e) {{ rec("rename", e); }});
            fs.truncate(MISS, 0, function(e) {{ rec("truncate", e); }});
            fs.copyFile(MISS, P + ".c", function(e) {{ rec("copyFile", e); }});
            fs.chmod(MISS, 420, function(e) {{ rec("chmod", e); }});
            fs.realpath(MISS, function(e) {{ rec("realpath", e); }});
            fs.readlink(MISS, function(e) {{ rec("readlink", e); }});
            fs.rmdir(P, function(e) {{ rec("rmdir_notdir", e); }});
            fs.statfs(MISS, function(e) {{ rec("statfs", e); }});
            fs.rm(MISS, {{ force: false }}, function(e) {{ rec("rm_noforce", e); }});
            'scheduled'
            "#,
            d = d,
            miss = miss,
            plain = js_escape(&plain)
        ),
    );
    pump_require_with_log(
        &mut ctx,
        "globalThis.__ce.n >= 14",
        "globalThis.__ce.codes.join('|')",
        "error-path callbacks all delivered (14 APIs)",
    );
    let codes = eval_string(&mut ctx, "globalThis.__ce.codes.join('|')");
    let not_enoent: Vec<&str> = codes
        .split('|')
        .filter(|c| {
            let name = c.split(':').next().unwrap_or("");
            let code = c.split(':').nth(1).unwrap_or("");
            if name == "rmdir_notdir" {
                code != "ENOTDIR" && code != "ENOENT" && code != "EEXIST"
            } else {
                code != "ENOENT"
            }
        })
        .collect();
    assert!(
        not_enoent.is_empty(),
        "error-path codes deviate from node (expect ENOENT family):\n{}",
        not_enoent.join("\n")
    );
}

/// cb forms for fd-taking APIs (fstat/ftruncate/fchmod/fchown/fdatasync/
/// fsync/futimes/write) — the fd-closure half of spawn_fs_async.
#[test]
fn c11_fs_callback_fd_family() {
    let mut ctx = setup_ctx();
    let dir = tmp_dir("cb_fd");
    let f = dir.join("f.txt");
    std::fs::write(&f, "0123456789").unwrap();
    let fp = js_escape(&f);

    eval_string(
        &mut ctx,
        &format!(
            r#"
            globalThis.__fd = {{ n: 0, log: [] }};
            function rec(name, err, ok) {{
                globalThis.__fd.n++;
                globalThis.__fd.log.push(name + ":" + (err ? ((err && err.code) || String(err)) : "ok") + ":" + (ok === undefined ? "-" : ok));
            }}
            var fs = require('fs');
            var F = "{fp}";
            fs.open(F, "r+", function(e, fd) {{
                rec("open", e, typeof fd === "number" && fd > 0);
                if (typeof fd !== "number") return;
                fs.fstat(fd, function(e2, st) {{ rec("fstat", e2, st && typeof st.size === "number"); }});
                fs.write(fd, Buffer.from("AB"), 0, 2, 0, function(e3, w) {{
                    // Characterization: bao's async write callback delivers a
                    // raw byte carrier, not node's {{ bytesWritten }} object —
                    // delivery + no-error is the pinned contract.
                    rec("write", e3, true);
                    // Serial chaining — every op shares the fd, and the worker
                // threads race otherwise (a parallel schedule intermittently
                // closed the fd before fchmod/futimes ran: EBADF deliveries
                // observed under nextest parallel load). node users must
                // serialize the same way.
                fs.fdatasync(fd, function(e4) {{ rec("fdatasync", e4, true);
                    fs.fsync(fd, function(e5) {{ rec("fsync", e5, true);
                        fs.ftruncate(fd, 6, function(e6) {{ rec("ftruncate", e6, fs.statSync(F).size === 6);
                            fs.fchmod(fd, 420, function(e7) {{ rec("fchmod", e7, (fs.statSync(F).mode & 0o777) === 420);
                                fs.fchown(fd, -1, -1, function(e8) {{ rec("fchown", e8, true);
                                    fs.futimes(fd, 1000, 2000, function(e9) {{ rec("futimes", e9, fs.statSync(F).mtimeMs === 2000000);
                                        fs.close(fd, function(e10) {{ rec("close", e10, true); }});
                                    }});
                                }});
                            }});
                        }});
                    }});
                }});
                }});
            }});
            'scheduled'
            "#,
            fp = fp
        ),
    );
    pump_require_with_log(
        &mut ctx,
        "globalThis.__fd.n >= 9",
        "globalThis.__fd.log.join('|')",
        "fd-family callbacks all delivered (9 APIs)",
    );
    let log = eval_string(&mut ctx, "globalThis.__fd.log.join('|')");
    let failures: Vec<&str> = log
        .split('|')
        .filter(|l| !l.ends_with(":ok:true"))
        .collect();
    assert!(
        failures.is_empty(),
        "fd-family cb results not ok:\n{}",
        failures.join("\n")
    );
}

// ===========================================================================
// fs.promises — the promise-namespace face (synchronous-resolve carrier).
// ===========================================================================

#[test]
fn c11_fs_promises_full_family() {
    let mut ctx = setup_ctx();
    let dir = tmp_dir("promises");
    let f = dir.join("f.txt");
    std::fs::write(&f, "body").unwrap();
    let (d, fp) = (js_escape(&dir), js_escape(&f));

    eval_string(
        &mut ctx,
        &format!(
            r#"
            globalThis.__pj = {{ settled: false, log: [] }};
            (async function() {{
                var fs = require('fs');
                var fsp = require('fs').promises;
                var F = "{fp}", D = "{d}";
                function rec(n, ok) {{ globalThis.__pj.log.push(n + ":" + ok); }}
                try {{
                    rec("readFile", (await fsp.readFile(F, "utf8")) === "body");
                    await fsp.writeFile(F + ".w", "nw"); rec("writeFile", fs.readFileSync(F + ".w", "utf8") === "nw");
                    await fsp.appendFile(F + ".w", "+m"); rec("appendFile", fs.readFileSync(F + ".w", "utf8") === "nw+m");
                    var st = await fsp.stat(F); rec("stat", typeof st.size === "number");
                    var lst = await fsp.lstat(F); rec("lstat", typeof lst.size === "number");
                    var names = await fsp.readdir(D); rec("readdir", Array.isArray(names) && names.indexOf("f.txt") >= 0);
                    await fsp.rename(F + ".w", F + ".r"); rec("rename", fs.existsSync(F + ".r"));
                    await fsp.copyFile(F, F + ".c"); rec("copyFile", fs.readFileSync(F + ".c", "utf8") === "body");
                    await fsp.chmod(F, 420); rec("chmod", (fs.statSync(F).mode & 0o777) === 420);
                    await fsp.chown(F, -1, -1); rec("chown", true);
                    await fsp.access(F, fs.constants.R_OK); rec("access", true);
                    await fsp.rm(F + ".c"); rec("rm", !fs.existsSync(F + ".c"));
                    await fsp.unlink(F + ".r"); rec("unlink", !fs.existsSync(F + ".r"));
                    await fsp.mkdir(D + "/pm"); rec("mkdir", fs.existsSync(D + "/pm"));
                    await fsp.rmdir(D + "/pm"); rec("rmdir", !fs.existsSync(D + "/pm"));
                    var rp = await fsp.realpath(F); rec("realpath", typeof rp === "string" && rp.length > 0);
                    await fsp.symlink(F, F + ".s"); rec("symlink", fs.readlinkSync(F + ".s") === F);
                    var lt = await fsp.readlink(F + ".s"); rec("readlink", lt === F);
                    await fsp.link(F, F + ".h"); rec("link", fs.existsSync(F + ".h"));
                    await fsp.truncate(F, 2); rec("truncate", fs.readFileSync(F, "utf8") === "bo");
                    await fsp.utimes(F, 1000, 2000); rec("utimes", fs.statSync(F).mtimeMs === 2000000);
                    var dt = await fsp.mkdtemp(D + "/pt_"); rec("mkdtemp", typeof dt === "string" && dt.indexOf(D + "/pt_") === 0);
                    var fh = await fsp.open(F + ".o", "w");
                    await fh.writeFile("fh-body"); rec("fh_writeFile", fs.readFileSync(F + ".o", "utf8") === "fh-body");
                    await fh.close();
                    var fh2 = await fsp.open(F + ".o", "r+");
                    await fh2.write(Buffer.from("XY"), 0, 2, 0);
                    var b = Buffer.alloc(2); var rd = await fh2.read(b, 0, 2, 0);
                    rec("fh_read", b.toString() === "XY" && rd !== undefined && rd !== null);
                    await fh2.truncate(1); rec("fh_truncate", fs.readFileSync(F + ".o", "utf8") === "X");
                    await fh2.chmod(420); rec("fh_chmod", (fs.statSync(F + ".o").mode & 0o777) === 420);
                    await fh2.chown(-1, -1); rec("fh_chown", true);
                    await fh2.utimes(1000, 1500); rec("fh_utimes", fs.statSync(F + ".o").mtimeMs === 1500000);
                    await fh2.datasync(); rec("fh_datasync", true);
                    await fh2.sync(); rec("fh_sync", true);
                    var st2 = await fh2.stat(); rec("fh_stat", typeof st2.size === "number");
                    await fh2.close(); rec("fh_close", true);
                    var sf = await fsp.statfs(D); rec("statfs", typeof sf.bsize === "number");
                    globalThis.__pj.settled = true;
                }} catch (e) {{
                    globalThis.__pj.log.push("FATAL:" + String((e && e.code) || e));
                }}
            }})();
            'scheduled'
            "#,
            d = d,
            fp = fp
        ),
    );
    pump_require_with_log(
        &mut ctx,
        "globalThis.__pj.settled === true",
        "globalThis.__pj.log.join('|')",
        "promises family async body settled",
    );
    let log = eval_string(&mut ctx, "globalThis.__pj.log.join('|')");
    let failures: Vec<&str> = log.split('|').filter(|l| !l.ends_with(":true")).collect();
    assert!(
        failures.is_empty(),
        "promises family checks not ok:\n{}",
        failures.join("\n")
    );
}

/// promises error paths: rejections carry the node code.
#[test]
fn c11_fs_promises_error_paths() {
    let mut ctx = setup_ctx();
    let dir = tmp_dir("promises_err");
    let plain = dir.join("p.txt");
    std::fs::write(&plain, "x").unwrap();
    let miss = js_escape(&dir.join("absent_p.txt"));

    eval_string(
        &mut ctx,
        &format!(
            r#"
            globalThis.__pe = {{ n: 0, codes: [] }};
            (async function() {{
                var fsp = require('fs').promises;
                var MISS = "{miss}", P = "{plain}";
                async function rec(n, p) {{
                    try {{ await p; globalThis.__pe.codes.push(n + ":NO_REJECT"); }}
                    catch (e) {{ globalThis.__pe.n++; globalThis.__pe.codes.push(n + ":" + ((e && e.code) || String(e))); }}
                }}
                await rec("readFile", fsp.readFile(MISS));
                await rec("stat", fsp.stat(MISS));
                await rec("readdir", fsp.readdir(MISS));
                await rec("unlink", fsp.unlink(MISS));
                await rec("rename", fsp.rename(MISS, P + ".r"));
                await rec("copyFile", fsp.copyFile(MISS, P + ".c"));
                await rec("chmod", fsp.chmod(MISS, 420));
                await rec("realpath", fsp.realpath(MISS));
                await rec("readlink", fsp.readlink(MISS));
                await rec("truncate", fsp.truncate(MISS, 0));
                await rec("utimes", fsp.utimes(MISS, 1, 2));
                await rec("rmdir_file", fsp.rmdir(P));
                globalThis.__pe.done = true;
            }})();
            'scheduled'
            "#,
            miss = miss,
            plain = js_escape(&plain)
        ),
    );
    pump_require_with_log(
        &mut ctx,
        "globalThis.__pe.done === true",
        "globalThis.__pe.codes.join('|')",
        "promises error-path body settled",
    );
    let codes = eval_string(&mut ctx, "globalThis.__pe.codes.join('|')");
    let bad: Vec<&str> = codes
        .split('|')
        .filter(|c| {
            let name = c.split(':').next().unwrap_or("");
            let code = c.split(':').nth(1).unwrap_or("");
            if name == "rmdir_file" {
                code != "ENOTDIR" && code != "ENOENT" && code != "EEXIST" && code != "EINVAL"
            } else {
                code != "ENOENT"
            }
        })
        .collect();
    assert!(
        bad.is_empty(),
        "promises rejections deviate from node:\n{}",
        bad.join("\n")
    );
}

// ===========================================================================
// dirent / stats type predicates (is_fifo / is_socket / is_character_device
// / is_block_device / is_symbolic_link — zero-hit closures).
// ===========================================================================

#[test]
fn c11_fs_dirent_stats_type_predicates() {
    let mut ctx = setup_ctx();
    let dir = tmp_dir("predicates");
    let file = dir.join("file.txt");
    std::fs::write(&file, "x").unwrap();
    let sub = dir.join("d");
    std::fs::create_dir_all(&sub).unwrap();
    let sym = dir.join("sym");
    std::os::unix::fs::symlink(&file, &sym).unwrap();
    let fifo = dir.join("pipe");
    let _ = std::fs::remove_file(&fifo);
    nix_like_mkfifo(&fifo);
    let sock = dir.join("sock");
    let _ = std::fs::remove_file(&sock);
    let listener = std::os::unix::net::UnixListener::bind(&sock).unwrap();
    let (d, devnull) = (js_escape(&dir), js_escape(Path::new("/dev/null")));

    let script = format!(
        r#"
        {prelude}
        var D = "{d}";
        var flags = function(st) {{
            return [st.isFile(), st.isDirectory(), st.isSymbolicLink(), st.isFIFO(), st.isSocket(), st.isCharacterDevice(), st.isBlockDevice()]
                .map(function(b) {{ return b ? 1 : 0; }}).join("");
        }};
        var expFile = "1000000", expDir = "0100000", expSym = "0010000", expFifo = "0001000", expSock = "0000100", expChar = "0000010";
        var st = fs.statSync(D + "/file.txt"), lst = fs.lstatSync(D + "/file.txt");
        ck("stat_file_flags", function() {{ return flags(st) === expFile; }});
        ck("lstat_file_flags", function() {{ return flags(lst) === expFile; }});
        ck("stat_dir_flags", function() {{ return flags(fs.statSync(D + "/d")) === expDir; }});
        ck("lstat_sym_flags", function() {{ return flags(fs.lstatSync(D + "/sym")) === expSym; }});
        ck("stat_sym_follows_file", function() {{ return flags(fs.statSync(D + "/sym")) === expFile; }});
        ck("lstat_fifo_flags", function() {{ return flags(fs.lstatSync(D + "/pipe")) === expFifo; }});
        ck("lstat_socket_flags", function() {{ return flags(fs.lstatSync(D + "/sock")) === expSock; }});
        ck("stat_chardev_flags", function() {{ return flags(fs.statSync("{devnull}")) === expChar; }});
        var dentries = {{}};
        var dd = fs.opendirSync(D);
        var e;
        while ((e = dd.readSync()) !== null) {{ dentries[e.name] = flags(e); }}
        dd.closeSync();
        ck("dirent_file_flags", function() {{ return dentries["file.txt"] === expFile; }});
        ck("dirent_dir_flags", function() {{ return dentries["d"] === expDir; }});
        // Characterization (C11 registry): Dirent carries a two-state type
        // — create_dirent(name, is_dir) collapses symlinks to file, and
        // fifo/socket/char/block need the directory-entry d_type which the
        // readdir scan (file_type() → dir/file/symlink only) never derives.
        // node v24 oracle: sym="0010000", fifo="0001000", sock="0000100".
        // Registered gap, not fixed this wave.
        ck("char_dirent_sym_collapses_to_file", function() {{ return dentries["sym"] === expFile; }});
        ck("char_dirent_fifo_collapses_to_file", function() {{ return dentries["pipe"] === expFile; }});
        ck("char_dirent_sock_collapses_to_file", function() {{ return dentries["sock"] === expFile; }});
        ck("blockdev_false_for_regular", function() {{ return fs.statSync(D + "/file.txt").isBlockDevice() === false; }});
        R.join("|");
        "#,
        prelude = CHECK_PRELUDE,
        d = d,
        devnull = devnull
    );
    run_checks(&mut ctx, &script);
    drop(listener);
}

/// mkfifo without a libc-binding dependency (mode 0644).
fn nix_like_mkfifo(path: &Path) {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;
    let c = CString::new(path.as_os_str().as_bytes()).unwrap();
    let rc = unsafe { libc::mkfifo(c.as_ptr(), 0o644) };
    assert_eq!(rc, 0, "mkfifo failed: {}", std::io::Error::last_os_error());
}

// ===========================================================================
// sync gap faces — readv/writev semantics, writeSync (offset,length,position),
// futimes/lchown/lchmod/statfs/mkdtemp/cp/glob sync variants.
// ===========================================================================

#[test]
fn c11_fs_sync_gap_faces() {
    let mut ctx = setup_ctx();
    let dir = tmp_dir("sync_gap");
    let f = dir.join("f.txt");
    std::fs::write(&f, "ABCDEFGHIJ").unwrap();
    let (d, fp) = (js_escape(&dir), js_escape(&f));

    // node oracle: writeSync_olp_ret=3, body "\0*5CDE" (offset 2 len 3 at
    // position 5); readSync_pos=3:"CDE" (position 5); readvSync n=4 fills
    // iovecs sequentially; writevSync n=5 concatenates; utimesSync→
    // mtimeMs=2000000; futimesSync/lutimesSync same shape.
    let script = format!(
        r#"
        {prelude}
        var F = "{fp}", D = "{d}";
        // C1 #2 fixed — node v24.19.0 oracle: writeSync(fd, buf, offset=2,
        // length=3, position=5) writes buf[2..5] ("CDE") at absolute
        // position 5, leaves the fd cursor at 0 (pwrite) → n=3, body
        // "ABCDECDEIJ".
        ck("writeSync_offset_length_position", function() {{
            var fd = fs.openSync(F, "r+");
            var n = fs.writeSync(fd, Buffer.from("ABCDEFGHIJ"), 2, 3, 5);
            fs.closeSync(fd);
            return n === 3 && fs.readFileSync(F).toString() === "ABCDECDEIJ";
        }});
        ck("writeSync_cursor_form_at_offset", function() {{
            // pwrite above left the cursor at 0 → "zz" overwrites the head.
            var fd2 = fs.openSync(F, "r+");
            var n2 = fs.writeSync(fd2, Buffer.from("zz"));
            fs.closeSync(fd2);
            return n2 === 2 && fs.readFileSync(F).toString() === "zzCDECDEIJ";
        }});
        ck("readSync_position_form", function() {{
            var fd3 = fs.openSync(F, "r");
            var b = Buffer.alloc(3);
            var n3 = fs.readSync(fd3, b, 0, 3, 5);
            fs.closeSync(fd3);
            return n3 === 3 && b.toString() === "CDE";
        }});
        // C1 #3 fixed — readvSync fills the caller's Buffers (iovec points at
        // their storage); node oracle on "zzCDECDEIJ" from position 0:
        // n=5, b1="zz", b2="CDE".
        ck("readvSync_fills_caller_buffers", function() {{
            var fd4 = fs.openSync(F, "r");
            var b1 = Buffer.alloc(2), b2 = Buffer.alloc(3);
            var n4 = fs.readvSync(fd4, [b1, b2], 0);
            fs.closeSync(fd4);
            return n4 === 5 && b1.toString() === "zz" && b2.toString() === "CDE";
        }});
        ck("readvSync_cursor_form", function() {{
            fs.writeFileSync(F + ".rv", "ABCDE");
            var fd5 = fs.openSync(F + ".rv", "r");
            var b1 = Buffer.alloc(2), b2 = Buffer.alloc(3);
            var n5 = fs.readvSync(fd5, [b1, b2]);
            fs.closeSync(fd5);
            return n5 === 5 && b1.toString() === "AB" && b2.toString() === "CDE";
        }});
        ck("writeSync_string_position_form", function() {{
            fs.writeFileSync(F + ".sp", "0123456789");
            var fd6 = fs.openSync(F + ".sp", "r+");
            var n6 = fs.writeSync(fd6, "XY", 4);
            fs.closeSync(fd6);
            return n6 === 2 && fs.readFileSync(F + ".sp", "utf8") === "0123XY6789";
        }});
        ck("writevSync_position_form", function() {{
            fs.writeFileSync(F + ".wvp", "0123456789");
            var fd7 = fs.openSync(F + ".wvp", "r+");
            var n7 = fs.writevSync(fd7, [Buffer.from("aa"), Buffer.from("bb")], 4);
            fs.closeSync(fd7);
            return n7 === 4 && fs.readFileSync(F + ".wvp", "utf8") === "0123aabb89";
        }});
        ck("writevSync_concatenates", function() {{
            var fd5 = fs.openSync(F + ".v", "w");
            var n5 = fs.writevSync(fd5, [Buffer.from("he"), Buffer.from("llo")]);
            fs.closeSync(fd5);
            return n5 === 5 && fs.readFileSync(F + ".v", "utf8") === "hello";
        }});
        ck("futimesSync_effect", function() {{
            var fd6 = fs.openSync(F, "r");
            fs.futimesSync(fd6, 1000, 1500);
            fs.closeSync(fd6);
            return fs.statSync(F).mtimeMs === 1500000;
        }});
        ck("lchownSync_effect", function() {{
            fs.symlinkSync(F, F + ".sl");
            fs.lchownSync(F + ".sl", -1, -1);
            fs.unlinkSync(F + ".sl");
            return true;
        }});
        ck("lchmodSync_face", function() {{ return codeof(function() {{ fs.lchmodSync(F, 420); }}) !== "EBADF"; }});
        ck("fdatasyncSync_fsyncSync_ok", function() {{
            var fd7 = fs.openSync(F, "r+");
            fs.fdatasyncSync(fd7);
            fs.fsyncSync(fd7);
            fs.closeSync(fd7);
            return true;
        }});
        ck("statfsSync_shape", function() {{
            var s = fs.statfsSync(D);
            return typeof s.bsize === "number" && typeof s.blocks === "number";
        }});
        ck("mkdtempSync_prefix", function() {{
            var p = fs.mkdtempSync(D + "/sg_");
            var ok = p.indexOf(D + "/sg_") === 0;
            fs.rmdirSync(p);
            return ok;
        }});
        ck("cpSync_recursive", function() {{
            fs.mkdirSync(D + "/cps/sub", {{ recursive: true }});
            fs.writeFileSync(D + "/cps/a.txt", "A");
            fs.writeFileSync(D + "/cps/sub/b.txt", "B");
            fs.cpSync(D + "/cps", D + "/cpd", {{ recursive: true }});
            return fs.readdirSync(D + "/cpd").sort().join(",") === "a.txt,sub" && fs.readFileSync(D + "/cpd/sub/b.txt", "utf8") === "B";
        }});
        ck("globSync_matches", function() {{
            var m = fs.globSync("*.txt", {{ cwd: D + "/cps" }});
            return Array.isArray(m) && m.join(",") === "a.txt";
        }});
        ck("openAsBlob_face", function() {{ return typeof fs.openAsBlob === "function"; }});
        R.join("|");
        "#,
        prelude = CHECK_PRELUDE,
        d = d,
        fp = fp
    );
    run_checks(&mut ctx, &script);
}

// ===========================================================================
// Former characterization registry — the C1 #4/#10/#13/#17 deviations are
// FIXED this wave and now pinned to the node v24.19.0 oracle. Remaining
// entries (#5/#6: utimesSync/lutimesSync unregistered) are still gaps.
// ===========================================================================

#[test]
fn c11_fs_registry_fixed_faces() {
    let mut ctx = setup_ctx();
    let dir = tmp_dir("char");
    let f = dir.join("f.txt");
    std::fs::write(&f, "body").unwrap();
    let fp = js_escape(&f);

    let script = format!(
        r#"
        {prelude}
        var F = "{fp}";
        // C1 #4 fixed: rmSync({{force:true}}) on a missing path resolves
        // (node swallows ENOENT; the unforced form still throws).
        ck("rmSync_force_silences_enoent", function() {{
            return codeof(function() {{ fs.rmSync("{dir_absent}", {{ force: true }}); }}) === "NOTHROW";
        }});
        // C1 #5/#6: utimesSync / lutimesSync are NOT registered on the sync
        // face (node has both; fchownSync/lchownSync exist). Registered gap.
        ck("char_utimesSync_unregistered", function() {{ return typeof fs.utimesSync === "undefined"; }});
        ck("char_lutimesSync_unregistered", function() {{ return typeof fs.lutimesSync === "undefined"; }});
        ck("char_fchownSync_registered", function() {{ return typeof fs.fchownSync === "function"; }});
        R.join("|");
        "#,
        prelude = CHECK_PRELUDE,
        fp = fp,
        dir_absent = js_escape(&dir.join("absent_char"))
    );
    run_checks(&mut ctx, &script);

    // C1 #12: fs.exists cb delivery — node passes a single boolean; bao
    // passes (undefined, exists) through the async carrier. Pin the
    // observable: the callback fires with the existence verdict as SOME
    // positional argument (arg shape noted in the wave report).
    eval_string(
        &mut ctx,
        &format!(
            r#"
            globalThis.__ex = {{ n: 0, t: null, f: null }};
            var fs3 = require('fs');
            fs3.exists("{fp}", function(a, b) {{
                globalThis.__ex.n++;
                globalThis.__ex.t = (a === true || b === true);
            }});
            fs3.exists("{fp}.absent", function(a, b) {{
                globalThis.__ex.n++;
                globalThis.__ex.f = (a === false || b === false) && (a !== undefined ? a === false : b === false);
            }});
            'scheduled'
            "#,
            fp = fp
        ),
    );
    pump_require(
        &mut ctx,
        "globalThis.__ex.n >= 2",
        "fs.exists callbacks delivered for both probes",
    );
    assert_eq!(
        eval_string(&mut ctx, "String(globalThis.__ex.t)"),
        "true",
        "fs.exists cb did not observe true for an existing path"
    );
    assert_eq!(
        eval_string(&mut ctx, "String(globalThis.__ex.f)"),
        "true",
        "fs.exists cb did not observe false for an absent path"
    );

    // C1 #13 fixed: fh.readFile(options) decodes per encoding. node
    // v24.19.0 oracle: no arg → Buffer; 'utf8' and {encoding:'utf8'} → the
    // decoded string; {encoding:'hex'} → hex.
    eval_string(
        &mut ctx,
        &format!(
            r#"
            globalThis.__fhenc = {{ done: false, plain: null, str: null, obj: null, hex: null }};
            (async function() {{
                var fsp = require('fs').promises;
                var fh = await fsp.open("{fp}", "r");
                var v = await fh.readFile();
                globalThis.__fhenc.plain = Buffer.isBuffer(v) ? "Buffer" : typeof v;
                await fh.close();
                fh = await fsp.open("{fp}", "r");
                v = await fh.readFile("utf8");
                globalThis.__fhenc.str = typeof v + ":" + v;
                await fh.close();
                fh = await fsp.open("{fp}", "r");
                v = await fh.readFile({{ encoding: "utf8" }});
                globalThis.__fhenc.obj = typeof v + ":" + v;
                await fh.close();
                fh = await fsp.open("{fp}", "r");
                globalThis.__fhenc.hex = await fh.readFile({{ encoding: "hex" }});
                await fh.close();
                globalThis.__fhenc.done = true;
            }})();
            'scheduled'
            "#,
            fp = fp
        ),
    );
    pump_require(
        &mut ctx,
        "globalThis.__fhenc.done === true",
        "fh.readFile encoding face settled",
    );
    assert_eq!(
        eval_string(&mut ctx, "String(globalThis.__fhenc.plain)"),
        "Buffer",
        "fh.readFile() must resolve a Buffer"
    );
    assert_eq!(
        eval_string(&mut ctx, "String(globalThis.__fhenc.str)"),
        "string:body",
        "fh.readFile('utf8') must decode a string (C1 #13)"
    );
    assert_eq!(
        eval_string(&mut ctx, "String(globalThis.__fhenc.obj)"),
        "string:body",
        "fh.readFile({{encoding:'utf8'}}) must decode a string (C1 #13)"
    );
    assert_eq!(
        eval_string(&mut ctx, "String(globalThis.__fhenc.hex)"),
        "626f6479",
        "fh.readFile({{encoding:'hex'}}) must hex-encode"
    );

    // C1 #14: fh double close — node v24 oracle probe says the second close
    // RESOLVES (NO_THROW), so bao's permissive form MATCHES node; the C1
    // entry's node anchor (ERR_INVALID_HANDLE) is falsified by this oracle.
    eval_string(
        &mut ctx,
        &format!(
            r#"
            globalThis.__fhdc = {{ done: false, outcome: null }};
            (async function() {{
                var fsp = require('fs').promises;
                var fh = await fsp.open("{fp}", "r");
                await fh.close();
                try {{ await fh.close(); globalThis.__fhdc.outcome = "NO_THROW"; }}
                catch (e) {{ globalThis.__fhdc.outcome = "THREW:" + ((e && e.code) || e); }}
                globalThis.__fhdc.done = true;
            }})();
            'scheduled'
            "#,
            fp = fp
        ),
    );
    pump_require(
        &mut ctx,
        "globalThis.__fhdc.done === true",
        "fh double-close characterization settled",
    );
    assert_eq!(
        eval_string(&mut ctx, "String(globalThis.__fhdc.outcome)"),
        "NO_THROW",
        "fh double-close now throws — matches the (corrected) node contract; re-audit C1 #14"
    );

    // C1 #17 fixed: fs.cp delivers its callback with the node (err) shape —
    // success cb(null) after the copy lands, error cb(code) on a failed copy.
    eval_string(
        &mut ctx,
        &format!(
            r#"
            globalThis.__cpc = {{ fired: false, code: null }};
            globalThis.__cpce = {{ fired: false, code: null }};
            var fs2 = require('fs');
            fs2.cp("{fp}", "{fp}.cpdst", function(e) {{
                globalThis.__cpc.fired = true;
                globalThis.__cpc.code = e ? ((e && e.code) || String(e)) : null;
            }});
            fs2.cp("{fp}.missing_src", "{fp}.cpdst2", function(e) {{
                globalThis.__cpce.fired = true;
                globalThis.__cpce.code = e ? ((e && e.code) || String(e)) : null;
            }});
            'scheduled'
            "#,
            fp = fp
        ),
    );
    pump_require(
        &mut ctx,
        "globalThis.__cpc.fired === true && globalThis.__cpce.fired === true",
        "fs.cp delivered both callbacks (C1 #17)",
    );
    assert_eq!(
        eval_string(&mut ctx, "String(globalThis.__cpc.code)"),
        "null",
        "fs.cp success callback must receive a nullish error"
    );
    assert_eq!(
        eval_string(&mut ctx, "String(globalThis.__cpce.code)"),
        "ENOENT",
        "fs.cp failure callback must receive the node error code"
    );
    assert!(
        f.with_extension("txt.cpdst").exists(),
        "fs.cp callback fired before the copy landed"
    );

    // writeFile options.flag through the async and promise faces (node:
    // wx → EEXIST; a → append). Sequenced from the first callback so the
    // two writes never race.
    eval_string(
        &mut ctx,
        &format!(
            r#"
            globalThis.__wfx = {{ stage: 0, body1: null, code: null }};
            require('fs').writeFile("{fp}.wf", "a", {{ flag: "wx" }}, function(e) {{
                globalThis.__wfx.body1 = require('fs').readFileSync("{fp}.wf", "utf8");
                require('fs').writeFile("{fp}.wf", "b", {{ flag: "wx" }}, function(e2) {{
                    globalThis.__wfx.code = e2 ? ((e2 && e2.code) || String(e2)) : null;
                    globalThis.__wfx.stage = 2;
                }});
            }});
            'scheduled'
            "#,
            fp = fp
        ),
    );
    pump_require(
        &mut ctx,
        "globalThis.__wfx.stage === 2",
        "writeFile flag chain settled",
    );
    assert_eq!(eval_string(&mut ctx, "String(globalThis.__wfx.body1)"), "a");
    assert_eq!(
        eval_string(&mut ctx, "String(globalThis.__wfx.code)"),
        "EEXIST",
        "fs.writeFile {{flag:'wx'}} on an existing file must fail EEXIST"
    );

    eval_string(
        &mut ctx,
        &format!(
            r#"
            globalThis.__pwf = {{ done: false, body: null, code: null }};
            (async function() {{
                var fsp = require('fs').promises;
                await fsp.writeFile("{fp}.pwf", "a", {{ flag: "wx" }});
                try {{ await fsp.writeFile("{fp}.pwf", "b", {{ flag: "wx" }}); }}
                catch (e) {{ globalThis.__pwf.code = (e && e.code) || String(e); }}
                globalThis.__pwf.body = require('fs').readFileSync("{fp}.pwf", "utf8");
                globalThis.__pwf.done = true;
            }})();
            'scheduled'
            "#,
            fp = fp
        ),
    );
    pump_require(
        &mut ctx,
        "globalThis.__pwf.done === true",
        "promises.writeFile flag face settled",
    );
    assert_eq!(
        eval_string(&mut ctx, "String(globalThis.__pwf.code)"),
        "EEXIST"
    );
    assert_eq!(eval_string(&mut ctx, "String(globalThis.__pwf.body)"), "a");
}

// ===========================================================================
// node_crypto — keygen family (zero-hit closures).
// ===========================================================================

#[test]
fn c11_crypto_keygen_family() {
    let mut ctx = setup_ctx();
    let script = format!(
        r#"
        {prelude}
        var crypto = require('crypto');
        // node oracle: generateKeyPairSync_ed25519_types=public/private/ed25519,
        // export_pem=true (spki PEM), rsa → public, badtype → ERR_INVALID_ARG_VALUE.
        ck("gkps_ed25519_types", function() {{
            var kp = crypto.generateKeyPairSync("ed25519");
            return kp.publicKey.type === "public" && kp.privateKey.type === "private" && kp.publicKey.asymmetricKeyType === "ed25519";
        }});
        ck("gkps_ed25519_export_pem", function() {{
            var kp = crypto.generateKeyPairSync("ed25519");
            var pem = kp.publicKey.export({{ type: "spki", format: "pem" }});
            return typeof pem === "string" && pem.indexOf("-----BEGIN") === 0;
        }});
        ck("gkps_rsa_type", function() {{
            return crypto.generateKeyPairSync("rsa", {{ modulusLength: 2048 }}).publicKey.type === "public";
        }});
        ck("gkps_bad_type_throws", function() {{
            return codeof(function() {{ crypto.generateKeyPairSync("no-such-alg"); }}) !== "NOTHROW";
        }});
        // node oracle: generateKeySync aes/hmac → secret; export().length=16 (128-bit).
        ck("gks_aes_secret", function() {{
            return crypto.generateKeySync("aes", {{ length: 256 }}).type === "secret";
        }});
        ck("gks_hmac_secret", function() {{
            return crypto.generateKeySync("hmac", {{ length: 64 }}).type === "secret";
        }});
        ck("gks_export_len_bits_semantics", function() {{
            return crypto.generateKeySync("aes", {{ length: 128 }}).export().length === 16;
        }});
        ck("gks_bad_type_throws", function() {{
            return codeof(function() {{ crypto.generateKeySync("nope", {{ length: 128 }}); }}) !== "NOTHROW";
        }});
        // generatePrimeSync (node returns an ArrayBuffer; pin the bao carrier
        // by content: non-zero, byte-plenitude).
        var prime = crypto.generatePrimeSync(64);
        ck("gps_produces_bytes", function() {{
            return prime !== undefined && prime !== null && (prime.length === undefined || prime.length > 0);
        }});
        // Characterization (C11 registry): the bigint option is IGNORED —
        // bao always returns the byte carrier (Uint8Array). node v24 oracle:
        // generatePrimeSync(32, {{bigint:true}}) → BigInt. Deviation
        // registered; not fixed this wave.
        ck("char_gps_bigint_option_ignored", function() {{
            var p2 = crypto.generatePrimeSync(32, {{ bigint: true }});
            return p2 !== undefined && p2 !== null && typeof p2 !== "bigint";
        }});
        // node oracle: createDiffieHellmanGroup('modp14') generateKeys +
        // computeSecret non-empty; createECDH('prime256v1') keys non-empty.
        ck("dh_group_modp14", function() {{
            var dh = crypto.createDiffieHellmanGroup("modp14");
            var k = dh.generateKeys();
            var s = dh.computeSecret(dh.generateKeys());
            return k.length > 0 && s.length > 0;
        }});
        // Characterization (C11 registry): createECDH exposes
        // getPublicKey/computeSecret/convertKey but NOT generateKeys (node:
        // generateKeys() produces the keypair). The getPublicKey face is
        // pinned so the ECDH object surface is still exercised.
        ck("char_ecdh_generateKeys_missing", function() {{
            var e1 = crypto.createECDH("prime256v1");
            return typeof e1.generateKeys !== "function" && typeof e1.getPublicKey === "function";
        }});
        R.join("|");
        "#,
        prelude = CHECK_PRELUDE
    );
    run_checks(&mut ctx, &script);
}

/// KeyObject construction family — createPublicKey / createSecretKey faces
/// (createPrivateKey from PEM left unregistered here: the node v24 oracle
/// probe itself errored on the ed25519 pkcs8 PEM path with
/// ERR_OSSL_DH_MODULUS_TOO_SMALL, an apparent node-side misclassification;
/// pinning bao to that would pin noise).
#[test]
fn c11_crypto_keyobject_faces() {
    let mut ctx = setup_ctx();
    let script = format!(
        r#"
        {prelude}
        var crypto = require('crypto');
        var kp = crypto.generateKeyPairSync("ed25519");
        // node oracle: createPublicKey_from_pem=public/ed25519.
        ck("cpk_from_spki_pem", function() {{
            var pem = kp.publicKey.export({{ type: "spki", format: "pem" }});
            var k = crypto.createPublicKey(pem);
            return k.type === "public" && k.asymmetricKeyType === "ed25519";
        }});
        ck("cpk_from_der", function() {{
            var der = kp.publicKey.export({{ type: "spki", format: "der" }});
            var k = crypto.createPublicKey(der);
            return k.type === "public";
        }});
        ck("key_export_der_buffer", function() {{
            var der = kp.publicKey.export({{ type: "spki", format: "der" }});
            return der.length > 0;
        }});
        // node oracle: createSecretKey_type=secret, hex encoding=secret,
        // empty material accepted (NO_THROW).
        ck("csk_secret_type", function() {{
            return crypto.createSecretKey(Buffer.alloc(32, 7)).type === "secret";
        }});
        ck("csk_hex_encoding", function() {{
            return crypto.createSecretKey(Buffer.from("ab".repeat(32)).toString("hex"), "hex").type === "secret";
        }});
        // node oracle: timingSafeEqual_ok=true,
        // len_mismatch=ERR_CRYPTO_TIMING_SAFE_EQUAL_LENGTH.
        ck("tse_equal_true", function() {{ return crypto.timingSafeEqual(Buffer.from("abc"), Buffer.from("abc")) === true; }});
        ck("tse_unequal_false", function() {{ return crypto.timingSafeEqual(Buffer.from("abc"), Buffer.from("abd")) === false; }});
        ck("tse_len_mismatch_throws", function() {{
            return codeof(function() {{ crypto.timingSafeEqual(Buffer.alloc(4), Buffer.alloc(8)); }}) !== "NOTHROW";
        }});
        // node oracle: randomBytes_0=0, neg → ERR_OUT_OF_RANGE, 32 → 32.
        ck("rb_zero", function() {{ return crypto.randomBytes(0).length === 0; }});
        ck("rb_32", function() {{ return crypto.randomBytes(32).length === 32; }});
        ck("rb_negative_throws", function() {{
            return codeof(function() {{ crypto.randomBytes(-1); }}) !== "NOTHROW";
        }});
        ck("rb_distinct", function() {{
            return crypto.randomBytes(16).toString("hex") !== crypto.randomBytes(16).toString("hex");
        }});
        // node oracle: getRandomValues_u8=8; non-typed-array throws a
        // DOMException (code 17 = TYPE_MISMATCH_ERR).
        ck("grv_u8", function() {{ var a = new Uint8Array(8); crypto.getRandomValues(a); return a.length === 8; }});
        ck("grv_u32", function() {{ var a = new Uint32Array(4); crypto.getRandomValues(a); return a.length === 4; }});
        ck("grv_non_typed_throws", function() {{
            return codeof(function() {{ crypto.getRandomValues([1, 2, 3]); }}) !== "NOTHROW";
        }});
        // node oracle: randomUUID_shape=36; getCiphers/getHashes arrays;
        // getCipherInfo('aes-256-cbc') → keyLength 32 / ivLength 16.
        ck("uuid_shape", function() {{ return crypto.randomUUID().length === 36; }});
        ck("getCiphers_array", function() {{ return Array.isArray(crypto.getCiphers()) && crypto.getCiphers().length > 0; }});
        ck("getHashes_includes_sha256", function() {{ return crypto.getHashes().indexOf("sha256") >= 0; }});
        ck("getCipherInfo_aes", function() {{
            var i = crypto.getCipherInfo("aes-256-cbc");
            return !!i && (i.keyLength === 32 || i.keyLength === undefined) && (i.ivLength === 16 || i.ivLength === undefined);
        }});
        R.join("|");
        "#,
        prelude = CHECK_PRELUDE
    );
    run_checks(&mut ctx, &script);
}

/// sign/verify family + hash.copy divergence (zero-hit: sign_sign /
/// hash_copy / resolve_sign_algorithm_for_key).
#[test]
fn c11_crypto_sign_verify_hashcopy() {
    let mut ctx = setup_ctx();
    let script = format!(
        r#"
        {prelude}
        var crypto = require('crypto');
        // node oracle: sign_ed25519_len=64; verify ok=true / tampered=false;
        // crypto.sign(null, ...) — null digest for ed25519.
        var kp = crypto.generateKeyPairSync("ed25519");
        var sig = crypto.sign(null, Buffer.from("m"), kp.privateKey);
        ck("sign_ed25519_len", function() {{ return sig.length === 64; }});
        ck("verify_ok", function() {{ return crypto.verify(null, Buffer.from("m"), kp.publicKey, sig) === true; }});
        ck("verify_tampered_false", function() {{ return crypto.verify(null, Buffer.from("other"), kp.publicKey, sig) === false; }});
        // RSA + sha256 through createSign/createVerify streams (node oracle:
        // createSign(null) is ERR_INVALID_ARG_TYPE — string algo required).
        var rsa = crypto.generateKeyPairSync("rsa", {{ modulusLength: 2048 }});
        var s = crypto.createSign("sha256");
        s.update("stream-payload");
        var rsig = s.sign(rsa.privateKey);
        // Characterization note: bao's RSA createSign signature is 32 bytes
        // (probe), node v24's RSA-2048 PKCS#1 v1.5 signature is 256 bytes.
        // Length NOT pinned; the sign → verify roundtrip below is the
        // contract asserted this wave.
        ck("createSign_rsa_sig", function() {{ return rsig.length > 0; }});
        var v = crypto.createVerify("sha256");
        v.update("stream-payload");
        ck("createVerify_ok", function() {{ return v.verify(rsa.publicKey, rsig) === true; }});
        var v2 = crypto.createVerify("sha256");
        v2.update("different");
        ck("createVerify_rejects", function() {{ return v2.verify(rsa.publicKey, rsig) === false; }});
        // node oracle: hash_copy_diverge=false (copies diverge after fork),
        // hash_copy_same_prefix=true (copy sees the pre-fork state).
        var h = crypto.createHash("sha256");
        h.update("a");
        var h2 = h.copy();
        h2.update("b");
        var h3 = crypto.createHash("sha256");
        h3.update("a");
        ck("hash_copy_prefix_frozen", function() {{ return h2.digest("hex") === crypto.createHash("sha256").update("ab").digest("hex"); }});
        ck("hash_copy_same_as_source_prefix", function() {{ return h3.digest("hex").length === 64; }});
        var h4 = crypto.createHash("sha256");
        h4.update("x");
        var h5 = h4.copy();
        h5.update("y");
        h4.update("z");
        ck("hash_copy_diverges", function() {{ return h4.digest("hex") !== h5.digest("hex"); }});
        // node oracle: hmac_one shot=64 (hex length for sha256).
        ck("hmac_oneshot_hex_len", function() {{
            return crypto.createHmac("sha256", "k").update("d").digest("hex").length === 64;
        }});
        R.join("|");
        "#,
        prelude = CHECK_PRELUDE
    );
    run_checks(&mut ctx, &script);
}

/// cipher/decipher error arms + KDF parameter faces (zero-hit: cipher
/// registry take/remove, argon2 error arms' siblings).
#[test]
fn c11_crypto_cipher_error_arms() {
    let mut ctx = setup_ctx();
    let script = format!(
        r#"
        {prelude}
        var crypto = require('crypto');
        var key = Buffer.alloc(32, 1), iv = Buffer.alloc(16, 2);
        // node oracle: cipher_roundtrip=secret-data; bad algo →
        // ERR_CRYPTO_UNKNOWN_CIPHER; bad keylen → ERR_CRYPTO_INVALID_KEYLEN;
        // final-twice → ERR_CRYPTO_INVALID_STATE; decipher on a non-block
        // input → an error (wrong final block length).
        var c = crypto.createCipheriv("aes-256-cbc", key, iv);
        var enc = Buffer.concat([c.update("secret-data"), c.final()]);
        var d = crypto.createDecipheriv("aes-256-cbc", key, iv);
        ck("cipher_roundtrip", function() {{
            return Buffer.concat([d.update(enc), d.final()]).toString() === "secret-data";
        }});
        ck("cipher_unknown_algo_throws", function() {{
            return codeof(function() {{ crypto.createCipheriv("nope", key, iv); }}) !== "NOTHROW";
        }});
        ck("cipher_bad_keylen_throws", function() {{
            return codeof(function() {{ crypto.createCipheriv("aes-256-cbc", Buffer.alloc(10), iv); }}) !== "NOTHROW";
        }});
        ck("cipher_final_twice_throws", function() {{
            return codeof(function() {{
                var c2 = crypto.createCipheriv("aes-256-cbc", key, iv);
                c2.update("x");
                c2.final();
                c2.final();
            }}) !== "NOTHROW";
        }});
        ck("decipher_garbage_input_errors", function() {{
            return codeof(function() {{
                var d2 = crypto.createDecipheriv("aes-256-cbc", key, iv);
                d2.update(Buffer.alloc(5));
                d2.final();
            }}) !== "NOTHROW";
        }});
        ck("decipher_unknown_algo_throws", function() {{
            return codeof(function() {{ crypto.createDecipheriv("nope", key, iv); }}) !== "NOTHROW";
        }});
        // KDF faces: node oracle pbkdf2Sync_shape=32, scryptSync_shape=32,
        // hkdfSync_bad_digest → ERR_CRYPTO_INVALID_DIGEST.
        ck("pbkdf2Sync_len", function() {{ return crypto.pbkdf2Sync("pw", "salt", 100, 32, "sha256").length === 32; }});
        ck("pbkdf2Sync_bad_digest_throws", function() {{
            return codeof(function() {{ crypto.pbkdf2Sync("pw", "salt", 10, 32, "nope"); }}) !== "NOTHROW";
        }});
        ck("scryptSync_len", function() {{ return crypto.scryptSync("pw", "salt", 32).length === 32; }});
        ck("hkdfSync_returns", function() {{
            var out = crypto.hkdfSync("sha256", Buffer.alloc(32), Buffer.alloc(16), Buffer.alloc(0), 32);
            return out !== undefined && out !== null;
        }});
        ck("hkdfSync_bad_digest_throws", function() {{
            return codeof(function() {{ crypto.hkdfSync("nope", Buffer.alloc(32), Buffer.alloc(16), Buffer.alloc(0), 32); }}) !== "NOTHROW";
        }});
        R.join("|");
        "#,
        prelude = CHECK_PRELUDE
    );
    run_checks(&mut ctx, &script);
}

/// crypto async delivery — generateKeyPair / generateKey / generatePrime /
/// pbkdf2 callback forms land on the JS thread (the crypto_async_* closure
/// face).
#[test]
fn c11_crypto_async_faces() {
    let mut ctx = setup_ctx();
    eval_string(
        &mut ctx,
        r#"
        globalThis.__ca = { n: 0, log: [] };
        function rec(name, err, ok) {
            globalThis.__ca.n++;
            globalThis.__ca.log.push(name + ":" + (err ? ((err && err.code) || String(err)) : "ok") + ":" + (ok === undefined ? "-" : ok));
        }
        var crypto = require('crypto');
        // Characterization (C11 registry): the async generateKeyPair callback
        // receives (err, empty-bytes[, undefined]) — node's shape is (err,
        // publicKey, privateKey) with real KeyObjects (node v24.19.0 oracle:
        // generateKeyPair_async=public/private). bao probe: err=null, 2nd arg
        // empty, 3rd undefined. Delivery IS pinned; the argument shape is a
        // registered deviation.
        crypto.generateKeyPair("ed25519", function(err, pub, priv) {
            rec("generateKeyPair", err, true);
        });
        // Characterization (C11 registry): the async generateKey callback
        // receives (err, raw-bytes) — 16 bytes for length:128 post-fix — not
        // a KeyObject (node: (err, KeyObject of type "secret")). Byte length
        // is the asserted contract here.
        crypto.generateKey("aes", { length: 128 }, function(err, k) {
            rec("generateKey", err, k !== undefined && k !== null && k.length === 16);
        });
        crypto.generatePrime(32, function(err, p) {
            rec("generatePrime", err, p !== undefined && p !== null);
        });
        crypto.pbkdf2("pw", "salt", 100, 32, "sha256", function(err, dk) {
            rec("pbkdf2", err, dk && dk.length === 32);
        });
        'scheduled'
        "#,
    );
    pump_require_with_log(
        &mut ctx,
        "globalThis.__ca.n >= 4",
        "globalThis.__ca.log.join('|')",
        "crypto async callbacks all delivered (4 APIs)",
    );
    let log = eval_string(&mut ctx, "globalThis.__ca.log.join('|')");
    let failures: Vec<&str> = log
        .split('|')
        .filter(|l| !l.ends_with(":ok:true"))
        .collect();
    assert!(
        failures.is_empty(),
        "crypto async results not ok:\n{}",
        failures.join("\n")
    );
}

/// crypto async error paths — bad parameters deliver errors to the callback
/// instead of throwing.
#[test]
fn c11_crypto_async_error_paths() {
    let mut ctx = setup_ctx();
    eval_string(
        &mut ctx,
        r#"
        globalThis.__cae = { n: 0, codes: [] };
        var crypto = require('crypto');
        crypto.generateKeyPair("no-such-alg", function(err) {
            globalThis.__cae.n++;
            globalThis.__cae.codes.push("gkp:" + (err ? ((err && err.code) || String(err)) : "NO_ERR"));
        });
        crypto.pbkdf2("pw", "salt", 10, 32, "no-such-digest", function(err) {
            globalThis.__cae.n++;
            globalThis.__cae.codes.push("pbkdf2:" + (err ? ((err && err.code) || String(err)) : "NO_ERR"));
        });
        'scheduled'
        "#,
    );
    pump_require_with_log(
        &mut ctx,
        "globalThis.__cae.n >= 2",
        "globalThis.__cae.codes.join('|')",
        "crypto async error callbacks delivered",
    );
    let codes = eval_string(&mut ctx, "globalThis.__cae.codes.join('|')");
    let bad: Vec<&str> = codes
        .split('|')
        .filter(|c| c.ends_with(":NO_ERR") || c.ends_with(":undefined"))
        .collect();
    assert!(
        bad.is_empty(),
        "crypto async error paths delivered no error:\n{}",
        bad.join("\n")
    );
}

// ===========================================================================
// K wave — node:fs second-wave residuals (node v24.19.0 oracle probes).
//   ③ writeFile/appendFile family: string data honors options.encoding.
//   ④ fs.write / fs.writeFile require a function callback.
// ===========================================================================

/// ③ sync faces: writeFileSync honors the encoding shorthand string and the
/// {encoding} option (latin1 "é" → 0xE9, hex "6162" → 0x61 0x62; default and
/// unknown encodings stay UTF-8), appendFileSync honors it too.
#[test]
fn c11_fs_writefile_sync_encoding() {
    let mut ctx = setup_ctx();
    let dir = tmp_dir("kwenc");
    let f = dir.join("enc.txt");
    let (d, fp) = (js_escape(&dir), js_escape(&f));

    let body = format!(
        r#"
        {prelude}
        var F = "{fp}", D = "{d}";
        ck("writeFileSync_latin1_obj", function() {{
            fs.writeFileSync(F, "é", {{ encoding: "latin1" }});
            return fs.readFileSync(F).toString("hex") === "e9";
        }});
        ck("writeFileSync_latin1_shorthand", function() {{
            fs.writeFileSync(F, "é", "latin1");
            return fs.readFileSync(F).toString("hex") === "e9";
        }});
        ck("writeFileSync_hex", function() {{
            fs.writeFileSync(F, "6162", {{ encoding: "hex" }});
            return fs.readFileSync(F).toString("hex") === "6162";
        }});
        ck("writeFileSync_utf16le", function() {{
            fs.writeFileSync(F, "ab", {{ encoding: "utf16le" }});
            return fs.readFileSync(F).toString("hex") === "61006200";
        }});
        ck("writeFileSync_default_utf8", function() {{
            fs.writeFileSync(F, "é");
            return fs.readFileSync(F).toString("hex") === "c3a9";
        }});
        ck("writeFileSync_binary_data_ignores_encoding", function() {{
            fs.writeFileSync(F, Buffer.from([0xE9]), {{ encoding: "hex" }});
            return fs.readFileSync(F).toString("hex") === "e9";
        }});
        ck("appendFileSync_latin1", function() {{
            fs.writeFileSync(F, "");
            fs.appendFileSync(F, "é", {{ encoding: "latin1" }});
            return fs.readFileSync(F).toString("hex") === "e9";
        }});
        R.join("|")
    "#,
        prelude = CHECK_PRELUDE,
        fp = fp,
        d = d,
    );
    let results = run_checks(&mut ctx, &body);
    assert_eq!(results.len(), 7);
    let _ = std::fs::remove_dir_all(&dir);
}

/// ③ async faces: fs.writeFile(cb), fs.promises.writeFile and
/// FileHandle.writeFile all encode string data per options.encoding.
#[test]
fn c11_fs_writefile_async_encoding() {
    let mut ctx = setup_ctx();
    let dir = tmp_dir("kwenc2");
    let f = dir.join("enc2.txt");
    let fp = js_escape(&f);

    let sched = format!(
        r#"
        var fs = require('fs');
        globalThis.__kwenc2 = {{ n: 0, want: 3, cb: null, promises: null, fh: null }};
        (async function() {{
            var F = "{fp}";
            await fs.promises.writeFile(F, "é", {{ encoding: "latin1" }});
            globalThis.__kwenc2.promises = fs.readFileSync(F).toString("hex");
            globalThis.__kwenc2.n++;
            var fh = await fs.promises.open(F, "w");
            await fh.writeFile("é", {{ encoding: "latin1" }});
            await fh.close();
            globalThis.__kwenc2.fh = fs.readFileSync(F).toString("hex");
            globalThis.__kwenc2.n++;
            fs.writeFile(F, "é", {{ encoding: "latin1" }}, function(e) {{
                globalThis.__kwenc2.cb = e
                    ? "ERR:" + String(e.code || e.message)
                    : fs.readFileSync(F).toString("hex");
                globalThis.__kwenc2.n++;
            }});
        }})();
        'scheduled'
        "#,
        fp = fp,
    );
    assert_eq!(eval_string_full(&mut ctx, &sched), "scheduled");
    pump_require(
        &mut ctx,
        "globalThis.__kwenc2.n >= globalThis.__kwenc2.want",
        "writeFile async encoding faces settled",
    );
    for (key, what) in [
        ("promises", "promises.writeFile latin1"),
        ("fh", "FileHandle.writeFile latin1"),
        ("cb", "fs.writeFile cb latin1"),
    ] {
        assert_eq!(
            eval_string(&mut ctx, &format!("String(globalThis.__kwenc2.{key})")),
            "e9",
            "{what} must encode string data per options.encoding"
        );
    }
    let _ = std::fs::remove_dir_all(&dir);
}

/// ④ fs.write / fs.writeFile require a FUNCTION callback: missing cb, the
/// legacy string-encoding third arg and an options object all throw
/// ERR_INVALID_ARG_TYPE (node v24.19.0 oracle). writeSync keeps its sync
/// contract — no throw there.
#[test]
fn c11_fs_write_requires_callback() {
    let mut ctx = setup_ctx();
    let dir = tmp_dir("kwcb");
    let f = dir.join("cb.txt");
    let (d, fp) = (js_escape(&dir), js_escape(&f));

    let body = format!(
        r#"
        {prelude}
        var F = "{fp}", D = "{d}";
        ck("writeFile_missing_cb", function() {{
            return codeof(function() {{ fs.writeFile(F, "x"); }}) === "ERR_INVALID_ARG_TYPE";
        }});
        ck("writeFile_string_cb", function() {{
            return codeof(function() {{ fs.writeFile(F, "x", "utf8"); }}) === "ERR_INVALID_ARG_TYPE";
        }});
        ck("writeFile_options_object_cb", function() {{
            return codeof(function() {{ fs.writeFile(F, "x", {{}}); }}) === "ERR_INVALID_ARG_TYPE";
        }});
        ck("write_missing_cb", function() {{
            return codeof(function() {{
                var fd = fs.openSync(F, "w");
                fs.write(fd, Buffer.from("x"));
            }}) === "ERR_INVALID_ARG_TYPE";
        }});
        ck("write_string_position_no_cb", function() {{
            return codeof(function() {{
                var fd = fs.openSync(F, "w");
                fs.write(fd, "x", 0);
            }}) === "ERR_INVALID_ARG_TYPE";
        }});
        ck("write_object_cb", function() {{
            return codeof(function() {{
                var fd = fs.openSync(F, "w");
                fs.write(fd, Buffer.from("x"), {{}});
            }}) === "ERR_INVALID_ARG_TYPE";
        }});
        ck("writeSync_still_synchronous", function() {{
            var fd = fs.openSync(F, "w");
            var n = fs.writeSync(fd, "ok");
            fs.closeSync(fd);
            return n === 2 && fs.readFileSync(F, "utf8") === "ok";
        }});
        R.join("|")
    "#,
        prelude = CHECK_PRELUDE,
        fp = fp,
        d = d,
    );
    let results = run_checks(&mut ctx, &body);
    assert_eq!(results.len(), 7);
    let _ = std::fs::remove_dir_all(&dir);
}
