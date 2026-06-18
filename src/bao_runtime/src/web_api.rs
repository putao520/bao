// @trace REQ-ENG-006
// WebSocket + Performance + TextEncoder/TextDecoder + atob/btoa + queueMicrotask
use ::std::cell::RefCell;
use bun_core::ZBox;
use ::std::io::{Read, Write};
use ::std::net::TcpStream;
use ::std::net::ToSocketAddrs;
use ::std::ptr::NonNull;
use ::std::sync::atomic::{AtomicU64, Ordering};
use ::std::time::Duration;

// @trace REQ-ENG-006 [code:bun_uws] — RFC 6455 codec primitives reused for
// both the plain ws:// (via WebSocketClient) and the wss:// (TLS-driven) path.
use bun_uws::ws_codec::apply_mask;

use mozjs::jsapi::*;
use mozjs::jsval::{JSVal, UndefinedValue, StringValue, Int32Value, ObjectValue, BooleanValue};
use mozjs::rooted;
use mozjs::rust::wrappers2::{JS_DefineFunction, JS_DefineProperty3, JS_NewPlainObject, NewArrayObject1, CallOriginalPromiseResolve, CallOriginalPromiseThen};
use mozjs::conversions::jsstr_to_string;

use crate::gc_store::{gc_store_insert, gc_store_get, gc_store_remove};

// @trace REQ-ENG-005 [algorithm:base64] base64 via workspace bun_base64 (SIMD-accelerated)

// ── WebSocket client ──
// @trace REQ-ENG-006 [api:WebSocket] [code:bun_uws] — RFC 6455 framing and the
// plain-text (ws://) client handshake are delegated to `bun_uws::ws_client`
// (WebSocketClient / parse_ws_url / RecvOutcome) and `bun_uws::ws_codec` /
// `ws_handshake`. The wss:// (TLS) variant drives `bao_boringssl_bridge`'s
// TlsConnection over the TCP socket and reuses the same `bun_uws` codec /
// handshake primitives so the two schemes share one wire-format code path.

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
        let plain = self.pump_inbound()?;
        let n = plain.len().min(buf.len());
        buf[..n].copy_from_slice(&plain[..n]);
        Ok(n)
    }
}

impl ::std::io::Write for TlsStream {
    fn write(&mut self, buf: &[u8]) -> ::std::io::Result<usize> {
        let written = self.tls.write(buf).map_err(|e| {
            ::std::io::Error::new(::std::io::ErrorKind::InvalidData, e.to_string())
        })?;
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
    /// Connect to a `ws://` or `wss://` URL.
    fn connect(url: &str) -> ::std::result::Result<Self, String> {
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
            Self::connect_tls(&host, port, &path)
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

    fn connect_tls(host: &str, port: u16, path: &str) -> ::std::result::Result<Self, String> {
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
        let mut tls = bao_boringssl_bridge::connection::TlsConnection::new_client(&tls_client, host)
            .map_err(|e| format!("tls conn: {}", e))?;

        // Complete the TLS handshake by pumping records until active.
        loop {
            let outgoing = tls.take_outgoing();
            if !outgoing.is_empty() && tcp.write_all(&outgoing).is_err() {
                return Err("tls handshake write failed".to_string());
            }
            match tls.process() {
                Ok(res) => {
                    use bao_boringssl_bridge::connection::TlsState;
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

        let mut stream = TlsStream { tcp, tls };
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
            WsConn::Plain(c) => c
                .send_text(text)
                .map_err(|e| format!("send failed: {}", e)),
            WsConn::Tls { stream, .. } => {
                let payload = text.as_bytes();
                let key = bun_uws::ws_codec::gen_mask_key();
                let mut frame = Vec::with_capacity(payload.len() + 14);
                frame.push(0x81); // FIN + text opcode
                push_masked_len(&mut frame, payload.len());
                frame.extend_from_slice(&key);
                let mut masked = payload.to_vec();
                apply_mask(&mut masked, &key);
                stream.write_all(&frame).map_err(|e| format!("send failed: {}", e))
            }
        }
    }

    fn read_message(&mut self) -> ::std::result::Result<WsMessage, String> {
        match self {
            WsConn::Plain(c) => {
                match c.recv().map_err(|e| format!("recv: {}", e))? {
                    bun_uws::ws_client::RecvOutcome::Message(opcode, payload) => match opcode {
                        bun_uws::ws_codec::Opcode::Text => {
                            Ok(WsMessage::Text(String::from_utf8_lossy(&payload).into_owned()))
                        }
                        bun_uws::ws_codec::Opcode::Binary => Ok(WsMessage::Binary(payload)),
                        _ => Ok(WsMessage::Binary(payload)),
                    },
                    bun_uws::ws_client::RecvOutcome::Closed => Ok(WsMessage::Close),
                    bun_uws::ws_client::RecvOutcome::Timeout => Err("wouldblock".to_string()),
                }
            }
            WsConn::Tls { stream, decoder, closed } => {
                if *closed {
                    return Ok(WsMessage::Close);
                }
                let header = loop {
                    match decoder.decode_frame(stream) {
                        Ok(Some(h)) => break h,
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
                    }
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
                    bun_uws::ws_codec::Opcode::Text => {
                        Ok(WsMessage::Text(String::from_utf8_lossy(&payload).into_owned()))
                    }
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
                        stream.write_all(&frame).map_err(|e| format!("pong: {}", e))?;
                        self.read_message()
                    }
                    bun_uws::ws_codec::Opcode::Pong | bun_uws::ws_codec::Opcode::Continuation => {
                        self.read_message()
                    }
                }
            }
        }
    }

    fn close(&mut self) -> ::std::result::Result<(), String> {
        match self {
            WsConn::Plain(c) => c.close().map_err(|e| format!("close failed: {}", e)),
            WsConn::Tls { stream, closed, .. } => {
                if *closed {
                    return Ok(());
                }
                *closed = true;
                let key = bun_uws::ws_codec::gen_mask_key();
                let mut frame = vec![0x88]; // FIN + close
                let payload = 1000u16.to_be_bytes();
                push_masked_len(&mut frame, payload.len());
                frame.extend_from_slice(&key);
                let mut masked = payload.to_vec();
                apply_mask(&mut masked, &key);
                frame.extend_from_slice(&masked);
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

#[allow(dead_code)]
struct WsEntry {
    client: WsConn,
    js_obj_key: String,
}

thread_local! {
    static WS_CONNECTIONS: RefCell<Vec<WsEntry>> = const { RefCell::new(Vec::new()) };
}

pub fn install_websocket_constructor(
    cx: &mut mozjs::context::JSContext,
    global: mozjs::rust::Handle<*mut JSObject>,
) {
    unsafe {
        let ws_fun = JS_NewFunction(cx.raw_cx(), Some(websocket_constructor), 1, JSFUN_CONSTRUCTOR, c"WebSocket".as_ptr());
        if !ws_fun.is_null() {
            let ctor_obj = JS_GetFunctionObject(ws_fun);
            if !ctor_obj.is_null() {
                let val = mozjs::jsval::ObjectValue(ctor_obj);
                rooted!(&in(cx) let v = val);
                JS_DefineProperty(cx.raw_cx(), global.into(), c"WebSocket".as_ptr(), v.handle().into(), (JSPROP_ENUMERATE | JSPROP_PERMANENT) as u32);

                let ctor_h = Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &ctor_obj };
                for (name, value) in &[("CONNECTING", 0i32), ("OPEN", 1), ("CLOSING", 2), ("CLOSED", 3)] {
                    let c_name = ZBox::from_bytes(name.as_bytes());
                    let v = Int32Value(*value);
                    let v_h = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &v };
                    JS_DefineProperty(cx.raw_cx(), ctor_h, c_name.as_ptr(), v_h, (JSPROP_ENUMERATE | JSPROP_READONLY) as u32);
                }
            }
        }
    }
}

unsafe fn ws_trigger_event(cx: *mut JSContext, ws_obj_key: &str, event_name: &str, data_val: Option<JSVal>) {
    let ws_obj = match gc_store_get(cx, ws_obj_key) {
        Some(obj) => obj,
        None => return,
    };
    let obj_h = Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &ws_obj };
    let mut handler_val = UndefinedValue();
    let c_name = ZBox::from_bytes(event_name.as_bytes());
    JS_GetProperty(cx, obj_h, c_name.as_ptr(), MutableHandle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &mut handler_val });
    if handler_val.is_object() {
        let handler_obj = handler_val.to_object();
        if JS_ObjectIsFunction(handler_obj) {
            let global = CurrentGlobalOrNull(cx);
            if !global.is_null() {
                let global_h = Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &global };
                let handler_jsval = ObjectValue(handler_obj);
                let handler_h = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &handler_jsval };

                let event_obj = mozjs_sys::jsapi::JS_NewPlainObject(cx);
                if !event_obj.is_null() {
                    let ev_h = Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &event_obj };
                    if let Some(dv) = data_val {
                        let dv_h = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &dv };
                        JS_DefineProperty(cx, ev_h, c"data".as_ptr(), dv_h, JSPROP_ENUMERATE as u32);
                    }
                    let ev_val = ObjectValue(event_obj);
                    let call_args = HandleValueArray { length_: 1, elements_: &ev_val };
                    let mut rval = UndefinedValue();
                    let _ = JS_CallFunctionValue(cx, global_h, handler_h, &call_args, MutableHandle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &mut rval });
                }
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

    let this_obj = args.thisv().to_object();
    let this_h = Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &this_obj };
    let mut idx_val = Int32Value(-1);
    JS_GetProperty(cx, this_h, c"_wsIdx".as_ptr(), MutableHandle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &mut idx_val });
    let idx = idx_val.to_int32() as usize;

    let send_result = WS_CONNECTIONS.with(|c| {
        let mut conns = c.borrow_mut();
        if idx < conns.len() {
            let s = jsstr_to_string(cx, NonNull::new_unchecked(msg_val.to_string()));
            conns[idx].client.send_text(&s)
        } else {
            Err("invalid WebSocket index".to_string())
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
    let this_obj = args.thisv().to_object();
    let this_h = Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &this_obj };

    let mut idx_val = Int32Value(-1);
    JS_GetProperty(cx, this_h, c"_wsIdx".as_ptr(), MutableHandle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &mut idx_val });
    let idx = idx_val.to_int32() as usize;

    WS_CONNECTIONS.with(|c| {
        let mut conns = c.borrow_mut();
        if idx < conns.len() {
            let _ = conns[idx].client.close();
        }
    });

    let closing_val = Int32Value(2);
    let closing_h = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &closing_val };
    JS_SetProperty(cx, this_h, c"readyState".as_ptr(), closing_h);
    args.rval().set(UndefinedValue());
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn websocket_constructor(
    cx: *mut JSContext,
    argc: u32,
    vp: *mut JSVal,
) -> bool {
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
    let url = jsstr_to_string(cx, NonNull::new_unchecked(url_val.to_string()));

    let wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    rooted!(&in(wrapped_cx) let ws_obj = mozjs_sys::jsapi::JS_NewPlainObject(cx));
    if ws_obj.get().is_null() {
        args.rval().set(UndefinedValue());
        return true;
    }

    let obj_h = Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &ws_obj.get() };

    {
        let c_url = ZBox::from_bytes(url.as_bytes());
        let js_str = JS_NewStringCopyZ(cx, c_url.as_ptr());
        if !js_str.is_null() {
            let v = StringValue(&*js_str);
            let v_h = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &v };
            JS_DefineProperty(cx, obj_h, c"url".as_ptr(), v_h, JSPROP_ENUMERATE as u32);
        }
    }

    let state_val = Int32Value(0);
    let state_h = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &state_val };
    JS_DefineProperty(cx, obj_h, c"readyState".as_ptr(), state_h, JSPROP_ENUMERATE as u32);

    let ba_val = Int32Value(0);
    let ba_h = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &ba_val };
    JS_DefineProperty(cx, obj_h, c"bufferedAmount".as_ptr(), ba_h, JSPROP_ENUMERATE as u32);

    for name in &["onopen", "onmessage", "onerror", "onclose"] {
        let c_name = ZBox::from_bytes(name.as_bytes());
        let ud = UndefinedValue();
        let ud_h = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &ud };
        JS_DefineProperty(cx, obj_h, c_name.as_ptr(), ud_h, JSPROP_ENUMERATE as u32);
    }

    mozjs_sys::jsapi::JS_DefineFunction(
        cx, obj_h, c"send".as_ptr(), Some(ws_send), 1, JSPROP_ENUMERATE as u32,
    );
    mozjs_sys::jsapi::JS_DefineFunction(
        cx, obj_h, c"close".as_ptr(), Some(ws_close_fn), 0, JSPROP_ENUMERATE as u32,
    );

    match WsConn::connect(&url) {
        Ok(mut client) => {
            let open_val = Int32Value(1);
            let open_h = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &open_val };
            JS_SetProperty(cx, obj_h, c"readyState".as_ptr(), open_h);

            // Store the JS WebSocket object in GcStore for GC safety
            let ws_id = WS_ID_COUNTER.fetch_add(1, Ordering::Relaxed);
            let ws_key = format!("ws_{}", ws_id);
            gc_store_insert(cx, &ws_key, ws_obj.get());

            // Set non-blocking to drain available messages
            client.set_nonblocking(true);
            loop {
                match client.read_message() {
                    Ok(WsMessage::Text(text)) => {
                        {
                            let c_text = ZBox::from_bytes(text.as_bytes());
                            let js_str = JS_NewStringCopyZ(cx, c_text.as_ptr());
                            if !js_str.is_null() {
                                let dv = StringValue(&*js_str);
                                ws_trigger_event(cx, &ws_key, "onmessage", Some(dv));
                            }
                        }
                    }
                    Ok(WsMessage::Binary(_)) => {}
                    Ok(WsMessage::Close) => {
                        let closed_val = Int32Value(3);
                        let closed_h = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &closed_val };
                        JS_SetProperty(cx, obj_h, c"readyState".as_ptr(), closed_h);
                        ws_trigger_event(cx, &ws_key, "onclose", None);
                        gc_store_remove(cx, &ws_key);
                        break;
                    }
                    Err(_) => break, // WouldBlock or other error
                }
            }
            client.set_nonblocking(false);

            let ws_idx = WS_CONNECTIONS.with(|c| {
                let mut conns = c.borrow_mut();
                conns.push(WsEntry { client, js_obj_key: ws_key.clone() });
                conns.len() - 1
            });
            let idx_val = Int32Value(ws_idx as i32);
            let idx_h = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &idx_val };
            JS_DefineProperty(cx, obj_h, c"_wsIdx".as_ptr(), idx_h, 0);

            ws_trigger_event(cx, &ws_key, "onopen", None);
        }
        Err(e) => {
            let msg = format!("WebSocket connection failed: {}", e);
            let c_msg = ZBox::from_bytes(msg.as_bytes());
            JS_ReportErrorUTF8(cx, c"%s".as_ptr(), c_msg.as_ptr());
            return false;
        }
    }

    args.rval().set(mozjs::jsval::ObjectValue(ws_obj.get()));
    true
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
        JS_DefineFunction(cx, perf_obj.handle(), c"now".as_ptr(), Some(performance_now), 0, JSPROP_ENUMERATE as u32);
        JS_DefineProperty3(cx, global, c"performance".as_ptr(), perf_obj.handle(), JSPROP_ENUMERATE as u32);
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
        let te_fun = JS_NewFunction(cx.raw_cx(), Some(text_encoder_constructor), 0, JSFUN_CONSTRUCTOR, c"TextEncoder".as_ptr());
        if !te_fun.is_null() {
            let te_obj = JS_GetFunctionObject(te_fun);
            if !te_obj.is_null() {
                rooted!(&in(cx) let te_obj_r = te_obj);
                rooted!(&in(cx) let proto = JS_NewPlainObject(cx));
                if !proto.get().is_null() {
                    JS_DefineFunction(cx, proto.handle(), c"encode".as_ptr(), Some(text_encoder_encode), 1, JSPROP_ENUMERATE as u32);
                    JS_DefineFunction(cx, proto.handle(), c"encodeInto".as_ptr(), Some(text_encoder_encode_into), 2, JSPROP_ENUMERATE as u32);
                    JS_DefineProperty3(cx, te_obj_r.handle(), c"prototype".as_ptr(), proto.handle(), JSPROP_PERMANENT as u32);
                }
                JS_DefineProperty3(cx, global, c"TextEncoder".as_ptr(), te_obj_r.handle(), (JSPROP_ENUMERATE | JSPROP_PERMANENT) as u32);
            }
        }

        let td_fun = JS_NewFunction(cx.raw_cx(), Some(text_decoder_constructor), 1, JSFUN_CONSTRUCTOR, c"TextDecoder".as_ptr());
        if !td_fun.is_null() {
            let td_obj = JS_GetFunctionObject(td_fun);
            if !td_obj.is_null() {
                rooted!(&in(cx) let td_obj_r = td_obj);
                rooted!(&in(cx) let proto = JS_NewPlainObject(cx));
                if !proto.get().is_null() {
                    JS_DefineFunction(cx, proto.handle(), c"decode".as_ptr(), Some(text_decoder_decode), 1, JSPROP_ENUMERATE as u32);
                    JS_DefineProperty3(cx, td_obj_r.handle(), c"prototype".as_ptr(), proto.handle(), JSPROP_PERMANENT as u32);
                }
                JS_DefineProperty3(cx, global, c"TextDecoder".as_ptr(), td_obj_r.handle(), (JSPROP_ENUMERATE | JSPROP_PERMANENT) as u32);
            }
        }
    }
}

pub fn install_atob_btoa(
    cx: &mut mozjs::context::JSContext,
    global: mozjs::rust::Handle<*mut JSObject>,
) {
    unsafe {
        JS_DefineFunction(cx, global, c"atob".as_ptr(), Some(atob_fn), 1, JSPROP_ENUMERATE as u32);
        JS_DefineFunction(cx, global, c"btoa".as_ptr(), Some(btoa_fn), 1, JSPROP_ENUMERATE as u32);
    }
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn atob_fn(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    if argc == 0 || !(*args.get(0).ptr).is_string() {
        args.rval().set(UndefinedValue());
        return true;
    }
    let s = jsstr_to_string(cx, ::std::ptr::NonNull::new_unchecked((*args.get(0).ptr).to_string()));
    match bun_base64::decode_alloc(s.as_bytes()) {
        Ok(bytes) => {
            let decoded = String::from_utf8_lossy(&bytes);
            let c_str = ZBox::from_vec(decoded.into_owned().into_bytes());
            let js_str = JS_NewStringCopyZ(cx, c_str.as_ptr());
            if js_str.is_null() { args.rval().set(UndefinedValue()); }
            else { args.rval().set(StringValue(&*js_str)); }
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
    let s = jsstr_to_string(cx, ::std::ptr::NonNull::new_unchecked((*args.get(0).ptr).to_string()));
    let encoded_bytes = bun_base64::encode_alloc(s.as_bytes());
    let encoded = ::std::str::from_utf8(&encoded_bytes).unwrap_or("");
    let c_str = ZBox::from_bytes(encoded.as_bytes());
    let js_str = JS_NewStringCopyZ(cx, c_str.as_ptr());
    if js_str.is_null() { args.rval().set(UndefinedValue()); }
    else { args.rval().set(StringValue(&*js_str)); }
    true
}

pub fn install_queue_microtask(
    cx: &mut mozjs::context::JSContext,
    global: mozjs::rust::Handle<*mut JSObject>,
) {
    unsafe {
        JS_DefineFunction(cx, global, c"queueMicrotask".as_ptr(), Some(queue_microtask_fn), 1, JSPROP_ENUMERATE as u32);
    }
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn text_encoder_constructor(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let obj = mozjs_sys::jsapi::JS_NewPlainObject(cx);
    if obj.is_null() {
        args.rval().set(UndefinedValue());
        return true;
    }
    let encoding_str = JS_NewStringCopyZ(cx, c"utf-8".as_ptr());
    if !encoding_str.is_null() {
        let val = StringValue(&*encoding_str);
        let obj_h = Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &obj };
        let val_h = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &val };
        JS_DefineProperty(cx, obj_h, c"encoding".as_ptr(), val_h, (JSPROP_ENUMERATE | JSPROP_READONLY) as u32);
    }

    let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    rooted!(&in(wrapped_cx) let obj_r = obj);
    JS_DefineFunction(&mut wrapped_cx, obj_r.handle(), c"encode".as_ptr(), Some(text_encoder_encode), 1, JSPROP_ENUMERATE as u32);
    JS_DefineFunction(&mut wrapped_cx, obj_r.handle(), c"encodeInto".as_ptr(), Some(text_encoder_encode_into), 2, JSPROP_ENUMERATE as u32);

    args.rval().set(ObjectValue(obj));
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn text_encoder_encode(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let input = if argc > 0 {
        let v = *args.get(0).ptr;
        if v.is_string() { crate::js_to_rust_string(cx, v) } else { String::new() }
    } else {
        String::new()
    };

    let bytes = input.as_bytes();
    let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));

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
        let data_ptr = mozjs_sys::jsapi::JS_GetUint8ArrayData(
            arr.get(),
            &mut is_shared,
            ::std::ptr::null(),
        );
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
unsafe extern "C" fn text_encoder_encode_into(_cx: *mut JSContext, _argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, _argc);
    args.rval().set(UndefinedValue());
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn text_decoder_constructor(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let obj = mozjs_sys::jsapi::JS_NewPlainObject(cx);
    if obj.is_null() {
        args.rval().set(UndefinedValue());
        return true;
    }
    let encoding = if argc > 0 {
        let v = *args.get(0).ptr;
        if v.is_string() { crate::js_to_rust_string(cx, v) } else { "utf-8".to_string() }
    } else {
        "utf-8".to_string()
    };
    let encoding_lower = encoding.to_lowercase();
    let encoding_str = JS_NewStringCopyZ(cx, ZBox::from_bytes(encoding_lower.as_bytes()).as_ptr());
    if !encoding_str.is_null() {
        let val = StringValue(&*encoding_str);
        let obj_h = Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &obj };
        let val_h = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &val };
        JS_DefineProperty(cx, obj_h, c"encoding".as_ptr(), val_h, (JSPROP_ENUMERATE | JSPROP_READONLY) as u32);
    }
    let fatal_val = BooleanValue(false);
    let obj_h = Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &obj };
    let fatal_h = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &fatal_val };
    JS_DefineProperty(cx, obj_h, c"fatal".as_ptr(), fatal_h, (JSPROP_ENUMERATE | JSPROP_READONLY) as u32);
    let bom_val = BooleanValue(false);
    let bom_h = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &bom_val };
    JS_DefineProperty(cx, obj_h, c"ignoreBOM".as_ptr(), bom_h, (JSPROP_ENUMERATE | JSPROP_READONLY) as u32);

    let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    rooted!(&in(wrapped_cx) let obj_r = obj);
    JS_DefineFunction(&mut wrapped_cx, obj_r.handle(), c"decode".as_ptr(), Some(text_decoder_decode), 1, JSPROP_ENUMERATE as u32);

    args.rval().set(ObjectValue(obj));
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn text_decoder_decode(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    if argc == 0 {
        let empty = JS_NewStringCopyZ(cx, c"".as_ptr());
        args.rval().set(if empty.is_null() { UndefinedValue() } else { StringValue(&*empty) });
        return true;
    }

    let input = *args.get(0).ptr;

    let bytes = if input.is_object() {
        let obj = input.to_object();
        let mut len_val = UndefinedValue();
        let obj_h = Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &obj };
        JS_GetProperty(cx, obj_h, c"length".as_ptr(), MutableHandle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &mut len_val });
        let len = if len_val.is_int32() { len_val.to_int32() as u32 } else { 0 };
        let mut result = Vec::with_capacity(len as usize);
        for i in 0..len {
            let mut elem = UndefinedValue();
            JS_GetElement(cx, obj_h, i, MutableHandle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &mut elem });
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
    args.rval().set(if js_str.is_null() { UndefinedValue() } else { StringValue(&*js_str) });
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn queue_microtask_fn(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    if argc == 0 || !(*args.get(0).ptr).is_object() {
        return true;
    }
    let callback = (*args.get(0).ptr).to_object();
    let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    let cx = &mut wrapped_cx;

    rooted!(&in(cx) let undef_val = UndefinedValue());
    let resolved = CallOriginalPromiseResolve(cx, undef_val.handle());
    if resolved.is_null() {
        args.rval().set(UndefinedValue());
        return true;
    }
    rooted!(&in(cx) let promise = resolved);
    rooted!(&in(cx) let on_fulfilled = callback);
    rooted!(&in(cx) let null_reject = ::std::ptr::null_mut::<JSObject>());
    CallOriginalPromiseThen(cx, promise.handle(), on_fulfilled.handle(), null_reject.handle());
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
}
