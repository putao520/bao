// @trace REQ-ENG-006
// WebSocket + Performance + TextEncoder/TextDecoder + atob/btoa + queueMicrotask
use ::std::cell::RefCell;
use ::std::ffi::CString;
use ::std::io::{Read, Write};
use ::std::net::TcpStream;
use ::std::net::ToSocketAddrs;
use ::std::ptr::NonNull;
use ::std::sync::atomic::{AtomicU64, Ordering};
use ::std::sync::{Arc, Mutex};
use ::std::time::Duration;
use bun_core::ZBox;

// @trace REQ-ENG-006 [code:bun_uws] — RFC 6455 codec primitives reused for
// both the plain ws:// (via WebSocketClient) and the wss:// (TLS-driven) path.
use bun_uws::ws_codec::apply_mask;

use mozjs::conversions::unsafe_jsstr_to_string;
use mozjs::jsapi::*;
use mozjs::jsval::{BooleanValue, Int32Value, JSVal, ObjectValue, StringValue, UndefinedValue};
use mozjs::realm::AutoRealm;
use mozjs::rooted;
use mozjs::rust::wrappers2::{
    CallOriginalPromiseResolve, CallOriginalPromiseThen, JS_DefineFunction, JS_DefineProperty3,
    JS_NewPlainObject, NewArrayObject1,
};

use crate::gc_store::{gc_store_get, gc_store_insert, gc_store_remove};

// @trace REQ-ENG-005 [algorithm:base64] base64 via workspace bun_base64 (SIMD-accelerated)

// ── WebSocket client ──
// @trace REQ-ENG-006 [api:WebSocket] [code:bun_uws] — RFC 6455 framing and the
// plain-text (ws://) client handshake are delegated to `bun_uws::ws_client`
// (WebSocketClient / parse_ws_url / RecvOutcome) and `bun_uws::ws_codec` /
// `ws_handshake`. The wss:// (TLS) variant drives `bao_boringssl_bridge`'s
// TlsConnection over the TCP socket and reuses the same `bun_uws` codec /
// handshake primitives so the two schemes share one wire-format code path.
//
// @trace REQ-STL-001 — the wss:// TLS handshake applies the page's
// StealthProfile through the exact same application path fetch() uses
// (`stealth_http::stealth_profile_to_ssl_config` →
// `bun_http::configure_http_client_with_alpn`), so a page's WebSocket and
// its fetch present an identical JA3/JA4 fingerprint.
//
// Async model (root fix for ScriptThread blocking): `new WebSocket(..)` never
// connects on the JS thread. The constructor captures the thread's stealth
// profile, spawns a background worker that performs the full blocking connect
// (TCP + TLS + RFC 6455 handshake, ≤10s), and returns immediately with
// readyState=CONNECTING. The worker's outcome lands in an
// `Arc<Mutex<Option<..>>>` slot — the ONLY cross-thread channel (no JSObject
// pointers cross threads; BCE-20260621-001 rule). The JS-thread drain pump
// (`ws_pump_all`, wired into `timers::drain_and_check` /
// `timers::drain_one_pass` / the servo node-realm evaluate entry) consumes
// the slot (onopen/onerror) and pumps inbound frames (onmessage/onclose)
// with the sockets in non-blocking mode.

#[derive(Debug)]
#[allow(dead_code)]
enum WsMessage {
    Text(String),
    Binary(Vec<u8>),
    Close,
}

/// A TLS-over-TCP adapter implementing `std::io::{Read, Write}`. It owns the
/// raw `TcpStream` plus a BoringSSL `TlsConnection` and transparently drives
/// the TLS state machine (handshake + record decrypt/encrypt) on every I/O.
///
/// `bun_uws`'s `ws_handshake::client_handshake<S: Read + Write>` and
/// `ws_codec::FrameDecoder` consume this directly, so the wss:// path reuses
/// the exact same RFC 6455 code as the ws:// path.
struct TlsStream {
    tcp: TcpStream,
    tls: bao_boringssl_bridge::connection::TlsConnection,
    /// Decrypted plaintext not yet handed to the reader. `Read` callers (the
    /// WS handshake reads byte-at-a-time) may take less than one TLS record
    /// per read(); the surplus must survive across calls (BCE-20260814-WS-TLS:
    /// the prior adapter dropped it, corrupting the handshake).
    pending_plain: Vec<u8>,
    pending_off: usize,
}

impl TlsStream {
    /// Pump the TLS state machine: flush any pending outgoing ciphertext to the
    /// socket, then process inbound records until the TLS layer has decrypted
    /// data ready (or WouldBlock). Returns the decrypted plaintext bytes.
    fn pump_inbound(&mut self) -> ::std::io::Result<Vec<u8>> {
        loop {
            // Drain any ciphertext BoringSSL wants to send first so a
            // mid-handshake flight isn't stranded in the write BIO.
            self.flush_outgoing()?;
            let res = self.tls.process().map_err(|e| {
                ::std::io::Error::new(::std::io::ErrorKind::InvalidData, e.to_string())
            })?;
            if !res.plaintext.is_empty() {
                let mut joined = Vec::new();
                for chunk in res.plaintext {
                    joined.extend_from_slice(&chunk);
                }
                return Ok(joined);
            }
            // No decrypted data yet — read more ciphertext from the socket.
            let mut buf = [0u8; 16_384];
            match self.tcp.read(&mut buf) {
                Ok(0) => {
                    return Err(::std::io::Error::new(
                        ::std::io::ErrorKind::UnexpectedEof,
                        "tls peer closed",
                    ));
                }
                Ok(n) => self.tls.feed(&buf[..n]),
                Err(ref e)
                    if e.kind() == ::std::io::ErrorKind::WouldBlock
                        || e.kind() == ::std::io::ErrorKind::TimedOut =>
                {
                    return Err(::std::io::Error::from(::std::io::ErrorKind::WouldBlock));
                }
                Err(e) => return Err(e),
            }
        }
    }

    /// Write any pending ciphertext from BoringSSL's write BIO to the socket.
    fn flush_outgoing(&mut self) -> ::std::io::Result<()> {
        let outgoing = self.tls.take_outgoing();
        if outgoing.is_empty() {
            return Ok(());
        }
        self.tcp.write_all(&outgoing)
    }
}

impl ::std::io::Read for TlsStream {
    fn read(&mut self, buf: &mut [u8]) -> ::std::io::Result<usize> {
        // Serve buffered plaintext first; only pump the TLS state machine
        // when the buffer is drained (records can exceed the caller's buf).
        if self.pending_off >= self.pending_plain.len() {
            self.pending_plain = self.pump_inbound()?;
            self.pending_off = 0;
        }
        let avail = &self.pending_plain[self.pending_off..];
        let n = avail.len().min(buf.len());
        buf[..n].copy_from_slice(&avail[..n]);
        self.pending_off += n;
        Ok(n)
    }
}

impl ::std::io::Write for TlsStream {
    fn write(&mut self, buf: &[u8]) -> ::std::io::Result<usize> {
        let written = self
            .tls
            .write(buf)
            .map_err(|e| ::std::io::Error::new(::std::io::ErrorKind::InvalidData, e.to_string()))?;
        self.flush_outgoing()?;
        Ok(written)
    }
    fn flush(&mut self) -> ::std::io::Result<()> {
        self.tcp.flush()
    }
}

/// Connection backend — plain ws:// over TCP, or wss:// over TLS.
enum WsConn {
    /// Plain WebSocket reusing `bun_uws::WebSocketClient` (RFC 6455 codec +
    /// handshake + masked client→server frames, all owned by bun_uws).
    Plain(bun_uws::ws_client::WebSocketClient),
    /// TLS WebSocket: a `TlsStream` driven through `bun_uws`'s codec/handshake.
    Tls {
        stream: TlsStream,
        decoder: bun_uws::ws_codec::FrameDecoder,
        closed: bool,
    },
}

impl WsConn {
    /// Connect to a `ws://` or `wss://` URL, applying the caller's stealth
    /// profile to the wss:// TLS handshake. Runs entirely on the caller's
    /// thread — the JS bridge only invokes this on a background worker.
    fn connect(
        url: &str,
        profile: &::std::option::Option<bao_stealth::StealthProfile>,
    ) -> ::std::result::Result<Self, String> {
        let (scheme, rest) = if let Some(r) = url.strip_prefix("ws://") {
            ("ws", r)
        } else if let Some(r) = url.strip_prefix("wss://") {
            ("wss", r)
        } else {
            // Fall back to ws:// semantics for bare hosts (preserves the prior
            // behavior where a scheme-less URL was treated as ws://).
            ("ws", url)
        };

        let (host, port, path) = split_authority_and_path(rest, scheme);
        if scheme == "wss" {
            Self::connect_tls(&host, port, &path, profile)
        } else {
            // ws:// — delegate to bun_uws::WebSocketClient (reconstructs the
            // canonical URL because bun_uws::parse_ws_url is scheme-strict).
            let canonical = if url.starts_with("ws://") || url.starts_with("wss://") {
                url.to_string()
            } else {
                format!("ws://{}", url)
            };
            let client = bun_uws::ws_client::WebSocketClient::connect(&canonical)
                .map_err(|e| format!("ws connect: {}", e))?;
            Ok(WsConn::Plain(client))
        }
    }

    fn connect_tls(
        host: &str,
        port: u16,
        path: &str,
        profile: &::std::option::Option<bao_stealth::StealthProfile>,
    ) -> ::std::result::Result<Self, String> {
        let addr = format!("{}:{}", host, port);
        let socket_addr = addr
            .to_socket_addrs()
            .map_err(|e| format!("invalid address: {}", e))?
            .next()
            .ok_or_else(|| format!("no address for {}", addr))?;
        let mut tcp = TcpStream::connect_timeout(&socket_addr, Duration::from_secs(10))
            .map_err(|e| format!("connect failed: {}", e))?;
        tcp.set_nonblocking(false).ok();
        tcp.set_read_timeout(Some(Duration::from_secs(10))).ok();

        // Build the BoringSSL client connection and drive the TLS handshake.
        let tls_client = bao_boringssl_bridge::client::TlsClient::new()
            .map_err(|e| format!("tls client init: {}", e))?;
        let mut tls =
            bao_boringssl_bridge::connection::TlsConnection::new_client(&tls_client, host)
                .map_err(|e| format!("tls conn: {}", e))?;

        // STEALTH (REQ-STL-001): apply the page's TLS fingerprint through the
        // same application path fetch() uses — cipher list / TLS 1.3 suites /
        // curves / sigalgs, plus SNI and ALPN(http/1.1) (what a browser
        // offers on a WebSocket TLS connection). Must run BEFORE the first
        // `process()` call so the config lands in the ClientHello.
        let ssl_config = crate::stealth_http::stealth_profile_to_ssl_config(profile);
        let host_c = CString::new(host).map_err(|_| format!("invalid host: {}", host))?;
        {
            let ssl = tls.ssl_ptr();
            if !ssl.is_null() {
                // SAFETY: `ssl_ptr` returns the live SSL handle of this
                // connection; `configure_http_client_with_alpn` only issues
                // SSL_set_* configuration calls on it.
                bun_http::configure_http_client_with_alpn(
                    unsafe { &mut *ssl },
                    host_c.as_ptr(),
                    bun_http::AlpnOffer::H1,
                    Some(&ssl_config),
                );

                // TLS session resumption: offer the cached session for this
                // origin before the handshake starts (same precondition as
                // the stealth config above — the ClientHello has not been
                // serialized yet). Salt semantics match the bun_http fetch
                // path: no stealth profile → salt 0 (the default-profile
                // pool shared across stacks); a profile → the SSLConfig
                // content hash, so sessions that short-circuit parameter
                // negotiation never cross profiles.
                let profile_salt = if profile.is_some() {
                    ssl_config.content_hash()
                } else {
                    0
                };
                bao_boringssl_bridge::session_cache::offer_session(ssl, host, port, profile_salt);
            }
        }

        // Complete the TLS handshake by pumping records until active.
        // BCE-20260814-WS-TLS: the flight produced by `process()` MUST be
        // flushed to the socket BEFORE blocking on read — the prior order
        // (take_outgoing → process → read) left the ClientHello stranded in
        // the write BIO while waiting for a ServerHello that could never
        // arrive (both sides reading → deadlock, surfaced as "handshake
        // stalled" after the 10s timeout).
        loop {
            match tls.process() {
                Ok(res) => {
                    use bao_boringssl_bridge::connection::TlsState;
                    // Flush every flight the state machine just produced
                    // (ClientHello / Finished) before waiting on the peer.
                    loop {
                        let outgoing = tls.take_outgoing();
                        if outgoing.is_empty() {
                            break;
                        }
                        if tcp.write_all(&outgoing).is_err() {
                            return Err("tls handshake write failed".to_string());
                        }
                    }
                    if res.state == TlsState::Active || res.state == TlsState::PeerClosed {
                        break;
                    }
                    // Still handshaking — read more ciphertext from the socket.
                    let mut buf = [0u8; 16_384];
                    match tcp.read(&mut buf) {
                        Ok(n) if n > 0 => tls.feed(&buf[..n]),
                        _ => {
                            return Err("tls handshake stalled".to_string());
                        }
                    }
                }
                Err(e) => return Err(format!("tls handshake: {}", e)),
            }
        }

        let mut stream = TlsStream {
            tcp,
            tls,
            pending_plain: Vec::new(),
            pending_off: 0,
        };
        // RFC 6455 client handshake over the TLS stream (bun_uws-owned).
        bun_uws::ws_handshake::client_handshake(&mut stream, host, &path)
            .map_err(|e| format!("ws handshake: {:?}", e))?;
        Ok(WsConn::Tls {
            stream,
            decoder: bun_uws::ws_codec::FrameDecoder::new(),
            closed: false,
        })
    }

    fn send_text(&mut self, text: &str) -> ::std::result::Result<(), String> {
        match self {
            WsConn::Plain(c) => c.send_text(text).map_err(|e| format!("send failed: {}", e)),
            WsConn::Tls { stream, .. } => {
                let payload = text.as_bytes();
                let key = bun_uws::ws_codec::gen_mask_key();
                let mut frame = Vec::with_capacity(payload.len() + 14);
                frame.push(0x81); // FIN + text opcode
                push_masked_len(&mut frame, payload.len());
                frame.extend_from_slice(&key);
                let mut masked = payload.to_vec();
                apply_mask(&mut masked, &key);
                // BCE-20260814-WS-TLS: the masked payload was never appended
                // to the frame — every wss:// send() transmitted header+key
                // only, dropping the message body (peer then misparsed the
                // next frame's bytes as this frame's payload).
                frame.extend_from_slice(&masked);
                stream
                    .write_all(&frame)
                    .map_err(|e| format!("send failed: {}", e))
            }
        }
    }

    fn read_message(&mut self) -> ::std::result::Result<WsMessage, String> {
        match self {
            WsConn::Plain(c) => match c.recv().map_err(|e| format!("recv: {}", e))? {
                bun_uws::ws_client::RecvOutcome::Message(opcode, payload) => match opcode {
                    bun_uws::ws_codec::Opcode::Text => Ok(WsMessage::Text(
                        String::from_utf8_lossy(&payload).into_owned(),
                    )),
                    bun_uws::ws_codec::Opcode::Binary => Ok(WsMessage::Binary(payload)),
                    _ => Ok(WsMessage::Binary(payload)),
                },
                bun_uws::ws_client::RecvOutcome::Closed => Ok(WsMessage::Close),
                bun_uws::ws_client::RecvOutcome::Timeout => Err("wouldblock".to_string()),
            },
            WsConn::Tls {
                stream,
                decoder,
                closed,
            } => {
                if *closed {
                    return Ok(WsMessage::Close);
                }
                let header = match decoder.decode_frame(stream) {
                    Ok(Some(h)) => h,
                    Ok(None) => return Err("wouldblock".to_string()),
                    Err(ref e)
                        if e.kind() == ::std::io::ErrorKind::WouldBlock
                            || e.kind() == ::std::io::ErrorKind::TimedOut =>
                    {
                        return Err("wouldblock".to_string());
                    }
                    Err(ref e) if e.kind() == ::std::io::ErrorKind::UnexpectedEof => {
                        *closed = true;
                        return Ok(WsMessage::Close);
                    }
                    Err(e) => return Err(format!("recv: {}", e)),
                };
                let mut payload = if header.mask {
                    let mask_key = decoder.take_mask();
                    let mut p = decoder.take_payload(&header);
                    apply_mask(&mut p, &mask_key);
                    p
                } else {
                    decoder.take_payload(&header)
                };
                match header.opcode {
                    bun_uws::ws_codec::Opcode::Text => Ok(WsMessage::Text(
                        String::from_utf8_lossy(&payload).into_owned(),
                    )),
                    bun_uws::ws_codec::Opcode::Binary => Ok(WsMessage::Binary(payload)),
                    bun_uws::ws_codec::Opcode::Close => {
                        *closed = true;
                        Ok(WsMessage::Close)
                    }
                    bun_uws::ws_codec::Opcode::Ping => {
                        // Echo pong (RFC 6455 §5.5.2) using bun_uws codec mask.
                        let key = bun_uws::ws_codec::gen_mask_key();
                        let mut frame = vec![0x8A]; // FIN + pong
                        push_masked_len(&mut frame, payload.len());
                        frame.extend_from_slice(&key);
                        apply_mask(&mut payload, &key);
                        frame.extend_from_slice(&payload);
                        stream
                            .write_all(&frame)
                            .map_err(|e| format!("pong: {}", e))?;
                        self.read_message()
                    }
                    bun_uws::ws_codec::Opcode::Pong | bun_uws::ws_codec::Opcode::Continuation => {
                        self.read_message()
                    }
                }
            }
        }
    }

    /// Send a masked close frame (code 1000). Deliberately does NOT delegate
    /// to `WebSocketClient::close()` for the Plain variant — that method
    /// `shutdown(Both)`s the socket immediately, killing the TCP connection
    /// before the peer's Close reply can arrive (no close handshake). Here
    /// only the frame is sent; the socket stays readable so the drain pump
    /// can observe the peer's Close reply and fire onclose.
    fn close(&mut self) -> ::std::result::Result<(), String> {
        let frame = encode_masked_close_frame();
        match self {
            WsConn::Plain(c) => {
                if c.is_closed() {
                    return Ok(());
                }
                c.stream_mut()
                    .write_all(&frame)
                    .map_err(|e| format!("close failed: {}", e))
            }
            WsConn::Tls { stream, .. } => {
                // Send the close frame only. Do NOT set the internal `closed`
                // flag here — `read_message` uses it to short-circuit, which
                // would discard inbound frames still in flight (post-close
                // messages are delivered until the close handshake completes,
                // browser semantics). The flag flips when the peer's Close
                // frame is actually read.
                let _ = stream.write_all(&frame);
                Ok(())
            }
        }
    }

    /// Switch the underlying socket between blocking and non-blocking so the
    /// initial drain loop can poll for buffered frames without hanging.
    fn set_nonblocking(&mut self, nonblocking: bool) {
        match self {
            WsConn::Plain(c) => {
                let _ = c.stream_mut().set_nonblocking(nonblocking);
            }
            WsConn::Tls { stream, .. } => {
                let _ = stream.tcp.set_nonblocking(nonblocking);
            }
        }
    }
}

/// Split `host[:port]/path` from the scheme-stripped remainder. Default port
/// is 80 for ws://, 443 for wss://.
fn split_authority_and_path(rest: &str, scheme: &str) -> (String, u16, String) {
    let default_port = if scheme == "wss" { 443 } else { 80 };
    let (authority, path) = match rest.find('/') {
        Some(i) => (&rest[..i], rest[i..].to_string()),
        None => (rest, "/".to_string()),
    };
    let (host, port) = match authority.rfind(':') {
        Some(i) => (
            authority[..i].to_string(),
            authority[i + 1..].parse::<u16>().unwrap_or(default_port),
        ),
        None => (authority.to_string(), default_port),
    };
    (host, port, path)
}

/// Masked close frame (FIN + Close, code 1000), client→server layout.
fn encode_masked_close_frame() -> Vec<u8> {
    let key = bun_uws::ws_codec::gen_mask_key();
    let mut frame = vec![0x88];
    let payload = 1000u16.to_be_bytes();
    push_masked_len(&mut frame, payload.len());
    frame.extend_from_slice(&key);
    let mut masked = payload.to_vec();
    apply_mask(&mut masked, &key);
    frame.extend_from_slice(&masked);
    frame
}

/// Append the masked-length + (caller-supplied) mask bytes layout for a
/// client→server frame, matching `bun_uws::ws_codec::FrameEncoder::encode_frame`.
fn push_masked_len(frame: &mut Vec<u8>, len: usize) {
    let mask_bit = 0x80u8;
    if len < 126 {
        frame.push((len as u8) | mask_bit);
    } else if len <= u16::MAX as usize {
        frame.push(126u8 | mask_bit);
        frame.extend_from_slice(&(len as u16).to_be_bytes());
    } else {
        frame.push(127u8 | mask_bit);
        frame.extend_from_slice(&(len as u64).to_be_bytes());
    }
}

// ── JS bridge ──

/// Global counter for generating unique GcStore keys for WebSocket objects.
static WS_ID_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Per-connection registry entry. Everything except `connect_slot`'s payload
/// is confined to the thread that constructed the WebSocket (the JS thread);
/// the background connect worker communicates exclusively through the
/// `Arc<Mutex<..>>` slot — no JSObject pointers cross threads
/// (BCE-20260621-001 rule).
struct WsEntry {
    /// Live connection once the background connect completed and onopen
    /// dispatched. Polled non-blocking by `ws_pump_all`.
    client: ::std::option::Option<WsConn>,
    /// Present while the background connect worker is in flight. The worker
    /// writes `Some(Ok(..))` / `Some(Err(..))` exactly once; the JS-thread
    /// drain pump consumes it. `None` + `client: None` = dead entry.
    connect_slot: ::std::option::Option<
        Arc<Mutex<::std::option::Option<::std::result::Result<WsConn, String>>>>,
    >,
    /// JS called close() while still CONNECTING — when the connect lands, the
    /// pump closes it immediately instead of firing onopen.
    close_requested: bool,
    /// JS called close() on an open connection — the close frame was sent;
    /// the pump fires onclose when the peer's Close reply (or transport
    /// error) lands (browser close-handshake semantics).
    close_initiated: bool,
    /// The realm global the WebSocket JS object lives in (captured at
    /// construction). Every dispatch AutoRealms into it (realm-per-context
    /// model, c943b1cc) so the GcStore property lookup and handler call run
    /// in the right compartment.
    realm_global: *mut JSObject,
    js_obj_key: String,
}

impl WsEntry {
    fn is_live(&self) -> bool {
        self.connect_slot.is_some() || self.client.is_some()
    }
}

thread_local! {
    static WS_CONNECTIONS: RefCell<Vec<WsEntry>> = const { RefCell::new(Vec::new()) };
}

/// True while any WebSocket on this thread is connecting or open. Wired into
/// the event-loop liveness checks (`timers::drain_and_check` return value) so
/// the eval loop keeps draining while WS traffic is in flight.
pub fn ws_has_pending() -> bool {
    WS_CONNECTIONS.with(|c| c.borrow().iter().any(|e| e.is_live()))
}

pub fn install_websocket_constructor(
    cx: &mut mozjs::context::JSContext,
    global: mozjs::rust::Handle<*mut JSObject>,
) {
    unsafe {
        let ws_fun = JS_NewFunction(
            cx.raw_cx(),
            Some(websocket_constructor),
            1,
            JSFUN_CONSTRUCTOR,
            c"WebSocket".as_ptr(),
        );
        if !ws_fun.is_null() {
            let ctor_obj = JS_GetFunctionObject(ws_fun);
            if !ctor_obj.is_null() {
                let val = mozjs::jsval::ObjectValue(ctor_obj);
                rooted!(&in(cx) let v = val);
                JS_DefineProperty(
                    cx.raw_cx(),
                    global.into(),
                    c"WebSocket".as_ptr(),
                    v.handle().into(),
                    (JSPROP_ENUMERATE | JSPROP_PERMANENT) as u32,
                );

                rooted!(&in(cx) let ctor_root = ctor_obj);
                for (name, value) in &[
                    ("CONNECTING", 0i32),
                    ("OPEN", 1),
                    ("CLOSING", 2),
                    ("CLOSED", 3),
                ] {
                    let c_name = ZBox::from_bytes(name.as_bytes());
                    rooted!(&in(cx) let iv = Int32Value(*value));
                    JS_DefineProperty(
                        cx.raw_cx(),
                        ctor_root.handle().into(),
                        c_name.as_ptr(),
                        iv.handle().into(),
                        (JSPROP_ENUMERATE | JSPROP_READONLY) as u32,
                    );
                }
            }
        }
    }
}

/// Fire an `onXXX` handler stored on the WebSocket object. `realm_global` is
/// the realm global captured at construction; dispatch AutoRealms into it
/// (the drain pump runs with no realm entered — realm-per-context model).
/// `this` for the call is the WebSocket object (browser semantics).
///
/// # Safety
/// `cx` must be a live JSContext on the current thread; `realm_global` must
/// be the (always-rooted) global of a live realm on that context.
unsafe fn ws_trigger_event(
    cx: *mut JSContext,
    realm_global: *mut JSObject,
    ws_obj_key: &str,
    event_name: &str,
    data_val: Option<JSVal>,
) {
    if realm_global.is_null() {
        return;
    }
    let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;
    rooted!(&in(cx_ref) let global_root = realm_global);
    let mut realm = AutoRealm::new_from_handle(cx_ref, global_root.handle());
    let cx_ref: &mut mozjs::context::JSContext = &mut realm;

    // Root the event data FIRST — gc_store_get / JS_GetProperty below can
    // trigger GC and an unrooted JSVal argument would dangle. Always root
    // (undefined placeholder when the event carries no data) so the guard
    // lives for the whole frame.
    let has_data = data_val.is_some();
    let dv_in = data_val.unwrap_or_else(UndefinedValue);
    rooted!(&in(cx_ref) let data_root = dv_in);

    let ws_obj = match gc_store_get(cx, ws_obj_key) {
        Some(obj) => obj,
        None => return,
    };
    rooted!(&in(cx_ref) let ws_obj_root = ws_obj);
    let mut handler_val = UndefinedValue();
    let c_name = ZBox::from_bytes(event_name.as_bytes());
    JS_GetProperty(
        cx,
        ws_obj_root.handle().into(),
        c_name.as_ptr(),
        MutableHandle::<Value> {
            _phantom_0: ::std::marker::PhantomData,
            ptr: &mut handler_val,
        },
    );
    if handler_val.is_object() {
        rooted!(&in(cx_ref) let handler_obj_root = handler_val.to_object());
        if JS_ObjectIsFunction(handler_obj_root.get()) {
            rooted!(&in(cx_ref) let handler_jsval = ObjectValue(handler_obj_root.get()));

            rooted!(&in(cx_ref) let event_obj = mozjs_sys::jsapi::JS_NewPlainObject(cx));
            if !event_obj.get().is_null() {
                if has_data {
                    JS_DefineProperty(
                        cx,
                        event_obj.handle().into(),
                        c"data".as_ptr(),
                        data_root.handle().into(),
                        JSPROP_ENUMERATE as u32,
                    );
                }
                let ev_val = ObjectValue(event_obj.get());
                let call_args = HandleValueArray {
                    length_: 1,
                    elements_: &ev_val,
                };
                let mut rval = UndefinedValue();
                let _ = JS_CallFunctionValue(
                    cx,
                    ws_obj_root.handle().into(),
                    handler_jsval.handle().into(),
                    &call_args,
                    MutableHandle::<Value> {
                        _phantom_0: ::std::marker::PhantomData,
                        ptr: &mut rval,
                    },
                );
            }
        }
    }
}

unsafe extern "C" fn ws_send(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    if argc == 0 {
        JS_ReportErrorUTF8(cx, c"WebSocket.send() requires a message argument".as_ptr());
        return false;
    }
    let msg_val = *args.get(0).ptr;

    let wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    rooted!(&in(wrapped_cx) let this_obj = args.thisv().to_object());
    let mut idx_val = Int32Value(-1);
    JS_GetProperty(
        cx,
        this_obj.handle().into(),
        c"_wsIdx".as_ptr(),
        MutableHandle::<Value> {
            _phantom_0: ::std::marker::PhantomData,
            ptr: &mut idx_val,
        },
    );
    let idx = idx_val.to_int32() as usize;

    let send_result = WS_CONNECTIONS.with(|c| {
        let mut conns = c.borrow_mut();
        match conns.get_mut(idx) {
            // Browser parity: send() while CONNECTING/CLOSED throws
            // InvalidStateError (never silently drops the message).
            Some(e) if e.client.is_some() => {
                let s = unsafe_jsstr_to_string(cx, NonNull::new_unchecked(msg_val.to_string()));
                e.client.as_mut().unwrap().send_text(&s)
            }
            Some(e) if e.connect_slot.is_some() => {
                Err("InvalidStateError: WebSocket is still connecting".to_string())
            }
            _ => Err("InvalidStateError: WebSocket is already closed".to_string()),
        }
    });

    if let Err(e) = send_result {
        let msg = format!("WebSocket send failed: {}", e);
        let c_msg = ZBox::from_bytes(msg.as_bytes());
        JS_ReportErrorUTF8(cx, c"%s".as_ptr(), c_msg.as_ptr());
        return false;
    }
    args.rval().set(UndefinedValue());
    true
}

unsafe extern "C" fn ws_close_fn(cx: *mut JSContext, _argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, _argc);
    let wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    rooted!(&in(wrapped_cx) let this_obj = args.thisv().to_object());

    let mut idx_val = Int32Value(-1);
    JS_GetProperty(
        cx,
        this_obj.handle().into(),
        c"_wsIdx".as_ptr(),
        MutableHandle::<Value> {
            _phantom_0: ::std::marker::PhantomData,
            ptr: &mut idx_val,
        },
    );
    let idx = idx_val.to_int32() as usize;

    WS_CONNECTIONS.with(|c| {
        let mut conns = c.borrow_mut();
        if let Some(e) = conns.get_mut(idx) {
            if e.client.is_some() && !e.close_initiated {
                // Send the close frame once; keep the socket alive so the
                // pump can see the peer's Close reply and fire onclose
                // (close handshake).
                if let Some(client) = &mut e.client {
                    let _ = client.close();
                }
                e.close_initiated = true;
            } else if e.connect_slot.is_some() {
                // Still CONNECTING — flag it; the pump closes immediately
                // when the background connect lands (no onopen).
                e.close_requested = true;
            }
        }
    });

    rooted!(&in(wrapped_cx) let closing_val = Int32Value(2));
    JS_SetProperty(
        cx,
        this_obj.handle().into(),
        c"readyState".as_ptr(),
        closing_val.handle().into(),
    );
    args.rval().set(UndefinedValue());
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn websocket_constructor(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    if argc == 0 {
        JS_ReportErrorUTF8(cx, c"WebSocket requires a URL argument".as_ptr());
        return false;
    }
    let url_val = *args.get(0).ptr;
    if !url_val.is_string() {
        JS_ReportErrorUTF8(cx, c"WebSocket URL must be a string".as_ptr());
        return false;
    }
    let url = unsafe_jsstr_to_string(cx, NonNull::new_unchecked(url_val.to_string()));

    let wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    rooted!(&in(wrapped_cx) let ws_obj = mozjs_sys::jsapi::JS_NewPlainObject(cx));
    if ws_obj.get().is_null() {
        args.rval().set(UndefinedValue());
        return true;
    }

    {
        let c_url = ZBox::from_bytes(url.as_bytes());
        let js_str = JS_NewStringCopyZ(cx, c_url.as_ptr());
        if !js_str.is_null() {
            rooted!(&in(wrapped_cx) let v = StringValue(&*js_str));
            JS_DefineProperty(
                cx,
                ws_obj.handle().into(),
                c"url".as_ptr(),
                v.handle().into(),
                JSPROP_ENUMERATE as u32,
            );
        }
    }

    rooted!(&in(wrapped_cx) let state_val = Int32Value(0));
    JS_DefineProperty(
        cx,
        ws_obj.handle().into(),
        c"readyState".as_ptr(),
        state_val.handle().into(),
        JSPROP_ENUMERATE as u32,
    );

    rooted!(&in(wrapped_cx) let ba_val = Int32Value(0));
    JS_DefineProperty(
        cx,
        ws_obj.handle().into(),
        c"bufferedAmount".as_ptr(),
        ba_val.handle().into(),
        JSPROP_ENUMERATE as u32,
    );

    for name in &["onopen", "onmessage", "onerror", "onclose"] {
        let c_name = ZBox::from_bytes(name.as_bytes());
        rooted!(&in(wrapped_cx) let ud = UndefinedValue());
        JS_DefineProperty(
            cx,
            ws_obj.handle().into(),
            c_name.as_ptr(),
            ud.handle().into(),
            JSPROP_ENUMERATE as u32,
        );
    }

    mozjs_sys::jsapi::JS_DefineFunction(
        cx,
        ws_obj.handle().into(),
        c"send".as_ptr(),
        Some(ws_send),
        1,
        JSPROP_ENUMERATE as u32,
    );
    mozjs_sys::jsapi::JS_DefineFunction(
        cx,
        ws_obj.handle().into(),
        c"close".as_ptr(),
        Some(ws_close_fn),
        0,
        JSPROP_ENUMERATE as u32,
    );

    // Permission parity with fetch(): the page's net scope governs
    // WebSocket egress too (Permission sandbox).
    {
        let (scheme, rest) = if let Some(r) = url.strip_prefix("ws://") {
            ("ws", r)
        } else if let Some(r) = url.strip_prefix("wss://") {
            ("wss", r)
        } else {
            ("ws", url.as_str())
        };
        let (host, _, _) = split_authority_and_path(rest, scheme);
        if let ::std::result::Result::Err(e) = crate::permission_bridge::check_net(&host) {
            let c_msg = ZBox::from_bytes(e.as_bytes());
            JS_ReportErrorUTF8(cx, c"%s".as_ptr(), c_msg.as_ptr());
            return false;
        }
    }

    // Store the JS WebSocket object in GcStore for GC safety.
    let ws_id = WS_ID_COUNTER.fetch_add(1, Ordering::Relaxed);
    let ws_key = format!("ws_{}", ws_id);
    gc_store_insert(cx, &ws_key, ws_obj.get());

    // Capture the realm global (dispatch AutoRealm target) and the thread's
    // stealth profile HERE, on the JS thread — both are thread-local state.
    let realm_global = CurrentGlobalOrNull(cx);
    let profile = crate::fetch_api::get_fetch_stealth_profile();

    // Register the entry with a pending connect slot; _wsIdx must exist
    // immediately (send()/close() may be called while CONNECTING).
    let slot: Arc<Mutex<::std::option::Option<::std::result::Result<WsConn, String>>>> =
        Arc::new(Mutex::new(None));
    let ws_idx = WS_CONNECTIONS.with(|c| {
        let mut conns = c.borrow_mut();
        conns.push(WsEntry {
            client: None,
            connect_slot: Some(Arc::clone(&slot)),
            close_requested: false,
            close_initiated: false,
            realm_global,
            js_obj_key: ws_key.clone(),
        });
        conns.len() - 1
    });
    rooted!(&in(wrapped_cx) let idx_val = Int32Value(ws_idx as i32));
    JS_DefineProperty(
        cx,
        ws_obj.handle().into(),
        c"_wsIdx".as_ptr(),
        idx_val.handle().into(),
        0,
    );

    // Background connect: the full blocking sequence (TCP + TLS with the
    // page's stealth fingerprint + RFC 6455 handshake, ≤10s) runs OFF the JS
    // thread. The Arc<Mutex> slot is the only cross-thread channel; the
    // JS-thread drain pump (`ws_pump_all`) consumes the outcome and fires
    // onopen / onerror(+onclose). No JSObject pointer crosses threads.
    let url_owned = url.clone();
    ::std::thread::spawn(move || {
        let result = WsConn::connect(&url_owned, &profile);
        if let Ok(mut guard) = slot.lock() {
            *guard = Some(result);
        }
        // If the JS thread is gone (process teardown), the slot leaks —
        // bounded by the 10s connect timeout.
    });

    // readyState stays 0 (CONNECTING). The constructor returns immediately —
    // connect failures surface as onerror + onclose (browser semantics),
    // never a constructor throw, never a silent swallow.
    args.rval().set(mozjs::jsval::ObjectValue(ws_obj.get()));
    true
}

// ── WebSocket drain pump ──
// Runs on the JS thread from the event-loop drain paths. Consumes completed
// background connects and pumps inbound frames. Never blocks (try_lock +
// non-blocking sockets); JS handlers run OUTSIDE the WS_CONNECTIONS borrow so
// they can call send()/close() reentrantly.

/// One action peeled off the registry per iteration (JS calls happen after
/// the borrow is dropped).
enum PumpAction {
    /// Background connect finished (slot outcome consumed).
    ConnectDone(usize, ::std::result::Result<WsConn, String>),
    /// Text frame received on an open connection.
    TextMessage(usize, String),
    /// Binary frame received on an open connection.
    BinaryMessage(usize, Vec<u8>),
    /// Close frame received / transport error — connection is dead.
    /// `msg` is Some for a transport error (fires onerror first), None for a
    /// clean close handshake.
    Closed(usize, ::std::option::Option<String>),
}

/// Pump all WebSockets on this thread. Called from `timers::drain_and_check`,
/// `timers::drain_one_pass`, and the servo node-realm evaluate entry.
pub fn ws_pump_all(raw_cx: *mut JSContext) {
    loop {
        let action = WS_CONNECTIONS.with(|c| {
            let mut conns = c.borrow_mut();
            for (idx, e) in conns.iter_mut().enumerate() {
                // 1. Completed background connects (try_lock — the worker
                //    may still hold the lock writing its outcome). Take the
                //    slot out first so the MutexGuard borrow ends before the
                //    assignment below.
                if e.connect_slot.is_some() {
                    let taken = e
                        .connect_slot
                        .as_ref()
                        .unwrap()
                        .try_lock()
                        .ok()
                        .and_then(|mut guard| guard.take());
                    if let ::std::option::Option::Some(res) = taken {
                        e.connect_slot = ::std::option::Option::None;
                        return ::std::option::Option::Some(PumpAction::ConnectDone(idx, res));
                    }
                    continue;
                }
                // 2. Inbound frames on open connections (non-blocking).
                //    Frames received after a locally-initiated close are
                //    still delivered (browser semantics: messages queue
                //    until the close handshake completes).
                let ::std::option::Option::Some(client) = &mut e.client else {
                    continue;
                };
                match client.read_message() {
                    Ok(WsMessage::Text(t)) => {
                        return ::std::option::Option::Some(PumpAction::TextMessage(idx, t))
                    }
                    Ok(WsMessage::Binary(b)) => {
                        return ::std::option::Option::Some(PumpAction::BinaryMessage(idx, b))
                    }
                    Ok(WsMessage::Close) => {
                        // Echo the close handshake unless we already sent
                        // our close frame (close_initiated).
                        if !e.close_initiated {
                            let _ = client.close();
                        }
                        e.client = ::std::option::Option::None;
                        return ::std::option::Option::Some(PumpAction::Closed(idx, None));
                    }
                    Err(err) if err == "wouldblock" => continue,
                    Err(err) => {
                        // Transport error — explicit surface, never silent.
                        let _ = client.close();
                        e.client = ::std::option::Option::None;
                        return ::std::option::Option::Some(PumpAction::Closed(
                            idx,
                            ::std::option::Option::Some(err),
                        ));
                    }
                }
            }
            ::std::option::Option::None
        });

        match action {
            ::std::option::Option::Some(PumpAction::ConnectDone(idx, res)) => unsafe {
                ws_connect_dispatch(raw_cx, idx, res);
            },
            ::std::option::Option::Some(PumpAction::TextMessage(idx, text)) => unsafe {
                ws_message_dispatch(raw_cx, idx, ::std::option::Option::Some(text), None);
            },
            ::std::option::Option::Some(PumpAction::BinaryMessage(idx, bytes)) => unsafe {
                ws_message_dispatch(raw_cx, idx, None, ::std::option::Option::Some(bytes));
            },
            ::std::option::Option::Some(PumpAction::Closed(idx, err)) => unsafe {
                ws_closed_dispatch(raw_cx, idx, err);
            },
            ::std::option::Option::None => break,
        }
    }
}

/// Snapshot of the dispatch-relevant fields of one entry.
struct WsDispatchInfo {
    realm_global: *mut JSObject,
    js_obj_key: String,
    is_open: bool,
}

fn ws_entry_info(idx: usize) -> ::std::option::Option<WsDispatchInfo> {
    WS_CONNECTIONS.with(|c| {
        c.borrow().get(idx).map(|e| WsDispatchInfo {
            realm_global: e.realm_global,
            js_obj_key: e.js_obj_key.clone(),
            is_open: e.client.is_some(),
        })
    })
}

/// Set `readyState` on the stored WebSocket object (inside its realm).
///
/// # Safety
/// `raw_cx` must be a live JSContext on the current thread.
unsafe fn ws_set_ready_state(
    raw_cx: *mut JSContext,
    realm_global: *mut JSObject,
    ws_key: &str,
    state: i32,
) {
    if realm_global.is_null() {
        return;
    }
    // Enter the realm FIRST — gc_store_get resolves through
    // CurrentGlobalOrNull, which is null while the drain pump runs.
    let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(raw_cx));
    let cx_ref = &mut wrapped_cx;
    rooted!(&in(cx_ref) let global_root = realm_global);
    let mut realm = AutoRealm::new_from_handle(cx_ref, global_root.handle());
    let cx_ref: &mut mozjs::context::JSContext = &mut realm;
    let ws_obj = match gc_store_get(raw_cx, ws_key) {
        Some(o) => o,
        None => return,
    };
    rooted!(&in(cx_ref) let ws_obj_root = ws_obj);
    rooted!(&in(cx_ref) let state_val = Int32Value(state));
    JS_SetProperty(
        raw_cx,
        ws_obj_root.handle().into(),
        c"readyState".as_ptr(),
        state_val.handle().into(),
    );
}

/// Background connect completed: install the connection and fire onopen, or
/// surface the failure as onerror + onclose (explicit, never silent).
///
/// # Safety
/// `raw_cx` must be a live JSContext on the current thread.
unsafe fn ws_connect_dispatch(
    raw_cx: *mut JSContext,
    idx: usize,
    res: ::std::result::Result<WsConn, String>,
) {
    let info = match ws_entry_info(idx) {
        Some(i) => i,
        None => return,
    };
    match res {
        Ok(mut client) => {
            // Non-blocking from here on — the drain pump polls this socket.
            client.set_nonblocking(true);
            enum Landed {
                Open,
                CloseNow,
            }
            let landed = WS_CONNECTIONS.with(|c| {
                let mut conns = c.borrow_mut();
                match conns.get_mut(idx) {
                    // close() was called while CONNECTING: never open it.
                    ::std::option::Option::Some(e) if e.close_requested => {
                        e.close_requested = false;
                        Landed::CloseNow
                    }
                    ::std::option::Option::Some(e) => {
                        e.client = ::std::option::Option::Some(client);
                        Landed::Open
                    }
                    ::std::option::Option::None => Landed::CloseNow,
                }
            });
            match landed {
                Landed::Open => {
                    ws_set_ready_state(raw_cx, info.realm_global, &info.js_obj_key, 1);
                    ws_trigger_event(raw_cx, info.realm_global, &info.js_obj_key, "onopen", None);
                }
                Landed::CloseNow => {
                    // `client` was not installed; dropping it closes the
                    // socket (TcpStream Drop).
                    ws_set_ready_state(raw_cx, info.realm_global, &info.js_obj_key, 3);
                    ws_trigger_event(raw_cx, info.realm_global, &info.js_obj_key, "onclose", None);
                    gc_store_remove(raw_cx, &info.js_obj_key);
                }
            }
        }
        Err(msg) => {
            // Connect failed: CLOSED + onerror(with the reason) + onclose.
            ws_set_ready_state(raw_cx, info.realm_global, &info.js_obj_key, 3);
            // Enter the realm before creating the error string — allocation
            // needs a valid zone (the pump runs with no realm entered).
            let data = if !info.realm_global.is_null() {
                let mut wrapped_cx =
                    mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(raw_cx));
                let cx_ref = &mut wrapped_cx;
                rooted!(&in(cx_ref) let global_root = info.realm_global);
                let mut realm = AutoRealm::new_from_handle(cx_ref, global_root.handle());
                let _cx_in_realm: &mut mozjs::context::JSContext = &mut realm;
                let c_msg = ZBox::from_bytes(msg.as_bytes());
                let js_str = JS_NewStringCopyZ(raw_cx, c_msg.as_ptr());
                if !js_str.is_null() {
                    ::std::option::Option::Some(StringValue(&*js_str))
                } else {
                    ::std::option::Option::None
                }
            } else {
                ::std::option::Option::None
            };
            ws_trigger_event(raw_cx, info.realm_global, &info.js_obj_key, "onerror", data);
            ws_trigger_event(raw_cx, info.realm_global, &info.js_obj_key, "onclose", None);
            gc_store_remove(raw_cx, &info.js_obj_key);
        }
    }
}

/// Inbound frame on an open connection: fire onmessage. Text frames arrive
/// as strings; binary frames as an Array of byte values (explicit — binary
/// was previously dropped silently).
///
/// # Safety
/// `raw_cx` must be a live JSContext on the current thread.
unsafe fn ws_message_dispatch(
    raw_cx: *mut JSContext,
    idx: usize,
    text: ::std::option::Option<String>,
    binary: ::std::option::Option<Vec<u8>>,
) {
    let info = match ws_entry_info(idx) {
        Some(i) => i,
        None => return,
    };
    if !info.is_open {
        return;
    }
    if info.realm_global.is_null() {
        return;
    }
    // Enter the realm BEFORE any JS value creation — JS_NewStringCopyZ /
    // JS_NewUint8Array allocate in the current zone, and the drain pump runs
    // with no realm entered (invalid zone → SIGSEGV in the allocator).
    let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(raw_cx));
    let cx_ref = &mut wrapped_cx;
    rooted!(&in(cx_ref) let global_root = info.realm_global);
    let mut realm = AutoRealm::new_from_handle(cx_ref, global_root.handle());
    let cx_ref: &mut mozjs::context::JSContext = &mut realm;
    let data = if let ::std::option::Option::Some(t) = text {
        let c_text = ZBox::from_bytes(t.as_bytes());
        let js_str = JS_NewStringCopyZ(raw_cx, c_text.as_ptr());
        if js_str.is_null() {
            return;
        }
        StringValue(&*js_str)
    } else if let ::std::option::Option::Some(bytes) = binary {
        let arr = mozjs_sys::jsapi::JS_NewUint8Array(raw_cx, bytes.len());
        if arr.is_null() {
            return;
        }
        rooted!(&in(cx_ref) let arr_root = arr);
        if !bytes.is_empty() {
            let mut is_shared = false;
            // SAFETY: same pattern as bun_api.rs — data pointer of the
            // just-created, rooted Uint8Array; copied before any GC point.
            let data_ptr = mozjs_sys::jsapi::JS_GetUint8ArrayData(
                arr_root.get(),
                &mut is_shared,
                ::std::ptr::null(),
            );
            if !data_ptr.is_null() {
                ::std::ptr::copy_nonoverlapping(bytes.as_ptr(), data_ptr, bytes.len());
            }
        }
        ObjectValue(arr_root.get())
    } else {
        return;
    };
    ws_trigger_event(
        raw_cx,
        info.realm_global,
        &info.js_obj_key,
        "onmessage",
        ::std::option::Option::Some(data),
    );
}

/// Connection is dead (close handshake finished or transport error): CLOSED +
/// onclose (+ onerror first for a transport error). Terminal cleanup of the
/// GcStore root.
///
/// # Safety
/// `raw_cx` must be a live JSContext on the current thread.
unsafe fn ws_closed_dispatch(
    raw_cx: *mut JSContext,
    idx: usize,
    err: ::std::option::Option<String>,
) {
    let info = match ws_entry_info(idx) {
        Some(i) => i,
        None => return,
    };
    ws_set_ready_state(raw_cx, info.realm_global, &info.js_obj_key, 3);
    if let ::std::option::Option::Some(msg) = err {
        // Enter the realm before creating the error string (valid zone for
        // allocation — the pump runs with no realm entered).
        let data = if !info.realm_global.is_null() {
            let mut wrapped_cx =
                mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(raw_cx));
            let cx_ref = &mut wrapped_cx;
            rooted!(&in(cx_ref) let global_root = info.realm_global);
            let mut realm = AutoRealm::new_from_handle(cx_ref, global_root.handle());
            let _cx_in_realm: &mut mozjs::context::JSContext = &mut realm;
            let c_msg = ZBox::from_bytes(msg.as_bytes());
            let js_str = JS_NewStringCopyZ(raw_cx, c_msg.as_ptr());
            if !js_str.is_null() {
                ::std::option::Option::Some(StringValue(&*js_str))
            } else {
                ::std::option::Option::None
            }
        } else {
            ::std::option::Option::None
        };
        ws_trigger_event(raw_cx, info.realm_global, &info.js_obj_key, "onerror", data);
    }
    ws_trigger_event(raw_cx, info.realm_global, &info.js_obj_key, "onclose", None);
    gc_store_remove(raw_cx, &info.js_obj_key);
}

// ── Performance ──

pub fn install_performance(
    cx: &mut mozjs::context::JSContext,
    global: mozjs::rust::Handle<*mut JSObject>,
) {
    unsafe {
        rooted!(&in(cx) let perf_obj = JS_NewPlainObject(cx));
        if perf_obj.get().is_null() {
            return;
        }
        JS_DefineFunction(
            cx,
            perf_obj.handle(),
            c"now".as_ptr(),
            Some(performance_now),
            0,
            JSPROP_ENUMERATE as u32,
        );
        JS_DefineProperty3(
            cx,
            global,
            c"performance".as_ptr(),
            perf_obj.handle(),
            JSPROP_ENUMERATE as u32,
        );
    }
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn performance_now(_cx: *mut JSContext, _argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, _argc);
    let now = ::std::time::SystemTime::now()
        .duration_since(::std::time::UNIX_EPOCH)
        .unwrap_or_default();
    let ms = now.as_secs_f64() * 1000.0;
    args.rval().set(mozjs::jsval::DoubleValue(ms));
    true
}

// ── TextEncoder / TextDecoder ──

pub fn install_web_encodings(
    cx: &mut mozjs::context::JSContext,
    global: mozjs::rust::Handle<*mut JSObject>,
) {
    unsafe {
        let te_fun = JS_NewFunction(
            cx.raw_cx(),
            Some(text_encoder_constructor),
            0,
            JSFUN_CONSTRUCTOR,
            c"TextEncoder".as_ptr(),
        );
        if !te_fun.is_null() {
            let te_obj = JS_GetFunctionObject(te_fun);
            if !te_obj.is_null() {
                rooted!(&in(cx) let te_obj_r = te_obj);
                rooted!(&in(cx) let proto = JS_NewPlainObject(cx));
                if !proto.get().is_null() {
                    JS_DefineFunction(
                        cx,
                        proto.handle(),
                        c"encode".as_ptr(),
                        Some(text_encoder_encode),
                        1,
                        JSPROP_ENUMERATE as u32,
                    );
                    JS_DefineFunction(
                        cx,
                        proto.handle(),
                        c"encodeInto".as_ptr(),
                        Some(text_encoder_encode_into),
                        2,
                        JSPROP_ENUMERATE as u32,
                    );
                    JS_DefineProperty3(
                        cx,
                        te_obj_r.handle(),
                        c"prototype".as_ptr(),
                        proto.handle(),
                        JSPROP_PERMANENT as u32,
                    );
                }
                JS_DefineProperty3(
                    cx,
                    global,
                    c"TextEncoder".as_ptr(),
                    te_obj_r.handle(),
                    (JSPROP_ENUMERATE | JSPROP_PERMANENT) as u32,
                );
            }
        }

        let td_fun = JS_NewFunction(
            cx.raw_cx(),
            Some(text_decoder_constructor),
            1,
            JSFUN_CONSTRUCTOR,
            c"TextDecoder".as_ptr(),
        );
        if !td_fun.is_null() {
            let td_obj = JS_GetFunctionObject(td_fun);
            if !td_obj.is_null() {
                rooted!(&in(cx) let td_obj_r = td_obj);
                rooted!(&in(cx) let proto = JS_NewPlainObject(cx));
                if !proto.get().is_null() {
                    JS_DefineFunction(
                        cx,
                        proto.handle(),
                        c"decode".as_ptr(),
                        Some(text_decoder_decode),
                        1,
                        JSPROP_ENUMERATE as u32,
                    );
                    JS_DefineProperty3(
                        cx,
                        td_obj_r.handle(),
                        c"prototype".as_ptr(),
                        proto.handle(),
                        JSPROP_PERMANENT as u32,
                    );
                }
                JS_DefineProperty3(
                    cx,
                    global,
                    c"TextDecoder".as_ptr(),
                    td_obj_r.handle(),
                    (JSPROP_ENUMERATE | JSPROP_PERMANENT) as u32,
                );
            }
        }
    }
}

pub fn install_atob_btoa(
    cx: &mut mozjs::context::JSContext,
    global: mozjs::rust::Handle<*mut JSObject>,
) {
    unsafe {
        JS_DefineFunction(
            cx,
            global,
            c"atob".as_ptr(),
            Some(atob_fn),
            1,
            JSPROP_ENUMERATE as u32,
        );
        JS_DefineFunction(
            cx,
            global,
            c"btoa".as_ptr(),
            Some(btoa_fn),
            1,
            JSPROP_ENUMERATE as u32,
        );
    }
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn atob_fn(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    if argc == 0 || !(*args.get(0).ptr).is_string() {
        args.rval().set(UndefinedValue());
        return true;
    }
    let s = unsafe_jsstr_to_string(
        cx,
        ::std::ptr::NonNull::new_unchecked((*args.get(0).ptr).to_string()),
    );
    match bun_base64::decode_alloc(s.as_bytes()) {
        Ok(bytes) => {
            let decoded = String::from_utf8_lossy(&bytes);
            let c_str = ZBox::from_vec(decoded.into_owned().into_bytes());
            let js_str = JS_NewStringCopyZ(cx, c_str.as_ptr());
            if js_str.is_null() {
                args.rval().set(UndefinedValue());
            } else {
                args.rval().set(StringValue(&*js_str));
            }
        }
        Err(_) => {
            JS_ReportErrorUTF8(cx, c"Failed to decode base64".as_ptr());
            return false;
        }
    }
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn btoa_fn(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    if argc == 0 || !(*args.get(0).ptr).is_string() {
        args.rval().set(UndefinedValue());
        return true;
    }
    let s = unsafe_jsstr_to_string(
        cx,
        ::std::ptr::NonNull::new_unchecked((*args.get(0).ptr).to_string()),
    );
    let encoded_bytes = bun_base64::encode_alloc(s.as_bytes());
    let encoded = ::std::str::from_utf8(&encoded_bytes).unwrap_or("");
    let c_str = ZBox::from_bytes(encoded.as_bytes());
    let js_str = JS_NewStringCopyZ(cx, c_str.as_ptr());
    if js_str.is_null() {
        args.rval().set(UndefinedValue());
    } else {
        args.rval().set(StringValue(&*js_str));
    }
    true
}

pub fn install_queue_microtask(
    cx: &mut mozjs::context::JSContext,
    global: mozjs::rust::Handle<*mut JSObject>,
) {
    unsafe {
        JS_DefineFunction(
            cx,
            global,
            c"queueMicrotask".as_ptr(),
            Some(queue_microtask_fn),
            1,
            JSPROP_ENUMERATE as u32,
        );
    }
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn text_encoder_constructor(
    cx: *mut JSContext,
    argc: u32,
    vp: *mut JSVal,
) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let obj = mozjs_sys::jsapi::JS_NewPlainObject(cx);
    if obj.is_null() {
        args.rval().set(UndefinedValue());
        return true;
    }
    let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    rooted!(&in(wrapped_cx) let obj_r = obj);
    let encoding_str = JS_NewStringCopyZ(cx, c"utf-8".as_ptr());
    if !encoding_str.is_null() {
        let val = StringValue(&*encoding_str);
        rooted!(&in(wrapped_cx) let val_root = val);
        JS_DefineProperty(
            cx,
            obj_r.handle().into(),
            c"encoding".as_ptr(),
            val_root.handle().into(),
            (JSPROP_ENUMERATE | JSPROP_READONLY) as u32,
        );
    }

    JS_DefineFunction(
        &mut wrapped_cx,
        obj_r.handle(),
        c"encode".as_ptr(),
        Some(text_encoder_encode),
        1,
        JSPROP_ENUMERATE as u32,
    );
    JS_DefineFunction(
        &mut wrapped_cx,
        obj_r.handle(),
        c"encodeInto".as_ptr(),
        Some(text_encoder_encode_into),
        2,
        JSPROP_ENUMERATE as u32,
    );

    args.rval().set(ObjectValue(obj));
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn text_encoder_encode(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let input = if argc > 0 {
        let v = *args.get(0).ptr;
        if v.is_string() {
            crate::js_to_rust_string(cx, v)
        } else {
            String::new()
        }
    } else {
        String::new()
    };

    let bytes = input.as_bytes();
    let wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));

    // @trace REQ-ENG-005 [api:TextEncoder.encode] — Return a real SM
    // Uint8Array (not a plain Array) so callers see .byteLength/.buffer and
    // pass `instanceof Uint8Array`. Buffer.test.js drives this via
    // `new TextEncoder().encode(str).byteLength`.
    let u8_obj = mozjs_sys::jsapi::JS_NewUint8Array(cx, bytes.len());
    if u8_obj.is_null() {
        args.rval().set(UndefinedValue());
        return true;
    }
    if !bytes.is_empty() {
        rooted!(&in(wrapped_cx) let arr = u8_obj);
        let mut is_shared = false;
        let data_ptr =
            mozjs_sys::jsapi::JS_GetUint8ArrayData(arr.get(), &mut is_shared, ::std::ptr::null());
        if !data_ptr.is_null() {
            ::std::ptr::copy_nonoverlapping(bytes.as_ptr(), data_ptr, bytes.len());
        }
        args.rval().set(ObjectValue(arr.get()));
    } else {
        args.rval().set(ObjectValue(u8_obj));
    }
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn text_encoder_encode_into(
    _cx: *mut JSContext,
    _argc: u32,
    vp: *mut JSVal,
) -> bool {
    let args = CallArgs::from_vp(vp, _argc);
    args.rval().set(UndefinedValue());
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn text_decoder_constructor(
    cx: *mut JSContext,
    argc: u32,
    vp: *mut JSVal,
) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let obj = mozjs_sys::jsapi::JS_NewPlainObject(cx);
    if obj.is_null() {
        args.rval().set(UndefinedValue());
        return true;
    }
    let encoding = if argc > 0 {
        let v = *args.get(0).ptr;
        if v.is_string() {
            crate::js_to_rust_string(cx, v)
        } else {
            "utf-8".to_string()
        }
    } else {
        "utf-8".to_string()
    };
    let encoding_lower = encoding.to_lowercase();
    let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    rooted!(&in(wrapped_cx) let obj_r = obj);
    let encoding_str = JS_NewStringCopyZ(cx, ZBox::from_bytes(encoding_lower.as_bytes()).as_ptr());
    if !encoding_str.is_null() {
        let val = StringValue(&*encoding_str);
        rooted!(&in(wrapped_cx) let val_root = val);
        JS_DefineProperty(
            cx,
            obj_r.handle().into(),
            c"encoding".as_ptr(),
            val_root.handle().into(),
            (JSPROP_ENUMERATE | JSPROP_READONLY) as u32,
        );
    }
    rooted!(&in(wrapped_cx) let fatal_val = BooleanValue(false));
    JS_DefineProperty(
        cx,
        obj_r.handle().into(),
        c"fatal".as_ptr(),
        fatal_val.handle().into(),
        (JSPROP_ENUMERATE | JSPROP_READONLY) as u32,
    );
    rooted!(&in(wrapped_cx) let bom_val = BooleanValue(false));
    JS_DefineProperty(
        cx,
        obj_r.handle().into(),
        c"ignoreBOM".as_ptr(),
        bom_val.handle().into(),
        (JSPROP_ENUMERATE | JSPROP_READONLY) as u32,
    );

    JS_DefineFunction(
        &mut wrapped_cx,
        obj_r.handle(),
        c"decode".as_ptr(),
        Some(text_decoder_decode),
        1,
        JSPROP_ENUMERATE as u32,
    );

    args.rval().set(ObjectValue(obj));
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn text_decoder_decode(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    if argc == 0 {
        let empty = JS_NewStringCopyZ(cx, c"".as_ptr());
        args.rval().set(if empty.is_null() {
            UndefinedValue()
        } else {
            StringValue(&*empty)
        });
        return true;
    }

    let input = *args.get(0).ptr;

    let bytes = if input.is_object() {
        let wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
        rooted!(&in(wrapped_cx) let obj = input.to_object());
        let mut len_val = UndefinedValue();
        JS_GetProperty(
            cx,
            obj.handle().into(),
            c"length".as_ptr(),
            MutableHandle::<Value> {
                _phantom_0: ::std::marker::PhantomData,
                ptr: &mut len_val,
            },
        );
        let len = if len_val.is_int32() {
            len_val.to_int32() as u32
        } else {
            0
        };
        let mut result = Vec::with_capacity(len as usize);
        for i in 0..len {
            let mut elem = UndefinedValue();
            JS_GetElement(
                cx,
                obj.handle().into(),
                i,
                MutableHandle::<Value> {
                    _phantom_0: ::std::marker::PhantomData,
                    ptr: &mut elem,
                },
            );
            if elem.is_int32() {
                result.push(elem.to_int32() as u8);
            }
        }
        result
    } else {
        Vec::new()
    };

    let decoded = match String::from_utf8(bytes) {
        Ok(s) => s,
        Err(_) => {
            JS_ReportErrorUTF8(cx, c"The encoded data was not valid".as_ptr());
            return false;
        }
    };

    let utf16: Vec<u16> = decoded.encode_utf16().collect();
    let js_str = JS_NewUCStringCopyN(cx, utf16.as_ptr(), utf16.len());
    args.rval().set(if js_str.is_null() {
        UndefinedValue()
    } else {
        StringValue(&*js_str)
    });
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn queue_microtask_fn(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    if argc == 0 || !(*args.get(0).ptr).is_object() {
        return true;
    }
    let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    let cx = &mut wrapped_cx;

    rooted!(&in(cx) let callback = (*args.get(0).ptr).to_object());
    rooted!(&in(cx) let undef_val = UndefinedValue());
    let resolved = CallOriginalPromiseResolve(cx, undef_val.handle());
    if resolved.is_null() {
        args.rval().set(UndefinedValue());
        return true;
    }
    rooted!(&in(cx) let promise = resolved);
    rooted!(&in(cx) let null_reject = ::std::ptr::null_mut::<JSObject>());
    CallOriginalPromiseThen(
        cx,
        promise.handle(),
        callback.handle(),
        null_reject.handle(),
    );
    args.rval().set(UndefinedValue());
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_ws_url_ws() {
        let (host, port, path) = split_authority_and_path("example.com/chat", "ws");
        assert_eq!(host, "example.com");
        assert_eq!(port, 80);
        assert_eq!(path, "/chat");
    }

    // @trace REQ-ENG-006 [api:WebSocket wss://] — wss:// is now supported
    // (default port 443); the prior behaviour rejected it outright.
    #[test]
    fn parse_ws_url_wss_default_port() {
        let (host, port, path) = split_authority_and_path("example.com/secure", "wss");
        assert_eq!(host, "example.com");
        assert_eq!(port, 443);
        assert_eq!(path, "/secure");
    }

    #[test]
    fn parse_ws_url_with_port() {
        let (host, port, path) = split_authority_and_path("localhost:8080/ws", "ws");
        assert_eq!(host, "localhost");
        assert_eq!(port, 8080);
        assert_eq!(path, "/ws");
    }

    #[test]
    fn parse_ws_url_default_path() {
        let (_, _, path) = split_authority_and_path("host/", "ws");
        assert_eq!(path, "/");
    }

    #[test]
    fn parse_ws_url_no_path_defaults_to_slash() {
        let (_, _, path) = split_authority_and_path("host", "ws");
        assert_eq!(path, "/");
    }

    #[test]
    fn parse_ws_url_empty_string() {
        let (host, port, path) = split_authority_and_path("", "ws");
        assert_eq!(host, "");
        assert_eq!(port, 80);
        assert_eq!(path, "/");
    }

    #[test]
    fn parse_ws_url_ipv4_with_port() {
        let (host, port, path) = split_authority_and_path("127.0.0.1:9222/json", "ws");
        assert_eq!(host, "127.0.0.1");
        assert_eq!(port, 9222);
        assert_eq!(path, "/json");
    }

    #[test]
    fn parse_ws_url_query_string() {
        let (host, port, path) = split_authority_and_path("example.com/ws?token=abc", "ws");
        assert_eq!(host, "example.com");
        assert_eq!(port, 80);
        assert!(path.starts_with("/ws"));
    }

    #[test]
    fn parse_ws_url_deep_path() {
        let (host, _, path) = split_authority_and_path("host/a/b/c/d", "ws");
        assert_eq!(host, "host");
        assert_eq!(path, "/a/b/c/d");
    }

    // @trace REQ-ENG-006 [code:bun_uws] — frame length encoding now shares
    // bun_uws::ws_codec's layout via push_masked_len (the client→server masked
    // length bytes). The masking key itself is per-frame random in the live
    // path, so these unit tests verify only the length-byte shape.
    #[test]
    fn push_masked_len_empty() {
        let mut frame = Vec::new();
        push_masked_len(&mut frame, 0);
        assert_eq!(frame.len(), 1);
        assert_eq!(frame[0] & 0x7F, 0); // length = 0
    }

    #[test]
    fn push_masked_len_short() {
        let mut frame = Vec::new();
        push_masked_len(&mut frame, 5);
        assert_eq!(frame.len(), 1);
        assert_eq!(frame[0] & 0x7F, 5); // length = 5
    }

    #[test]
    fn push_masked_len_medium() {
        let mut frame = Vec::new();
        let payload = vec![0u8; 200];
        push_masked_len(&mut frame, payload.len());
        // 1 byte + 2 bytes extended length
        assert_eq!(frame.len(), 3);
        assert_eq!(frame[0] & 0x7F, 126); // 126 signals 16-bit length
        let ext_len = u16::from_be_bytes([frame[1], frame[2]]);
        assert_eq!(ext_len, 200);
    }

    #[test]
    fn push_masked_len_large() {
        let mut frame = Vec::new();
        let payload = vec![0u8; 70000];
        push_masked_len(&mut frame, payload.len());
        // 1 byte + 8 bytes extended length
        assert_eq!(frame.len(), 9);
        assert_eq!(frame[0] & 0x7F, 127); // 127 signals 64-bit length
    }

    #[test]
    fn ws_message_debug_variants() {
        let text = WsMessage::Text("hello".to_string());
        let binary = WsMessage::Binary(vec![1, 2, 3]);
        let close = WsMessage::Close;
        assert!(format!("{:?}", text).contains("Text"));
        assert!(format!("{:?}", binary).contains("Binary"));
        assert!(format!("{:?}", close).contains("Close"));
    }

    // Async WS state machine: liveness transitions of a registry entry.
    // dead (no client, no slot) → connecting (slot) → open (client) → dead.
    #[test]
    fn ws_entry_liveness_transitions() {
        let connecting = WsEntry {
            client: None,
            connect_slot: Some(Arc::new(Mutex::new(None))),
            close_requested: false,
            close_initiated: false,
            realm_global: ::std::ptr::null_mut(),
            js_obj_key: "ws_test".to_string(),
        };
        assert!(connecting.is_live(), "connecting entry is live");

        let dead = WsEntry {
            client: None,
            connect_slot: None,
            close_requested: false,
            close_initiated: false,
            realm_global: ::std::ptr::null_mut(),
            js_obj_key: "ws_test".to_string(),
        };
        assert!(
            !dead.is_live(),
            "dead entry (post-close/failure) is not live"
        );
    }

    // Stealth plumbing: the wss path consumes the exact same profile source
    // fetch() uses. `get_fetch_stealth_profile` must round-trip what
    // `set_fetch_stealth_profile` stored (default-None when unset).
    #[test]
    fn fetch_stealth_profile_getter_roundtrip() {
        let saved = crate::fetch_api::get_fetch_stealth_profile();
        crate::fetch_api::set_fetch_stealth_profile(Some(
            bao_stealth::StealthProfile::firefox_default(),
        ));
        let got = crate::fetch_api::get_fetch_stealth_profile();
        crate::fetch_api::set_fetch_stealth_profile(saved);
        let p = got.expect("profile must round-trip");
        assert!(
            !p.tls.cipher_suites.is_empty(),
            "firefox profile must carry cipher suites (wss stealth source)"
        );
    }
}
