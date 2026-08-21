//! CVE-2024-28182 regression tests for the fetch() HTTP/2 client
//! (upstream Bun a5c86aec7): a header block is bounded not just by
//! accumulated bytes (`LOCAL_MAX_HEADER_LIST_SIZE`) but by CONTINUATION
//! frame count (`LOCAL_MAX_CONTINUATIONS` = 8, nghttp2
//! `NGHTTP2_DEFAULT_MAX_CONTINUATIONS`). A hostile origin that answers
//! HEADERS without END_HEADERS and then streams zero-length CONTINUATION
//! frames never advances the byte cap — without the frame-count cap the
//! fetch() promise never settles (connection pinned, CPU burned on 9-byte
//! frame headers).
//!
//! Wire-level pure-Rust harness: ALPN-h2 BoringSSL TlsServer speaking
//! hand-rolled h2 framing (static-table + literal HPACK, no huffman) driven
//! through `AsyncHTTP` + `HTTPThread` — the same shape as
//! `tls_info_and_streaming_tests.rs`, no JS runtime involved.

mod common;

use std::io::{Read, Write};
use std::net::TcpListener;
use std::time::{Duration, Instant};

use bao_boringssl_bridge::{TlsServer, generate_self_signed_pem};
use common::fetch_harness::{Delivery, run_h2_fetch};
use common::h2_framing::{FLAG_ACK, FLAG_END_HEADERS, FLAG_END_STREAM, FT_CONTINUATION, FT_GOAWAY,
                         FT_HEADERS, FT_SETTINGS, frame, read_exact_deadline};
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

// ─── Adversarial response scripts ──────────────────────────────────────────

/// Which hostile/benign CONTINUATION shape the server emits for the first
/// request stream it sees.
#[derive(Clone, Copy)]
enum Scenario {
    /// HEADERS + exactly 8 CONTINUATIONs (last one END_HEADERS): the full
    /// budget is legal and must deliver a 200.
    FullBudgetAccepted,
    /// HEADERS + 9 CONTINUATIONs: the 9th is rejected even though it carries
    /// END_HEADERS and would complete the block (nghttp2 / node 22 parity).
    NinthFrameRejected,
    /// HEADERS + 10 000 zero-length CONTINUATIONs: the CVE-2024-28182 repro.
    /// Empty payloads never advance the byte cap; only the frame-count cap
    /// ends this.
    ZeroLengthFlood,
    /// A 103 informational block and the final 200 block on one stream each
    /// split across all 8 CONTINUATIONs: the budget is per header block, not
    /// per stream or connection (16 CONTINUATIONs total are fine).
    BudgetResetsPerBlock,
}

/// HPACK `:status` literal with indexed name (static index 8) — covers 103;
/// 200 uses the fully indexed form `0x88`.
fn hpack_status(code: &str) -> Vec<u8> {
    let mut block = vec![0x08, code.len() as u8];
    block.extend_from_slice(code.as_bytes());
    block
}

/// One header block split over HEADERS + 8 CONTINUATIONs (the last carries
/// END_HEADERS). `headers_flags` distinguishes END_STREAM (final response)
/// from 0 (informational block that keeps the stream open).
fn split_block(headers_flags: u8, block: &[u8], stream: u32) -> Vec<u8> {
    let mut out = frame(FT_HEADERS, headers_flags, stream, block);
    for _ in 0..7 {
        out.extend_from_slice(&frame(FT_CONTINUATION, 0, stream, &[]));
    }
    out.extend_from_slice(&frame(FT_CONTINUATION, FLAG_END_HEADERS, stream, &[]));
    out
}

// ─── ALPN-h2 TLS fixture ────────────────────────────────────────────────────

/// Spawn the adversarial h2 origin. One connection is served: the h2
/// handshake (preface + SETTINGS exchange) completes normally, then the
/// scripted CONTINUATION shape is written for the first request HEADERS the
/// client sends. Write errors after the client GOAWAYs are ignored — the
/// point is what the client does with the frames it already parsed.
fn spawn_adversarial_h2_server(scenario: Scenario) -> u16 {
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
        serve_adversarial_h2(&mut io, scenario);
    });
    port
}

/// h2 connection loop: read the client preface, ACK every non-ACK SETTINGS,
/// and answer the first request HEADERS with the scenario script.
fn serve_adversarial_h2(io: &mut ServerTlsIo, scenario: Scenario) {
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
        // Frame header.
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
                // Server SETTINGS (empty) + ACK of the client's.
                let _ = io.write_all(&frame(FT_SETTINGS, 0, 0, &[]));
                let _ = io.write_all(&frame(FT_SETTINGS, FLAG_ACK, 0, &[]));
                let _ = io.flush();
            },
            FT_HEADERS if !answered => {
                answered = true;
                let script = adversarial_script(scenario, stream);
                // The client GOAWAYs and closes once the cap fires mid-script
                // (NinthFrame / ZeroLengthFlood); a write error there is the
                // expected outcome, not a failure of the fixture.
                let _ = io.write_all(&script);
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

/// The hostile/benign byte sequence for one request stream.
fn adversarial_script(scenario: Scenario, stream: u32) -> Vec<u8> {
    match scenario {
        Scenario::FullBudgetAccepted => split_block(FLAG_END_STREAM, &[0x88], stream),
        Scenario::NinthFrameRejected => {
            let mut out = frame(FT_HEADERS, FLAG_END_STREAM, stream, &[0x88]);
            for _ in 0..8 {
                out.extend_from_slice(&frame(FT_CONTINUATION, 0, stream, &[]));
            }
            out.extend_from_slice(&frame(FT_CONTINUATION, FLAG_END_HEADERS, stream, &[]));
            out
        },
        Scenario::ZeroLengthFlood => {
            let mut out = frame(FT_HEADERS, FLAG_END_STREAM, stream, &[0x88]);
            // 10 000 empty CONTINUATION frames = 90 000 bytes of 9-byte
            // headers with zero payload; the accumulated header block stays
            // at 1 byte forever, so only the frame-count cap can end this.
            for _ in 0..10_000 {
                out.extend_from_slice(&frame(FT_CONTINUATION, 0, stream, &[]));
            }
            out
        },
        Scenario::BudgetResetsPerBlock => {
            let mut out = split_block(0, &hpack_status("103"), stream);
            out.extend_from_slice(&split_block(FLAG_END_STREAM, &[0x88], stream));
            out
        },
    }
}

/// Terminal delivery of a successful fetch: not failed, status 200.
fn assert_ok_200(deliveries: &[Delivery], ctx: &str) {
    let Some(last) = deliveries.last() else {
        panic!("{}: no delivery before deadline (fetch hung — the CONTINUATION cap regressed?)", ctx);
    };
    assert!(!last.has_more, "{}: no terminal delivery", ctx);
    assert!(
        last.fail.is_none(),
        "{}: expected success, got fail {:?}",
        ctx,
        last.fail.map(|e| e.name())
    );
    assert_eq!(last.status, Some(200), "{}: expected 200", ctx);
}

/// Terminal delivery fails with HTTP2EnhanceYourCalm.
fn assert_enhance_your_calm(deliveries: &[Delivery], ctx: &str) {
    let Some(last) = deliveries.last() else {
        panic!("{}: no delivery before deadline (fetch hung—CVE-2024-28182 repro)", ctx);
    };
    assert!(!last.has_more, "{}: no terminal delivery", ctx);
    let fail = last
        .fail
        .unwrap_or_else(|| panic!("{}: expected a failure, got status {:?}", ctx, last.status));
    assert_eq!(
        fail,
        bun_core::err!(HTTP2EnhanceYourCalm),
        "{}: expected HTTP2EnhanceYourCalm, got {}",
        ctx,
        fail.name()
    );
}

// ─── Tests ──────────────────────────────────────────────────────────────────

/// HEADERS + exactly 8 CONTINUATIONs (END_HEADERS on the 8th) is inside the
/// budget and must complete the response (nghttp2 parity: the cap is `> 8`,
/// not `>= 8`).
#[test]
fn h2_continuation_full_budget_is_accepted() {
    let port = spawn_adversarial_h2_server(Scenario::FullBudgetAccepted);
    let deliveries = run_h2_fetch(port, |_| {});
    assert_ok_200(&deliveries, "full 8-frame budget");
}

/// The 9th CONTINUATION is rejected even when it carries END_HEADERS and
/// would have completed the block (pins the cap at exactly 8 and the
/// comparison at `>`).
#[test]
fn h2_continuation_ninth_frame_is_rejected() {
    let port = spawn_adversarial_h2_server(Scenario::NinthFrameRejected);
    let deliveries = run_h2_fetch(port, |_| {});
    assert_enhance_your_calm(&deliveries, "9th CONTINUATION");
}

/// CVE-2024-28182 repro: zero-length CONTINUATION flood. Empty payloads
/// never advance `LOCAL_MAX_HEADER_LIST_SIZE`, so without the frame-count
/// cap the fetch pins forever; with it the connection fails
/// `HTTP2EnhanceYourCalm` promptly.
#[test]
fn h2_continuation_zero_length_flood_is_rejected() {
    let start = Instant::now();
    let port = spawn_adversarial_h2_server(Scenario::ZeroLengthFlood);
    let deliveries = run_h2_fetch(port, |_| {});
    assert_enhance_your_calm(&deliveries, "zero-length flood");
    // The 9th frame of the flood is already over budget, so the failure must
    // arrive long before the fixture's 10 000 frames / 15 s deadline.
    assert!(
        start.elapsed() < Duration::from_secs(10),
        "flood rejection took {:?} — cap fired too late",
        start.elapsed()
    );
}

/// The budget is per header block, not per stream or connection: a 103
/// informational block and the final 200 block on one stream may each use
/// all 8 CONTINUATIONs (16 total). Fails if the counter is not reset at
/// HEADERS.
#[test]
fn h2_continuation_budget_resets_per_block() {
    let port = spawn_adversarial_h2_server(Scenario::BudgetResetsPerBlock);
    let deliveries = run_h2_fetch(port, |_| {});
    assert_ok_200(&deliveries, "budget resets per block");
}
