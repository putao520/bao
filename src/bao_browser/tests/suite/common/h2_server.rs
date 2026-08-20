// H2 fixture: ALPN-h2 TLS server + minimal h2 framing.
//
// Extracted from page_net_bun_full_matrix_e2e_tests (U2 stage 3) so both the
// page-network matrix and the Node-fetch h2 coalescing smoke share ONE
// fixture: BoringSSL TlsServer + hand-rolled h2 framing layer (response
// HPACK encode: static-table :status plus literal headers, no huffman).
// Records every request stream id and counts ALPN-h2 vs non-h2 connections.
//
// Decode side (h2 e2e evidence): request HEADERS/CONTINUATION blocks are
// HPACK-decoded through lshpack (the same bun_http decoder the h2 client
// uses for responses — one library, both directions), producing
// [`H2RequestRecord`]s with method/path/authority/scheme + regular headers
// + DATA bodies. The client's first non-ACK SETTINGS payload is recorded
// raw per connection (`client_settings`) so tests can assert the stealth
// profile's h2 SETTINGS bytes on the wire.

use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use bun_http::lshpack::HpackHandle;

/// Length-prefixed ALPN wire entry for "h2".
const ALPN_H2: &[u8] = b"\x02h2";

unsafe extern "C" fn alpn_select_h2(
    _ssl: *mut bun_boringssl_sys::SSL,
    out: *mut *const u8,
    out_len: *mut u8,
    in_: *const u8,
    in_len: core::ffi::c_uint,
    _arg: *mut core::ffi::c_void,
) -> core::ffi::c_int {
    // Walk the client's length-prefixed protocol list; select "h2".
    let list = unsafe { std::slice::from_raw_parts(in_, in_len as usize) };
    let mut offset = 0usize;
    while offset < list.len() {
        let len = list[offset] as usize;
        offset += 1;
        if offset + len > list.len() {
            break;
        }
        if &list[offset..offset + len] == b"h2" {
            unsafe {
                *out = ALPN_H2.as_ptr().add(1); // point at "h2", past the length byte
                *out_len = 2;
            }
            return bun_boringssl_sys::SSL_TLSEXT_ERR_OK;
        }
        offset += len;
    }
    bun_boringssl_sys::SSL_TLSEXT_ERR_NOACK
}

/// TLS stream adapter driving the server-side BoringSSL state machine over
/// the raw TCP socket (mirror of tls_info_and_streaming_tests' ServerTlsIo).
struct ServerTlsIo {
    tcp: TcpStream,
    tls: bao_boringssl_bridge::TlsConnection,
    pending_plain: Vec<u8>,
    pending_off: usize,
}

impl ServerTlsIo {
    fn handshake(tcp: &mut TcpStream, tls: &mut bao_boringssl_bridge::TlsConnection) -> std::io::Result<()> {
        loop {
            let res = tls.process().map_err(|e| {
                std::io::Error::new(std::io::ErrorKind::InvalidData, e.to_string())
            })?;
            loop {
                let outgoing = tls.take_outgoing();
                if outgoing.is_empty() {
                    break;
                }
                tcp.write_all(&outgoing)?;
            }
            if res.state == bao_boringssl_bridge::TlsState::Active ||
                res.state == bao_boringssl_bridge::TlsState::PeerClosed
            {
                return Ok(());
            }
            let mut buf = [0u8; 16_384];
            match tcp.read(&mut buf) {
                Ok(0) => {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::UnexpectedEof,
                        "peer closed during tls handshake",
                    ))
                },
                Ok(n) => tls.feed(&buf[..n]),
                Err(e) => return Err(e),
            }
        }
    }

    /// Blocking read of plaintext with a deadline.
    fn read_plain(&mut self, deadline: Instant) -> std::io::Result<Option<Vec<u8>>> {
        loop {
            let outgoing = self.tls.take_outgoing();
            if !outgoing.is_empty() {
                self.tcp.write_all(&outgoing)?;
            }
            let res = self.tls.process().map_err(|e| {
                std::io::Error::new(std::io::ErrorKind::InvalidData, e.to_string())
            })?;
            if !res.plaintext.is_empty() {
                let mut joined = Vec::new();
                for chunk in res.plaintext {
                    joined.extend_from_slice(&chunk);
                }
                return Ok(Some(joined));
            }
            if Instant::now() > deadline {
                return Ok(None);
            }
            let mut buf = [0u8; 16_384];
            self.tcp.set_read_timeout(Some(Duration::from_millis(200)))?;
            match self.tcp.read(&mut buf) {
                Ok(0) => {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::UnexpectedEof,
                        "tls peer closed",
                    ))
                },
                Ok(n) => self.tls.feed(&buf[..n]),
                Err(ref e)
                    if e.kind() == std::io::ErrorKind::WouldBlock ||
                        e.kind() == std::io::ErrorKind::TimedOut => {},
                Err(e) => return Err(e),
            }
        }
    }

    fn write_plain(&mut self, bytes: &[u8]) -> std::io::Result<()> {
        self.tls
            .write(bytes)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e.to_string()))?;
        loop {
            let outgoing = self.tls.take_outgoing();
            if outgoing.is_empty() {
                break;
            }
            self.tcp.write_all(&outgoing)?;
        }
        Ok(())
    }
}

impl Read for ServerTlsIo {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        if self.pending_off >= self.pending_plain.len() {
            self.pending_plain = self
                .read_plain(Instant::now() + Duration::from_millis(500))?
                .ok_or_else(|| {
                    std::io::Error::new(std::io::ErrorKind::WouldBlock, "no plaintext yet")
                })?;
            self.pending_off = 0;
        }
        let avail = &self.pending_plain[self.pending_off..];
        let n = avail.len().min(buf.len());
        buf[..n].copy_from_slice(&avail[..n]);
        self.pending_off += n;
        Ok(n)
    }
}

impl Write for ServerTlsIo {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.write_plain(buf)?;
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        self.tcp.flush()
    }
}

/// Minimal HTTP/2 framing constants (RFC 9113).
pub mod h2frame {
    pub const SETTINGS: u8 = 0x4;
    pub const HEADERS: u8 = 0x1;
    pub const DATA: u8 = 0x0;
    pub const GOAWAY: u8 = 0x7;
    pub const CONTINUATION: u8 = 0x9;
    pub const FLAG_ACK: u8 = 0x1;
    pub const FLAG_END_HEADERS: u8 = 0x4;
    pub const FLAG_END_STREAM: u8 = 0x1;

    pub fn header(len: usize, frame_type: u8, flags: u8, stream: u32) -> [u8; 9] {
        let mut head = [0u8; 9];
        head[0] = (len >> 16) as u8;
        head[1] = (len >> 8) as u8;
        head[2] = len as u8;
        head[3] = frame_type;
        head[4] = flags;
        head[5..9].copy_from_slice(&stream.to_be_bytes());
        head
    }
}

/// One HPACK-decoded request (headers complete at END_HEADERS; the DATA body
/// is appended as END_STREAM frames arrive — poll for `body_done` on POST).
#[derive(Debug, Clone)]
pub struct H2RequestRecord {
    pub stream_id: u32,
    pub method: String,
    pub path: String,
    pub authority: String,
    pub scheme: String,
    /// Regular (non-pseudo) headers in wire order.
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
    /// True once a DATA frame with END_STREAM completed the body (GET with
    /// END_STREAM on HEADERS counts too — the block just carried no body).
    pub body_done: bool,
    /// Set when the HPACK block failed to decode (never silently fake-green:
    /// tests assert exact method/path and will fail on the empty fields).
    pub decode_error: bool,
}

/// HPACK block: `:status 200` (static-table index 8 → 0x88) plus
/// literal-never-indexed entries (raw strings, no huffman).
fn hpack_response_headers(extra: &[(&str, &str)]) -> Vec<u8> {
    let mut block = vec![0x88]; // :status: 200
    for (name, value) in extra {
        block.push(0x10); // literal, never indexed, new name
        block.push(name.len() as u8);
        block.extend_from_slice(name.as_bytes());
        block.push(value.len() as u8);
        block.extend_from_slice(value.as_bytes());
    }
    block
}

pub struct H2Server {
    pub port: u16,
    /// The fixture's certificate PEM (the same material `TlsServer` serves)
    /// — tests that verify-on (fetch `init.tls.ca`) feed this to the client.
    pub cert_pem: String,
    shutdown: Arc<AtomicBool>,
    /// Every request stream, in arrival order: (stream id, label).
    pub streams: Arc<Mutex<Vec<(u32, String)>>>,
    /// HPACK-decoded request records, in header-block-completion order.
    pub requests: Arc<Mutex<Vec<H2RequestRecord>>>,
    /// Per connection (arrival order): the client's FIRST non-ACK SETTINGS
    /// frame payload, raw bytes — the stealth profile's h2 SETTINGS on the
    /// wire (connection preface order: magic + SETTINGS).
    pub client_settings: Arc<Mutex<Vec<Vec<u8>>>>,
    pub alpn_h2_count: Arc<AtomicUsize>,
    pub non_h2_count: Arc<AtomicUsize>,
}

impl H2Server {
    pub fn spawn() -> Self {
        let (cert, key) =
            bao_boringssl_bridge::generate_self_signed_pem("127.0.0.1", 365).expect("self-signed cert");
        Self::spawn_with_cert(&cert, &key)
    }

    /// Cert-providing variant: the caller picks the certificate material
    /// (e.g. CN=localhost so a verifying client's hostname check can pass —
    /// the default 127.0.0.1-CN cert can never satisfy an IP-host identity
    /// check, RFC 2818 §3.1) and holds the PEM to feed `init.tls.ca`.
    pub fn spawn_with_cert(cert: &str, key: &str) -> Self {
        let cert_pem = cert.to_string();
        let server = Arc::new(bao_boringssl_bridge::TlsServer::new(cert, key).expect("TlsServer"));
        // SAFETY: TlsServer::ctx returns its live SSL_CTX; installing the
        // ALPN select callback before any accept is thread-free.
        unsafe {
            bun_boringssl_sys::SSL_CTX_set_alpn_select_cb(
                server.ctx(),
                Some(alpn_select_h2),
                core::ptr::null_mut(),
            );
        }
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind h2 fixture");
        let port = listener.local_addr().unwrap().port();
        let shutdown = Arc::new(AtomicBool::new(false));
        let streams: Arc<Mutex<Vec<(u32, String)>>> = Arc::new(Mutex::new(Vec::new()));
        let requests: Arc<Mutex<Vec<H2RequestRecord>>> = Arc::new(Mutex::new(Vec::new()));
        let client_settings: Arc<Mutex<Vec<Vec<u8>>>> = Arc::new(Mutex::new(Vec::new()));
        let alpn_h2_count = Arc::new(AtomicUsize::new(0));
        let non_h2_count = Arc::new(AtomicUsize::new(0));

        let shutdown_c = Arc::clone(&shutdown);
        let streams_c = Arc::clone(&streams);
        let requests_c = Arc::clone(&requests);
        let settings_c = Arc::clone(&client_settings);
        let alpn_c = Arc::clone(&alpn_h2_count);
        let non_h2_c = Arc::clone(&non_h2_count);
        std::thread::Builder::new()
            .name("h2-fixture".into())
            .spawn(move || {
                listener
                    .set_nonblocking(true)
                    .expect("nonblocking h2 listener");
                while !shutdown_c.load(Ordering::SeqCst) {
                    match listener.accept() {
                        Ok((mut tcp, _)) => {
                            let _ = tcp.set_nonblocking(false);
                            let Ok(mut tls) = server.accept() else {
                                continue;
                            };
                            if ServerTlsIo::handshake(&mut tcp, &mut tls).is_err() {
                                continue;
                            }
                            if tls.alpn_protocol() != Some(&b"h2"[..]) {
                                non_h2_c.fetch_add(1, Ordering::SeqCst);
                                continue;
                            }
                            alpn_c.fetch_add(1, Ordering::SeqCst);
                            // Per-connection thread: the accept loop must
                            // keep accepting while a connection is served
                            // (the page multiplexes or opens new ones).
                            let streams = Arc::clone(&streams_c);
                            let requests = Arc::clone(&requests_c);
                            let settings = Arc::clone(&settings_c);
                            std::thread::spawn(move || {
                                let mut io = ServerTlsIo {
                                    tcp,
                                    tls,
                                    pending_plain: Vec::new(),
                                    pending_off: 0,
                                };
                                serve_h2_connection(&mut io, &streams, &requests, &settings);
                            });
                        },
                        Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                            std::thread::sleep(Duration::from_millis(5));
                        },
                        Err(_) => return,
                    }
                }
            })
            .expect("spawn h2 fixture");
        H2Server {
            port,
            cert_pem,
            shutdown,
            streams,
            requests,
            client_settings,
            alpn_h2_count,
            non_h2_count,
        }
    }

    pub fn url(&self, path: &str) -> String {
        format!("https://127.0.0.1:{}{}", self.port, path)
    }

    pub fn shutdown(&self) {
        self.shutdown.store(true, Ordering::SeqCst);
    }
}

/// Serve h2 requests on one connection: read the client preface, record the
/// client's first SETTINGS payload, ACK it, then answer every completed
/// header block (HEADERS + CONTINUATIONs through END_HEADERS) with a minimal
/// 200 response (HTML — the page only needs the bytes to arrive). Request
/// header blocks are HPACK-decoded via lshpack into `requests`; DATA frame
/// payloads are appended to the stream's record body.
fn serve_h2_connection(
    io: &mut ServerTlsIo,
    streams: &Arc<Mutex<Vec<(u32, String)>>>,
    requests: &Arc<Mutex<Vec<H2RequestRecord>>>,
    client_settings: &Arc<Mutex<Vec<Vec<u8>>>>,
) {
    let deadline = Instant::now() + Duration::from_secs(30);
    // Client preface: 24-byte magic.
    let mut magic = [0u8; 24];
    if read_exact_deadline(io, &mut magic, deadline).is_err() {
        return;
    }
    if &magic != b"PRI * HTTP/2.0\r\n\r\nSM\r\n\r\n" {
        return;
    }
    // One decoder per connection (HPACK dynamic table is connection-scoped).
    // Capacity 65536 = the largest table the stealth profiles advertise, so
    // any client dynamic-table size update stays decodable.
    let mut hpack = HpackHandle::new(65536);
    // Partial header block per stream: (fragment bytes, END_STREAM seen on
    // any frame of the block). RFC 9113 §4.2: END_STREAM rides the HEADERS
    // frame, END_HEADERS may arrive on a later CONTINUATION.
    let mut header_fragments: HashMap<u32, (Vec<u8>, bool)> = HashMap::new();
    let mut buffer: Vec<u8> = Vec::new();
    let mut settings_seen = false;
    loop {
        // Frame header.
        while buffer.len() < 9 {
            let mut chunk = [0u8; 4096];
            match io.read(&mut chunk) {
                Ok(0) => return,
                Ok(n) => buffer.extend_from_slice(&chunk[..n]),
                Err(ref e)
                    if e.kind() == std::io::ErrorKind::WouldBlock ||
                        e.kind() == std::io::ErrorKind::TimedOut =>
                {
                    if Instant::now() > deadline {
                        return;
                    }
                    continue;
                },
                Err(_) => return,
            }
        }
        let frame_len = ((buffer[0] as usize) << 16) | ((buffer[1] as usize) << 8) | buffer[2] as usize;
        let frame_type = buffer[3];
        let flags = buffer[4];
        let stream = u32::from_be_bytes([buffer[5], buffer[6], buffer[7], buffer[8]]) & 0x7fff_ffff;
        while buffer.len() < 9 + frame_len {
            let mut chunk = [0u8; 16384];
            match io.read(&mut chunk) {
                Ok(0) => return,
                Ok(n) => buffer.extend_from_slice(&chunk[..n]),
                Err(ref e)
                    if e.kind() == std::io::ErrorKind::WouldBlock ||
                        e.kind() == std::io::ErrorKind::TimedOut =>
                {
                    if Instant::now() > deadline {
                        return;
                    }
                    continue;
                },
                Err(_) => return,
            }
        }
        let payload = buffer[9..9 + frame_len].to_vec();
        buffer.drain(..9 + frame_len);

        match frame_type {
            h2frame::SETTINGS if flags & h2frame::FLAG_ACK == 0 && !settings_seen => {
                settings_seen = true;
                client_settings.lock().unwrap().push(payload);
                // Server SETTINGS (empty) + ACK of the client's.
                let _ = io.write_all(&h2frame::header(0, h2frame::SETTINGS, 0, 0));
                let _ = io.write_all(&h2frame::header(0, h2frame::SETTINGS, h2frame::FLAG_ACK, 0));
                let _ = io.flush();
            },
            h2frame::HEADERS | h2frame::CONTINUATION => {
                let entry = header_fragments.entry(stream).or_default();
                entry.0.extend_from_slice(&payload);
                entry.1 |= flags & h2frame::FLAG_END_STREAM != 0;
                if flags & h2frame::FLAG_END_HEADERS != 0 {
                    let (block, end_stream_on_headers) =
                        header_fragments.remove(&stream).unwrap_or_default();
                    streams.lock().unwrap().push((stream, format!("stream-{}", stream)));
                    let record = decode_request_block(&mut hpack, stream, &block);
                    requests.lock().unwrap().push(H2RequestRecord {
                        body_done: end_stream_on_headers,
                        ..record
                    });
                    let body = b"<html><head><title>h2</title></head><body><p id=\"t\">h2 doc</p></body></html>".to_vec();
                    let head_block = hpack_response_headers(&[
                        ("content-type", "text/html; charset=utf-8"),
                        ("content-length", &body.len().to_string()),
                    ]);
                    let mut out = h2frame::header(head_block.len(), h2frame::HEADERS, h2frame::FLAG_END_HEADERS, stream).to_vec();
                    out.extend_from_slice(&head_block);
                    let mut data = h2frame::header(body.len(), h2frame::DATA, h2frame::FLAG_END_STREAM, stream).to_vec();
                    data.extend_from_slice(&body);
                    if io.write_all(&out).is_err() || io.write_all(&data).is_err() {
                        return;
                    }
                    let _ = io.flush();
                }
            },
            h2frame::DATA => {
                let end = flags & h2frame::FLAG_END_STREAM != 0;
                let mut guard = requests.lock().unwrap();
                if let Some(record) = guard.iter_mut().rev().find(|r| r.stream_id == stream) {
                    record.body.extend_from_slice(&payload);
                    record.body_done |= end;
                }
            },
            h2frame::GOAWAY => return,
            _ => {},
        }
        if Instant::now() > deadline {
            return;
        }
    }
}

/// HPACK-decode one complete header block (HEADERS + CONTINUATIONs) into the
/// pseudo-header fields + regular header list. Decode errors are recorded on
/// the record (`decode_error`) instead of silently fabricating fields.
fn decode_request_block(hpack: &mut HpackHandle, stream_id: u32, block: &[u8]) -> H2RequestRecord {
    let mut record = H2RequestRecord {
        stream_id,
        method: String::new(),
        path: String::new(),
        authority: String::new(),
        scheme: String::new(),
        headers: Vec::new(),
        body: Vec::new(),
        body_done: false,
        decode_error: false,
    };
    let mut offset: usize = 0;
    while offset < block.len() {
        let result = match hpack.decode(&block[offset..]) {
            Ok(r) => r,
            Err(_) => {
                record.decode_error = true;
                return record;
            },
        };
        // DecodeResult name/value borrow lshpack's thread-local scratch
        // buffer — valid only until the next decode call, so copy now.
        let name = result.name.to_vec();
        let value = result.value.to_vec();
        offset += result.next;
        match name.as_slice() {
            b":method" => record.method = String::from_utf8_lossy(&value).into_owned(),
            b":path" => record.path = String::from_utf8_lossy(&value).into_owned(),
            b":authority" => record.authority = String::from_utf8_lossy(&value).into_owned(),
            b":scheme" => record.scheme = String::from_utf8_lossy(&value).into_owned(),
            _ => record.headers.push((
                String::from_utf8_lossy(&name).into_owned(),
                String::from_utf8_lossy(&value).into_owned(),
            )),
        }
    }
    record
}

fn read_exact_deadline(io: &mut ServerTlsIo, buf: &mut [u8], deadline: Instant) -> std::io::Result<()> {
    let mut filled = 0;
    while filled < buf.len() {
        if Instant::now() > deadline {
            return Err(std::io::Error::new(std::io::ErrorKind::TimedOut, "h2 preface timeout"));
        }
        match io.read(&mut buf[filled..]) {
            Ok(0) => {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::UnexpectedEof,
                    "eof during preface",
                ))
            },
            Ok(n) => filled += n,
            Err(ref e)
                if e.kind() == std::io::ErrorKind::WouldBlock ||
                    e.kind() == std::io::ErrorKind::TimedOut => {},
            Err(e) => return Err(e),
        }
    }
    Ok(())
}
