//! TLS connection wrapper using rustls Unbuffered API.
//!
//! Provides zero-copy TLS operations driven by a state machine:
//! `EncodeTlsData` → `TransmitTlsData` → `ReadTraffic` / `WriteTraffic` /
//! `PeerClosed` / `Closed` / `BlockedHandshake`.
//!
//! The connection holds internal buffers for incoming/outgoing TLS data
//! and exposes `read()` / `write()` for application-level plaintext I/O.

use std::sync::Arc;

use rustls::unbuffered::{
    AppDataRecord, ConnectionState, EncodeTlsData, ReadTraffic, WriteTraffic,
};
use rustls::ClientConfig;
use rustls::ServerConfig;
use rustls::pki_types::ServerName;

/// Maximum TLS record size (16 KiB + header overhead).
const TLS_RECORD_MAX: usize = 17_000;
/// Initial capacity for the outgoing TLS buffer.
const OUTGOING_INITIAL: usize = TLS_RECORD_MAX;
/// Initial capacity for the incoming TLS buffer.
const INCOMING_INITIAL: usize = TLS_RECORD_MAX * 4;

// ─── TlsConnection ───────────────────────────────────────────────────

/// A TLS connection using the rustls Unbuffered API.
///
/// Wraps either a client or server connection and drives the TLS state
/// machine internally. Application code calls [`TlsConnection::process()`]
/// to drive the state machine and [`TlsConnection::write()`] to encrypt
/// outgoing data.
pub enum TlsConnection {
    Client(ClientConn),
    Server(ServerConn),
}

impl core::fmt::Debug for TlsConnection {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Client(_) => f.debug_struct("TlsConnection::Client").finish_non_exhaustive(),
            Self::Server(_) => f.debug_struct("TlsConnection::Server").finish_non_exhaustive(),
        }
    }
}

pub struct ClientConn {
    conn: rustls::client::UnbufferedClientConnection,
    incoming: Vec<u8>,
    outgoing: Vec<u8>,
    handshake_done: bool,
    saw_peer_closed: bool,
}

pub struct ServerConn {
    conn: rustls::server::UnbufferedServerConnection,
    incoming: Vec<u8>,
    outgoing: Vec<u8>,
    handshake_done: bool,
    saw_peer_closed: bool,
}

impl TlsConnection {
    /// Create a new client-side TLS connection.
    pub fn new_client(
        config: Arc<ClientConfig>,
        name: ServerName<'static>,
    ) -> Result<Self, TlsError> {
        let conn = rustls::client::UnbufferedClientConnection::new(config, name)
            .map_err(TlsError::Rustls)?;
        Ok(Self::Client(ClientConn {
            conn,
            incoming: Vec::with_capacity(INCOMING_INITIAL),
            outgoing: Vec::with_capacity(OUTGOING_INITIAL),
            handshake_done: false,
            saw_peer_closed: false,
        }))
    }

    /// Create a new server-side TLS connection.
    pub fn new_server(config: Arc<ServerConfig>) -> Result<Self, TlsError> {
        let conn = rustls::server::UnbufferedServerConnection::new(config)
            .map_err(TlsError::Rustls)?;
        Ok(Self::Server(ServerConn {
            conn,
            incoming: Vec::with_capacity(INCOMING_INITIAL),
            outgoing: Vec::with_capacity(OUTGOING_INITIAL),
            handshake_done: false,
            saw_peer_closed: false,
        }))
    }

    /// Which side this connection represents.
    pub fn side(&self) -> rustls::Side {
        match self {
            Self::Client(_) => rustls::Side::Client,
            Self::Server(_) => rustls::Side::Server,
        }
    }

    /// Whether the TLS handshake has not yet completed.
    pub fn is_handshaking(&self) -> bool {
        match self {
            Self::Client(c) => !c.handshake_done,
            Self::Server(c) => !c.handshake_done,
        }
    }

    /// Feed raw TLS bytes received from the network into the connection.
    ///
    /// Call this when the transport layer (e.g. uSockets `on_data`) delivers
    /// ciphertext. After feeding, call [`process()`] to drive the state machine.
    pub fn feed(&mut self, data: &[u8]) {
        match self {
            Self::Client(c) => c.incoming.extend_from_slice(data),
            Self::Server(c) => c.incoming.extend_from_slice(data),
        }
    }

    /// Drive the TLS state machine and extract outgoing ciphertext.
    ///
    /// Returns decrypted application data and the number of outgoing
    /// ciphertext bytes ready to send.
    pub fn process(&mut self) -> Result<ProcessResult, TlsError> {
        match self {
            Self::Client(c) => c.process(),
            Self::Server(c) => c.process(),
        }
    }

    /// Encrypt application data and queue it for sending.
    ///
    /// Returns the number of plaintext bytes encrypted.
    pub fn write(&mut self, plaintext: &[u8]) -> Result<usize, TlsError> {
        match self {
            Self::Client(c) => c.write(plaintext),
            Self::Server(c) => c.write(plaintext),
        }
    }

    /// Take the outgoing ciphertext buffer for transmission.
    pub fn take_outgoing(&mut self) -> Vec<u8> {
        match self {
            Self::Client(c) => core::mem::take(&mut c.outgoing),
            Self::Server(c) => core::mem::take(&mut c.outgoing),
        }
    }

    /// Clear the outgoing buffer after bytes have been transmitted.
    pub fn clear_outgoing(&mut self) {
        match self {
            Self::Client(c) => c.outgoing.clear(),
            Self::Server(c) => c.outgoing.clear(),
        }
    }

    /// Initiate a clean TLS shutdown.
    pub fn queue_close_notify(&mut self) -> Result<(), TlsError> {
        match self {
            Self::Client(c) => c.queue_close_notify(),
            Self::Server(c) => c.queue_close_notify(),
        }
    }

    /// Whether the peer has closed their side of the connection.
    pub fn peer_closed(&self) -> bool {
        match self {
            Self::Client(c) => c.saw_peer_closed,
            Self::Server(c) => c.saw_peer_closed,
        }
    }

    /// ALPN protocol negotiated during handshake, if any.
    pub fn alpn_protocol(&self) -> Option<&[u8]> {
        match self {
            Self::Client(c) => c.conn.alpn_protocol(),
            Self::Server(c) => c.conn.alpn_protocol(),
        }
    }

    /// The negotiated cipher suite, if handshake completed.
    pub fn negotiated_cipher_suite(&self) -> Option<rustls::SupportedCipherSuite> {
        match self {
            Self::Client(c) => c.conn.negotiated_cipher_suite(),
            Self::Server(c) => c.conn.negotiated_cipher_suite(),
        }
    }

    /// The negotiated protocol version, if handshake completed.
    pub fn protocol_version(&self) -> Option<rustls::ProtocolVersion> {
        match self {
            Self::Client(c) => c.conn.protocol_version(),
            Self::Server(c) => c.conn.protocol_version(),
        }
    }
}

// ─── ProcessResult ───────────────────────────────────────────────────

/// Result of driving the TLS state machine.
#[derive(Debug)]
pub struct ProcessResult {
    /// Decrypted application data records.
    pub plaintext: Vec<Vec<u8>>,
    /// Number of outgoing ciphertext bytes ready to send.
    pub outgoing_bytes: usize,
    /// The TLS connection state after processing.
    pub state: TlsState,
}

/// Summarized TLS connection state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TlsState {
    /// Handshake in progress, waiting for more data from the peer.
    Handshaking,
    /// Handshake complete, ready for application data.
    Active,
    /// Peer sent close_notify; no more data will be received.
    PeerClosed,
    /// Both sides closed; this is a terminal state.
    Closed,
}

// ─── ClientConn implementation ───────────────────────────────────────

impl ClientConn {
    fn process(&mut self) -> Result<ProcessResult, TlsError> {
        let mut plaintext = Vec::new();
        let mut state = TlsState::Handshaking;

        loop {
            // Need incoming data or pending outgoing to make progress.
            if self.incoming.is_empty() && self.outgoing.is_empty() && self.handshake_done {
                state = TlsState::Active;
                break;
            }

            let status = self.conn.process_tls_records(&mut self.incoming);

            // Extract discard before consuming status.state — the status
            // borrows self.incoming via its 'i lifetime, so we must drain
            // *after* we're done with the ConnectionState.
            let discard = status.discard;

            let conn_state = match status.state {
                Ok(s) => s,
                Err(e) => return Err(TlsError::Rustls(e)),
            };

            // Now handle the state. After this match, conn_state is dropped
            // and we can safely drain self.incoming.
            match conn_state {
                ConnectionState::ReadTraffic(mut rt) => {
                    self.handshake_done = true;
                    drain_read_traffic(&mut rt, &mut plaintext);
                    state = TlsState::Active;
                }

                ConnectionState::EncodeTlsData(mut etd) => {
                    encode_to_outgoing(&mut self.outgoing, &mut etd)?;
                }

                ConnectionState::TransmitTlsData(mut ttd) => {
                    if let Some(wt) = ttd.may_encrypt_app_data() {
                        self.handshake_done = true;
                        drop(wt);
                    }
                    ttd.done();
                }

                ConnectionState::WriteTraffic(wt) => {
                    self.handshake_done = true;
                    drop(wt);
                    state = TlsState::Active;
                    break;
                }

                ConnectionState::PeerClosed => {
                    self.saw_peer_closed = true;
                    state = TlsState::PeerClosed;
                    break;
                }

                ConnectionState::Closed => {
                    self.saw_peer_closed = true;
                    state = TlsState::Closed;
                    break;
                }

                ConnectionState::BlockedHandshake => {
                    // Need more data from the peer.
                    break;
                }

                ConnectionState::ReadEarlyData(..) => {
                    // Client-side does not receive early data.
                    break;
                }

                // #[non_exhaustive] — future rustls states
                _ => break,
            }

            // Discard consumed bytes after ConnectionState is dropped.
            if discard > 0 {
                self.incoming.drain(..discard.min(self.incoming.len()));
            }
        }

        let outgoing_bytes = self.outgoing.len();
        Ok(ProcessResult {
            plaintext,
            outgoing_bytes,
            state,
        })
    }

    fn write(&mut self, plaintext: &[u8]) -> Result<usize, TlsError> {
        if !self.handshake_done {
            return Err(TlsError::NotReady);
        }

        let mut tmp_out = vec![0u8; plaintext.len() + TLS_RECORD_MAX];

        let status = self.conn.process_tls_records(&mut self.incoming);
        let discard = status.discard;

        let result = match status.state {
            Ok(ConnectionState::WriteTraffic(mut wt)) => {
                encrypt_into(&mut wt, plaintext, &mut tmp_out, &mut self.outgoing)
            }
            Ok(_) => Err(TlsError::NotReady),
            Err(e) => Err(TlsError::Rustls(e)),
        };

        if discard > 0 {
            self.incoming.drain(..discard.min(self.incoming.len()));
        }

        result
    }

    fn queue_close_notify(&mut self) -> Result<(), TlsError> {
        let mut tmp_out = vec![0u8; TLS_RECORD_MAX];
        let status = self.conn.process_tls_records(&mut self.incoming);
        let discard = status.discard;

        let result = match status.state {
            Ok(ConnectionState::WriteTraffic(mut wt)) => {
                match wt.queue_close_notify(&mut tmp_out) {
                    Ok(n) => {
                        self.outgoing.extend_from_slice(&tmp_out[..n]);
                        Ok(())
                    }
                    Err(_) => Err(TlsError::EncryptFailed),
                }
            }
            Ok(_) => Err(TlsError::NotReady),
            Err(e) => Err(TlsError::Rustls(e)),
        };

        if discard > 0 {
            self.incoming.drain(..discard.min(self.incoming.len()));
        }

        result
    }
}

// ─── ServerConn implementation ───────────────────────────────────────

impl ServerConn {
    fn process(&mut self) -> Result<ProcessResult, TlsError> {
        let mut plaintext = Vec::new();
        let mut state = TlsState::Handshaking;

        loop {
            if self.incoming.is_empty() && self.outgoing.is_empty() && self.handshake_done {
                state = TlsState::Active;
                break;
            }

            let status = self.conn.process_tls_records(&mut self.incoming);

            let discard = status.discard;

            let conn_state = match status.state {
                Ok(s) => s,
                Err(e) => return Err(TlsError::Rustls(e)),
            };

            match conn_state {
                ConnectionState::ReadTraffic(mut rt) => {
                    self.handshake_done = true;
                    drain_read_traffic(&mut rt, &mut plaintext);
                    state = TlsState::Active;
                }

                ConnectionState::EncodeTlsData(mut etd) => {
                    encode_to_outgoing_server(&mut self.outgoing, &mut etd)?;
                }

                ConnectionState::TransmitTlsData(mut ttd) => {
                    if let Some(wt) = ttd.may_encrypt_app_data() {
                        self.handshake_done = true;
                        drop(wt);
                    }
                    ttd.done();
                }

                ConnectionState::WriteTraffic(wt) => {
                    self.handshake_done = true;
                    drop(wt);
                    state = TlsState::Active;
                    break;
                }

                ConnectionState::PeerClosed => {
                    self.saw_peer_closed = true;
                    state = TlsState::PeerClosed;
                    break;
                }

                ConnectionState::Closed => {
                    self.saw_peer_closed = true;
                    state = TlsState::Closed;
                    break;
                }

                ConnectionState::BlockedHandshake => {
                    break;
                }

                ConnectionState::ReadEarlyData(mut red) => {
                    while let Some(record) = red.next_record() {
                        match record {
                            Ok(AppDataRecord { payload, .. }) => {
                                plaintext.push(payload.to_vec());
                            }
                            Err(e) => return Err(TlsError::Rustls(e)),
                        }
                    }
                    break;
                }

                // #[non_exhaustive] — future rustls states
                _ => break,
            }

            // Discard consumed bytes after ConnectionState is dropped.
            if discard > 0 {
                self.incoming.drain(..discard.min(self.incoming.len()));
            }
        }

        let outgoing_bytes = self.outgoing.len();
        Ok(ProcessResult {
            plaintext,
            outgoing_bytes,
            state,
        })
    }

    fn write(&mut self, plaintext: &[u8]) -> Result<usize, TlsError> {
        if !self.handshake_done {
            return Err(TlsError::NotReady);
        }

        let mut tmp_out = vec![0u8; plaintext.len() + TLS_RECORD_MAX];
        let status = self.conn.process_tls_records(&mut self.incoming);
        let discard = status.discard;

        let result = match status.state {
            Ok(ConnectionState::WriteTraffic(mut wt)) => {
                encrypt_into_server(&mut wt, plaintext, &mut tmp_out, &mut self.outgoing)
            }
            Ok(_) => Err(TlsError::NotReady),
            Err(e) => Err(TlsError::Rustls(e)),
        };

        if discard > 0 {
            self.incoming.drain(..discard.min(self.incoming.len()));
        }

        result
    }

    fn queue_close_notify(&mut self) -> Result<(), TlsError> {
        let mut tmp_out = vec![0u8; TLS_RECORD_MAX];
        let status = self.conn.process_tls_records(&mut self.incoming);
        let discard = status.discard;

        let result = match status.state {
            Ok(ConnectionState::WriteTraffic(mut wt)) => {
                match wt.queue_close_notify(&mut tmp_out) {
                    Ok(n) => {
                        self.outgoing.extend_from_slice(&tmp_out[..n]);
                        Ok(())
                    }
                    Err(_) => Err(TlsError::EncryptFailed),
                }
            }
            Ok(_) => Err(TlsError::NotReady),
            Err(e) => Err(TlsError::Rustls(e)),
        };

        if discard > 0 {
            self.incoming.drain(..discard.min(self.incoming.len()));
        }

        result
    }
}

// ─── Shared helpers ──────────────────────────────────────────────────

/// Drain all app-data records from a ReadTraffic state.
fn drain_read_traffic<Data>(
    rt: &mut ReadTraffic<'_, '_, Data>,
    plaintext: &mut Vec<Vec<u8>>,
) {
    while let Some(record) = rt.next_record() {
        match record {
            Ok(AppDataRecord { payload, .. }) => {
                plaintext.push(payload.to_vec());
            }
            Err(_) => break,
        }
    }
}

/// Encode handshake data into the outgoing buffer (client variant).
fn encode_to_outgoing(
    outgoing: &mut Vec<u8>,
    etd: &mut EncodeTlsData<'_, rustls::client::ClientConnectionData>,
) -> Result<(), TlsError> {
    encode_to_outgoing_inner(outgoing, etd)
}

/// Encode handshake data into the outgoing buffer (server variant).
fn encode_to_outgoing_server(
    outgoing: &mut Vec<u8>,
    etd: &mut EncodeTlsData<'_, rustls::server::ServerConnectionData>,
) -> Result<(), TlsError> {
    encode_to_outgoing_inner(outgoing, etd)
}

/// Generic encode implementation — works for both client and server
/// because `EncodeTlsData::encode` has the same signature regardless of `Data`.
fn encode_to_outgoing_inner<Data>(
    outgoing: &mut Vec<u8>,
    etd: &mut EncodeTlsData<'_, Data>,
) -> Result<(), TlsError> {
    let start = outgoing.len();
    outgoing.resize(start + TLS_RECORD_MAX, 0);
    match etd.encode(&mut outgoing[start..]) {
        Ok(n) => {
            outgoing.truncate(start + n);
            Ok(())
        }
        Err(rustls::unbuffered::EncodeError::InsufficientSize(e)) => {
            outgoing.resize(start + e.required_size, 0);
            match etd.encode(&mut outgoing[start..]) {
                Ok(n) => {
                    outgoing.truncate(start + n);
                    Ok(())
                }
                Err(_) => Err(TlsError::EncodeFailed),
            }
        }
        Err(rustls::unbuffered::EncodeError::AlreadyEncoded) => Ok(()),
    }
}

/// Encrypt plaintext into the outgoing buffer (client variant).
fn encrypt_into(
    wt: &mut WriteTraffic<'_, rustls::client::ClientConnectionData>,
    plaintext: &[u8],
    tmp_out: &mut [u8],
    outgoing: &mut Vec<u8>,
) -> Result<usize, TlsError> {
    encrypt_into_inner(wt, plaintext, tmp_out, outgoing)
}

/// Encrypt plaintext into the outgoing buffer (server variant).
fn encrypt_into_server(
    wt: &mut WriteTraffic<'_, rustls::server::ServerConnectionData>,
    plaintext: &[u8],
    tmp_out: &mut [u8],
    outgoing: &mut Vec<u8>,
) -> Result<usize, TlsError> {
    encrypt_into_inner(wt, plaintext, tmp_out, outgoing)
}

/// Generic encrypt implementation.
fn encrypt_into_inner<Data>(
    wt: &mut WriteTraffic<'_, Data>,
    plaintext: &[u8],
    tmp_out: &mut [u8],
    outgoing: &mut Vec<u8>,
) -> Result<usize, TlsError> {
    match wt.encrypt(plaintext, tmp_out) {
        Ok(n) => {
            outgoing.extend_from_slice(&tmp_out[..n]);
            Ok(plaintext.len())
        }
        Err(rustls::unbuffered::EncryptError::InsufficientSize(e)) => {
            // Resize and retry — this should succeed since we sized exactly.
            tmp_out[..e.required_size].fill(0);
            match wt.encrypt(plaintext, &mut tmp_out[..e.required_size]) {
                Ok(n) => {
                    outgoing.extend_from_slice(&tmp_out[..n]);
                    Ok(plaintext.len())
                }
                Err(_) => Err(TlsError::EncryptFailed),
            }
        }
        Err(_) => Err(TlsError::EncryptFailed),
    }
}

// ─── TlsError ────────────────────────────────────────────────────────

/// Errors that can occur during TLS operations.
#[derive(Debug, thiserror::Error)]
pub enum TlsError {
    #[error("rustls error: {0}")]
    Rustls(#[from] rustls::Error),
    #[error("connection not ready for application data")]
    NotReady,
    #[error("TLS encoding failed")]
    EncodeFailed,
    #[error("TLS encryption failed")]
    EncryptFailed,
    #[error("invalid server name: {0}")]
    InvalidServerName(String),
    #[error("invalid certificate/key: {0}")]
    InvalidCertKey(String),
}
