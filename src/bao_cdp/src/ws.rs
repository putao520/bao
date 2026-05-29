// REQ-CDP-002: CDP WebSocket server for external clients
use std::io::{Read, Write};
use std::net::TcpStream;

const WS_OPCODE_TEXT: u8 = 0x1;
const WS_OPCODE_CLOSE: u8 = 0x8;
const WS_OPCODE_PING: u8 = 0x9;
const WS_OPCODE_PONG: u8 = 0xA;

pub fn read_message(stream: &mut TcpStream) -> Result<Option<String>, ()> {
    let mut header = [0u8; 2];
    match stream.read(&mut header) {
        Ok(0) | Err(_) => return Err(()),
        Ok(1) => {
            stream.read_exact(&mut header[1..2]).map_err(|_| ())?;
        }
        Ok(_) => {}
    }

    let opcode = header[0] & 0x0F;
    let masked = (header[1] & 0x80) != 0;
    let payload_len = header[1] & 0x7F;

    let length: u64 = match payload_len {
        126 => {
            let mut ext = [0u8; 2];
            stream.read_exact(&mut ext).map_err(|_| ())?;
            u16::from_be_bytes(ext) as u64
        }
        127 => {
            let mut ext = [0u8; 8];
            stream.read_exact(&mut ext).map_err(|_| ())?;
            u64::from_be_bytes(ext)
        }
        n => n as u64,
    };

    let mut mask_key = [0u8; 4];
    if masked {
        stream.read_exact(&mut mask_key).map_err(|_| ())?;
    }

    let mut payload = vec![0u8; length as usize];
    if length > 0 {
        stream.read_exact(&mut payload).map_err(|_| ())?;
    }

    if masked {
        for (i, byte) in payload.iter_mut().enumerate() {
            *byte ^= mask_key[i % 4];
        }
    }

    match opcode {
        WS_OPCODE_TEXT => {
            let text = std::str::from_utf8(&payload).map_err(|_| ())?;
            Ok(Some(text.to_string()))
        }
        WS_OPCODE_CLOSE => Err(()),
        WS_OPCODE_PING => {
            let _ = write_pong(stream, &payload);
            Ok(None)
        }
        _ => Ok(None),
    }
}

pub fn write_message(stream: &mut TcpStream, data: &str) -> Result<(), ()> {
    let payload = data.as_bytes();
    let len = payload.len();

    let mut frame = Vec::with_capacity(len + 10);
    frame.push(0x81);

    if len < 126 {
        frame.push(len as u8);
    } else if len < 65536 {
        frame.push(126);
        frame.extend_from_slice(&(len as u16).to_be_bytes());
    } else {
        frame.push(127);
        frame.extend_from_slice(&(len as u64).to_be_bytes());
    }

    frame.extend_from_slice(payload);

    stream.write_all(&frame).map_err(|_| ())?;
    stream.flush().map_err(|_| ())?;
    Ok(())
}

fn write_pong(stream: &mut TcpStream, payload: &[u8]) -> Result<(), ()> {
    let mut frame = Vec::with_capacity(payload.len() + 2);
    frame.push(0x8A);

    if payload.len() < 126 {
        frame.push(payload.len() as u8);
    } else {
        return Err(());
    }

    frame.extend_from_slice(payload);
    stream.write_all(&frame).map_err(|_| ())?;
    stream.flush().map_err(|_| ())?;
    Ok(())
}

pub fn compute_accept_key(client_key: &str) -> String {
    const WS_MAGIC: &[u8] = b"258EAFA5-E914-47DA-95CA-5AB5DC65B286";
    let mut hasher = sha1::Sha1::new();
    sha1::Digest::update(&mut hasher, client_key.as_bytes());
    sha1::Digest::update(&mut hasher, WS_MAGIC);
    let digest = sha1::Digest::finalize(hasher);
    base64::engine::Engine::encode(&base64::engine::general_purpose::STANDARD, digest)
}

mod sha1 {
    pub struct Sha1 {
        state: [u32; 5],
        count: u64,
        buffer: [u8; 64],
        buffer_idx: usize,
    }

    impl Sha1 {
        pub fn new() -> Self {
            Sha1 {
                state: [0x67452301, 0xEFCDAB89, 0x98BADCFE, 0x10325476, 0xC3D2E1F0],
                count: 0,
                buffer: [0; 64],
                buffer_idx: 0,
            }
        }

        pub fn update(&mut self, data: &[u8]) {
            for &byte in data {
                self.buffer[self.buffer_idx] = byte;
                self.buffer_idx += 1;
                if self.buffer_idx == 64 {
                    self.process_block();
                }
            }
            self.count += data.len() as u64;
        }

        pub fn finalize(mut self) -> [u8; 20] {
            let bit_len = self.count * 8;
            self.buffer[self.buffer_idx] = 0x80;
            self.buffer_idx += 1;

            if self.buffer_idx > 56 {
                while self.buffer_idx < 64 {
                    self.buffer[self.buffer_idx] = 0;
                    self.buffer_idx += 1;
                }
                self.process_block();
                self.buffer_idx = 0;
            }

            while self.buffer_idx < 56 {
                self.buffer[self.buffer_idx] = 0;
                self.buffer_idx += 1;
            }

            for (i, &b) in bit_len.to_be_bytes().iter().enumerate() {
                self.buffer[56 + i] = b;
            }
            self.process_block();

            let mut result = [0u8; 20];
            for (i, &s) in self.state.iter().enumerate() {
                result[i * 4..][..4].copy_from_slice(&s.to_be_bytes());
            }
            result
        }

        fn process_block(&mut self) {
            let mut w = [0u32; 80];
            for i in 0..16 {
                w[i] = u32::from_be_bytes([
                    self.buffer[i * 4],
                    self.buffer[i * 4 + 1],
                    self.buffer[i * 4 + 2],
                    self.buffer[i * 4 + 3],
                ]);
            }
            for i in 16..80 {
                w[i] = (w[i - 3] ^ w[i - 8] ^ w[i - 14] ^ w[i - 16]).rotate_left(1);
            }

            let [mut a, mut b, mut c, mut d, mut e] = self.state;

            for i in 0..80 {
                let (f, k) = match i {
                    0..=19 => ((b & c) | ((!b) & d), 0x5A827999u32),
                    20..=39 => (b ^ c ^ d, 0x6ED9EBA1u32),
                    40..=59 => ((b & c) | (b & d) | (c & d), 0x8F1BBCDCu32),
                    _ => (b ^ c ^ d, 0xCA62C1D6u32),
                };
                let temp = a.rotate_left(5).wrapping_add(f).wrapping_add(k).wrapping_add(w[i]).wrapping_add(e);
                e = d;
                d = c;
                c = b.rotate_left(30);
                b = a;
                a = temp;
            }

            self.state[0] = self.state[0].wrapping_add(a);
            self.state[1] = self.state[1].wrapping_add(b);
            self.state[2] = self.state[2].wrapping_add(c);
            self.state[3] = self.state[3].wrapping_add(d);
            self.state[4] = self.state[4].wrapping_add(e);
            self.buffer_idx = 0;
        }
    }

    pub trait Digest {
        fn new() -> Self where Self: Sized;
        fn update(&mut self, data: &[u8]);
        fn finalize(self) -> [u8; 20];
    }

    impl Digest for Sha1 {
        fn new() -> Self { Sha1::new() }
        fn update(&mut self, data: &[u8]) { self.update(data); }
        fn finalize(self) -> [u8; 20] { self.finalize() }
    }
}
