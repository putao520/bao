// @trace REQ-BRW-017 [level:e2e]
// @trace REQ-BRW-018 [level:e2e]
// @trace REQ-BRW-019 [level:e2e]
// @trace REQ-BRW-024 [level:e2e]
// @trace REQ-BRW-025 [level:e2e]
// @trace REQ-BRW-027 [level:e2e]
// @trace REQ-BRW-030 [level:e2e]
//
// ═══════════════════════════════════════════════════════════════════════════
// 页面平台 API conformance 观测面(REQ-BRW-017/18/19/24/25/27/30)
// ═══════════════════════════════════════════════════════════════════════════
//
// **背景**:e-cover71 覆盖审计(2026-10-16)实测这批 REQ 全域 grep 零专项载体
// ——servo vendor 已实现这些页面 API,但 bao 侧无任何 e2e 观测面,SPEC
// 覆盖矩阵把它们记为「无测试」。本文件补观测面:真页面导航 + evaluate
// 探针断言 getter 存在性与基础行为。
//
// **断言纪律**:每条 criterion 一条具名断言,断言真实行为(构造成功/回调真
// 触发/对象真返回),非形状壳。servo Opts 是 per-process 单例,故每 #[test]
// 单建 BrowserRuntime(nextest 每 test 独立进程隔离;plain cargo test 须
// --test-threads=1)。
//
// **观测面 = Window Realm**(evaluate_js_web):页面脚本真实可见的 API。
//
// **已知实现缺口(非测试缺口,本文件不补)**:
//   - REQ-BRW-027-C3/C4/C5 `Cache.put`/`match`/`delete`:servo vendor 的
//     `Cache` 接口只实现了 `keys()`(Cache.webidl 仅声明 keys),三方法
//     undefined —— 实现缺口,另立单;本文件只断言既有 C1/C2 + keys。

use bao_browser::{BaoConfig, BrowserRuntime, PageConfig, PageHandle, PagePool, PageState};
use std::time::{Duration, Instant};

// ─── fixture helpers ─────────────────────────────────────────────────────────

/// data: URL 包装('#' 转义 %23 — data URL fragment 保留字)。
fn data_url(html: &str) -> String {
    format!("data:text/html;charset=utf-8,{}", html.replace('#', "%23"))
}

/// 等待 Page 进入 Interactive/Idle;每轮 noop evaluate 驱动 servo 泵
/// (memory live-test-wait-loop-must-drive-pump:只轮询 Rust 侧会饿死惰性泵)。
fn wait_for_load_and_drain(page: &PageHandle, max_ms: u64) {
    let start = Instant::now();
    while start.elapsed().as_millis() < max_ms as u128 {
        let _ = page.evaluate_js("");
        if matches!(page.get_state(), PageState::Interactive | PageState::Idle) {
            return;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
}

/// 单页 fixture:创建 page + 泵驱动等 load。
fn probe_page(pool: &PagePool, html: &str) -> PageHandle {
    let page = pool
        .create_page(&PageConfig {
            url: Some(data_url(html)),
            ..Default::default()
        })
        .expect("pool.create_page failed");
    wait_for_load_and_drain(&page, 20_000);
    page
}

/// 泵驱动等待页面侧 `globalThis.<slot>` 离开 'pending'(异步回调在 servo
/// 脚本线程事件循环转圈时触发;每轮 noop evaluate 驱动泵)。
fn pump_until(page: &PageHandle, slot: &str, max_ms: u64) -> String {
    let start = Instant::now();
    while start.elapsed().as_millis() < max_ms as u128 {
        let _ = page.evaluate_js("");
        if let Ok(s) = page.evaluate_js_web(&format!("String(globalThis.{slot})")) {
            let s = s.trim().trim_matches('"').to_string();
            if s != "pending" {
                return s;
            }
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    "TIMEOUT".to_string()
}

// ═══════════════════════════════════════════════════════════════════════════
// REQ-BRW-017 Notification API
// ═══════════════════════════════════════════════════════════════════════════
//
// servo 实现:vendor/servo/components/script/dom/serviceworker/notification.rs
// (constructor/静态 permission/requestPermission/事件 handler 全集)。
// bao 侧 BrowserRuntime::new 翻转 `dom_notification_enabled`(lib.rs),否则
// webidl `[Pref=]` 门使 Notification 全局不可见。

#[test]
// @trace REQ-BRW-017 [level:e2e]
fn notification_api_surface_and_construction() {
    let runtime = BrowserRuntime::new(BaoConfig::default()).expect("BrowserRuntime::new failed");
    let pool: &PagePool = runtime.page_pool();
    let page = probe_page(pool, "<!DOCTYPE html><html><body>notif</body></html>");

    // C1: typeof Notification === 'function';C2: permission 是状态字符串;
    // C3: requestPermission() 返回 Promise;C4: 构造器接受 title/options。
    let dump = page
        .evaluate_js_web(
            r#"(function(){
                var out = [];
                function p(k, v){ out.push(k + '=' + v); }
                p('ctype', typeof Notification);
                p('permission', typeof Notification === 'function' ? Notification.permission : 'n/a');
                p('maxActions', typeof Notification === 'function' ? Notification.maxActions : 'n/a');
                try {
                    var rp = Notification.requestPermission();
                    p('rp_is_promise', (rp && typeof rp.then === 'function') ? 'true' : 'false');
                } catch(e) { p('rp_is_promise', 'ERR:'+e); }
                try {
                    var n = new Notification('bao-title', { body: 'bao-body' });
                    p('ctor_title', n.title);
                    p('ctor_body', n.body);
                    p('onerror', typeof n.onerror);
                    p('onshow', typeof n.onshow);
                    p('onclick', typeof n.onclick);
                    p('onclose', typeof n.onclose);
                } catch(e) { p('ctor_title', 'ERR:'+e); }
                return out.join('|');
            })()"#,
        )
        .unwrap_or_else(|e| panic!("evaluate failed: {e}"));

    let fields: std::collections::HashMap<_, _> = dump
        .trim()
        .trim_matches('"')
        .split('|')
        .filter_map(|kv| kv.split_once('='))
        .collect();

    assert_eq!(fields.get("ctype"), Some(&"function"), "C1: typeof Notification");
    assert_eq!(
        fields.get("permission"),
        Some(&"default"),
        "C2: Notification.permission (secure context, nothing granted => default)"
    );
    assert_eq!(fields.get("maxActions"), Some(&"2"), "Notification.maxActions");
    assert_eq!(
        fields.get("rp_is_promise"),
        Some(&"true"),
        "C3: requestPermission() returns a Promise"
    );
    assert_eq!(
        fields.get("ctor_title"),
        Some(&"bao-title"),
        "C4: new Notification(title, options) constructs and exposes .title"
    );
    assert_eq!(fields.get("ctor_body"), Some(&"bao-body"), "C4: .body echoes options");
    // C5: 事件 handler 属性在位(未注册时为 null => typeof 'object')。
    for h in ["onerror", "onshow", "onclick", "onclose"] {
        assert_eq!(fields.get(h), Some(&"object"), "C5: notification.{h} event handler slot");
    }

    // C5(行为面):权限非 granted 时构造器 queue 一个 error 事件 —— 证明
    // 事件派发通路真活(非仅属性槽)。
    let _ = page.evaluate_js_web(
        r#"(function(){
            globalThis.__ne = 'pending';
            try { var n = new Notification('t', { body: 'm' }); n.onerror = function(){ globalThis.__ne = 'error-fired'; }; }
            catch(e) { globalThis.__ne = 'ERR:'+e; }
        })()"#,
    );
    let event = pump_until(&page, "__ne", 5_000);
    assert_eq!(
        event, "error-fired",
        "C5: notification error event fires when permission is not granted"
    );
}

// ═══════════════════════════════════════════════════════════════════════════
// REQ-BRW-018 IntersectionObserver
// ═══════════════════════════════════════════════════════════════════════════
//
// servo 实现:vendor/servo/components/script/dom/intersectionobserver/ ——
// notification algorithm 由 script_thread.rs 的 update-the-rendering steps 驱动。
// bao 侧翻转 `dom_intersection_observer_enabled`。

#[test]
// @trace REQ-BRW-018 [level:e2e]
fn intersection_observer_surface_and_callback() {
    let runtime = BrowserRuntime::new(BaoConfig::default()).expect("BrowserRuntime::new failed");
    let pool: &PagePool = runtime.page_pool();
    let page = probe_page(
        pool,
        "<!DOCTYPE html><html><body>\
         <div id='vis' style='width:120px;height:120px;background:red'>v</div>\
         <div style='height:4000px'>spacer</div>\
         </body></html>",
    );

    // C1: typeof === 'function';C2: new IntersectionObserver(cb, {threshold});
    // C3: observe(element) 不抛。
    let issued = page
        .evaluate_js_web(
            r#"(function(){
                globalThis.__io_cb = 'pending';
                globalThis.__io_ctor = 'pending';
                globalThis.__io_observe = 'pending';
                globalThis.__io_disconnect = 'pending';
                globalThis.__io_entry_type = 'pending';
                var out = [];
                function p(k, v){ out.push(k + '=' + v); }
                p('ctype', typeof IntersectionObserver);
                try {
                    var io = new IntersectionObserver(function(entries){
                        globalThis.__io_cb = 'fired:' + entries.length;
                        if (entries.length > 0 && entries[0]) {
                            var isEntry = (typeof IntersectionObserverEntry === 'function')
                                ? (entries[0] instanceof IntersectionObserverEntry ? 'instanceof' : 'notinstance')
                                : 'noEntryCtor';
                            globalThis.__io_entry_type = typeof entries[0].isIntersecting + ',' + isEntry;
                        }
                    }, { threshold: 0.5 });
                    globalThis.__io_ctor = (typeof io === 'object' && io !== null) ? 'ok' : 'bad';
                    io.observe(document.getElementById('vis'));
                    globalThis.__io_observe = 'ok';
                    p('disconnect_type', typeof io.disconnect);
                    io.disconnect();
                    globalThis.__io_disconnect = 'ok';
                    // re-observe after disconnect so the callback can fire below.
                    io.observe(document.getElementById('vis'));
                } catch(e) { globalThis.__io_ctor = 'ERR:'+e; }
                return out.join('|');
            })()"#,
        )
        .unwrap_or_else(|e| panic!("evaluate failed: {e}"));
    let fields: std::collections::HashMap<_, _> = issued
        .trim()
        .trim_matches('"')
        .split('|')
        .filter_map(|kv| kv.split_once('='))
        .collect();

    assert_eq!(fields.get("ctype"), Some(&"function"), "C1: typeof IntersectionObserver");
    assert_eq!(fields.get("disconnect_type"), Some(&"function"), "C5: observer.disconnect is a function");
    let ctor = page.evaluate_js_web("String(globalThis.__io_ctor)").unwrap_or_default();
    assert_eq!(ctor.trim().trim_matches('"'), "ok", "C2: new IntersectionObserver(callback, {{threshold}})");
    let observe = page.evaluate_js_web("String(globalThis.__io_observe)").unwrap_or_default();
    assert_eq!(observe.trim().trim_matches('"'), "ok", "C3: observer.observe(element)");
    let disc = page.evaluate_js_web("String(globalThis.__io_disconnect)").unwrap_or_default();
    assert_eq!(disc.trim().trim_matches('"'), "ok", "C5: observer.disconnect() does not throw");

    // C4: callback 收到 IntersectionObserverEntry 数组(渲染帧驱动)。
    let cb = pump_until(&page, "__io_cb", 8_000);
    assert_eq!(cb, "fired:1", "C4: callback receives IntersectionObserverEntry array");
    let entry = page.evaluate_js_web("String(globalThis.__io_entry_type)").unwrap_or_default();
    let entry = entry.trim().trim_matches('"');
    assert_eq!(
        entry, "boolean,instanceof",
        "C4: entry.isIntersecting is boolean and entry is an IntersectionObserverEntry (got {entry})"
    );
}

// ═══════════════════════════════════════════════════════════════════════════
// REQ-BRW-019 ResizeObserver
// ═══════════════════════════════════════════════════════════════════════════
//
// servo 实现:vendor/servo/components/script/dom/resizeobserver/,由
// script_thread 的 resize observer steps 驱动。

#[test]
// @trace REQ-BRW-019 [level:e2e]
fn resize_observer_surface_and_callback() {
    let runtime = BrowserRuntime::new(BaoConfig::default()).expect("BrowserRuntime::new failed");
    let pool: &PagePool = runtime.page_pool();
    let page = probe_page(
        pool,
        "<!DOCTYPE html><html><body><div id='rz' style='width:150px;height:150px;background:blue'>r</div></body></html>",
    );

    let issued = page
        .evaluate_js_web(
            r#"(function(){
                globalThis.__ro_cb = 'pending';
                globalThis.__ro_ctor = 'pending';
                globalThis.__ro_observe = 'pending';
                globalThis.__ro_entry = 'pending';
                var out = [];
                function p(k, v){ out.push(k + '=' + v); }
                p('ctype', typeof ResizeObserver);
                try {
                    var ro = new ResizeObserver(function(entries){
                        globalThis.__ro_cb = 'fired:' + entries.length;
                        if (entries.length > 0 && entries[0]) {
                            globalThis.__ro_entry = (typeof ResizeObserverEntry === 'function') ? (entries[0] instanceof ResizeObserverEntry) : 'noEntryCtor';
                        }
                    });
                    globalThis.__ro_ctor = (typeof ro === 'object' && ro !== null) ? 'ok' : 'bad';
                    ro.observe(document.getElementById('rz'));
                    globalThis.__ro_observe = 'ok';
                    p('disconnect_type', typeof ro.disconnect);
                    p('unobserve_type', typeof ro.unobserve);
                } catch(e) { globalThis.__ro_ctor = 'ERR:'+e; }
                return out.join('|');
            })()"#,
        )
        .unwrap_or_else(|e| panic!("evaluate failed: {e}"));
    let fields: std::collections::HashMap<_, _> = issued
        .trim()
        .trim_matches('"')
        .split('|')
        .filter_map(|kv| kv.split_once('='))
        .collect();

    assert_eq!(fields.get("ctype"), Some(&"function"), "C1: typeof ResizeObserver");
    assert_eq!(fields.get("disconnect_type"), Some(&"function"), "C5: observer.disconnect is a function");
    assert_eq!(fields.get("unobserve_type"), Some(&"function"), "observer.unobserve is a function");
    let ctor = page.evaluate_js_web("String(globalThis.__ro_ctor)").unwrap_or_default();
    assert_eq!(ctor.trim().trim_matches('"'), "ok", "C2: new ResizeObserver(callback)");
    let observe = page.evaluate_js_web("String(globalThis.__ro_observe)").unwrap_or_default();
    assert_eq!(observe.trim().trim_matches('"'), "ok", "C3: observer.observe(element)");

    // C4: callback 收到 ResizeObserverEntry 数组。
    let cb = pump_until(&page, "__ro_cb", 8_000);
    assert_eq!(cb, "fired:1", "C4: callback receives ResizeObserverEntry array");
    let entry = page.evaluate_js_web("String(globalThis.__ro_entry)").unwrap_or_default();
    assert_eq!(
        entry.trim().trim_matches('"'),
        "true",
        "C4: entry is a ResizeObserverEntry"
    );

    // C5(行为面):disconnect() 停止观察 —— 追加后不再触发。
    let _ = page.evaluate_js_web(
        r#"(function(){
            // ro is not visible here; create a fresh one, observe, disconnect, mutate.
            globalThis.__ro_disc = 'pending';
            var ro2 = new ResizeObserver(function(){ globalThis.__ro_disc = 'fired-after-disconnect'; });
            var el = document.getElementById('rz');
            ro2.observe(el);
            ro2.disconnect();
            el.style.width = '275px';
            // give the rendering steps a chance to run.
            globalThis.__ro_disc_base = 'set';
        })()"#,
    );
    // Drive several rendering turns, then confirm no callback fired.
    let start = Instant::now();
    while start.elapsed() < Duration::from_millis(1500) {
        let _ = page.evaluate_js("");
        std::thread::sleep(Duration::from_millis(20));
    }
    let disc = page.evaluate_js_web("String(globalThis.__ro_disc)").unwrap_or_default();
    assert_eq!(
        disc.trim().trim_matches('"'),
        "pending",
        "C5: disconnect() stops observation (no callback after disconnect)"
    );
}

// ═══════════════════════════════════════════════════════════════════════════
// REQ-BRW-024 Web Components (Custom Elements / Shadow DOM)
// ═══════════════════════════════════════════════════════════════════════════

#[test]
// @trace REQ-BRW-024 [level:e2e]
fn web_components_custom_elements_and_shadow_dom() {
    let runtime = BrowserRuntime::new(BaoConfig::default()).expect("BrowserRuntime::new failed");
    let pool: &PagePool = runtime.page_pool();
    let page = probe_page(pool, "<!DOCTYPE html><html><body>ce</body></html>");

    let dump = page
        .evaluate_js_web(
            r#"(function(){
                var out = [];
                var MyEl;
                function p(k, v){ out.push(k + '=' + v); }
                // C1: customElements object + define method
                p('ce_type', typeof customElements);
                p('define_type', typeof (customElements && customElements.define));
                p('get_type', typeof (customElements && customElements.get));
                p('whenDefined_type', typeof (customElements && customElements.whenDefined));
                // C2: define a custom element
                try {
                    MyEl = class extends HTMLElement {};
                    customElements.define('bao-my-elem', MyEl);
                    p('define_ok', 'true');
                    p('get_name', customElements.get('bao-my-elem') === MyEl ? 'match' : 'mismatch');
                } catch(e) { p('define_ok', 'ERR:'+e); }
                // C3: createElement returns an instance of the registered class
                try {
                    var el = document.createElement('bao-my-elem');
                    p('instanceof', (el instanceof MyEl) ? 'true' : 'false');
                    p('tagName', el.tagName);
                } catch(e) { p('instanceof', 'ERR:'+e); }
                // C4/C5: attachShadow + shadowRoot.querySelector
                try {
                    var host = document.createElement('div');
                    document.body.appendChild(host);
                    var sr = host.attachShadow({ mode: 'open' });
                    p('shadow_mode', sr.mode);
                    sr.innerHTML = '<b id="inner">shadow-content</b>';
                    p('shadow_query', host.shadowRoot.querySelector('#inner').textContent);
                    p('shadow_identity', host.shadowRoot === sr ? 'same' : 'diff');
                } catch(e) { p('shadow_mode', 'ERR:'+e); }
                return out.join('|');
            })()"#,
        )
        .unwrap_or_else(|e| panic!("evaluate failed: {e}"));

    let fields: std::collections::HashMap<_, _> = dump
        .trim()
        .trim_matches('"')
        .split('|')
        .filter_map(|kv| kv.split_once('='))
        .collect();

    assert_eq!(fields.get("ce_type"), Some(&"object"), "C1: typeof customElements");
    assert_eq!(fields.get("define_type"), Some(&"function"), "C1: customElements.define");
    assert_eq!(fields.get("get_type"), Some(&"function"), "customElements.get");
    assert_eq!(fields.get("whenDefined_type"), Some(&"function"), "customElements.whenDefined");
    assert_eq!(fields.get("define_ok"), Some(&"true"), "C2: customElements.define(name, class)");
    assert_eq!(fields.get("get_name"), Some(&"match"), "customElements.get round-trips constructor");
    assert_eq!(fields.get("instanceof"), Some(&"true"), "C3: createElement returns registered class instance");
    assert_eq!(fields.get("tagName"), Some(&"BAO-MY-ELEM"), "C3: custom element tagName");
    assert_eq!(fields.get("shadow_mode"), Some(&"open"), "C4: attachShadow({{mode:'open'}}) returns ShadowRoot");
    assert_eq!(fields.get("shadow_identity"), Some(&"same"), "C4: host.shadowRoot === returned ShadowRoot");
    assert_eq!(fields.get("shadow_query"), Some(&"shadow-content"), "C5: shadowRoot.querySelector reaches shadow content");
}

// ═══════════════════════════════════════════════════════════════════════════
// REQ-BRW-025 MutationObserver
// ═══════════════════════════════════════════════════════════════════════════

#[test]
// @trace REQ-BRW-025 [level:e2e]
fn mutation_observer_surface_and_callback() {
    let runtime = BrowserRuntime::new(BaoConfig::default()).expect("BrowserRuntime::new failed");
    let pool: &PagePool = runtime.page_pool();
    let page = probe_page(pool, "<!DOCTYPE html><html><body><div id='mo'>m</div></body></html>");

    let issued = page
        .evaluate_js_web(
            r#"(function(){
                globalThis.__mo_cb = 'pending';
                globalThis.__mo_ctor = 'pending';
                globalThis.__mo_observe = 'pending';
                globalThis.__mo_record = 'pending';
                var out = [];
                function p(k, v){ out.push(k + '=' + v); }
                p('ctype', typeof MutationObserver);
                try {
                    var mo = new MutationObserver(function(records){
                        globalThis.__mo_cb = 'fired:' + records.length;
                        if (records.length > 0 && records[0]) {
                            var isRec = (typeof MutationRecord === 'function')
                                ? (records[0] instanceof MutationRecord ? 'instanceof' : 'notinstance')
                                : 'noRecordCtor';
                            globalThis.__mo_record = records[0].type + ',' + isRec;
                        }
                    });
                    globalThis.__mo_ctor = (typeof mo === 'object' && mo !== null) ? 'ok' : 'bad';
                    mo.observe(document.getElementById('mo'), { childList: true });
                    globalThis.__mo_observe = 'ok';
                    p('disconnect_type', typeof mo.disconnect);
                    p('takeRecords_type', typeof mo.takeRecords);
                    // trigger a childList mutation
                    var span = document.createElement('span');
                    span.textContent = 'added';
                    document.getElementById('mo').appendChild(span);
                } catch(e) { globalThis.__mo_ctor = 'ERR:'+e; }
                return out.join('|');
            })()"#,
        )
        .unwrap_or_else(|e| panic!("evaluate failed: {e}"));
    let fields: std::collections::HashMap<_, _> = issued
        .trim()
        .trim_matches('"')
        .split('|')
        .filter_map(|kv| kv.split_once('='))
        .collect();

    assert_eq!(fields.get("ctype"), Some(&"function"), "C1: typeof MutationObserver");
    assert_eq!(fields.get("disconnect_type"), Some(&"function"), "C5: observer.disconnect is a function");
    assert_eq!(fields.get("takeRecords_type"), Some(&"function"), "observer.takeRecords is a function");
    let ctor = page.evaluate_js_web("String(globalThis.__mo_ctor)").unwrap_or_default();
    assert_eq!(ctor.trim().trim_matches('"'), "ok", "C2: new MutationObserver(callback)");
    let observe = page.evaluate_js_web("String(globalThis.__mo_observe)").unwrap_or_default();
    assert_eq!(
        observe.trim().trim_matches('"'),
        "ok",
        "C3: observer.observe(target, {{childList:true}})"
    );

    // C4: callback 收到 MutationRecord 数组(微任务 checkpoint 触发)。
    let cb = pump_until(&page, "__mo_cb", 5_000);
    assert_eq!(cb, "fired:1", "C4: callback receives MutationRecord array");
    let record = page.evaluate_js_web("String(globalThis.__mo_record)").unwrap_or_default();
    let record = record.trim().trim_matches('"');
    assert_eq!(
        record, "childList,instanceof",
        "C4: record.type === 'childList' and record is a MutationRecord (got {record})"
    );

    // C5(行为面):disconnect() 后不再触发回调。
    let _ = page.evaluate_js_web(
        r#"(function(){
            globalThis.__mo_disc = 'pending';
            var mo2 = new MutationObserver(function(){ globalThis.__mo_disc = 'fired-after-disconnect'; });
            var t = document.getElementById('mo');
            mo2.observe(t, { childList: true });
            mo2.disconnect();
            t.appendChild(document.createElement('i'));
        })()"#,
    );
    let start = Instant::now();
    while start.elapsed() < Duration::from_millis(1500) {
        let _ = page.evaluate_js("");
        std::thread::sleep(Duration::from_millis(20));
    }
    let disc = page.evaluate_js_web("String(globalThis.__mo_disc)").unwrap_or_default();
    assert_eq!(
        disc.trim().trim_matches('"'),
        "pending",
        "C5: disconnect() stops observation (no callback after disconnect)"
    );
}

// ═══════════════════════════════════════════════════════════════════════════
// REQ-BRW-027 Cache API (CacheStorage / Cache)
// ═══════════════════════════════════════════════════════════════════════════
//
// servo 实现:vendor/servo/components/script/dom/serviceworker/{cachestorage,cache}.rs。
// bao 侧翻转 `dom_serviceworker_enabled`(webidl `[Pref=]` 门 + SecureContext)。
//
// **覆盖边界**:vendor 的 Cache 接口当前只实现 `keys()`(Cache.webidl 仅声明
// keys)——C3 put / C4 match / C5 delete 在 bao 构建里 undefined,属实现
// 缺口(另立单),本测试只断言既有 C1/C2 + keys,不伪造缺失方法断言。

#[test]
// @trace REQ-BRW-027 [level:e2e]
fn cache_api_caches_open_and_keys_surface() {
    let runtime = BrowserRuntime::new(BaoConfig::default()).expect("BrowserRuntime::new failed");
    let pool: &PagePool = runtime.page_pool();
    let page = probe_page(pool, "<!DOCTYPE html><html><body>cache</body></html>");

    // C1: typeof caches === 'object', typeof CacheStorage === 'function'。
    let statics = page
        .evaluate_js_web(
            r#"(function(){
                return [typeof caches, typeof CacheStorage, (typeof caches === 'object' && caches !== null) ?
                    [typeof caches.open, typeof caches.has, typeof caches.delete].join('+') : 'n/a'].join('|');
            })()"#,
        )
        .unwrap_or_else(|e| panic!("evaluate failed: {e}"));
    let parts: Vec<&str> = statics.trim().trim_matches('"').split('|').collect();
    assert_eq!(parts.first(), Some(&"object"), "C1: typeof caches");
    assert_eq!(parts.get(1), Some(&"function"), "C1: typeof CacheStorage");
    assert_eq!(
        parts.get(2),
        Some(&"function+function+function"),
        "C1: caches.open/has/delete are functions"
    );

    // C2: caches.open('v1') resolves a Cache;既有方法 keys() 在位。
    let _ = page.evaluate_js_web(
        r#"(function(){
            globalThis.__cache = 'pending';
            try {
                caches.open('bao-conformance-v1').then(function(c){
                    globalThis.__cache = 'ok:' + c.constructor.name +
                        ',keys=' + (typeof c.keys);
                }, function(e){ globalThis.__cache = 'rejected:' + (e && (e.name || e)); });
            } catch(e) { globalThis.__cache = 'threw:' + e; }
        })()"#,
    );
    let opened = pump_until(&page, "__cache", 10_000);
    assert_eq!(
        opened, "ok:Cache,keys=function",
        "C2: caches.open('v1') resolves a Cache exposing keys()"
    );

    // C2 续:Cache.keys() 真返回 Promise(既有实现面真活)。
    let _ = page.evaluate_js_web(
        r#"(function(){
            globalThis.__cache_keys = 'pending';
            caches.open('bao-conformance-v1').then(function(c){
                var p = c.keys();
                globalThis.__cache_keys = (p && typeof p.then === 'function') ? 'promise' : 'notapromise';
            }, function(e){ globalThis.__cache_keys = 'rejected:' + (e && (e.name || e)); });
        })()"#,
    );
    let keys = pump_until(&page, "__cache_keys", 10_000);
    assert_eq!(keys, "promise", "C2: Cache.keys() returns a Promise");

    // has() 真往返(既有的另一方法面):打开后 has 应 resolve true。
    let _ = page.evaluate_js_web(
        r#"(function(){
            globalThis.__cache_has = 'pending';
            caches.has('bao-conformance-v1').then(function(v){
                globalThis.__cache_has = 'has:' + v;
            }, function(e){ globalThis.__cache_has = 'rejected:' + (e && (e.name || e)); });
        })()"#,
    );
    let has = pump_until(&page, "__cache_has", 10_000);
    assert_eq!(has, "has:true", "C2: caches.has(name) round-trips true after open");
}

// ═══════════════════════════════════════════════════════════════════════════
// REQ-BRW-030 Gamepad API
// ═══════════════════════════════════════════════════════════════════════════
//
// servo 实现:vendor/servo/components/script/dom/gamepad/(Gamepad/
// GamepadButton/GamepadEvent/GamepadHapticActuator/Navigator.getGamepads)。
// bao 侧:BrowserRuntime 的 servo 依赖启用 `gamepad` cargo feature
// (2026-10-16,bao_browser/Cargo.toml)——此前 `skip-unless
// CARGO_FEATURE_GAMEPAD` 把整套 webidl 编译期剔除,navigator.getGamepads/
// Gamepad 在 bao 构建里 undefined(e-gapfix 实测)。运行时 pref 门
// `dom_gamepad_enabled` 上游默认 true(config/prefs.rs),无需翻转。
//
// headless 语义(Chrome headless 同形):无 embedder 提供物理手柄 =>
// getGamepads() 返回空数组 = 正确行为,非缺口。

#[test]
// @trace REQ-BRW-030 [level:e2e]
fn gamepad_api_surface_headless() {
    let runtime = BrowserRuntime::new(BaoConfig::default()).expect("BrowserRuntime::new failed");
    let pool: &PagePool = runtime.page_pool();
    let page = probe_page(pool, "<!DOCTYPE html><html><body>gamepad</body></html>");

    // C1: 构造器面 —— getGamepads 是函数 + 4 个接口对象可见 + window 侧
    // ongamepadconnected/ongamepaddisconnected handler 槽在位(未注册为 null
    // => typeof 'object')。
    let surface = page
        .evaluate_js_web(
            r#"(function(){
                var out = [];
                function p(k, v){ out.push(k + '=' + v); }
                p('getgamepads', typeof navigator.getGamepads);
                p('gamepad', typeof Gamepad);
                p('button', typeof GamepadButton);
                p('event', typeof GamepadEvent);
                p('haptic', typeof GamepadHapticActuator);
                p('onconnected', typeof window.ongamepadconnected);
                p('ondisconnected', typeof window.ongamepaddisconnected);
                return out.join('|');
            })()"#,
        )
        .unwrap_or_else(|e| panic!("evaluate failed: {e}"));
    let fields: std::collections::HashMap<_, _> = surface
        .trim()
        .trim_matches('"')
        .split('|')
        .filter_map(|kv| kv.split_once('='))
        .collect();

    assert_eq!(
        fields.get("getgamepads"),
        Some(&"function"),
        "C1: typeof navigator.getGamepads"
    );
    for (k, name) in [
        ("gamepad", "Gamepad"),
        ("button", "GamepadButton"),
        ("event", "GamepadEvent"),
        ("haptic", "GamepadHapticActuator"),
    ] {
        assert_eq!(fields.get(k), Some(&"function"), "C1: typeof {name}");
    }
    for (k, name) in [
        ("onconnected", "ongamepadconnected"),
        ("ondisconnected", "ongamepaddisconnected"),
    ] {
        assert_eq!(
            fields.get(k),
            Some(&"object"),
            "C3: window.{name} handler slot present (null => typeof 'object')"
        );
    }

    // C2: headless 无手柄 => 空数组。spec 行为:has_gamepad_gesture=false /
    // 无注册 pad => 空 list(navigator.rs GetGamepads Step 4/6-7)。
    let pads = page
        .evaluate_js_web(
            r#"(function(){
                try {
                    var g = navigator.getGamepads();
                    return Array.isArray(g) + ',' + g.length;
                } catch(e) { return 'ERR:' + (e && (e.name || e)); }
            })()"#,
        )
        .unwrap_or_else(|e| panic!("evaluate failed: {e}"));
    let pads = pads.trim().trim_matches('"');
    assert_eq!(
        pads, "true,0",
        "C2: navigator.getGamepads() returns an empty array in headless (got {pads})"
    );

    // C3: 事件面 —— 合成 GamepadEvent 经 window.dispatchEvent 真触发
    // gamepadconnected listener(headless 无物理 connect,用合成事件证明
    // 派发通路真活,非仅属性槽)。GamepadEvent 构造器接受 {gamepad: null}
    // (webidl 注明 gamepad 可空)。
    let _ = page.evaluate_js_web(
        r#"(function(){
            globalThis.__gp_evt = 'pending';
            try {
                window.addEventListener('gamepadconnected', function(){
                    globalThis.__gp_evt = 'connected-fired';
                });
                var ev = new GamepadEvent('gamepadconnected', { gamepad: null });
                window.dispatchEvent(ev);
            } catch(e) { globalThis.__gp_evt = 'ERR:' + (e && (e.name || e)); }
        })()"#,
    );
    let evt = pump_until(&page, "__gp_evt", 5_000);
    assert_eq!(
        evt, "connected-fired",
        "C3: synthetic GamepadEvent dispatches to the window gamepadconnected handler"
    );
}
