// @trace REQ-ENG-006
// WebSocket + Performance + TextEncoder/TextDecoder + atob/btoa + queueMicrotask
use ::std::cell::RefCell;
use ::std::ffi::CString;
use ::std::io::{Read, Write};
use ::std::net::TcpStream;
use ::std::ptr::NonNull;
use ::std::time::Duration;

use mozjs::jsapi::*;
use mozjs::jsval::{JSVal, UndefinedValue, StringValue, Int32Value, ObjectValue, BooleanValue};
use mozjs::rooted;
use mozjs::rust::wrappers2::{JS_DefineFunction, JS_DefineProperty3, JS_NewPlainObject, NewArrayObject1, CallOriginalPromiseResolve, CallOriginalPromiseThen};
use mozjs::conversions::jsstr_to_string;

use base64::Engine;

// ── Minimal WebSocket client (RFC 6455) ──

#[derive(Debug)]
#[allow(dead_code)]
enum WsMessage {
    Text(String),
    Binary(Vec<u8>),
    Close,
}

struct WsClient {
    stream: TcpStream,
}

impl WsClient {
    fn connect(url_str: &str) -> ::std::result::Result<Self, String> {
        let (host, port, path) = parse_ws_url(url_str)?;
        let addr = format!("{}:{}", host, port);
        let mut stream = TcpStream::connect_timeout(
            &addr.parse().map_err(|e| format!("invalid address: {}", e))?,
            Duration::from_secs(10),
        ).map_err(|e| format!("connect failed: {}", e))?;
        stream.set_nonblocking(false).ok();

        let key_base: [u8; 16] = rand::random();
        let key = base64::engine::general_purpose::STANDARD.encode(key_base);

        let request = format!(
            "GET {} HTTP/1.1\r\nHost: {}\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Key: {}\r\nSec-WebSocket-Version: 13\r\n\r\n",
            path, host, key
        );
        stream.write_all(request.as_bytes()).map_err(|e| format!("write failed: {}", e))?;

        let mut response = vec![0u8; 4096];
        let n = stream.read(&mut response).map_err(|e| format!("read failed: {}", e))?;
        let response_str = String::from_utf8_lossy(&response[..n]);
        if !response_str.starts_with("HTTP/1.1 101") && !response_str.starts_with("HTTP/1.0 101") {
            return Err(format!("upgrade failed: {}", response_str.lines().next().unwrap_or("")));
        }

        Ok(Self { stream })
    }

    fn send_text(&mut self, text: &str) -> ::std::result::Result<(), String> {
        let payload = text.as_bytes();
        let mut frame = Vec::with_capacity(payload.len() + 10);
        frame.push(0x81); // FIN + text opcode
        write_masked_payload(&mut frame, payload);
        self.stream.write_all(&frame).map_err(|e| format!("send failed: {}", e))
    }

    #[allow(dead_code)]
    fn send_binary(&mut self, data: &[u8]) -> ::std::result::Result<(), String> {
        let mut frame = Vec::with_capacity(data.len() + 10);
        frame.push(0x82); // FIN + binary opcode
        write_masked_payload(&mut frame, data);
        self.stream.write_all(&frame).map_err(|e| format!("send failed: {}", e))
    }

    fn read_message(&mut self) -> ::std::result::Result<WsMessage, String> {
        let mut header = [0u8; 2];
        self.stream.read_exact(&mut header).map_err(|e| format!("read header: {}", e))?;

        let opcode = header[0] & 0x0F;
        let masked = (header[1] & 0x80) != 0;
        let mut payload_len = (header[1] & 0x7F) as u64;

        if payload_len == 126 {
            let mut ext = [0u8; 2];
            self.stream.read_exact(&mut ext).map_err(|e| format!("read len: {}", e))?;
            payload_len = u16::from_be_bytes(ext) as u64;
        } else if payload_len == 127 {
            let mut ext = [0u8; 8];
            self.stream.read_exact(&mut ext).map_err(|e| format!("read len: {}", e))?;
            payload_len = u64::from_be_bytes(ext);
        }

        let mask_key = if masked {
            let mut key = [0u8; 4];
            self.stream.read_exact(&mut key).map_err(|e| format!("read mask: {}", e))?;
            Some(key)
        } else {
            None
        };

        let mut payload = vec![0u8; payload_len as usize];
        if payload_len > 0 {
            self.stream.read_exact(&mut payload).map_err(|e| format!("read payload: {}", e))?;
        }

        if let Some(key) = mask_key {
            for (i, byte) in payload.iter_mut().enumerate() {
                *byte ^= key[i % 4];
            }
        }

        match opcode {
            0x1 => Ok(WsMessage::Text(String::from_utf8_lossy(&payload).into_owned())),
            0x2 => Ok(WsMessage::Binary(payload)),
            0x8 => Ok(WsMessage::Close),
            0x9 => {
                self.send_pong(&payload)?;
                self.read_message()
            }
            _ => Err(format!("unsupported opcode: {}", opcode)),
        }
    }

    fn send_pong(&mut self, data: &[u8]) -> ::std::result::Result<(), String> {
        let mut frame = vec![0x8A]; // FIN + pong opcode
        write_masked_payload(&mut frame, data);
        self.stream.write_all(&frame).map_err(|e| format!("pong failed: {}", e))
    }

    fn close(&mut self) -> ::std::result::Result<(), String> {
        let frame = [0x88, 0x80, 0x00, 0x00, 0x00, 0x00]; // FIN + close + empty masked
        self.stream.write_all(&frame).map_err(|e| format!("close failed: {}", e))
    }
}

fn parse_ws_url(url: &str) -> ::std::result::Result<(String, u16, String), String> {
    let rest = if let Some(r) = url.strip_prefix("ws://") {
        r
    } else if url.starts_with("wss://") {
        return Err("wss:// not yet supported; use ws:// for plain WebSocket".to_string());
    } else {
        url
    };

    let (host_port, path) = match rest.find('/') {
        Some(i) => (&rest[..i], rest[i..].to_string()),
        None => (rest, "/".to_string()),
    };

    let (host, port) = match host_port.rfind(':') {
        Some(i) => (host_port[..i].to_string(), host_port[i + 1..].parse::<u16>().unwrap_or(80)),
        None => (host_port.to_string(), 80),
    };

    Ok((host, port, path))
}

fn write_masked_payload(frame: &mut Vec<u8>, payload: &[u8]) {
    let mask_key: [u8; 4] = rand::random();
    let len = payload.len();
    if len < 126 {
        frame.push(0x80 | len as u8);
    } else if len < 65536 {
        frame.push(0x80 | 126);
        frame.extend_from_slice(&(len as u16).to_be_bytes());
    } else {
        frame.push(0x80 | 127);
        frame.extend_from_slice(&(len as u64).to_be_bytes());
    }
    frame.extend_from_slice(&mask_key);
    for (i, byte) in payload.iter().enumerate() {
        frame.push(byte ^ mask_key[i % 4]);
    }
}

// ── JS bridge ──

#[allow(dead_code)]
struct WsEntry {
    client: WsClient,
    js_obj: *mut JSObject,
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
                    let c_name = ::std::ffi::CString::new(*name).unwrap_or_default();
                    let v = Int32Value(*value);
                    let v_h = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &v };
                    JS_DefineProperty(cx.raw_cx(), ctor_h, c_name.as_ptr(), v_h, (JSPROP_ENUMERATE | JSPROP_READONLY) as u32);
                }
            }
        }
    }
}

unsafe fn ws_trigger_event(cx: *mut JSContext, ws_obj: *mut JSObject, event_name: &str, data_val: Option<JSVal>) {
    let obj_h = Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &ws_obj };
    let mut handler_val = UndefinedValue();
    let c_name = ::std::ffi::CString::new(event_name).unwrap_or_default();
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
        JS_ReportErrorUTF8(cx, b"WebSocket.send() requires a message argument\0".as_ptr() as *const ::std::os::raw::c_char);
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
        let c_msg = CString::new(msg).unwrap_or_default();
        JS_ReportErrorUTF8(cx, b"%s\0".as_ptr() as *const ::std::os::raw::c_char, c_msg.as_ptr());
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
        JS_ReportErrorUTF8(cx, b"WebSocket requires a URL argument\0".as_ptr() as *const ::std::os::raw::c_char);
        return false;
    }
    let url_val = *args.get(0).ptr;
    if !url_val.is_string() {
        JS_ReportErrorUTF8(cx, b"WebSocket URL must be a string\0".as_ptr() as *const ::std::os::raw::c_char);
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

    if let Ok(c_url) = ::std::ffi::CString::new(url.as_str()) {
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
        let c_name = ::std::ffi::CString::new(*name).unwrap_or_default();
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

    match WsClient::connect(&url) {
        Ok(mut client) => {
            let open_val = Int32Value(1);
            let open_h = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &open_val };
            JS_SetProperty(cx, obj_h, c"readyState".as_ptr(), open_h);

            // Set non-blocking to drain available messages
            let _ = client.stream.set_nonblocking(true);
            loop {
                match client.read_message() {
                    Ok(WsMessage::Text(text)) => {
                        if let Ok(c_text) = ::std::ffi::CString::new(text.as_str()) {
                            let js_str = JS_NewStringCopyZ(cx, c_text.as_ptr());
                            if !js_str.is_null() {
                                let dv = StringValue(&*js_str);
                                ws_trigger_event(cx, ws_obj.get(), "onmessage", Some(dv));
                            }
                        }
                    }
                    Ok(WsMessage::Binary(_)) => {}
                    Ok(WsMessage::Close) => {
                        let closed_val = Int32Value(3);
                        let closed_h = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &closed_val };
                        JS_SetProperty(cx, obj_h, c"readyState".as_ptr(), closed_h);
                        ws_trigger_event(cx, ws_obj.get(), "onclose", None);
                        break;
                    }
                    Err(_) => break, // WouldBlock or other error
                }
            }
            let _ = client.stream.set_nonblocking(false);

            let ws_idx = WS_CONNECTIONS.with(|c| {
                let mut conns = c.borrow_mut();
                conns.push(WsEntry { client, js_obj: ws_obj.get() });
                conns.len() - 1
            });
            let idx_val = Int32Value(ws_idx as i32);
            let idx_h = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &idx_val };
            JS_DefineProperty(cx, obj_h, c"_wsIdx".as_ptr(), idx_h, 0);

            ws_trigger_event(cx, ws_obj.get(), "onopen", None);
        }
        Err(e) => {
            let msg = format!("WebSocket connection failed: {}", e);
            let c_msg = ::std::ffi::CString::new(msg).unwrap_or_default();
            JS_ReportErrorUTF8(cx, b"%s\0".as_ptr() as *const ::std::os::raw::c_char, c_msg.as_ptr());
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
    match base64::engine::general_purpose::STANDARD.decode(s.as_bytes()) {
        Ok(bytes) => {
            let decoded = String::from_utf8_lossy(&bytes);
            let c_str = ::std::ffi::CString::new(decoded.into_owned()).unwrap_or_default();
            let js_str = JS_NewStringCopyZ(cx, c_str.as_ptr());
            if js_str.is_null() { args.rval().set(UndefinedValue()); }
            else { args.rval().set(StringValue(&*js_str)); }
        }
        Err(_) => {
            JS_ReportErrorUTF8(cx, b"Failed to decode base64\0".as_ptr() as *const ::std::os::raw::c_char);
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
    let encoded = base64::engine::general_purpose::STANDARD.encode(s.as_bytes());
    let c_str = ::std::ffi::CString::new(encoded).unwrap_or_default();
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
    let encoding_str = JS_NewStringCopyZ(cx, b"utf-8\0".as_ptr() as *const ::std::os::raw::c_char);
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
    rooted!(&in(wrapped_cx) let arr = NewArrayObject1(&mut wrapped_cx, bytes.len()));

    for (i, &byte) in bytes.iter().enumerate() {
        let val = Int32Value(byte as i32);
        rooted!(&in(wrapped_cx) let v = val);
        JS_DefineElement(cx, arr.handle().into(), i as u32, v.handle().into(), JSPROP_ENUMERATE as u32);
    }

    let global = CurrentGlobalOrNull(cx);
    if !global.is_null() {
        let global_h = Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &global };
        let mut buf_ctor = UndefinedValue();
        JS_GetProperty(cx, global_h, c"Uint8Array".as_ptr(), MutableHandle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &mut buf_ctor });
        if buf_ctor.is_object() {
            let arr_val = ObjectValue(arr.get());
            rooted!(&in(wrapped_cx) let av = arr_val);
            let call_args = HandleValueArray { length_: 1, elements_: &av.get() };
            let ctor_h = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &buf_ctor };
            let mut rval = UndefinedValue();
            JS_CallFunctionValue(cx, global_h, ctor_h, &call_args, MutableHandle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &mut rval });
            if rval.is_object() {
                args.rval().set(rval);
                return true;
            }
        }
    }

    args.rval().set(ObjectValue(arr.get()));
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
    let encoding_str = JS_NewStringCopyZ(cx, ::std::ffi::CString::new(encoding_lower).unwrap_or_default().as_ptr());
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
        let empty = JS_NewStringCopyZ(cx, b"\0".as_ptr() as *const ::std::os::raw::c_char);
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
        let mut result = Vec::new();
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
            JS_ReportErrorUTF8(cx, b"The encoded data was not valid\0".as_ptr() as *const ::std::os::raw::c_char);
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
