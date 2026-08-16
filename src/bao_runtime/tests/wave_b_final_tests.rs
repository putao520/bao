// @trace TEST-ENG-007 [req:REQ-ENG-006 REQ-ENG-007 REQ-ENG-009] [level:integration]
//
// 终局 Wave B:node/ffi/sqlite/杂项域残留 — 8 silent-fake 修复 + 3 missing 补齐
// 的行为回归。每项驱动 REAL 路径并断言可观察行为(无 typeof-only 检查)。
//
// 覆盖:
//   1  punycode toASCII/encode/decode(此前 encode 缺 digitToBasic、adapt 用
//      pre-2.0 公式 → xn-- 恒错)
//   3  Buffer.concat 非 view 项 TypeError(此前静默返空)
//   4  zlib.crc32 u32 无符号(此前 Int32 符号化)
//   5  bun:sqlite backup 成功路径返回目标路径(此前 undefined)
//   6  bun:ffi dlopen 双入口 lib.sym / lib.symbols.sym(此前 symbols 缺失)
//   7  dgram bind 全参数形态(此前 bind(port, cb) 的 cb 落 addr 位不触发)
//   9  util.TextEncoder/TextDecoder 与 global 身份恒等
//   11 ffi callback(argCount, returns, fn) 带 C→JS 实参编组 + js_function
//      参数类型(以 libc qsort e2e 驱动真实闭包调用)

use std::time::Duration;

use bao_engine::context::JsContext;
use bao_engine::value::JsValue;

fn eval_string(ctx: &mut JsContext, source: &str) -> String {
    match ctx.eval(source, "<wave-b>") {
        Ok(JsValue::String(s)) => s,
        Ok(JsValue::Number(n)) => format!("{}", n),
        Ok(JsValue::Bool(b)) => if b { "true" } else { "false" }.to_string(),
        Ok(JsValue::Null) => "null".to_string(),
        Ok(JsValue::Undefined) => "undefined".to_string(),
        Ok(JsValue::Object(_)) => "[object]".to_string(),
        Err(e) => format!("ERROR:{}", e.message),
    }
}

fn drive_event_loop(ctx: &mut JsContext, max_iters: usize) {
    for _ in 0..max_iters {
        let mut cxm = ctx.cx();
        bun_runtime::timers::drain_and_check(&mut cxm);
        std::thread::sleep(Duration::from_millis(1));
    }
}

fn setup_ctx() -> JsContext {
    bun_runtime::install_exit_handler();
    bun_runtime::bun_api::init_process_start();
    let mut ctx = JsContext::for_test().expect("JsContext");
    ctx.set_global_setup(bun_runtime::globals::install_all);
    ctx
}

// ══════════════════════════════════════════════════════════════════════════
// Item 1 — punycode: RFC 3492 真算法(ground truth 与 npm/Node 内置一致)
// ══════════════════════════════════════════════════════════════════════════

#[test]
fn test_punycode_to_ascii_idn() {
    let mut ctx = setup_ctx();
    // Ground truth: Python/Node 内置 punycode 均产出这些标签。
    assert_eq!(
        eval_string(&mut ctx, r#"require('punycode').toASCII('日本.jp')"#),
        "xn--wgv71a.jp"
    );
    assert_eq!(
        eval_string(&mut ctx, r#"require('punycode').toASCII('mémé.com')"#),
        "xn--mm-bjab.com"
    );
    // 纯 ASCII 直通。
    assert_eq!(
        eval_string(&mut ctx, r#"require('punycode').toASCII('example.com')"#),
        "example.com"
    );
}

#[test]
fn test_punycode_encode_decode_roundtrip() {
    let mut ctx = setup_ctx();
    assert_eq!(
        eval_string(&mut ctx, r#"require('punycode').encode('日本')"#),
        "wgv71a"
    );
    assert_eq!(
        eval_string(&mut ctx, r#"require('punycode').decode('wgv71a')"#),
        "日本"
    );
    // toUnicode(toASCII(x)) === x
    assert_eq!(
        eval_string(
            &mut ctx,
            r#"var p=require('punycode'); p.toUnicode(p.toASCII('日本.jp'))"#
        ),
        "日本.jp"
    );
    // 非法输入:canonical 2.1.0 抛 RangeError(旧实现静默返部分串)。
    assert_eq!(
        eval_string(
            &mut ctx,
            r#"try { require('punycode').decode('!'); 'NO-THROW'; } catch (e) { e.constructor.name; }"#
        ),
        "RangeError"
    );
}

// ══════════════════════════════════════════════════════════════════════════
// Item 3 — Buffer.concat:非 view 项 TypeError(Node ERR_INVALID_ARG_TYPE)
// ══════════════════════════════════════════════════════════════════════════

#[test]
fn test_buffer_concat_type_error_for_non_views() {
    let mut ctx = setup_ctx();
    // 正常路径仍工作(view 项:Buffer + Uint8Array)。
    assert_eq!(
        eval_string(
            &mut ctx,
            r#"Buffer.concat([Buffer.from('ab'), new Uint8Array([67,68])]).toString()"#
        ),
        "abCD"
    );
    assert_eq!(
        eval_string(&mut ctx, r#"Buffer.concat([]).length"#),
        "0"
    );
    // totalLength 截断语义保持。
    assert_eq!(
        eval_string(&mut ctx, r#"Buffer.concat([Buffer.from('abcdef')], 3).toString()"#),
        "abc"
    );
    // 非 view 项:字符串 / 数字 / plain object → TypeError(此前静默返空)。
    for bad in ["'nope'", "42", "{length:2,0:65,1:66}", "null", "undefined"] {
        let src = format!(
            "try {{ Buffer.concat([{}]); 'NO-THROW'; }} catch (e) {{ e.constructor.name; }}",
            bad
        );
        assert_eq!(
            eval_string(&mut ctx, &src),
            "TypeError",
            "Buffer.concat([{}]) must throw TypeError",
            bad
        );
    }
}

// ══════════════════════════════════════════════════════════════════════════
// Item 4 — zlib.crc32:u32 无符号语义(值 ≥ 2^31 不再符号化)
// ══════════════════════════════════════════════════════════════════════════

#[test]
fn test_zlib_crc32_unsigned_u32() {
    let mut ctx = setup_ctx();
    // Ground truth:Python zlib.crc32 / Node zlib.crc32。
    assert_eq!(
        eval_string(&mut ctx, r#"require('zlib').crc32(Buffer.from([255,255,255,255]))"#),
        "4294967295", // 旧实现返 -1
    );
    assert_eq!(
        eval_string(&mut ctx, r#"require('zlib').crc32('the quick brown fox')"#),
        "2445345482", // 旧实现返 -1849621814
    );
    assert_eq!(
        eval_string(&mut ctx, r#"require('zlib').crc32('hello world')"#),
        "222957957"
    );
    // 续算语义:crc32(' world', crc32('hello')) === crc32('hello world')。
    assert_eq!(
        eval_string(
            &mut ctx,
            r#"var z=require('zlib'); z.crc32(' world', z.crc32('hello'))"#
        ),
        "222957957"
    );
}

// ══════════════════════════════════════════════════════════════════════════
// Item 5 — bun:sqlite backup:成功路径返回目标路径(可观察成功)
// ══════════════════════════════════════════════════════════════════════════

#[test]
fn test_sqlite_backup_returns_destination() {
    let mut ctx = setup_ctx();
    let out = eval_string(
        &mut ctx,
        r#"
        var { Database } = require('bun:sqlite');
        var db = new Database(':memory:');
        db.exec('CREATE TABLE t(x); INSERT INTO t VALUES (42);');
        var path = require('os').tmpdir() + '/waveb-backup-test.db';
        try { require('fs').rmSync(path); } catch (e) {}
        var ret = db.backup(path);
        var first = require('fs').existsSync(path) + '|' + (ret === path);
        // 重复 backup 到同一目标:VACUUM INTO 要求新文件 → 显式错误(fail-closed)。
        var dup;
        try { db.backup(path); dup = 'NO-THROW'; }
        catch (e) { dup = 'THREW:' + (e.message.indexOf('already exists') >= 0); }
        first + '|' + dup;
    "#,
    );
    assert_eq!(
        out, "true|true|THREW:true",
        "backup must return the destination path, write the snapshot, and fail closed on duplicates"
    );
}

// ══════════════════════════════════════════════════════════════════════════
// Item 6 — bun:ffi dlopen 双入口:lib.sym() 与 lib.symbols.sym() 同一可调用
// ══════════════════════════════════════════════════════════════════════════

#[test]
fn test_ffi_dlopen_symbols_face() {
    let mut ctx = setup_ctx();
    let out = eval_string(
        &mut ctx,
        r#"
        var { dlopen } = require('bun:ffi');
        var lib = dlopen('/usr/lib/x86_64-linux-gnu/libc.so.6',
                         { getpid: { args: [], returns: 'i32' } });
        (typeof lib.symbols) + '|' + (lib.symbols.getpid === lib.getpid) + '|' +
        (lib.symbols.getpid() === lib.getpid() && lib.getpid() > 0);
    "#,
    );
    assert_eq!(out, "object|true|true", "symbols face must expose the same callables");
}

// ══════════════════════════════════════════════════════════════════════════
// Item 7 — dgram bind:Node 全参数形态(port/addr/options/callback 任意位置)
// ══════════════════════════════════════════════════════════════════════════

#[test]
fn test_dgram_bind_callback_forms() {
    let mut ctx = setup_ctx();
    // bind(port, cb) — 此前 cb 落在 addr 位,永不触发。
    ctx.eval(
        r#"
        globalThis.__dgram_results = [];
        var dgram = require('dgram');
        var s1 = dgram.createSocket('udp4');
        s1.bind(0, function () {
            __dgram_results.push('form2:' + (typeof s1.address().port === 'number'));
            s1.close();
        });
    "#,
        "<wave-b>",
    )
    .expect("setup");
    drive_event_loop(&mut ctx, 60);

    let got = eval_string(&mut ctx, "JSON.stringify(__dgram_results)");
    assert_eq!(got, r#"["form2:true"]"#, "bind(port, cb) callback must fire");

    // bind(cb) 与 bind(options, cb) 形态。
    ctx.eval(
        r#"
        var s2 = dgram.createSocket('udp4');
        s2.bind(function () { __dgram_results.push('cbOnly'); s2.close(); });
        var s3 = dgram.createSocket('udp4');
        s3.bind({ port: 0 }, function () { __dgram_results.push('optsForm:' + (s3.address().port > 0)); s3.close(); });
    "#,
        "<wave-b>",
    )
    .expect("setup2");
    drive_event_loop(&mut ctx, 60);

    let got = eval_string(&mut ctx, "JSON.stringify(__dgram_results)");
    assert_eq!(
        got,
        r#"["form2:true","cbOnly","optsForm:true"]"#,
        "all bind forms must fire their callbacks"
    );
}

// ══════════════════════════════════════════════════════════════════════════
// Item 9 — util.TextEncoder/TextDecoder === globalThis 同一构造器
// ══════════════════════════════════════════════════════════════════════════

#[test]
fn test_util_textencoder_identity() {
    let mut ctx = setup_ctx();
    assert_eq!(
        eval_string(
            &mut ctx,
            "var u = require('util'); (u.TextEncoder === globalThis.TextEncoder) + '|' + (u.TextDecoder === globalThis.TextDecoder)"
        ),
        "true|true"
    );
    // 可实例化且行为正确。
    assert_eq!(
        eval_string(&mut ctx, "new (require('util').TextEncoder)().encode('hi').length"),
        "2"
    );
}

// ══════════════════════════════════════════════════════════════════════════
// Item 2 — TextDecoder.decode:WHATWG BufferSource(ArrayBuffer + view + DataView)
// ══════════════════════════════════════════════════════════════════════════

#[test]
fn test_text_decoder_buffer_source() {
    let mut ctx = setup_ctx();
    // 裸 ArrayBuffer(此前恒空串——通用 length+GetElement 提取对 AB 无 length)。
    assert_eq!(
        eval_string(&mut ctx, "new TextDecoder().decode(new Uint8Array([104,105]).buffer)"),
        "hi"
    );
    // view(byteOffset 调整)+ DataView。
    assert_eq!(
        eval_string(
            &mut ctx,
            "new TextDecoder().decode(new Uint8Array([65,66,67,68]).subarray(1,3))"
        ),
        "BC"
    );
    assert_eq!(
        eval_string(
            &mut ctx,
            "new TextDecoder().decode(new DataView(new Uint8Array([120,121]).buffer))"
        ),
        "xy"
    );
    // fatal:false(默认)→ 非法序列替换 U+FFFD,不抛错。
    assert_eq!(
        eval_string(&mut ctx, "new TextDecoder().decode(new Uint8Array([0xff,0xfe]))"),
        "\u{FFFD}\u{FFFD}"
    );
    // 非 BufferSource → TypeError;无参 → ""。
    for bad in ["42", "'str'", "null", "{}"] {
        let src = format!(
            "try {{ new TextDecoder().decode({}); 'NO-THROW'; }} catch (e) {{ e.constructor.name; }}",
            bad
        );
        assert_eq!(
            eval_string(&mut ctx, &src),
            "TypeError",
            "decode({}) must throw TypeError",
            bad
        );
    }
    assert_eq!(eval_string(&mut ctx, "JSON.stringify(new TextDecoder().decode())"), "\"\"");
}

// ══════════════════════════════════════════════════════════════════════════
// Item 11 — ffi callback:C 实参 → JS 编组 + 返回值回写(libc qsort e2e)
// ══════════════════════════════════════════════════════════════════════════

#[test]
fn test_ffi_callback_with_args_qsort_e2e() {
    let mut ctx = setup_ctx();
    // qsort(base, 2, 4, compar) over a REAL calloc'd region (glibc's merge
    // sort memcpy's from base, so the base must be mapped memory; freeing is
    // omitted — cross-allocator free (glibc calloc vs mimalloc-interposed
    // free) aborts and 32 bytes are process-lifetime anyway). The comparator
    // receives POINTERS into that region — pointer-typed callbacks must use
    // the typed-array form (['ptr','ptr']): the argCount shorthand types
    // args as f64, which libffi marshals from the SSE registers, not rdi/rsi.
    let out = eval_string(
        &mut ctx,
        r#"
        var ffi = require('bun:ffi');
        var dlopen = ffi.dlopen, callback = ffi.callback, toBuffer = ffi.toBuffer;
        var lib = dlopen('/usr/lib/x86_64-linux-gnu/libc.so.6', {
          calloc: { args: ['usize', 'usize'], returns: 'ptr' },
          qsort:  { args: ['ptr', 'usize', 'usize', 'js_function'], returns: 'void' }
        });
        var region = lib.calloc(8, 4); // 8 zeroed int32 slots (mapped, writable)
        var seen = [];
        var cb = callback(['ptr', 'ptr'], 'i32', function (a, b) {
          var va = toBuffer(a, 4).readInt32LE(0);
          var vb = toBuffer(b, 4).readInt32LE(0);
          seen.push([typeof a, b - a, va - vb]);
          return 0;
        });
        lib.qsort(region, 2, 4, cb);
        JSON.stringify(seen);
    "#,
    );
    assert_eq!(
        out, r#"[["number",4,0]]"#,
        "C must invoke the JS closure with two real pointer args 4 bytes apart, readable via toBuffer"
    );

    // argCount 简写形态(argCount × f64):验证闭包被 C 调用 + 返回路径,
    // 实参从 SSE 寄存器编组(f64 定型),断言类型与调用次数。
    let out2 = eval_string(
        &mut ctx,
        r#"
        var region2 = lib.calloc(8, 4);
        var seen2 = [];
        var cb2 = callback(2, 'i32', function (a, b) {
          seen2.push(typeof a + ':' + typeof b);
          return 0;
        });
        lib.qsort(region2, 2, 4, cb2);
        JSON.stringify([seen2.length, seen2[0]]);
    "#,
    );
    assert_eq!(out2, r#"[1,"number:number"]"#, "argCount callback form must be invoked from C with numeric args");

    // 非函数实参被拒(fail-closed):js_function 槽只收 callback() 包装对象
    // (错误类型对齐模块既有的 spec_arg 报错路径 = 普通 Error)。
    let out3 = eval_string(
        &mut ctx,
        r#"
        try { lib.qsort(lib.calloc(8, 4), 2, 4, function () { return 0; }); 'NO-THROW'; }
        catch (e) { e.constructor.name + ':' + (e.message.indexOf('callback') >= 0); }
    "#,
    );
    assert_eq!(
        out3, "Error:true",
        "js_function slot must reject plain JS functions (only callback() wrappers)"
    );
}
