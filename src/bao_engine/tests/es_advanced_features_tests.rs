// @trace TEST-ENG-001-ADV [req:REQ-ENG-001] [level:unit]
// Advanced ES features deep tests: Proxy, Reflect, Symbol, Generator, WeakRef, iterators
// Single test due to mozjs single-init.

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
        Err(e) => format!("ERROR:{}", e.message),
    }
}

fn eval_bool(ctx: &mut JsContext, source: &str) -> bool {
    match ctx.eval(source, "<test>") {
        Ok(JsValue::Bool(b)) => b,
        _ => false,
    }
}

fn eval_number(ctx: &mut JsContext, source: &str) -> f64 {
    match ctx.eval(source, "<test>") {
        Ok(JsValue::Number(n)) => n,
        _ => f64::NAN,
    }
}

unsafe fn install_test_globals(
    cx: &mut mozjs::context::JSContext,
    global: mozjs::rust::Handle<*mut mozjs::jsapi::JSObject>,
) {
    bao_engine::host_fn::install_console(cx, global);
}

#[test]
fn test_es_advanced_features() {
    let mut ctx = JsContext::for_test().expect("JsContext");
    ctx.set_global_setup(install_test_globals);

    // =============================================
    // === Proxy deep tests ===
    // =============================================

    // Proxy get trap
    let result = eval_string(&mut ctx, r#"
        var p = new Proxy({x: 10}, {
            get: function(target, prop) { return prop in target ? target[prop] * 2 : 0; }
        });
        String(p.x)
    "#);
    assert_eq!(result, "20", "Proxy get should double value");

    // Proxy set trap
    let result = eval_number(&mut ctx, r#"
        var logged = [];
        var p = new Proxy({}, {
            set: function(target, prop, value) { logged.push(prop + '=' + value); target[prop] = value; return true; }
        });
        p.a = 1; p.b = 2;
        logged.length
    "#);
    assert_eq!(result, 2.0, "Proxy set should log 2 operations");

    // Proxy has trap
    assert!(eval_bool(&mut ctx, r#"
        var p = new Proxy({secret: true}, {
            has: function(target, prop) { return prop !== 'secret' && prop in target; }
        });
        'secret' in p === false && 'toString' in p === true
    "#), "Proxy has should hide 'secret'");

    // Proxy deleteProperty trap
    assert!(eval_bool(&mut ctx, r#"
        var deleted = [];
        var p = new Proxy({a: 1, b: 2}, {
            deleteProperty: function(target, prop) { deleted.push(prop); delete target[prop]; return true; }
        });
        delete p.a;
        deleted.length === 1 && deleted[0] === 'a'
    "#), "Proxy deleteProperty should work");

    // Proxy ownKeys trap
    let result = eval_string(&mut ctx, r#"
        var p = new Proxy({a: 1, b: 2, c: 3}, {
            ownKeys: function() { return ['a', 'b']; }
        });
        Object.keys(p).join(',')
    "#);
    assert_eq!(result, "a,b", "Proxy ownKeys should filter keys");

    // Proxy apply trap (function proxy)
    let result = eval_number(&mut ctx, r#"
        var fn = function(x, y) { return x + y; };
        var p = new Proxy(fn, {
            apply: function(target, thisArg, args) { return target(...args) * 10; }
        });
        p(3, 4)
    "#);
    assert_eq!(result, 70.0, "Proxy apply should multiply result by 10");

    // Proxy construct trap
    assert!(eval_bool(&mut ctx, r#"
        var Orig = function(x) { this.x = x; };
        var P = new Proxy(Orig, {
            construct: function(target, args) { return {x: args[0] * 2, proxied: true}; }
        });
        var obj = new P(5);
        obj.x === 10 && obj.proxied === true
    "#), "Proxy construct should work");

    // =============================================
    // === Reflect deep tests ===
    // =============================================

    // Reflect.get
    let result = eval_number(&mut ctx, r#"
        Reflect.get({x: 42}, 'x')
    "#);
    assert_eq!(result, 42.0, "Reflect.get should work");

    // Reflect.set
    assert!(eval_bool(&mut ctx, r#"
        var obj = {};
        Reflect.set(obj, 'key', 'val') && obj.key === 'val'
    "#), "Reflect.set should work");

    // Reflect.has
    assert!(eval_bool(&mut ctx, r#"
        Reflect.has({a: 1}, 'a') && !Reflect.has({a: 1}, 'b')
    "#), "Reflect.has should work");

    // Reflect.deleteProperty
    assert!(eval_bool(&mut ctx, r#"
        var obj = {x: 1};
        Reflect.deleteProperty(obj, 'x') && !('x' in obj)
    "#), "Reflect.deleteProperty should work");

    // Reflect.ownKeys
    let result = eval_string(&mut ctx, r#"
        Reflect.ownKeys({a: 1, b: 2}).join(',')
    "#);
    assert!(result.contains("a") && result.contains("b"), "Reflect.ownKeys should list keys, got: {}", result);

    // Reflect.apply
    let result = eval_number(&mut ctx, r#"
        Reflect.apply(Math.max, null, [1, 5, 3])
    "#);
    assert_eq!(result, 5.0, "Reflect.apply should call Math.max");

    // Reflect.construct
    assert!(eval_bool(&mut ctx, r#"
        function Foo(x) { this.x = x; }
        var obj = Reflect.construct(Foo, [99]);
        obj.x === 99 && obj instanceof Foo
    "#), "Reflect.construct should work");

    // Reflect.getPrototypeOf
    assert!(eval_bool(&mut ctx, r#"
        var proto = {method: function() {}};
        var obj = Object.create(proto);
        Reflect.getPrototypeOf(obj) === proto
    "#), "Reflect.getPrototypeOf should work");

    // =============================================
    // === Symbol deep tests ===
    // =============================================

    // Symbol unique
    assert!(eval_bool(&mut ctx, r#"
        var a = Symbol('x');
        var b = Symbol('x');
        a !== b
    "#), "Same-description Symbols should be unique");

    // Symbol.for (global registry)
    assert!(eval_bool(&mut ctx, r#"
        var a = Symbol.for('shared');
        var b = Symbol.for('shared');
        a === b
    "#), "Symbol.for should return same symbol for same key");

    // Symbol.keyFor
    let result = eval_string(&mut ctx, r#"
        var s = Symbol.for('mykey');
        Symbol.keyFor(s)
    "#);
    assert_eq!(result, "mykey", "Symbol.keyFor should return key");

    // Symbol as property key
    let result = eval_number(&mut ctx, r#"
        var sym = Symbol('id');
        var obj = {};
        obj[sym] = 123;
        obj[sym]
    "#);
    assert_eq!(result, 123.0, "Symbol should work as property key");

    // Symbol.iterator
    assert!(eval_bool(&mut ctx, r#"
        typeof Symbol.iterator === 'symbol'
    "#), "Symbol.iterator should be a symbol");

    // Well-known symbols exist
    assert!(eval_bool(&mut ctx, r#"
        typeof Symbol.toPrimitive === 'symbol' &&
        typeof Symbol.toStringTag === 'symbol' &&
        typeof Symbol.hasInstance === 'symbol'
    "#), "Well-known symbols should exist");

    // Symbol.toPrimitive customization
    let result = eval_string(&mut ctx, r#"
        var obj = {
            [Symbol.toPrimitive](hint) { return hint; }
        };
        String(obj) + '|' + Number(obj)
    "#);
    assert!(result.contains("string") || result.contains("number") || result.contains("default"),
        "Symbol.toPrimitive should be called, got: {}", result);

    // =============================================
    // === Generator deep tests ===
    // =============================================

    // Generator basic
    let result = eval_string(&mut ctx, r#"
        function* gen() { yield 1; yield 2; yield 3; }
        var g = gen();
        var r = [];
        var n;
        while (!(n = g.next()).done) r.push(n.value);
        r.join(',')
    "#);
    assert_eq!(result, "1,2,3", "Generator should yield values");

    // Generator with return
    let result = eval_string(&mut ctx, r#"
        function* gen() { yield 'a'; return 'done'; }
        var g = gen();
        g.next();
        var last = g.next();
        last.value + ':' + last.done
    "#);
    assert_eq!(result, "done:true", "Generator return should set done=true");

    // yield* delegation
    let result = eval_string(&mut ctx, r#"
        function* inner() { yield 'x'; yield 'y'; }
        function* outer() { yield 'a'; yield* inner(); yield 'b'; }
        var r = [];
        for (var v of outer()) r.push(v);
        r.join(',')
    "#);
    assert_eq!(result, "a,x,y,b", "yield* should delegate to inner generator");

    // Generator as iterable
    let result = eval_string(&mut ctx, r#"
        function* range(n) { for (var i = 0; i < n; i++) yield i; }
        Array.from(range(5)).join(',')
    "#);
    assert_eq!(result, "0,1,2,3,4", "Generator should be iterable");

    // Custom iterator with Symbol.iterator
    let result = eval_string(&mut ctx, r#"
        var obj = {
            data: [10, 20, 30],
            [Symbol.iterator]() {
                var i = 0;
                var data = this.data;
                return { next() { return i < data.length ? {value: data[i++], done: false} : {done: true}; } };
            }
        };
        var r = [];
        for (var v of obj) r.push(v);
        r.join(',')
    "#);
    assert_eq!(result, "10,20,30", "Custom iterator should work");

    // =============================================
    // === WeakRef + FinalizationRegistry ===
    // =============================================

    // WeakRef basic
    assert!(eval_bool(&mut ctx, r#"
        var target = {data: 'test'};
        var ref = new WeakRef(target);
        ref.deref() === target
    "#), "WeakRef should dereference to target");

    // WeakRef deref after target lost (cannot force GC, just check method exists)
    assert!(eval_bool(&mut ctx, r#"
        var ref = new WeakRef({x: 1});
        typeof ref.deref === 'function'
    "#), "WeakRef.deref should be function");

    // FinalizationRegistry exists
    assert!(eval_bool(&mut ctx, r#"
        typeof FinalizationRegistry === 'function'
    "#), "FinalizationRegistry should exist");

    // FinalizationRegistry register/unregister
    assert!(eval_bool(&mut ctx, r#"
        var fr = new FinalizationRegistry(function() {});
        var target = {id: 1};
        fr.register(target, 'held');
        typeof fr.unregister === 'function'
    "#), "FinalizationRegistry should have register/unregister");

    // =============================================
    // === Array/TypedArray advanced ===
    // =============================================

    // Array.from with mapping
    let result = eval_string(&mut ctx, r#"
        Array.from({length: 3}, function(_, i) { return i * 2; }).join(',')
    "#);
    assert_eq!(result, "0,2,4", "Array.from with map should work");

    // Array.flat
    let result = eval_string(&mut ctx, r#"
        [1, [2, 3], [4, [5]]].flat().join(',')
    "#);
    assert_eq!(result, "1,2,3,4,5", "Array.flat should flatten one level");

    // Array.flatMap
    let result = eval_string(&mut ctx, r#"
        [1, 2, 3].flatMap(function(x) { return [x, x * 2]; }).join(',')
    "#);
    assert_eq!(result, "1,2,2,4,3,6", "Array.flatMap should work");

    // TypedArray basic
    assert!(eval_bool(&mut ctx, r#"
        var arr = new Uint8Array([1, 2, 3, 4]);
        arr.length === 4 && arr[0] === 1 && arr instanceof Uint8Array
    "#), "Uint8Array should work");

    // Multiple TypedArray types
    assert!(eval_bool(&mut ctx, r#"
        typeof Int32Array === 'function' &&
        typeof Float64Array === 'function' &&
        typeof Uint16Array === 'function' &&
        typeof Int8Array === 'function'
    "#), "Multiple TypedArray types should exist");

    // =============================================
    // === Object advanced ===
    // =============================================

    // Object.assign
    let result = eval_string(&mut ctx, r#"
        var a = {x: 1}; var b = {y: 2};
        Object.assign(a, b);
        a.x + ',' + a.y
    "#);
    assert_eq!(result, "1,2", "Object.assign should merge");

    // Object.freeze
    assert!(eval_bool(&mut ctx, r#"
        var obj = {x: 1};
        Object.freeze(obj);
        obj.x = 2;
        obj.x === 1
    "#), "Object.freeze should prevent mutation");

    // Object.seal
    assert!(eval_bool(&mut ctx, r#"
        var obj = {x: 1};
        Object.seal(obj);
        obj.x = 2;
        obj.x === 2 && !('y' in obj)
    "#), "Object.seal should allow value changes but not add properties");

    // Object.getOwnPropertyDescriptors
    assert!(eval_bool(&mut ctx, r#"
        var desc = Object.getOwnPropertyDescriptors({get x() { return 1; }});
        desc.x && typeof desc.x.get === 'function'
    "#), "Object.getOwnPropertyDescriptors should return descriptor");

    // Object.is
    assert!(eval_bool(&mut ctx, r#"
        Object.is(NaN, NaN) && !Object.is(0, -0) && Object.is(1, 1)
    "#), "Object.is should distinguish NaN and -0");

    // =============================================
    // === Optional chaining + nullish coalescing ===
    // =============================================

    // Optional chaining
    let result = eval_string(&mut ctx, r#"
        var obj = {a: {b: {c: 42}}};
        obj?.a?.b?.c + ''
    "#);
    assert_eq!(result, "42", "Optional chaining should work");

    let result2 = eval_string(&mut ctx, r#"
        var obj = null;
        obj?.a?.b ?? 'fallback'
    "#);
    assert_eq!(result2, "fallback", "Optional chaining with null should work");

    // Nullish coalescing
    let result3 = eval_string(&mut ctx, r#"
        (null ?? 'default') + '|' + (undefined ?? 'default') + '|' + (0 ?? 'default') + '|' + ('' ?? 'default')
    "#);
    assert_eq!(result3, "default|default|0|", "Nullish coalescing should only trigger for null/undefined");

    std::mem::forget(ctx);
}
