//! RFC 9110 §5.5 response-header value trim regression tests for the fetch()
//! HTTP/2 client (upstream Bun b8c4a9b629, #43118): HPACK and QPACK deliver a
//! field value as a length-prefixed byte string, so leading/trailing SP / HTAB
//! arrive as part of the value. HTTP/1.1 (picohttpparser) strips them; before
//! the fix the h2/h3 paths stored the padded value verbatim, so
//! `res.headers.get("x-ws")` was `" v\t"` over h2 and `"v"` over h1.1.
//!
//! Wire-level harness mirroring `hpack_table_size_update_tests`: an ALPN-h2
//! BoringSSL TlsServer answers the request HEADERS with a literal HPACK block
//! carrying padded values, driven through the real `AsyncHTTP` + `HTTPThread`.

mod common;

use std::io::{Read, Write};
use std::net::TcpListener;
use std::time::{Duration, Instant};

use bao_boringssl_bridge::{TlsServer, generate_self_signed_pem};
use common::fetch_harness::{Delivery, run_h2_fetch};
use common::h2_framing::{FLAG_ACK, FLAG_END_HEADERS, FLAG_END_STREAM, FT_GOAWAY, FT_HEADERS,
                         FT_SETTINGS, frame, read_exact_deadline};
use common::{ServerTlsIo, install_alpn_h2};

// Link seam: bun_io's posix event loop dispatches through
// `__bun_run_file_poll`, owned by bun_runtime::dispatch in product binaries.
#[unsafe(no_mangle)]
extern "Rust" fn __bun_run_file_poll(_poll: *mut bun_io::FilePoll, _size_or_offset: i64) {}

// Link seam for `__bun_crash_handler_out_of_memory` (see
// tls_info_and_streaming_tests for rationale).
#[unsafe(no_mangle)]
extern "Rust" fn __bun_crash_handler_out_of_memory() -> ! {
    eprintln!("bun: out of memory");
    std::process::abort()
}

/// HPACK literal-without-indexing for a new name (0x00 prefix, 7-bit
/// non-huffman lengths): `name: value` with the value bytes as given.
fn literal_header(name: &[u8], value: &[u8]) -> Vec<u8> {
    assert!(name.len() < 127 && value.len() < 127);
    let mut block = vec![0x00, name.len() as u8];
    block.extend_from_slice(name);
    block.push(value.len() as u8);
    block.extend_from_slice(value);
    block
}

/// `:status: 200` (static index 8) + the padded-value header set the upstream
/// table pins: an outer-padded value, an inner-whitespace value, and a
/// whitespace-only value.
fn padded_block() -> Vec<u8> {
    let mut block = vec![0x88];
    block.extend_from_slice(&literal_header(b"x-ws", b" v\t"));
    block.extend_from_slice(&literal_header(b"x-inner", b"a b\tc"));
    block.extend_from_slice(&literal_header(b"x-only-ws", b" \t "));
    block
}

/// Spawn the scripted h2 origin: normal preface + SETTINGS exchange, then the
/// first request HEADERS is answered with `padded_block()`.
fn spawn_padded_h2_server() -> u16 {
    let (cert, key) =
        generate_self_signed_pem("127.0.0.1", 365).expect("self-signed cert");
    let server = std::sync::Arc::new(TlsServer::new(&cert, &key).expect("TlsServer"));
    install_alpn_h2(&server);
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let port = listener.local_addr().unwrap().port();
    std::thread::spawn(move || {
        let Ok((mut tcp, _)) = listener.accept() else {
            return;
        };
        let Ok(mut tls) = server.accept() else {
            return;
        };
        let piggybacked = match ServerTlsIo::handshake(&mut tcp, &mut tls) {
            Ok(p) => p,
            Err(_) => return,
        };
        if tls.alpn_protocol() != Some(&b"h2"[..]) {
            return;
        }
        let mut io = ServerTlsIo::new(tcp, tls, piggybacked);
        serve_padded_h2(&mut io);
    });
    port
}

fn serve_padded_h2(io: &mut ServerTlsIo) {
    let deadline = Instant::now() + Duration::from_secs(15);
    let mut magic = [0u8; 24];
    if read_exact_deadline(io, &mut magic, deadline).is_err() {
        return;
    }
    if &magic != b"PRI * HTTP/2.0\r\n\r\nSM\r\n\r\n" {
        return;
    }
    let mut buffer: Vec<u8> = Vec::new();
    let mut answered = false;
    loop {
        while buffer.len() < 9 {
            let mut chunk = [0u8; 4096];
            match io.read(&mut chunk) {
                Ok(0) => return,
                Ok(n) => buffer.extend_from_slice(&chunk[..n]),
                Err(ref e)
                    if e.kind() == std::io::ErrorKind::WouldBlock
                        || e.kind() == std::io::ErrorKind::TimedOut =>
                {
                    if Instant::now() > deadline {
                        return;
                    }
                    continue;
                }
                Err(_) => return,
            }
        }
        let frame_len =
            ((buffer[0] as usize) << 16) | ((buffer[1] as usize) << 8) | buffer[2] as usize;
        let frame_type = buffer[3];
        let flags = buffer[4];
        let stream =
            u32::from_be_bytes([buffer[5], buffer[6], buffer[7], buffer[8]]) & 0x7fff_ffff;
        while buffer.len() < 9 + frame_len {
            let mut chunk = [0u8; 16384];
            match io.read(&mut chunk) {
                Ok(0) => return,
                Ok(n) => buffer.extend_from_slice(&chunk[..n]),
                Err(ref e)
                    if e.kind() == std::io::ErrorKind::WouldBlock
                        || e.kind() == std::io::ErrorKind::TimedOut =>
                {
                    if Instant::now() > deadline {
                        return;
                    }
                    continue;
                }
                Err(_) => return,
            }
        }
        buffer.drain(..9 + frame_len);

        match frame_type {
            FT_SETTINGS if flags & FLAG_ACK == 0 => {
                let _ = io.write_all(&frame(FT_SETTINGS, 0, 0, &[]));
                let _ = io.write_all(&frame(FT_SETTINGS, FLAG_ACK, 0, &[]));
                let _ = io.flush();
            },
            FT_HEADERS if !answered => {
                answered = true;
                let block = padded_block();
                let _ = io.write_all(&frame(
                    FT_HEADERS,
                    FLAG_END_STREAM | FLAG_END_HEADERS,
                    stream,
                    &block,
                ));
                let _ = io.flush();
            },
            FT_GOAWAY => return,
            _ => {},
        }
        if Instant::now() > deadline {
            return;
        }
    }
}

fn header_of<'a>(deliveries: &'a [Delivery], name: &[u8]) -> &'a [u8] {
    deliveries
        .iter()
        .rev()
        .flat_map(|d| d.headers.iter())
        .find(|(n, _)| n == name)
        .map(|(_, v)| v.as_slice())
        .unwrap_or_else(|| panic!("header {} missing from deliveries", String::from_utf8_lossy(name)))
}

// ─── Tests ──────────────────────────────────────────────────────────────────

/// The upstream table's core row: `" v\t"` decodes to `"v"` over h2, as it
/// always did over HTTP/1.1. Before the fix the padded bytes reached
/// `handle_response_metadata` and `FetchHeaders` verbatim.
#[test]
fn h2_outer_whitespace_is_stripped_from_response_values() {
    let port = spawn_padded_h2_server();
    let deliveries = run_h2_fetch(port, |_| {});
    let last = deliveries
        .last()
        .unwrap_or_else(|| panic!("no delivery before deadline (fetch hung)"));
    assert!(!last.has_more, "no terminal delivery");
    assert!(
        last.fail.is_none(),
        "expected success, got fail {:?}",
        last.fail.map(|e| e.name())
    );
    assert_eq!(header_of(&deliveries, b"x-ws"), b"v");
}

/// RFC 9110 §5.5 excludes only leading and trailing SP / HTAB: inner
/// whitespace is part of the value.
#[test]
fn h2_inner_whitespace_is_kept() {
    let port = spawn_padded_h2_server();
    let deliveries = run_h2_fetch(port, |_| {});
    assert_eq!(header_of(&deliveries, b"x-inner"), b"a b\tc");
    // A whitespace-only value trims to the empty value, not to a dropped field.
    assert_eq!(header_of(&deliveries, b"x-only-ws"), b"");
}

/// The pure rule the decode loop applies (also consumed by the h3 QPACK path).
#[test]
fn trim_response_value_strips_sp_and_htab_only() {
    assert_eq!(bun_http::h2_client::dispatch::trim_response_value(b" v\t"), b"v");
    assert_eq!(bun_http::h2_client::dispatch::trim_response_value(b" \t "), b"");
    assert_eq!(bun_http::h2_client::dispatch::trim_response_value(b"a b\tc"), b"a b\tc");
    assert_eq!(bun_http::h2_client::dispatch::trim_response_value(b""), b"");
    // A non-OWS byte at either end is not stripped.
    assert_eq!(bun_http::h2_client::dispatch::trim_response_value(b"\rv\n"), b"\rv\n");
}
