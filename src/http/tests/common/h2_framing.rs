//! Shared hand-rolled HTTP/2 wire helpers (RFC 9113) for the scripted h2
//! fixture binaries — the frame encoder, the frame-type/flag constants, and
//! the deadline-bounded preface reader. Each test binary uses the subset its
//! scenario needs (e.g. only `transport_backpressure_tests` frames DATA /
//! WINDOW_UPDATE), so per-binary dead code here is the module-inclusion
//! mechanism at work, not rot.
#![allow(dead_code)]

use std::io::Read;
use std::time::Instant;

use super::ServerTlsIo;

/// Minimal HTTP/2 framing (RFC 9113): header + payload.
pub(crate) fn frame(frame_type: u8, flags: u8, stream: u32, payload: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(9 + payload.len());
    out.push((payload.len() >> 16) as u8);
    out.push((payload.len() >> 8) as u8);
    out.push(payload.len() as u8);
    out.push(frame_type);
    out.push(flags);
    out.extend_from_slice(&stream.to_be_bytes());
    out.extend_from_slice(payload);
    out
}

pub(crate) const FT_DATA: u8 = 0x0;
pub(crate) const FT_HEADERS: u8 = 0x1;
pub(crate) const FT_SETTINGS: u8 = 0x4;
pub(crate) const FT_GOAWAY: u8 = 0x7;
pub(crate) const FT_WINDOW_UPDATE: u8 = 0x8;
pub(crate) const FT_CONTINUATION: u8 = 0x9;
pub(crate) const FLAG_ACK: u8 = 0x1;
pub(crate) const FLAG_END_STREAM: u8 = 0x1;
pub(crate) const FLAG_END_HEADERS: u8 = 0x4;

/// Fill `buf` from `io` exactly, tolerating WouldBlock/TimedOut (fixtures
/// that arm a socket read timeout) until `deadline`.
pub(crate) fn read_exact_deadline(
    io: &mut ServerTlsIo,
    buf: &mut [u8],
    deadline: Instant,
) -> std::io::Result<()> {
    let mut filled = 0;
    while filled < buf.len() {
        if Instant::now() > deadline {
            return Err(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                "h2 preface timeout",
            ));
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
                if e.kind() == std::io::ErrorKind::WouldBlock
                    || e.kind() == std::io::ErrorKind::TimedOut => {},
            Err(e) => return Err(e),
        }
    }
    Ok(())
}
