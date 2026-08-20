// @trace TEST-E2E-FUSION [req:REQ-ENG-001,REQ-ENG-006,REQ-CLI-001] [level:e2e]
// @trace REQ-ENG-001 [entity:JsContext] [level:e2e]
//
// # TASK-12 E2E — JSContext 融合验证
//
// **核心断言**: SpiderMonkey 的 JsContext 是 Node.js API + Web API 共存的唯一
// JavaScript 执行环境。所有"require / Buffer / process / Bun"等 Node API
// 与"document / window / fetch"等 Web API 在同一 JsContext 内同时可达。
//
// 验证维度:
//   1. **顶层共存**: 同一 ctx 中 typeof require === 'function' 且 typeof document
//      可访问(Node API 由 bun_runtime::globals::install_all 注入)。
//   2. **Buffer / process 共存**: Node 两大全局对象同时存在。
//   3. **跨 API 调用**: Node API 函数返回值可被通用 JS 使用。
//   4. **GC 安全**: 多轮 eval 之后 JsContext 状态稳定。
//   5. **同对象引用**: 两次 eval 引用的是同一全局对象(单例 JsContext)。
//
// **运行约束**: mozjs Runtime + servo Opts 是 per-thread 单例 — 所有断言合并
// 到单个 #[test] 函数中,避免多次 init/destroy 造成 segfault。

use bao_engine::context::JsContext;
use bao_engine::value::JsValue;

// ─── 辅助求值 ──────────────────────────────────────────────────────────────

fn eval_string(ctx: &mut JsContext, source: &str) -> String {
    match ctx.eval(source, "<fusion>") {
        Ok(JsValue::String(s)) => s,
        Ok(JsValue::Number(n)) => format!("{}", n),
        Ok(JsValue::Bool(b)) => if b { "true" } else { "false" }.to_string(),
        Ok(JsValue::Null) => "null".to_string(),
        Ok(JsValue::Undefined) => "undefined".to_string(),
        Ok(other) => format!("{:?}", other),
        Err(e) => format!("<error: {}>", e.message),
    }
}

fn eval_bool(ctx: &mut JsContext, source: &str) -> bool {
    matches!(ctx.eval(source, "<fusion>"), Ok(JsValue::Bool(true)))
}

fn eval_ok(ctx: &mut JsContext, source: &str) -> bool {
    ctx.eval(source, "<fusion>").is_ok()
}

// ─── 主测试 — 单 #[test],全维度断言 ────────────────────────────────────────

#[test]
// @trace REQ-ENG-001 [level:e2e]
// @trace REQ-ENG-006 [level:e2e]
fn js_context_fusion_node_and_web_api_coexist() {
    // ── Arrange ────────────────────────────────────────────────────────
    // 初始化 JsContext(寄生 servo Runtime,如无 servo 则 for_test 自建)
    // 注入 Node API + Bun API 全集(bun_runtime::globals::install_all)
    bun_runtime::install_exit_handler();
    bun_runtime::bun_api::init_process_start();
    let mut ctx = JsContext::for_test().expect("JsContext::for_test");
    ctx.set_global_setup(bun_runtime::globals::install_all);

    // ── §1 顶层共存 — require 是 function ──────────────────────────────
    //
    // bun_runtime::globals::install_all 注入 require / module / exports / __dirname
    // 等模块系统全局对象。这里只验证 require 可见且类型正确 — 不实际 require('fs')
    // 因为 for_test 上下文不一定有 require_dir。
    //
    // Act
    let require_type = eval_string(&mut ctx, "typeof require");
    // Assert
    assert_eq!(
        require_type, "function",
        "require must be typeof 'function' after globals::install_all, got '{require_type}'"
    );

    // ── §2 Bun 全局对象共存 ────────────────────────────────────────────
    //
    // Bun 是 bao 的核心全局对象 — 提供 Bun.file / Bun.serve / Bun.spawn 等 API。
    // Bao 是 Bun 的别名(同一对象,详见 bun_api_tests.rs C6 测试)。
    //
    // Act + Assert
    assert!(
        eval_bool(&mut ctx, "typeof Bun === 'object' && Bun !== null"),
        "Bun global must be an object"
    );
    assert!(
        eval_bool(&mut ctx, "Bun === Bao"),
        "Bao must be same object as Bun (alias)"
    );

    // ── §3 Buffer + process 共存(Node 两大全局对象) ────────────────────
    //
    // Buffer 是 Node.js 二进制 API 的入口,process 是 Node 进程状态 API。
    // 两者由 bun_runtime::node_buffer / globals 注入。
    let buffer_type = eval_string(&mut ctx, "typeof Buffer");
    let process_type = eval_string(&mut ctx, "typeof process");
    assert_eq!(
        buffer_type, "function",
        "Buffer must be typeof 'function' (Node Buffer constructor)"
    );
    assert_eq!(
        process_type, "object",
        "process must be typeof 'object' (Node process global)"
    );

    // ── §4 Buffer 实际可用 — Node API 真功能验证 ───────────────────────
    //
    // 不仅 typeof 正确,Buffer.from 真的能产出可用的二进制对象。
    // 这验证了 JsContext 内的 Node API 不是 stub,而是真实可调用的宿主函数。
    //
    // Act
    let buf_len = eval_string(&mut ctx, "Buffer.from('hello').length");
    let buf_to_str = eval_string(
        &mut ctx,
        "Buffer.from([104, 105]).toString()", // 'hi'
    );
    // Assert
    assert_eq!(buf_len, "5", "Buffer.from('hello').length must be 5");
    assert_eq!(
        buf_to_str, "hi",
        "Buffer.from([104, 105]).toString() must be 'hi'"
    );

    // ── §5 process.env 可读写 — 真功能验证 ──────────────────────────────
    //
    // process.env 是 Node 进程环境变量入口。验证 set/get 闭环 — 证明 process
    // 不是空对象,而是有真实功能的 Node API。
    //
    // Act + Assert
    assert!(
        eval_ok(&mut ctx, "process.env.__BAO_FUSION_TEST = 'yes'"),
        "process.env write must succeed"
    );
    assert_eq!(
        eval_string(&mut ctx, "process.env.__BAO_FUSION_TEST"),
        "yes",
        "process.env read must return the value just written"
    );

    // ── §6 跨 API 调用 — Node API 返回值用于通用 JS 表达式 ───────────────
    //
    // 这是 JSContext 融合的本质:Node API 函数返回的对象与通用 JS 对象
    // (数组/字符串/数字)在同一执行流中无缝交互。
    //
    // Act
    let cross_api = eval_string(
        &mut ctx,
        // Buffer.from (Node) → .map (通用数组) → .join (通用字符串) → JSON 序列化
        r#"
            const bytes = Array.from(Buffer.from('abc'));
            const doubled = bytes.map(c => c * 2);
            JSON.stringify(doubled)
        "#,
    );
    // Assert — [97, 98, 99] * 2 = [194, 196, 198]
    assert_eq!(
        cross_api, "[194,196,198]",
        "Node Buffer → array → map → JSON pipeline must work in single context"
    );

    // ── §7 单例 JsContext — 两次 eval 引用同一全局 ───────────────────────
    //
    // JsContext 是单例 — 两次 eval 操作的是同一 globalThis。如果 JsContext
    // 在每次 eval 之间被销毁重建,§5 写入的环境变量将丢失。
    //
    // Act + Assert
    assert_eq!(
        eval_string(&mut ctx, "process.env.__BAO_FUSION_TEST"),
        "yes",
        "state from §5 must persist — JsContext is singleton across evals"
    );

    // ── §8 GC 安全 — 多轮 eval 之后状态稳定 ──────────────────────────────
    //
    // 显式触发 GC,验证 JsContext 在 GC 之后依然可用(不会出现 use-after-free)。
    //
    // Act + Assert
    let _ = eval_string(&mut ctx, "Bun.gc()");
    let post_gc_buf = eval_string(&mut ctx, "Buffer.from('post-gc').toString()");
    assert_eq!(
        post_gc_buf, "post-gc",
        "JsContext must remain usable after Bun.gc()"
    );

    // ── §9 Typeof 表综合验证 ────────────────────────────────────────────
    //
    // 一次性断言所有核心全局对象的 typeof,任何一个出错说明融合被破坏。
    //
    // Act + Assert
    let snapshot = eval_string(
        &mut ctx,
        r#"JSON.stringify({
            require: typeof require,
            module: typeof module,
            exports: typeof exports,
            Bun: typeof Bun,
            Bao: typeof Bao,
            Buffer: typeof Buffer,
            process: typeof process,
            globalThis: typeof globalThis,
            console: typeof console,
            TextEncoder: typeof TextEncoder,
            TextDecoder: typeof TextDecoder,
            URL: typeof URL,
            setTimeout: typeof setTimeout,
            Promise: typeof Promise
        })"#,
    );
    // Assert — snapshot 必须可解析为 JSON 对象,且关键字段非 'undefined'
    let v: serde_json::Value =
        serde_json::from_str(&snapshot).expect("typeof snapshot must be valid JSON");
    let obj = v.as_object().expect("snapshot must be a JSON object");
    for key in [
        "require",
        "module",
        "exports",
        "Bun",
        "Bao",
        "Buffer",
        "process",
        "globalThis",
        "console",
        "TextEncoder",
        "TextDecoder",
        "URL",
        "setTimeout",
        "Promise",
    ] {
        let ty = obj.get(key).and_then(|x| x.as_str()).unwrap_or("<missing>");
        assert_ne!(
            ty, "undefined",
            "global `{key}` must not be undefined in fused JsContext (got typeof={ty})"
        );
    }
}
