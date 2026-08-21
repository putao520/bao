//! HPACK Dynamic Table Size Update acceptance tests (Google-domain
//! `HTTP2CompressionError` regression).
//!
//! Root cause this pins: the client preface advertises
//! SETTINGS_HEADER_TABLE_SIZE (stealth profiles: 65536), which per RFC 7541
//! §4.2 entitles the peer's encoder to size its dynamic table up to that
//! value and signal it with a Dynamic Table Size Update at the start of a
//! header block. Google's GFE answers with an update to 12288 (`3f e1 5f`);
//! a decoder left at the 4096 protocol default rejects it as BAD_DATA, which
//! surfaces as a connection `HTTP2CompressionError` — googleapis/google.com
//! failed 100% while cloudflare worked (its update is exactly 4096) and curl
//! worked (it advertises no table size, so GFE never grows).
//!
//! Two layers of coverage:
//!   1. Pure: `advertised_hpack_table_size` + `HpackHandle` decoder caps.
//!   2. Wire-level e2e (same harness shape as `h2_continuation_cap_tests`):
//!      an ALPN-h2 origin answering with a >4096 size update must deliver a
//!      200 when the preface advertised 65536, and still fail loudly with
//!      `HTTP2CompressionError` when nothing was advertised (the peer
//!      exceeding the advertised bound is a real protocol violation).

mod common;

use std::io::{Read, Write};
use std::net::TcpListener;
use std::time::{Duration, Instant};

use bao_boringssl_bridge::{TlsServer, generate_self_signed_pem};
use common::fetch_harness::Delivery;
use common::h2_framing::{FLAG_ACK, FLAG_END_HEADERS, FLAG_END_STREAM, FT_GOAWAY, FT_HEADERS,
                         FT_SETTINGS, frame, read_exact_deadline};
use common::{ServerTlsIo, install_alpn_h2};

// Link seam: bun_io's posix event loop dispatches through
// `__bun_run_file_poll`, owned by bun_runtime::dispatch in product binaries.
// No FilePoll sources are registered anywhere in these tests, so a no-op
// satisfies the link-time reference. (Mirror of tls_info_and_streaming_tests.)
#[unsafe(no_mangle)]
extern "Rust" fn __bun_run_file_poll(_poll: *mut bun_io::FilePoll, _size_or_offset: i64) {}

// Link seam for `__bun_crash_handler_out_of_memory` (see
// tls_info_and_streaming_tests for rationale).
#[unsafe(no_mangle)]
extern "Rust" fn __bun_crash_handler_out_of_memory() -> ! {
    eprintln!("bun: out of memory");
    std::process::abort()
}

// ─── Pure layer: advertised-table-size scan ────────────────────────────────

/// One SETTINGS wire unit: setting id + value, both big-endian.
fn setting(id: u16, value: u32) -> [u8; 6] {
    let mut unit = [0u8; 6];
    unit[0..2].copy_from_slice(&id.to_be_bytes());
    unit[2..6].copy_from_slice(&value.to_be_bytes());
    unit
}

#[test]
fn advertised_table_size_found_in_preface_payload() {
    // Firefox/Chrome stealth shape: HEADER_TABLE_SIZE=65536 leads the frame.
    let mut payload = Vec::new();
    payload.extend_from_slice(&setting(0x0001, 65536));
    payload.extend_from_slice(&setting(0x0002, 0));
    payload.extend_from_slice(&setting(0x0004, 131072));
    assert_eq!(
        bun_http::h2_client::advertised_hpack_table_size(&payload),
        Some(65536)
    );
}

#[test]
fn advertised_table_size_absent_when_not_advertised() {
    // Default preface (ENABLE_PUSH/INITIAL_WINDOW/MAX_HEADER_LIST): no 0x0001
    // unit, so the 4096 protocol default governs the decoder.
    let mut payload = Vec::new();
    payload.extend_from_slice(&setting(0x0002, 0));
    payload.extend_from_slice(&setting(0x0004, 1 << 24));
    payload.extend_from_slice(&setting(0x0006, 256 * 1024));
    assert_eq!(bun_http::h2_client::advertised_hpack_table_size(&payload), None);
    assert_eq!(bun_http::h2_client::advertised_hpack_table_size(&[]), None);
    assert_eq!(bun_http::h2_client::DEFAULT_HPACK_TABLE_SIZE, 4096);
}

#[test]
fn advertised_table_size_last_duplicate_wins_and_partial_tail_ignored() {
    let mut payload = Vec::new();
    payload.extend_from_slice(&setting(0x0001, 4096));
    payload.extend_from_slice(&setting(0x0001, 65536));
    // Trailing garbage shorter than a full unit is not a setting.
    payload.extend_from_slice(&[0x00, 0x01, 0x00]);
    assert_eq!(
        bun_http::h2_client::advertised_hpack_table_size(&payload),
        Some(65536)
    );
}

// ─── Pure layer: decoder bound ──────────────────────────────────────────────

/// `3f e1 5f` = Dynamic Table Size Update to 12288 (Google GFE's answer to an
/// advertised 65536), followed by `0x88` = indexed `:status: 200`.
const TSU_12288_STATUS_200: &[u8] = &[0x3f, 0xe1, 0x5f, 0x88];

#[test]
fn decoder_accepts_update_up_to_advertised_capacity() {
    let mut hpack = bun_http::lshpack::HpackHandle::new(4096);
    hpack.set_decoder_max_capacity(65536);
    let r = hpack
        .decode(TSU_12288_STATUS_200)
        .expect("size update ≤ advertised must decode");
    assert_eq!(r.name, b":status");
    assert_eq!(r.value, b"200");
    assert_eq!(r.next, TSU_12288_STATUS_200.len());
}

#[test]
fn decoder_still_rejects_update_above_advertised_capacity() {
    // Nothing advertised → decoder stays at the 4096 default → a 12288
    // update is a peer protocol violation and MUST stay a loud
    // COMPRESSION_ERROR, not a silent accept.
    let mut hpack = bun_http::lshpack::HpackHandle::new(4096);
    assert!(hpack.decode(TSU_12288_STATUS_200).is_err());
    // Explicitly advertising 4096 behaves the same (cloudflare's exact-4096
    // update still decodes).
    hpack.set_decoder_max_capacity(4096);
    assert!(hpack.decode(TSU_12288_STATUS_200).is_err());
    let r = hpack
        .decode(&[0x3f, 0xe1, 0x1f, 0x88]) // update to exactly 4096
        .expect("size update == advertised must decode");
    assert_eq!(r.value, b"200");
}

// ─── Wire-level e2e fixture ─────────────────────────────────────────────────

/// The response header block the scripted origin answers with.
#[derive(Clone, Copy, PartialEq)]
enum BlockShape {
    /// `3f e1 5f` + `88`: Dynamic Table Size Update to 12288 (Google GFE's
    /// reply to an advertised 65536) then indexed `:status: 200`.
    Tsu12288ThenStatus200,
    /// `3f e1 1f` + `88`: update to exactly 4096 (cloudflare's shape) —
    /// must always decode, with or without an advertised table size.
    Tsu4096ThenStatus200,
    /// Bare `88` (no size update): baseline that the harness itself works.
    Status200Only,
}

/// Spawn the scripted h2 origin: preface + SETTINGS exchange complete
/// normally, then the first request HEADERS is answered with the given
/// header-block shape. `advertised_table_size` (when `Some`) is installed as
/// the client's SETTINGS_HEADER_TABLE_SIZE via `AsyncHTTP::Options` — the
/// same channel the stealth profile uses (`SSLConfig.h2_settings_payload`).
fn spawn_tsu_h2_server(shape: BlockShape) -> u16 {
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
        serve_tsu_h2(&mut io, shape);
    });
    port
}

/// h2 connection loop: read the client preface, ACK every non-ACK SETTINGS,
/// answer the first request HEADERS with the scripted block.
fn serve_tsu_h2(io: &mut ServerTlsIo, shape: BlockShape) {
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
                let block: &[u8] = match shape {
                    BlockShape::Tsu12288ThenStatus200 => &[0x3f, 0xe1, 0x5f, 0x88],
                    BlockShape::Tsu4096ThenStatus200 => &[0x3f, 0xe1, 0x1f, 0x88],
                    BlockShape::Status200Only => &[0x88],
                };
                let _ = io.write_all(&frame(
                    FT_HEADERS,
                    FLAG_END_STREAM | FLAG_END_HEADERS,
                    stream,
                    block,
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

/// Drive one GET through the real HTTPThread over ALPN-negotiated h2 and
/// collect deliveries until the terminal (`has_more == false`) one.
/// `advertised_table_size` installs a stealth-style SETTINGS payload with
/// SETTINGS_HEADER_TABLE_SIZE set to that value (the exact channel
/// `stealth_profile_to_ssl_config` → `write_preface` uses in production).
fn run_h2_fetch(port: u16, advertised_table_size: Option<u32>) -> Vec<Delivery> {
    common::fetch_harness::run_h2_fetch(port, |options| {
        if let Some(table_size) = advertised_table_size {
            let mut cfg = bun_http::ssl_config::SSLConfig::default();
            cfg.h2_settings_payload = Some(setting(0x0001, table_size).to_vec().into_boxed_slice());
            options.tls_props = Some(bun_http::ssl_config::SharedPtr::new(cfg));
        }
    })
}

fn assert_ok_200(deliveries: &[Delivery], ctx: &str) {
    let Some(last) = deliveries.last() else {
        panic!("{ctx}: no delivery before deadline (fetch hung)");
    };
    assert!(!last.has_more, "{ctx}: no terminal delivery");
    assert!(
        last.fail.is_none(),
        "{ctx}: expected success, got fail {:?}",
        last.fail.map(|e| e.name())
    );
    assert_eq!(last.status, Some(200), "{ctx}: expected 200");
}

fn assert_compression_error(deliveries: &[Delivery], ctx: &str) {
    let Some(last) = deliveries.last() else {
        panic!("{ctx}: no delivery before deadline (fetch hung)");
    };
    assert!(!last.has_more, "{ctx}: no terminal delivery");
    let fail = last
        .fail
        .unwrap_or_else(|| panic!("{ctx}: expected a failure, got status {:?}", last.status));
    assert_eq!(
        fail,
        bun_core::err!(HTTP2CompressionError),
        "{ctx}: expected HTTP2CompressionError, got {}",
        fail.name()
    );
}

// ─── Tests ──────────────────────────────────────────────────────────────────

/// The Google repro: preface advertises HEADER_TABLE_SIZE=65536 (stealth
/// profile shape), origin answers with a Dynamic Table Size Update to 12288.
/// Must deliver a 200 — before the fix this failed HTTP2CompressionError on
/// every googleapis/google.com fetch.
#[test]
fn h2_tsu_above_4096_decodes_when_advertised() {
    let port = spawn_tsu_h2_server(BlockShape::Tsu12288ThenStatus200);
    let deliveries = run_h2_fetch(port, Some(65536));
    assert_ok_200(&deliveries, "advertised 65536, TSU 12288");
}

/// Without an advertised table size the bound stays at the 4096 protocol
/// default, so a 12288 update is a peer protocol violation that MUST fail
/// loudly (HTTP2CompressionError) — never silently accepted.
#[test]
fn h2_tsu_above_default_is_a_loud_compression_error() {
    let port = spawn_tsu_h2_server(BlockShape::Tsu12288ThenStatus200);
    let deliveries = run_h2_fetch(port, None);
    assert_compression_error(&deliveries, "no advertisement, TSU 12288");
}

/// The cloudflare shape: update to exactly 4096 decodes with or without an
/// advertisement (equality is allowed — the bound is inclusive).
#[test]
fn h2_tsu_exactly_4096_always_decodes() {
    let port = spawn_tsu_h2_server(BlockShape::Tsu4096ThenStatus200);
    let deliveries = run_h2_fetch(port, None);
    assert_ok_200(&deliveries, "no advertisement, TSU 4096");

    let port = spawn_tsu_h2_server(BlockShape::Tsu4096ThenStatus200);
    let deliveries = run_h2_fetch(port, Some(65536));
    assert_ok_200(&deliveries, "advertised 65536, TSU 4096");
}

/// Harness baseline: a bare indexed `:status: 200` (no size update) still
/// delivers — guards against the two tests above passing vacuously.
#[test]
fn h2_status_only_baseline_still_works() {
    let port = spawn_tsu_h2_server(BlockShape::Status200Only);
    let deliveries = run_h2_fetch(port, Some(65536));
    assert_ok_200(&deliveries, "advertised 65536, no TSU");
}
