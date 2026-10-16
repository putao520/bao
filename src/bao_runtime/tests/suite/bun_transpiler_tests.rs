// @trace TEST-ENG-19 [req:REQ-ENG-19] [level:integration]
// Bun.Transpiler 验收标准逐条测试(SPEC REQ-ENG-19 C1-C5),按验收标准
// 一条一测函数:
//   c1_constructor_surface — C1 typeof Bun.Transpiler === 'function'
//   c2_loader_option       — C2 new Bun.Transpiler({loader:'ts'}) 创建转译器
//   c3_transform_sync      — C3 transformSync('const x: number = 1') 转译 TS
//   c4_transform_async     — C4 transform 返回 Promise(异步版本)
//   c5_jsx_tsx             — C5 JSX/TSX 转译支持
//
// evaluate_js 探针形态(Rust 测试函数内 JS 探针取值断言,同 node_crypto/
// node_zlib 族);runtime-only face:纯本地转译,无 DISPLAY/网络依赖,
// canonical env 无需门控,默认真跑。
//
// nextest 每 test 独立进程,各函数独立建 JSContext(common::setup_ctx,
// JSEngine 进程级单例由 JSENGINE_INIT_LOCK 串行化)。
#[path = "common/mod.rs"]
mod common;
use common::{drive_event_loop, eval_string, setup_ctx};

fn eval_ok(source: &str) -> bool {
    let mut ctx = setup_ctx();
    ctx.eval(source, "<bun-transpiler-test>").is_ok()
}

/// C1: typeof Bun.Transpiler === 'function'
#[test]
fn c1_constructor_surface() {
    let mut ctx = setup_ctx();
    assert_eq!(
        eval_string(&mut ctx, "typeof Bun.Transpiler"),
        "function",
        "C1: Bun.Transpiler 必须以构造器函数暴露"
    );
}

/// C2: new Bun.Transpiler({loader:'ts'}) 创建转译器
#[test]
fn c2_loader_option() {
    let mut ctx = setup_ctx();
    assert_eq!(
        eval_string(
            &mut ctx,
            r#"(function(){
                var t = new Bun.Transpiler({ loader: 'ts' });
                if (typeof t !== 'object') return 'not-object:' + typeof t;
                if (typeof t.transformSync !== 'function') return 'no-transformSync';
                if (typeof t.transform !== 'function') return 'no-transform';
                return 'ok';
            })()"#
        ),
        "ok",
        "C2: new Bun.Transpiler(loader:'ts') 必须产出带 transformSync/transform 的实例"
    );

    // 负向面:loader 校验 fail-closed(禁静默回落)。
    assert!(
        !eval_ok("new Bun.Transpiler({ loader: 'css' })"),
        "未知 loader 必须抛错(fail-closed)"
    );
    assert!(
        !eval_ok("new Bun.Transpiler({ loader: 42 })"),
        "非字符串 loader 必须抛错"
    );

    // 省略 options 的构造(上游默认 loader jsx)必须可用。
    assert_eq!(
        eval_string(
            &mut ctx,
            r#"(function(){
                var t = new Bun.Transpiler();
                var out = t.transformSync('const a = 1');
                return out.indexOf('const a = 1') >= 0 ? 'ok' : 'default-loader-broken:' + out;
            })()"#
        ),
        "ok",
        "省略 options 的构造必须产出可用转译器(默认 loader)"
    );
}

/// C3: transformSync 转译 TypeScript(类型擦除 + 产物可执行语义)
#[test]
fn c3_transform_sync() {
    let mut ctx = setup_ctx();
    assert_eq!(
        eval_string(
            &mut ctx,
            r#"(function(){
                var t = new Bun.Transpiler({ loader: 'ts' });
                var out = t.transformSync('const x: number = 1');
                if (out.indexOf('const x = 1') < 0) return 'missing-strip:' + out;
                if (out.indexOf('number') >= 0) return 'type-annotation-left:' + out;
                return 'ok';
            })()"#
        ),
        "ok",
        "C3: transformSync('const x: number = 1') 必须擦除类型注记"
    );

    // 语义保真:转译产物在本 runtime 内求值后真调用(不是字符串比对冒充)。
    assert_eq!(
        eval_string(
            &mut ctx,
            r#"(function(){
                var t = new Bun.Transpiler({ loader: 'ts' });
                var out = t.transformSync('function add(a: number, b: number): number { return a + b; }');
                eval(out);
                return typeof add === 'function' && add(2, 3) === 5 ? 'ok' : 'bad-call:' + out;
            })()"#
        ),
        "ok",
        "C3: 转译产物必须是本 runtime 可执行的真实 JS(转译后 add(2,3)===5)"
    );

    assert_eq!(
        eval_string(
            &mut ctx,
            r#"(function(){
                var t = new Bun.Transpiler({ loader: 'ts' });
                var out = t.transformSync('interface User { id: number } type ID = number; const u: User = { id: 1 };');
                if (out.indexOf('interface') >= 0 || out.indexOf('type ID') >= 0) return 'type-shape-left:' + out;
                if (out.indexOf('const u =') < 0) return 'value-lost:' + out;
                return 'ok';
            })()"#
        ),
        "ok",
        "C3: interface/type alias 擦除,值形态保留"
    );

    // 负向面:非字符串入参必须抛错。
    assert!(
        !eval_ok("new Bun.Transpiler({ loader: 'ts' }).transformSync(42)"),
        "transformSync 非字符串入参必须抛错"
    );
}

/// C4: transform 返回 Promise(异步版本,resolve 真实转译产物)
#[test]
fn c4_transform_async() {
    let mut ctx = setup_ctx();
    assert_eq!(
        eval_string(
            &mut ctx,
            r#"(function(){
                var t = new Bun.Transpiler({ loader: 'ts' });
                var p = t.transform('const y: number = 2');
                return p instanceof Promise ? 'promise' : 'not-promise:' + p;
            })()"#
        ),
        "promise",
        "C4: transform 必须返回 Promise"
    );
    ctx.eval(
        r#"
        globalThis.__tr = "unset";
        var t4 = new Bun.Transpiler({ loader: 'ts' });
        t4.transform('const y: number = 2').then(function(s) {
            globalThis.__tr = s;
        }, function(e) {
            globalThis.__tr = "ERR:" + e;
        });
        "#,
        "<bun-transpiler-test>",
    )
    .expect("transform then setup");
    drive_event_loop(&mut ctx, 20);
    assert_eq!(
        eval_string(&mut ctx, "globalThis.__tr"),
        "const y = 2;\n",
        "C4: transform 的 Promise 必须 resolve 真实转译产物"
    );
}

/// C5: JSX/TSX 转译支持
#[test]
fn c5_jsx_tsx() {
    let mut ctx = setup_ctx();
    assert_eq!(
        eval_string(
            &mut ctx,
            r#"(function(){
                var t = new Bun.Transpiler({ loader: 'tsx' });
                var out = t.transformSync('const el: JSX.Element = <div>{42}</div>;');
                if (out.indexOf('<div>') < 0) return 'jsx-lost:' + out;
                if (out.indexOf('42') < 0) return 'child-lost:' + out;
                if (out.indexOf('JSX.Element') >= 0) return 'type-left:' + out;
                return 'ok';
            })()"#
        ),
        "ok",
        "C5: TSX 源必须转译成功(JSX 保留 + TS 注记擦除)"
    );
    assert_eq!(
        eval_string(
            &mut ctx,
            r#"(function(){
                var t = new Bun.Transpiler({ loader: 'jsx' });
                var out = t.transformSync('const el = <span>hi</span>;');
                return out.indexOf('<span>') >= 0 ? 'ok' : 'jsx-lost:' + out;
            })()"#
        ),
        "ok",
        "C5: JSX loader 解析 JSX 语法"
    );
}
