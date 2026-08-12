//! `Bao.browser` JS 全局对象 — 通过 `connect(url)` 暴露 CDP client。
//!
//! ## JS API
//!
//! ```javascript
//! // 连接内嵌 servo(同进程零网络往返)
//! const browser = Bao.browser.connect("memory://bao");
//! console.log(browser.url);        // "memory://bao"
//! console.log(browser.scheme);     // "memory"
//! console.log(browser.transportKind); // "InMemory"
//!
//! // 连接外部 Chrome
//! const chrome = Bao.browser.connect("ws://127.0.0.1:9222");
//! console.log(chrome.transportKind); // "WebSocket"
//!
//! // 异常 scheme 抛出 JS Error
//! try {
//!   Bao.browser.connect("ftp://x");
//! } catch (e) {
//!   console.error(e.message); // "invalid URL scheme: \"ftp\" ..."
//! }
//! ```
//!
//! ## Rust 后端
//!
//! `connect(url)` 内部调用 [`bao_cdp_client::Browser::connect`],通过 URL scheme
//! 路由到 InMemoryTransport(memory://) 或 WebSocketTransport(ws://)。
//! 返回一个 JS Browser proxy 对象,包装 [`bao_cdp_client::Browser`]。
//!
//! ## 与 Bun.* 的关系
//!
//! `Bao.*` 是 `Bun.*` 的别名(同一对象),共享所有 Bun API。
//! `Bao.browser` 是 Bao 独有的扩展属性(在 Bun 对象上不存在),
//! 提供 CDP client 入口。
//!
//! ## 限制
//!
//! 当前实现:
//! - 只暴露 connect(url) + url/scheme/transportKind/isInMemory/isWebSocket 属性
//! - 实际的 Transport 握手 / Page 操作需要后续 TASK 扩展(注入 servo bridge)
//! - 不暴露 send_command 等底层 API(避免 JS 端构造 CDP JSON-RPC 帧)
//!
//! @trace REQ-BAO-API-008 [level:library]

use ::std::cell::RefCell;
use ::std::ptr;
use ::std::ptr::NonNull;
use ::std::sync::Arc;

use mozjs::jsapi::*;
use mozjs::jsval::{BooleanValue, JSVal, ObjectValue, StringValue, UndefinedValue};
use mozjs::rooted;
use mozjs::rust::wrappers2::{JS_DefineFunction, JS_DefineProperty3, JS_NewPlainObject};

use bun_core::ZBox;

// 持有已 connect 的 Browser 实例(供 JS Browser proxy 引用)。
//
// 用 `RefCell<Vec<Arc<...>>>` 存储 — 每个 connect() 调用 push 一条,
// 返回的 JS 对象持有索引(通过 private slot)。这样:
// - JS 端对象轻量(只持 index + Arc clone)
// - Rust 端 Browser 实例可达(供后续 Page 操作 / transport 握手)
//
// 单线程模型:与 servo JSContext 一致,无需 Send + Sync。
thread_local! {
    static BROWSER_REGISTRY: RefCell<Vec<Arc<bao_cdp_client::Browser>>> =
        const { RefCell::new(Vec::new()) };
}

/// 注册一个 Browser 实例,返回 index(供 JS 对象引用)。
///
/// @trace REQ-BAO-API-008 [level:library]
fn register_browser(browser: bao_cdp_client::Browser) -> u32 {
    BROWSER_REGISTRY.with(|reg| {
        let mut reg = reg.borrow_mut();
        let idx = reg.len() as u32;
        reg.push(Arc::new(browser));
        idx
    })
}

/// 通过 index 取 Browser Arc clone(JS callback 内部用)。
fn with_browser<F, R>(idx: u32, f: F) -> Option<R>
where
    F: FnOnce(&bao_cdp_client::Browser) -> R,
{
    BROWSER_REGISTRY.with(|reg| {
        let reg = reg.borrow();
        reg.get(idx as usize).map(|b| f(b))
    })
}

/// 在 Bun/Bao 对象上安装 `Bao.browser` 属性。
///
/// 此函数应当在 `install_bun_global` 之后调用,以便把 `browser` 子对象
/// 挂到 Bun 对象上(同时通过 `Bao` 别名可见)。
///
/// # Safety
///
/// Caller must ensure `cx` is a valid JSContext pointer and `bun_obj` is a
/// valid handle to the Bun/Bao global object.
///
/// @trace REQ-BAO-API-008 [level:library]
pub unsafe fn install_bao_browser_on_bun(
    cx: &mut mozjs::context::JSContext,
    bun_obj: mozjs::rust::Handle<*mut JSObject>,
) {
    rooted!(&in(cx) let browser_obj = JS_NewPlainObject(cx));
    if browser_obj.get().is_null() {
        return;
    }

    // Bao.browser.connect(url) — URL scheme 路由
    JS_DefineFunction(
        cx,
        browser_obj.handle(),
        c"connect".as_ptr(),
        Some(browser_connect_fn),
        1,
        JSPROP_ENUMERATE as u32,
    );

    // Bao.browser.version() — 返回 bao_cdp_client 版本
    JS_DefineFunction(
        cx,
        browser_obj.handle(),
        c"version".as_ptr(),
        Some(browser_version_fn),
        0,
        JSPROP_ENUMERATE as u32,
    );

    // 静态属性 Bao.browser.transportKinds(数组)
    install_transport_kinds_array(cx, browser_obj.handle());

    JS_DefineProperty3(
        cx,
        bun_obj,
        c"browser".as_ptr(),
        browser_obj.handle(),
        JSPROP_ENUMERATE as u32,
    );
}

/// 安装 Bao.browser.transportKinds 数组(`["InMemory", "WebSocket"]`)。
unsafe fn install_transport_kinds_array(
    cx: &mut mozjs::context::JSContext,
    browser_obj: mozjs::rust::Handle<*mut JSObject>,
) {
    use mozjs::rust::wrappers2::NewArrayObject1;
    rooted!(&in(cx) let arr = NewArrayObject1(cx, 2));
    if arr.get().is_null() {
        return;
    }

    let items = ["InMemory", "WebSocket"];
    for (i, item) in items.iter().enumerate() {
        let c_str = ZBox::from_bytes(item.as_bytes());
        let js_str = JS_NewStringCopyZ(cx.raw_cx(), c_str.as_ptr());
        if !js_str.is_null() {
            rooted!(&in(cx) let v = StringValue(&*js_str));
            JS_DefineElement(
                cx.raw_cx(),
                arr.handle().into(),
                i as u32,
                v.handle().into(),
                JSPROP_ENUMERATE as u32,
            );
        }
    }

    JS_DefineProperty3(
        cx,
        browser_obj,
        c"transportKinds".as_ptr(),
        arr.handle(),
        JSPROP_ENUMERATE as u32,
    );
}

/// `Bao.browser.connect(url)` JS callback。
///
/// 内部调用 `bao_cdp_client::Browser::connect(url)`,成功则:
/// 1. 注册到 BROWSER_REGISTRY
/// 2. 构造 JS Browser proxy 对象(含 url/scheme/transportKind 属性 + private slot)
/// 3. 返回 proxy 给 JS
///
/// 失败抛出 JS Error(对应 ConnectError 变体)。
#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn browser_connect_fn(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    if argc == 0 {
        JS_ReportErrorUTF8(cx, c"Bao.browser.connect requires a URL argument".as_ptr());
        return false;
    }

    let url_val = *args.get(0).ptr;
    let url = crate::js_to_rust_string(cx, url_val);

    match bao_cdp_client::Browser::connect(&url) {
        Ok(browser) => {
            let idx = register_browser(browser);
            let proxy = make_browser_proxy(cx, idx);
            if proxy.is_null() {
                args.rval().set(UndefinedValue());
                return true;
            }
            args.rval().set(ObjectValue(proxy));
            true
        }
        Err(err) => {
            let msg = format!("{}", err);
            let c_msg = ZBox::from_bytes(msg.as_bytes());
            JS_ReportErrorUTF8(cx, c"%s".as_ptr(), c_msg.as_ptr());
            false
        }
    }
}

/// `Bao.browser.version()` JS callback — 返回 bao_cdp_client 版本字符串。
#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn browser_version_fn(cx: *mut JSContext, _argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, _argc);
    let ver = bao_cdp_client::version();
    let c_ver = ZBox::from_bytes(ver.as_bytes());
    let js_str = JS_NewStringCopyZ(cx, c_ver.as_ptr());
    if !js_str.is_null() {
        args.rval().set(StringValue(&*js_str));
    } else {
        args.rval().set(UndefinedValue());
    }
    true
}

/// 构造 JS Browser proxy 对象。
///
/// 设置:
/// - `url`: 原 URL(string)
/// - `scheme`: scheme(string)
/// - `transportKind`: "InMemory" / "WebSocket"(string)
/// - `isInMemory`: bool
/// - `isWebSocket`: bool
/// - `index`: private slot(usize)— 用于查 BROWSER_REGISTRY
/// - `disconnect()` 方法:从 registry 移除
#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn make_browser_proxy(cx: *mut JSContext, idx: u32) -> *mut JSObject {
    let mut cx_ref = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    rooted!(&in(cx_ref) let proxy = JS_NewPlainObject(&mut cx_ref));
    if proxy.get().is_null() {
        return ptr::null_mut();
    }

    let populated = with_browser(idx, |browser| {
        // url
        let c_url = ZBox::from_bytes(browser.url().as_bytes());
        let js_url = JS_NewStringCopyZ(cx, c_url.as_ptr());
        if !js_url.is_null() {
            rooted!(&in(cx_ref) let v = StringValue(&*js_url));
            JS_DefineProperty(
                cx,
                proxy.handle().into(),
                c"url".as_ptr(),
                v.handle().into(),
                JSPROP_ENUMERATE as u32,
            );
        }

        // scheme
        let c_scheme = ZBox::from_bytes(browser.scheme().as_bytes());
        let js_scheme = JS_NewStringCopyZ(cx, c_scheme.as_ptr());
        if !js_scheme.is_null() {
            rooted!(&in(cx_ref) let v = StringValue(&*js_scheme));
            JS_DefineProperty(
                cx,
                proxy.handle().into(),
                c"scheme".as_ptr(),
                v.handle().into(),
                JSPROP_ENUMERATE as u32,
            );
        }

        // transportKind (string repr)
        let tk_str = match browser.transport_kind() {
            bao_cdp_client::TransportKind::InMemory => "InMemory",
            bao_cdp_client::TransportKind::WebSocket => "WebSocket",
        };
        let c_tk = ZBox::from_bytes(tk_str.as_bytes());
        let js_tk = JS_NewStringCopyZ(cx, c_tk.as_ptr());
        if !js_tk.is_null() {
            rooted!(&in(cx_ref) let v = StringValue(&*js_tk));
            JS_DefineProperty(
                cx,
                proxy.handle().into(),
                c"transportKind".as_ptr(),
                v.handle().into(),
                JSPROP_ENUMERATE as u32,
            );
        }

        // isInMemory / isWebSocket (bool)
        rooted!(&in(cx_ref) let im = BooleanValue(browser.is_in_memory()));
        JS_DefineProperty(
            cx,
            proxy.handle().into(),
            c"isInMemory".as_ptr(),
            im.handle().into(),
            JSPROP_ENUMERATE as u32,
        );

        rooted!(&in(cx_ref) let ws = BooleanValue(browser.is_websocket()));
        JS_DefineProperty(
            cx,
            proxy.handle().into(),
            c"isWebSocket".as_ptr(),
            ws.handle().into(),
            JSPROP_ENUMERATE as u32,
        );
    });

    if populated.is_none() {
        return ptr::null_mut();
    }

    // 索引(存为整数 private 属性 — 非 enumerable)
    rooted!(&in(cx_ref) let idx_val = mozjs::jsval::Int32Value(idx as i32));
    JS_DefineProperty(
        cx,
        proxy.handle().into(),
        c"__registry_idx".as_ptr(),
        idx_val.handle().into(),
        0u32, // non-enumerable
    );

    // disconnect() 方法
    JS_DefineFunction(
        &mut cx_ref,
        proxy.handle(),
        c"disconnect".as_ptr(),
        Some(browser_disconnect_fn),
        0,
        0u32,
    );

    proxy.get()
}

/// `Bao.browser.connect(url).disconnect()` JS callback — 从 registry 移除。
///
/// 移除后,Browser Arc 引用计数归零时自动 drop(后续 send_command / recv_event
/// 返回 ConnectionClosed)。
#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn browser_disconnect_fn(_cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let this = args.thisv();
    if !this.is_object() {
        args.rval().set(BooleanValue(false));
        return true;
    }

    let cx_ref = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(_cx));
    rooted!(&in(cx_ref) let obj_root = this.to_object());
    let mut idx_val = UndefinedValue();
    JS_GetProperty(
        _cx,
        obj_root.handle().into(),
        c"__registry_idx".as_ptr(),
        MutableHandle::<Value> {
            _phantom_0: ::std::marker::PhantomData,
            ptr: &mut idx_val,
        },
    );

    if idx_val.is_int32() {
        let idx = idx_val.to_int32() as u32;
        BROWSER_REGISTRY.with(|reg| {
            let mut reg = reg.borrow_mut();
            if (idx as usize) < reg.len() {
                reg[idx as usize] = Arc::new(
                    bao_cdp_client::Browser::connect("memory://__disconnected__").unwrap(),
                );
            }
        });
        args.rval().set(BooleanValue(true));
    } else {
        args.rval().set(BooleanValue(false));
    }
    true
}

// ─── 单元测试 ──────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registry_round_trip() {
        // 清空(可能跨 test 残留)
        BROWSER_REGISTRY.with(|r| r.borrow_mut().clear());

        let b1 = bao_cdp_client::Browser::connect("memory://bao").unwrap();
        let b2 = bao_cdp_client::Browser::connect("ws://127.0.0.1:9222").unwrap();

        let i1 = register_browser(b1);
        let i2 = register_browser(b2);
        assert_eq!(i1, 0);
        assert_eq!(i2, 1);

        // 取回
        let url1 = with_browser(i1, |b| b.url().to_string());
        let url2 = with_browser(i2, |b| b.url().to_string());
        assert_eq!(url1.as_deref(), Some("memory://bao"));
        assert_eq!(url2.as_deref(), Some("ws://127.0.0.1:9222"));

        // 越界返回 None
        let none = with_browser(99, |b| b.url().to_string());
        assert!(none.is_none());

        BROWSER_REGISTRY.with(|r| r.borrow_mut().clear());
    }

    #[test]
    fn bao_cdp_client_version_is_exposed() {
        let v = bao_cdp_client::version();
        assert!(!v.is_empty());
        assert!(v.contains('.'));
    }

    #[test]
    fn registry_can_hold_multiple_browsers() {
        BROWSER_REGISTRY.with(|r| r.borrow_mut().clear());

        for i in 0..5 {
            let url = format!("memory://bao{}", i);
            let b = bao_cdp_client::Browser::connect(&url).unwrap();
            let idx = register_browser(b);
            assert_eq!(idx, i);
        }

        BROWSER_REGISTRY.with(|r| {
            assert_eq!(r.borrow().len(), 5);
        });

        BROWSER_REGISTRY.with(|r| r.borrow_mut().clear());
    }
}
