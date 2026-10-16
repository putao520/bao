// @trace TEST-ENG-21 [req:REQ-ENG-21] [level:integration]
// Bun.DNSResolver 验收标准逐条测试(SPEC REQ-ENG-21 C1-C4),按验收标准
// 一条一测函数:
//   c1_surface            — C1 typeof Bun.DNSResolver === 'function'(默认真跑)
//   c2_constructor        — C2 new Bun.DNSResolver() 创建解析器(默认真跑)
//   arg_validation_faces  — 抛错语义面(默认真跑;失败路径零系统调用)
//   c3_resolve_async      — C3 resolve() 返回 Promise<DNSRecord[]>(
//                           真查 resolver,#[ignore] env-gate)
//   c4_resolve4_resolve6  — C4 resolve4/resolve6 IPv4/IPv6 查询(
//                           真查 resolver,#[ignore] env-gate)
//
// evaluate_js 探针形态(Rust 测试函数内 JS 探针取值断言)。
// 环境分界:构造器/typeof/抛错语义是 runtime-only face 默认真跑;真查
// resolver 属真外部依赖 → `#[ignore]` 显式 env-gate(nextest 显示 SKIP,
// 非静默 return),真跑载体:
//   BAO_TEST_DNS=1 cargo nextest run --run-ignored=all -p bun_runtime -E 'test(bun_dns_resolver)'
// 门内主机名用 `localhost`(OS resolver /etc/hosts 确定可达,零外网硬需;
// 门为 resolver 查询面的统一环境闸,缺外网 DNS 的环境同样适用)。
#[path = "common/mod.rs"]
mod common;
use common::{drive_event_loop, eval_string, setup_ctx};

/// C1: typeof Bun.DNSResolver === 'function'
#[test]
fn c1_surface() {
    let mut ctx = setup_ctx();
    assert_eq!(
        eval_string(&mut ctx, "typeof Bun.DNSResolver"),
        "function",
        "C1: Bun.DNSResolver 必须以构造器函数暴露"
    );
}

/// C2: new Bun.DNSResolver() 创建 DNS 解析器
#[test]
fn c2_constructor() {
    let mut ctx = setup_ctx();
    assert_eq!(
        eval_string(
            &mut ctx,
            r#"(function(){
                var r = new Bun.DNSResolver();
                if (typeof r !== 'object') return 'not-object:' + typeof r;
                if (typeof r.resolve !== 'function') return 'no-resolve';
                if (typeof r.resolve4 !== 'function') return 'no-resolve4';
                if (typeof r.resolve6 !== 'function') return 'no-resolve6';
                return 'ok';
            })()"#
        ),
        "ok",
        "C2: new Bun.DNSResolver() 必须产出带 resolve/resolve4/resolve6 的实例"
    );
}

/// 抛错语义面(默认真跑;两条失败路径都在 getaddrinfo 之前拒绝,零
/// 系统 调用零外部依赖,确定性):
///   * 非字符串主机名 → reject ERR_INVALID_ARG_TYPE
///   * 内嵌 NUL 主机名(getaddrinfo 入参不可表达)→ reject ENOTFOUND
#[test]
fn arg_validation_faces() {
    let mut ctx = setup_ctx();

    ctx.eval(
        r#"
        globalThis.__dnsarg = "unset";
        new Bun.DNSResolver().resolve(42).then(function(recs) {
            globalThis.__dnsarg = "unexpected-resolve:" + JSON.stringify(recs);
        }, function(e) {
            globalThis.__dnsarg = "REJ:" + (e && e.code ? e.code : e);
        });
        "#,
        "<bun-dns-test>",
    )
    .expect("arg-check setup");
    drive_event_loop(&mut ctx, 20);
    let argrej = eval_string(&mut ctx, "globalThis.__dnsarg");
    assert!(
        argrej.starts_with("REJ:") && argrej.contains("ERR_INVALID_ARG_TYPE"),
        "非字符串主机名必须 reject ERR_INVALID_ARG_TYPE,got: {argrej}"
    );

    // 'a(\u0000)':JS 侧含 NUL 的字符串,Rust 侧 CString 化失败/宽字符管道
    // NUL 检查直接失败(所有平台确定性,零网络)。
    ctx.eval(
        r#"
        globalThis.__dnsbad = "unset";
        new Bun.DNSResolver().resolve('a\u0000b').then(function(recs) {
            globalThis.__dnsbad = "unexpected-resolve:" + JSON.stringify(recs);
        }, function(e) {
            globalThis.__dnsbad = "REJ:" + (e && e.code ? e.code : e);
        });
        "#,
        "<bun-dns-test>",
    )
    .expect("reject-path setup");
    drive_event_loop(&mut ctx, 20);
    let rej = eval_string(&mut ctx, "globalThis.__dnsbad");
    assert!(
        rej.starts_with("REJ:"),
        "内嵌 NUL 主机名必须 reject(真实失败,禁静默返空),got: {rej}"
    );
    assert!(
        rej.contains("ENOTFOUND"),
        "解析面失败必须携带 ENOTFOUND 错误码,got: {rej}"
    );
}

/// C3: resolver.resolve('localhost') 返回 Promise<DNSRecord[]>
/// (DNSRecord = { address, family, ttl };真查 OS resolver)
#[test]
#[ignore = "[env-gate] 需 BAO_TEST_DNS=1; 真跑: BAO_TEST_DNS=1 cargo nextest run --run-ignored=all -p bun_runtime -E 'test(bun_dns_resolver)'"]
fn c3_resolve_async() {
    let mut ctx = setup_ctx();
    assert_eq!(
        eval_string(
            &mut ctx,
            r#"(function(){
                var r = new Bun.DNSResolver();
                var p = r.resolve('localhost');
                return p instanceof Promise ? 'promise' : 'not-promise:' + p;
            })()"#
        ),
        "promise",
        "C3: resolve 必须返回 Promise"
    );
    ctx.eval(
        r#"
        globalThis.__dns = "unset";
        var r3 = new Bun.DNSResolver();
        r3.resolve('localhost').then(function(recs) {
            if (!Array.isArray(recs)) { globalThis.__dns = "not-array"; return; }
            if (recs.length === 0) { globalThis.__dns = "empty"; return; }
            for (var i = 0; i < recs.length; i++) {
                var rec = recs[i];
                if (typeof rec.address !== 'string' || rec.address.length === 0) {
                    globalThis.__dns = "bad-address@" + i; return;
                }
                if (rec.family !== 4 && rec.family !== 6) {
                    globalThis.__dns = "bad-family@" + i; return;
                }
                if (typeof rec.ttl !== 'number') {
                    globalThis.__dns = "bad-ttl@" + i; return;
                }
            }
            globalThis.__dns = "ok:" + recs.length;
        }, function(e) {
            globalThis.__dns = "ERR:" + (e && e.code ? e.code : e);
        });
        "#,
        "<bun-dns-test>",
    )
    .expect("resolve then setup");
    drive_event_loop(&mut ctx, 20);
    let c3 = eval_string(&mut ctx, "globalThis.__dns");
    assert!(
        c3.starts_with("ok:"),
        "C3: resolve('localhost') 必须 resolve 非空 DNSRecord(shape address/family/ttl),got: {c3}"
    );
}

/// C4: resolver.resolve4/resolve6 IPv4/IPv6 查询正常(真查 OS resolver)
#[test]
#[ignore = "[env-gate] 需 BAO_TEST_DNS=1; 真跑: BAO_TEST_DNS=1 cargo nextest run --run-ignored=all -p bun_runtime -E 'test(bun_dns_resolver)'"]
fn c4_resolve4_resolve6() {
    let mut ctx = setup_ctx();

    // C4a: resolve4 — 必须全 family=4 的点分四段记录。
    ctx.eval(
        r#"
        globalThis.__dns4 = "unset";
        new Bun.DNSResolver().resolve4('localhost').then(function(recs) {
            if (!Array.isArray(recs) || recs.length === 0) {
                globalThis.__dns4 = "empty"; return;
            }
            for (var i = 0; i < recs.length; i++) {
                if (recs[i].family !== 4) { globalThis.__dns4 = "family@" + i; return; }
                if (!/^\d+\.\d+\.\d+\.\d+$/.test(recs[i].address)) {
                    globalThis.__dns4 = "v4-shape@" + i + ":" + recs[i].address; return;
                }
            }
            globalThis.__dns4 = "ok:" + recs[0].address;
        }, function(e) {
            globalThis.__dns4 = "ERR:" + (e && e.code ? e.code : e);
        });
        "#,
        "<bun-dns-test>",
    )
    .expect("resolve4 then setup");
    drive_event_loop(&mut ctx, 20);
    let c4a = eval_string(&mut ctx, "globalThis.__dns4");
    assert!(
        c4a.starts_with("ok:"),
        "C4: resolve4('localhost') 必须 resolve 全 family=4 的 IPv4 记录,got: {c4a}"
    );

    // C4b: resolve6 — resolve 全 family=6;无 v6 栈主机上 OS resolver 如实
    // 失败(Bun 同形)即干净 reject,机制两臂均判。
    ctx.eval(
        r#"
        globalThis.__dns6 = "unset";
        new Bun.DNSResolver().resolve6('localhost').then(function(recs) {
            if (!Array.isArray(recs) || recs.length === 0) {
                globalThis.__dns6 = "empty"; return;
            }
            for (var i = 0; i < recs.length; i++) {
                if (recs[i].family !== 6) { globalThis.__dns6 = "family@" + i; return; }
                if (recs[i].address.indexOf(':') < 0) {
                    globalThis.__dns6 = "v6-shape@" + i + ":" + recs[i].address; return;
                }
            }
            globalThis.__dns6 = "ok:" + recs[0].address;
        }, function(e) {
            globalThis.__dns6 = "REJ:" + (e && e.code ? e.code : e);
        });
        "#,
        "<bun-dns-test>",
    )
    .expect("resolve6 then setup");
    drive_event_loop(&mut ctx, 20);
    let c4b = eval_string(&mut ctx, "globalThis.__dns6");
    assert!(
        c4b.starts_with("ok:") || c4b.starts_with("REJ:"),
        "C4: resolve6('localhost') 必须 resolve 全 family=6 或 v4-only 环境干净 reject,got: {c4b}"
    );
}
