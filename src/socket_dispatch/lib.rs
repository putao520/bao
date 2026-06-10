//! Socket event dispatch for Bao.
//!
//! This crate owns the `us_dispatch_*` `#[no_mangle]` exports that `loop.c`
//! calls for every socket event. It resolves `s->kind` to the correct vtable
//! and calls the appropriate handler.
//!
//! ## Architecture
//!
//! This is a Tier-1 crate (depends only on `bun_uws_sys`, `bun_uws`, `bun_http`).
//! It registers `HttpClient`/`HttpClientTls` vtables at init time and provides
//! `register_kind()` for Tier-10+ crates (e.g. `bun_runtime`) to add JSC-bound
//! handler kinds (BunSocket, Postgres, MySQL, Valkey, WSClient, IPC).

use core::ffi::{c_int, c_void};
use core::ptr::NonNull;
use core::sync::atomic::{AtomicPtr, Ordering};

use bun_uws::NewSocketHandler;
use bun_uws_sys::socket_group::VTable;
use bun_uws_sys::vtable;
use bun_uws_sys::{CloseCode, ConnectingSocket, SocketKind, us_bun_verify_error_t, us_socket_t};

// ── HTTPClient handler ─────────────────────────────────────────────────

/// The `HttpClient`/`HttpClientTls` handler adapter. Implements `VHandler`
/// by forwarding events to `bun_http::http_context::Handler<SSL>`.
///
/// Ext slot holds `Option<NonNull<c_void>>` — the `ActiveSocket` tagged
/// pointer word (not a typed struct pointer).
pub struct HTTPClient<const SSL: bool>;

type HttpH<const SSL: bool> = bun_http::http_context::Handler<SSL>;

/// Wrap a raw `*mut us_socket_t` in the const-generic `NewSocketHandler`.
#[inline(always)]
fn wrap<const SSL: bool>(s: *mut us_socket_t) -> NewSocketHandler<SSL> {
    NewSocketHandler::<SSL>::from(s)
}

impl<const SSL: bool> vtable::Handler for HTTPClient<SSL> {
    type Ext = Option<NonNull<c_void>>;

    const HAS_ON_OPEN: bool = true;
    const HAS_ON_DATA: bool = true;
    const HAS_ON_WRITABLE: bool = true;
    const HAS_ON_CLOSE: bool = true;
    const HAS_ON_TIMEOUT: bool = true;
    const HAS_ON_LONG_TIMEOUT: bool = true;
    const HAS_ON_END: bool = true;
    const HAS_ON_CONNECT_ERROR: bool = true;
    const HAS_ON_CONNECTING_ERROR: bool = true;
    const HAS_ON_HANDSHAKE: bool = true;

    fn on_open(ext: &mut Self::Ext, s: *mut us_socket_t, _is_client: bool, _ip: &[u8]) {
        let Some(owner) = *ext else { return };
        HttpH::<SSL>::on_open(owner.as_ptr(), wrap::<SSL>(s));
    }
    fn on_data(ext: &mut Self::Ext, s: *mut us_socket_t, data: &[u8]) {
        let Some(owner) = *ext else { return };
        HttpH::<SSL>::on_data(owner.as_ptr(), wrap::<SSL>(s), data);
    }
    fn on_writable(ext: &mut Self::Ext, s: *mut us_socket_t) {
        let Some(owner) = *ext else { return };
        HttpH::<SSL>::on_writable(owner.as_ptr(), wrap::<SSL>(s));
    }
    fn on_close(ext: &mut Self::Ext, s: *mut us_socket_t, code: i32, reason: Option<*mut c_void>) {
        let Some(owner) = *ext else { return };
        HttpH::<SSL>::on_close(owner.as_ptr(), wrap::<SSL>(s), code, reason);
    }
    fn on_timeout(ext: &mut Self::Ext, s: *mut us_socket_t) {
        let Some(owner) = *ext else { return };
        HttpH::<SSL>::on_timeout(owner.as_ptr(), wrap::<SSL>(s));
    }
    fn on_long_timeout(ext: &mut Self::Ext, s: *mut us_socket_t) {
        let Some(owner) = *ext else { return };
        HttpH::<SSL>::on_long_timeout(owner.as_ptr(), wrap::<SSL>(s));
    }
    fn on_end(ext: &mut Self::Ext, s: *mut us_socket_t) {
        let Some(owner) = *ext else { return };
        HttpH::<SSL>::on_end(owner.as_ptr(), wrap::<SSL>(s));
    }
    fn on_connect_error(ext: &mut Self::Ext, s: *mut us_socket_t, code: i32) {
        let owner = *ext;
        us_socket_t::opaque_mut(s).close(CloseCode::failure);
        let Some(owner) = owner else { return };
        HttpH::<SSL>::on_connect_error(owner.as_ptr(), wrap::<SSL>(s), code);
    }
    fn on_connecting_error(cs: *mut ConnectingSocket, code: i32) {
        let Some(owner) = *ConnectingSocket::opaque_mut(cs).ext::<Option<NonNull<c_void>>>() else {
            return;
        };
        HttpH::<SSL>::on_connect_error(
            owner.as_ptr(),
            NewSocketHandler::<SSL>::from_connecting(cs),
            code,
        );
    }
    fn on_handshake(ext: &mut Self::Ext, s: *mut us_socket_t, ok: bool, err: us_bun_verify_error_t) {
        let Some(owner) = *ext else { return };
        HttpH::<SSL>::on_handshake(owner.as_ptr(), wrap::<SSL>(s), ok as i32, err);
    }
}

// ── Kind → vtable table ────────────────────────────────────────────────

const SOCKET_KIND_COUNT: usize = SocketKind::UwsWsTls as usize + 1;

/// Global kind→vtable table. Each slot is an `AtomicPtr` so that Tier-10+
/// crates can register handlers at runtime without needing a mutable global.
///
/// Init: `HttpClient`/`HttpClientTls` are populated at first use. Other kinds
/// are registered by `bun_runtime::uws_dispatch` which layers on top.
///
/// `Invalid` is intentionally null so a missed `kind` stamp panics
/// instead of dispatching into the wrong handler.
static TABLES: [AtomicPtr<VTable>; SOCKET_KIND_COUNT] = {
    const NULL: AtomicPtr<VTable> = AtomicPtr::new(core::ptr::null_mut());
    [NULL; SOCKET_KIND_COUNT]
};

/// Register a vtable for a `SocketKind`. Called once per kind during init.
/// Tier-1 (this crate) registers HttpClient/HttpClientTls; Tier-10+
/// (`bun_runtime`) registers JSC-bound kinds.
///
/// # Panics
/// Panics if `kind` is out of range.
pub fn register_kind(kind: SocketKind, vt: &'static VTable) {
    let idx = kind as usize;
    assert!(idx < SOCKET_KIND_COUNT, "SocketKind out of range");
    TABLES[idx].store(vt as *const VTable as *mut VTable, Ordering::Release);
}

/// Resolve the vtable for a connected socket.
#[inline]
pub fn vt(s: *mut us_socket_t) -> &'static VTable {
    let s = us_socket_t::opaque_mut(s);
    let kind = s.kind();
    match kind {
        SocketKind::Invalid => {
            panic!("us_socket_t with kind=invalid (group={:p})", s.raw_group())
        }
        SocketKind::Dynamic
        | SocketKind::UwsHttp
        | SocketKind::UwsHttpTls
        | SocketKind::UwsWs
        | SocketKind::UwsWsTls => {
            s.raw_group().vtable.expect("group vtable")
        }
        _ => {
            let ptr = TABLES[kind as usize].load(Ordering::Acquire);
            assert!(!ptr.is_null(), "kind vtable not registered for {:?}", kind);
            unsafe { &*ptr }
        }
    }
}

/// Resolve the vtable for a connecting socket.
#[inline]
pub fn vtc(c: *mut ConnectingSocket) -> &'static VTable {
    let c = ConnectingSocket::opaque_mut(c);
    let kind = c.kind();
    match kind {
        SocketKind::Invalid => {
            panic!("us_connecting_socket_t with kind=invalid")
        }
        SocketKind::Dynamic
        | SocketKind::UwsHttp
        | SocketKind::UwsHttpTls
        | SocketKind::UwsWs
        | SocketKind::UwsWsTls => {
            unsafe { (*c.raw_group()).vtable.expect("group vtable") }
        }
        _ => {
            let ptr = TABLES[kind as usize].load(Ordering::Acquire);
            assert!(!ptr.is_null(), "kind vtable not registered for {:?}", kind);
            unsafe { &*ptr }
        }
    }
}

/// Initialize the Tier-1 vtable entries. Called once at startup.
pub fn init_http_vtables() {
    register_kind(SocketKind::HttpClient, vtable::make::<HTTPClient<false>>());
    register_kind(SocketKind::HttpClientTls, vtable::make::<HTTPClient<true>>());
}

// ── us_dispatch_* exports ─────────────────────────────────────────────

macro_rules! us_dispatch_shims {
    ($(
        fn $name:ident($recv:ident: *mut $Recv:ty $(, $a:ident: $t:ty)* $(,)?) -> $ret:ty
            = $lookup:ident.$field:ident($($call:expr),* $(,)?) or $default:expr;
    )*) => {$(
        #[unsafe(no_mangle)]
        #[allow(clippy::unused_unit)]
        pub unsafe extern "C" fn $name($recv: *mut $Recv $(, $a: $t)*) -> $ret {
            match $lookup($recv).$field {
                Some(f) => unsafe { f($($call),*) }
                None => $default,
            }
        }
    )*};
}

us_dispatch_shims! {
    fn us_dispatch_open(s: *mut us_socket_t, is_client: c_int, ip: *mut u8, ip_len: c_int) -> *mut us_socket_t
        = vt.on_open(s, is_client, ip, ip_len) or s;
    fn us_dispatch_data(s: *mut us_socket_t, data: *mut u8, len: c_int) -> *mut us_socket_t
        = vt.on_data(s, data, len) or s;
    fn us_dispatch_fd(s: *mut us_socket_t, fd: c_int) -> *mut us_socket_t
        = vt.on_fd(s, fd) or s;
    fn us_dispatch_writable(s: *mut us_socket_t) -> *mut us_socket_t
        = vt.on_writable(s) or s;
    fn us_dispatch_close(s: *mut us_socket_t, code: c_int, reason: *mut c_void) -> *mut us_socket_t
        = vt.on_close(s, code, reason) or s;
    fn us_dispatch_timeout(s: *mut us_socket_t) -> *mut us_socket_t
        = vt.on_timeout(s) or s;
    fn us_dispatch_long_timeout(s: *mut us_socket_t) -> *mut us_socket_t
        = vt.on_long_timeout(s) or s;
    fn us_dispatch_end(s: *mut us_socket_t) -> *mut us_socket_t
        = vt.on_end(s) or s;
    fn us_dispatch_connect_error(s: *mut us_socket_t, code: c_int) -> *mut us_socket_t
        = vt.on_connect_error(s, code) or s;
    fn us_dispatch_connecting_error(c: *mut ConnectingSocket, code: c_int) -> *mut ConnectingSocket
        = vtc.on_connecting_error(c, code) or c;
    fn us_dispatch_handshake(s: *mut us_socket_t, ok: c_int, err: us_bun_verify_error_t) -> ()
        = vt.on_handshake(s, ok, err, core::ptr::null_mut()) or ();
}
