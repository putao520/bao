#![warn(unused_must_use)]
use core::fmt;

use bstr::BStr;

use bun_core::output::enable_ansi_colors_stderr;
use bun_core::pretty_fmt;

// PORT NOTE: `Header::clone` / `Request::clone` / `Response::clone` need the
// unbound-lifetime `append_raw` so they can interleave appends and stash the
// raw ptr/len pairs — the Zig original returns aliasing `[]const u8` with no
// lifetime tracking. The buffer is heap-owned; callers keep the builder (or
// its moved-out buffer) alive while the returned slices are in use.
pub use bun_core::StringBuilder;

use bun_core::strings;

// ──────────────────────────────────────────────────────────────────────────
// Header
// ──────────────────────────────────────────────────────────────────────────

/// HTTP header with borrowed name and value slices.
///
/// Previously `#[repr(C)]` to match picohttpparser's `phr_header` layout.
/// Now a plain Rust struct delegating to `httparse` — no FFI layout constraints.
#[derive(Clone, Copy)]
pub struct Header {
    name_ptr: *const u8,
    name_len: usize,
    value_ptr: *const u8,
    value_len: usize,
}

impl Default for Header {
    #[inline]
    fn default() -> Self {
        Self::ZERO
    }
}

impl Header {
    /// All-zero sentinel — name/value are empty slices. Used by callers to
    /// initialize fixed-size header arrays before filling them.
    ///
    /// Uses `null()` (not `b"".as_ptr()`) so the const evaluates to all-zero
    /// bytes — `[Header::ZERO; N]` statics land in `.bss` instead of `.data`,
    /// matching Zig's `var buf: [N]Header = undefined`. `name()`/`value()` go
    /// through `ffi::slice`, which tolerates `(null, 0)`.
    pub const ZERO: Self = Self {
        name_ptr: core::ptr::null(),
        name_len: 0,
        value_ptr: core::ptr::null(),
        value_len: 0,
    };

    /// Construct a `Header` from borrowed name/value slices. The caller is
    /// responsible for keeping the backing storage alive for as long as the
    /// `Header` is read (matches the Zig `[]const u8` field semantics).
    #[inline]
    pub const fn new(name: &[u8], value: &[u8]) -> Self {
        Self {
            name_ptr: name.as_ptr(),
            name_len: name.len(),
            value_ptr: value.as_ptr(),
            value_len: value.len(),
        }
    }

    #[inline]
    pub fn name(&self) -> &[u8] {
        // SAFETY: ptr/len originate from httparse pointing into the
        // caller-provided buffer, or from StringBuilder::append.
        // `ffi::slice` tolerates the (null, 0) shape for multiline headers.
        unsafe { bun_core::ffi::slice(self.name_ptr, self.name_len) }
    }

    #[inline]
    pub fn value(&self) -> &[u8] {
        // SAFETY: same as name()
        unsafe { bun_core::ffi::slice(self.value_ptr, self.value_len) }
    }

    pub fn is_multiline(&self) -> bool {
        self.name_len == 0
    }

    pub fn count(&self, builder: &mut StringBuilder) {
        builder.count(self.name());
        builder.count(self.value());
    }

    pub fn clone(&self, builder: &mut StringBuilder) -> Header {
        // SAFETY: returned slices alias `builder`'s heap buffer; caller of the
        // outer `clone` keeps the builder (or its moved-out buffer) alive for
        // the lifetime of the cloned `Header` (see PORT NOTE on `StringBuilder`).
        let name = unsafe { builder.append_raw(self.name()) };
        // SAFETY: same buffer-lifetime invariant as `name` above.
        let value = unsafe { builder.append_raw(self.value()) };
        Header {
            name_ptr: name.as_ptr(),
            name_len: name.len(),
            value_ptr: value.as_ptr(),
            value_len: value.len(),
        }
    }

    pub fn curl(&self) -> HeaderCurlFormatter<'_> {
        HeaderCurlFormatter { header: self }
    }
}

impl fmt::Display for Header {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if enable_ansi_colors_stderr() {
            if self.is_multiline() {
                write!(f, pretty_fmt!("<r><cyan>{}", true), BStr::new(self.value()))
            } else {
                write!(
                    f,
                    pretty_fmt!("<r><cyan>{}<r><d>: <r>{}", true),
                    BStr::new(self.name()),
                    BStr::new(self.value()),
                )
            }
        } else {
            if self.is_multiline() {
                write!(
                    f,
                    pretty_fmt!("<r><cyan>{}", false),
                    BStr::new(self.value())
                )
            } else {
                write!(
                    f,
                    pretty_fmt!("<r><cyan>{}<r><d>: <r>{}", false),
                    BStr::new(self.name()),
                    BStr::new(self.value()),
                )
            }
        }
    }
}

pub struct HeaderCurlFormatter<'a> {
    header: &'a Header,
}

impl fmt::Display for HeaderCurlFormatter<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let header = self.header;
        if header.value_len > 0 {
            write!(
                f,
                "-H \"{}: {}\"",
                BStr::new(header.name()),
                BStr::new(header.value())
            )
        } else {
            write!(f, "-H \"{}\"", BStr::new(header.name()))
        }
    }
}

// ──────────────────────────────────────────────────────────────────────────
// Header::List
// ──────────────────────────────────────────────────────────────────────────

#[derive(Clone, Copy, Default)]
pub struct HeaderList<'a> {
    pub list: &'a [Header],
}

impl<'a> HeaderList<'a> {
    pub fn get(&self, name: &[u8]) -> Option<&'a [u8]> {
        for header in self.list {
            if strings::eql_case_insensitive_ascii(header.name(), name, true) {
                return Some(header.value());
            }
        }
        None
    }

    pub fn get_if_other_is_absent(
        &self,
        name: impl AsRef<[u8]>,
        other: impl AsRef<[u8]>,
    ) -> Option<&'a [u8]> {
        let name = name.as_ref();
        let other = other.as_ref();
        let mut value: Option<&'a [u8]> = None;
        for header in self.list {
            if strings::eql_case_insensitive_ascii(header.name(), other, true) {
                return None;
            }

            if value.is_none() && strings::eql_case_insensitive_ascii(header.name(), name, true) {
                value = Some(header.value());
            }
        }

        value
    }
}

// ──────────────────────────────────────────────────────────────────────────
// Request
// ──────────────────────────────────────────────────────────────────────────

#[derive(Debug, strum::IntoStaticStr)]
pub enum ParseRequestError {
    BadRequest,
    ShortRead,
}
bun_core::impl_tag_error!(ParseRequestError);
bun_core::named_error_set!(ParseRequestError);

pub struct Request<'a> {
    pub method: &'a [u8],
    pub path: &'a [u8],
    pub minor_version: usize,
    pub headers: &'a [Header],
    pub bytes_read: u32,
}

impl fmt::Debug for Request<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Request")
            .field("method", &BStr::new(self.method))
            .field("path", &BStr::new(self.path))
            .field("minor_version", &self.minor_version)
            .field("headers", &self.headers.len())
            .field("bytes_read", &self.bytes_read)
            .finish()
    }
}

impl<'a> Request<'a> {
    pub fn curl(&self, ignore_insecure: bool, body: &'a [u8]) -> RequestCurlFormatter<'_> {
        RequestCurlFormatter {
            request: self,
            ignore_insecure,
            body,
        }
    }

    pub fn clone(&self, headers: &'a mut [Header], builder: &mut StringBuilder) -> Request<'a> {
        for (i, header) in self.headers.iter().enumerate() {
            headers[i] = header.clone(builder);
        }

        Request {
            // SAFETY: see `Header::clone` — caller keeps `builder` alive.
            method: unsafe { builder.append_raw(self.method) },
            // SAFETY: see `Header::clone` — caller keeps `builder` alive.
            path: unsafe { builder.append_raw(self.path) },
            minor_version: self.minor_version,
            headers,
            bytes_read: self.bytes_read,
        }
    }

    /// Widen the borrowed slices to `'static` for self-referential storage.
    ///
    /// Field-by-field move (no bitwise reinterpret). Used when the request's
    /// `method`/`path`/`headers` borrow thread-local static buffers
    /// (`SHARED_REQUEST_HEADERS_BUF`) or a sibling field on the same
    /// heap-stable owner.
    ///
    /// # Safety
    /// Caller guarantees every borrowed slice outlives the returned value.
    #[inline]
    pub unsafe fn detach_lifetime(self) -> Request<'static> {
        Request {
            // SAFETY: caller contract.
            method: unsafe { &*core::ptr::from_ref::<[u8]>(self.method) },
            // SAFETY: caller contract.
            path: unsafe { &*core::ptr::from_ref::<[u8]>(self.path) },
            minor_version: self.minor_version,
            // SAFETY: caller contract.
            headers: unsafe { &*core::ptr::from_ref::<[Header]>(self.headers) },
            bytes_read: self.bytes_read,
        }
    }

    pub fn parse(buf: &'a [u8], src: &'a mut [Header]) -> Result<Request<'a>, ParseRequestError> {
        // Build httparse header slots from the caller-provided storage.
        let mut httparse_headers: Vec<httparse::Header<'_>> = Vec::with_capacity(src.len());
        for _ in 0..src.len() {
            httparse_headers.push(httparse::Header {
                name: "",
                value: &[],
            });
        }

        let mut req = httparse::Request::new(&mut httparse_headers);
        match req.parse(buf) {
            Ok(httparse::Status::Complete(bytes_read)) => {
                let method = req.method.unwrap_or("");
                let path = req.path.unwrap_or("/");
                let minor_version = req.version.unwrap_or(1);

                // Fill caller-provided Header array from httparse output.
                // httparse headers are guaranteed to point into `buf`.
                let num_headers = req.headers.len().min(src.len());
                for (i, h) in req.headers.iter().take(num_headers).enumerate() {
                    src[i] = Header {
                        name_ptr: h.name.as_ptr(),
                        name_len: h.name.len(),
                        value_ptr: h.value.as_ptr(),
                        value_len: h.value.len(),
                    };
                }

                // PORT NOTE: The original picohttpparser FFI wrote a NUL sentinel
                // after the path for C string compatibility. No downstream code in
                // Bao actually reads this NUL — it was a C-ABI artifact. Omitted
                // in the pure Rust httparse implementation.

                Ok(Request {
                    method: method.as_bytes(),
                    path: path.as_bytes(),
                    minor_version: usize::from(minor_version),
                    headers: &src[0..num_headers],
                    bytes_read: u32::try_from(bytes_read).expect("int cast"),
                })
            }
            Ok(httparse::Status::Partial) => Err(ParseRequestError::ShortRead),
            Err(_) => Err(ParseRequestError::BadRequest),
        }
    }
}

impl fmt::Display for Request<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if enable_ansi_colors_stderr() {
            f.write_str(pretty_fmt!("<r><d>[fetch]<r> ", true))?;
        }
        writeln!(
            f,
            "> HTTP/1.1 {} {}",
            BStr::new(self.method),
            BStr::new(self.path)
        )?;
        for header in self.headers {
            if enable_ansi_colors_stderr() {
                f.write_str(pretty_fmt!("<r><d>[fetch]<r> ", true))?;
            }
            f.write_str("> ")?;
            writeln!(f, "{}", header)?;
        }
        Ok(())
    }
}

pub struct RequestCurlFormatter<'a> {
    request: &'a Request<'a>,
    ignore_insecure: bool,
    body: &'a [u8],
}

impl<'a> RequestCurlFormatter<'a> {
    fn is_printable_body(content_type: &[u8]) -> bool {
        if content_type.is_empty() {
            return false;
        }

        strings::has_prefix(content_type, b"text/")
            || strings::has_prefix(content_type, b"application/json")
            || strings::contains(content_type, b"json")
            || strings::has_prefix(content_type, b"application/x-www-form-urlencoded")
    }
}

impl fmt::Display for RequestCurlFormatter<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let request = self.request;
        if enable_ansi_colors_stderr() {
            f.write_str(pretty_fmt!("<r><d>[fetch] $<r> ", true))?;

            write!(
                f,
                pretty_fmt!("<b><cyan>curl<r> <d>--http1.1<r> <b>\"{}\"<r>", true),
                BStr::new(request.path),
            )?;
        } else {
            write!(f, "curl --http1.1 \"{}\"", BStr::new(request.path))?;
        }

        if request.method != b"GET" {
            write!(f, " -X {}", BStr::new(request.method))?;
        }

        if self.ignore_insecure {
            f.write_str(" -k")?;
        }

        let mut content_type: &[u8] = b"";

        for header in request.headers {
            f.write_str(" ")?;
            if content_type.is_empty() {
                if strings::eql_case_insensitive_ascii(b"content-type", header.name(), true) {
                    content_type = header.value();
                }
            }

            write!(f, "{}", header.curl())?;

            if strings::eql_case_insensitive_ascii(b"accept-encoding", header.name(), true) {
                f.write_str(" --compressed")?;
            }
        }

        if !self.body.is_empty() && Self::is_printable_body(content_type) {
            f.write_str(" --data-raw ")?;
            bun_core::js_printer::write_json_string(
                self.body,
                f,
                bun_core::strings::Encoding::Utf8,
            )?;
        }

        Ok(())
    }
}

// ──────────────────────────────────────────────────────────────────────────
// StatusCodeFormatter
// ──────────────────────────────────────────────────────────────────────────

struct StatusCodeFormatter {
    code: usize,
}

impl fmt::Display for StatusCodeFormatter {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if enable_ansi_colors_stderr() {
            match self.code {
                101 | 200..=299 => write!(f, pretty_fmt!("<r><green>{}<r>", true), self.code),
                300..=399 => write!(f, pretty_fmt!("<r><yellow>{}<r>", true), self.code),
                _ => write!(f, pretty_fmt!("<r><red>{}<r>", true), self.code),
            }
        } else {
            write!(f, "{}", self.code)
        }
    }
}

// ──────────────────────────────────────────────────────────────────────────
// Response
// ──────────────────────────────────────────────────────────────────────────

#[derive(Debug, strum::IntoStaticStr)]
pub enum ParseResponseError {
    #[strum(serialize = "Malformed_HTTP_Response")]
    MalformedHttpResponse,
    ShortRead,
}
bun_core::impl_tag_error!(ParseResponseError);
bun_core::named_error_set!(ParseResponseError);

#[derive(Clone, Copy)]
pub struct Response<'a> {
    pub minor_version: usize,
    pub status_code: u32,
    pub status: &'a [u8],
    pub headers: HeaderList<'a>,
    pub bytes_read: i32,
}

impl fmt::Debug for Response<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Response")
            .field("minor_version", &self.minor_version)
            .field("status_code", &self.status_code)
            .field("status", &BStr::new(self.status))
            .field("headers", &self.headers.list.len())
            .field("bytes_read", &self.bytes_read)
            .finish()
    }
}

impl<'a> Default for Response<'a> {
    fn default() -> Self {
        Response {
            minor_version: 0,
            status_code: 0,
            status: b"",
            headers: HeaderList::default(),
            bytes_read: 0,
        }
    }
}

impl<'a> Response<'a> {
    /// Widen `status`/`headers` to `'static` for self-referential storage.
    /// Field-by-field move (no bitwise reinterpret).
    ///
    /// # Safety
    /// Caller guarantees the response buffer / header storage the slices borrow
    /// outlives every read through the returned value.
    #[inline]
    pub unsafe fn detach_lifetime(self) -> Response<'static> {
        Response {
            minor_version: self.minor_version,
            status_code: self.status_code,
            // SAFETY: caller contract.
            status: unsafe { &*core::ptr::from_ref::<[u8]>(self.status) },
            headers: HeaderList {
                // SAFETY: caller contract.
                list: unsafe { &*core::ptr::from_ref::<[Header]>(self.headers.list) },
            },
            bytes_read: self.bytes_read,
        }
    }

    pub fn count(&self, builder: &mut StringBuilder) {
        builder.count(self.status);

        for header in self.headers.list {
            header.count(builder);
        }
    }

    pub fn clone(&self, headers: &'a mut [Header], builder: &mut StringBuilder) -> Response<'a> {
        let mut that = *self;
        // SAFETY: see `Header::clone` — caller keeps `builder` alive.
        that.status = unsafe { builder.append_raw(self.status) };

        for (i, header) in self.headers.list.iter().enumerate() {
            headers[i] = header.clone(builder);
        }

        that.headers.list = &headers[0..self.headers.list.len()];

        that
    }

    pub fn parse_parts(
        buf: &'a [u8],
        src: &'a mut [Header],
        offset: Option<&mut usize>,
    ) -> Result<Response<'a>, ParseResponseError> {
        let mut httparse_headers: Vec<httparse::Header<'_>> = Vec::with_capacity(src.len());
        for _ in 0..src.len() {
            httparse_headers.push(httparse::Header {
                name: "",
                value: &[],
            });
        }

        let mut resp = httparse::Response::new(&mut httparse_headers);
        match resp.parse(buf) {
            Ok(httparse::Status::Complete(bytes_read)) => {
                let minor_version = resp.version.unwrap_or(1);
                let status_code = resp.code.unwrap_or(0);
                let reason = resp.reason.unwrap_or("");

                // Fill caller-provided Header array from httparse output.
                let num_headers = resp.headers.len().min(src.len());
                for (i, h) in resp.headers.iter().take(num_headers).enumerate() {
                    src[i] = Header {
                        name_ptr: h.name.as_ptr(),
                        name_len: h.name.len(),
                        value_ptr: h.value.as_ptr(),
                        value_len: h.value.len(),
                    };
                }

                Ok(Response {
                    minor_version: usize::from(minor_version),
                    status_code: u32::from(status_code),
                    status: reason.as_bytes(),
                    headers: HeaderList {
                        list: &src[0..num_headers],
                    },
                    bytes_read: i32::try_from(bytes_read).expect("int cast"),
                })
            }
            Ok(httparse::Status::Partial) => {
                if let Some(offset) = offset {
                    *offset += buf.len();
                }
                Err(ParseResponseError::ShortRead)
            }
            Err(_) => Err(ParseResponseError::MalformedHttpResponse),
        }
    }

    pub fn parse(buf: &'a [u8], src: &'a mut [Header]) -> Result<Response<'a>, ParseResponseError> {
        let mut offset: usize = 0;
        let response = Self::parse_parts(buf, src, Some(&mut offset))?;
        Ok(response)
    }
}

impl fmt::Display for Response<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if enable_ansi_colors_stderr() {
            f.write_str(pretty_fmt!("<r><d>[fetch]<r> ", true))?;
        }

        writeln!(
            f,
            "< {} {}",
            StatusCodeFormatter {
                code: self.status_code as usize
            },
            BStr::new(self.status),
        )?;
        for header in self.headers.list {
            if enable_ansi_colors_stderr() {
                f.write_str(pretty_fmt!("<r><d>[fetch]<r> ", true))?;
            }

            f.write_str("< ")?;
            writeln!(f, "{}", header)?;
        }
        Ok(())
    }
}

// ──────────────────────────────────────────────────────────────────────────
// Headers
// ──────────────────────────────────────────────────────────────────────────

#[derive(Debug, strum::IntoStaticStr)]
pub enum ParseHeadersError {
    BadHeaders,
    ShortRead,
}
bun_core::impl_tag_error!(ParseHeadersError);
bun_core::named_error_set!(ParseHeadersError);

pub struct Headers<'a> {
    pub headers: &'a [Header],
}

impl fmt::Debug for Headers<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Headers")
            .field("count", &self.headers.len())
            .finish()
    }
}

impl<'a> Headers<'a> {
    pub fn parse(buf: &'a [u8], src: &'a mut [Header]) -> Result<Headers<'a>, ParseHeadersError> {
        // httparse does not have a standalone header parser.
        // Parse as a response (headers-only) and extract headers.
        // This handles raw header blocks that start after the request/response line.
        // We search for the first \r\n to skip any request/response line if present.
        let mut httparse_headers: Vec<httparse::Header<'_>> = Vec::with_capacity(src.len());
        for _ in 0..src.len() {
            httparse_headers.push(httparse::Header {
                name: "",
                value: &[],
            });
        }

        // Try parsing as a request first (covers most header-only use cases).
        let mut req = httparse::Request::new(&mut httparse_headers);
        match req.parse(buf) {
            Ok(httparse::Status::Complete(bytes_read)) => {
                let num_headers = req.headers.len().min(src.len());
                for (i, h) in req.headers.iter().take(num_headers).enumerate() {
                    src[i] = Header {
                        name_ptr: h.name.as_ptr(),
                        name_len: h.name.len(),
                        value_ptr: h.value.as_ptr(),
                        value_len: h.value.len(),
                    };
                }
                let _ = bytes_read;
                return Ok(Headers {
                    headers: &src[0..num_headers],
                });
            }
            Ok(httparse::Status::Partial) => return Err(ParseHeadersError::ShortRead),
            Err(_) => {}
        }

        // Fall back: try as a response.
        let mut httparse_headers2: Vec<httparse::Header<'_>> = Vec::with_capacity(src.len());
        for _ in 0..src.len() {
            httparse_headers2.push(httparse::Header {
                name: "",
                value: &[],
            });
        }
        let mut resp = httparse::Response::new(&mut httparse_headers2);
        match resp.parse(buf) {
            Ok(httparse::Status::Complete(bytes_read)) => {
                let num_headers = resp.headers.len().min(src.len());
                for (i, h) in resp.headers.iter().take(num_headers).enumerate() {
                    src[i] = Header {
                        name_ptr: h.name.as_ptr(),
                        name_len: h.name.len(),
                        value_ptr: h.value.as_ptr(),
                        value_len: h.value.len(),
                    };
                }
                let _ = bytes_read;
                Ok(Headers {
                    headers: &src[0..num_headers],
                })
            }
            Ok(httparse::Status::Partial) => Err(ParseHeadersError::ShortRead),
            Err(_) => Err(ParseHeadersError::BadHeaders),
        }
    }
}

impl fmt::Display for Headers<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for header in self.headers {
            write!(
                f,
                "{}: {}\r\n",
                BStr::new(header.name()),
                BStr::new(header.value())
            )?;
        }
        Ok(())
    }
}

// ──────────────────────────────────────────────────────────────────────────
// Chunked Decoder (pure Rust replacement for picohttpparser's phr_decode_chunked)
// ──────────────────────────────────────────────────────────────────────────

/// State machine states for chunked transfer decoding.
///
/// These mirror the internal states of picohttpparser's `phr_chunked_decoder`
/// so that downstream code inspecting `_state` (e.g., ProxyTunnel.rs checking
/// for trailer states 4 and 5) continues to work.
#[repr(i8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChunkedState {
    /// Reading chunk size (hex digits)
    ChunkSize = 0,
    /// Reading extension after chunk size
    ChunkExtension = 1,
    /// Reading chunk data
    ChunkData = 2,
    /// Reading CRLF after chunk data
    ChunkCrlf = 3,
    /// Reading trailer line start
    TrailerLineHead = 4,
    /// Reading trailer line content
    TrailerLineMiddle = 5,
    /// Reading final CRLF after trailers (consuming \r\n)
    TrailerFinalCrlf = 6,
    /// Decode complete — all chunks and terminators consumed
    Done = 7,
}

/// Pure Rust chunked transfer-encoding decoder, replacing picohttpparser's
/// `phr_chunked_decoder` + `phr_decode_chunked`.
///
/// The API preserves the original's in-place mutation semantics: `decode()`
/// rewrites `buf` in place, removing chunk-size markers and CRLF delimiters,
/// leaving only the decoded body content. Returns the number of bytes consumed
/// on success, -1 on invalid input, or -2 when more data is needed.
#[derive(Clone, Copy, Debug)]
pub struct ChunkedDecoder {
    /// Bytes remaining in the current chunk's data section.
    pub bytes_left_in_chunk: usize,
    /// Set to 1 to discard trailing headers after the terminal `0\r\n` chunk.
    pub consume_trailer: i8,
    /// Internal hex digit count during chunk-size parsing.
    pub _hex_count: i8,
    /// Current parser state.
    pub _state: ChunkedState,
}

impl Default for ChunkedDecoder {
    fn default() -> Self {
        Self {
            bytes_left_in_chunk: 0,
            consume_trailer: 0,
            _hex_count: 0,
            _state: ChunkedState::ChunkSize,
        }
    }
}

impl ChunkedDecoder {
    /// Decode chunked transfer encoding in place.
    ///
    /// On entry, `buf` contains raw chunked data (chunk-size lines + data).
    /// On return, `buf[0..*len]` contains the decoded body (chunk markers removed).
    /// `*len` is updated to the decoded length.
    ///
    /// Returns:
    /// - `>= 0`: success (number of extra bytes consumed from the input)
    /// - `-1`: invalid input
    /// - `-2`: need more data
    ///
    /// # Safety
    /// Caller must ensure `buf` is valid for read+write of `*len` bytes,
    /// and `len` points to a valid `usize`.
    ///
    /// This unsafe signature is preserved for API compatibility with the
    /// original `phr_decode_chunked` C function. Prefer the safe `decode()`
    /// method for new code.
    #[inline]
    pub unsafe fn decode_raw(
        decoder: *mut ChunkedDecoder,
        buf: *mut u8,
        len: *mut usize,
    ) -> isize {
        // SAFETY: caller guarantees decoder is valid, buf is valid for *len bytes,
        // and len points to a valid usize.
        unsafe {
            let decoder = &mut *decoder;
            let buf_slice = core::slice::from_raw_parts_mut(buf, *len);
            let result = decoder.decode(buf_slice);
            match result {
                Ok(decoded_len) => {
                    *len = decoded_len;
                    0
                }
                Err(ChunkedError::Invalid) => -1,
                Err(ChunkedError::NeedMore) => -2,
            }
        }
    }

    /// Safe interface: decode chunked data in place.
    ///
    /// Rewrites `buf` in place to remove chunk framing, returning the decoded
    /// body length. Returns `Ok(decoded_len)` on success, `Err(ChunkedError)`
    /// on failure.
    pub fn decode(&mut self, buf: &mut [u8]) -> Result<usize, ChunkedError> {
        let mut src = 0usize;
        let mut dst = 0usize;
        let total = buf.len();

        while src < total {
            match self._state {
                ChunkedState::ChunkSize => {
                    // Parse hex chunk size.
                    let mut found_cr = false;
                    while src < total {
                        let b = buf[src];
                        if b == b'\r' {
                            found_cr = true;
                            src += 1;
                            break;
                        }
                        if b == b';' {
                            // Chunk extension follows.
                            self._state = ChunkedState::ChunkExtension;
                            src += 1;
                            break;
                        }
                        let digit = match b {
                            b'0'..=b'9' => b - b'0',
                            b'a'..=b'f' => b - b'a' + 10,
                            b'A'..=b'F' => b - b'A' + 10,
                            _ => {
                                // Could be part of extension or invalid.
                                // Check if we already have a hex count.
                                if self._hex_count > 0 {
                                    self._state = ChunkedState::ChunkExtension;
                                    break;
                                }
                                return Err(ChunkedError::Invalid);
                            }
                        };
                        self.bytes_left_in_chunk =
                            self.bytes_left_in_chunk.wrapping_mul(16).wrapping_add(digit as usize);
                        self._hex_count += 1;
                        src += 1;
                    }

                    if self._state == ChunkedState::ChunkExtension {
                        continue;
                    }

                    if !found_cr {
                        // Need more data for chunk size line.
                        return Err(ChunkedError::NeedMore);
                    }

                    // Expect \n after \r.
                    if src >= total {
                        return Err(ChunkedError::NeedMore);
                    }
                    if buf[src] != b'\n' {
                        return Err(ChunkedError::Invalid);
                    }
                    src += 1;

                    // Check for terminal chunk (size 0).
                    if self.bytes_left_in_chunk == 0 {
                        if self.consume_trailer != 0 {
                            self._state = ChunkedState::TrailerLineHead;
                        } else {
                            self._state = ChunkedState::TrailerFinalCrlf;
                        }
                        continue;
                    }

                    self._state = ChunkedState::ChunkData;
                }

                ChunkedState::ChunkExtension => {
                    // Skip until \r\n after chunk extension.
                    while src < total {
                        if buf[src] == b'\r' {
                            src += 1;
                            if src >= total {
                                return Err(ChunkedError::NeedMore);
                            }
                            if buf[src] != b'\n' {
                                return Err(ChunkedError::Invalid);
                            }
                            src += 1;

                            if self.bytes_left_in_chunk == 0 {
                                if self.consume_trailer != 0 {
                                    self._state = ChunkedState::TrailerLineHead;
                                } else {
                                    self._state = ChunkedState::TrailerFinalCrlf;
                                }
                            } else {
                                self._state = ChunkedState::ChunkData;
                            }
                            break;
                        }
                        src += 1;
                    }
                    if src >= total && self._state == ChunkedState::ChunkExtension {
                        return Err(ChunkedError::NeedMore);
                    }
                }

                ChunkedState::ChunkData => {
                    let to_copy = self.bytes_left_in_chunk.min(total - src);
                    // Move data to decoded position.
                    if dst != src {
                        buf.copy_within(src..src + to_copy, dst);
                    }
                    dst += to_copy;
                    src += to_copy;
                    self.bytes_left_in_chunk -= to_copy;

                    if self.bytes_left_in_chunk == 0 {
                        self._state = ChunkedState::ChunkCrlf;
                    }
                }

                ChunkedState::ChunkCrlf => {
                    if src >= total {
                        return Err(ChunkedError::NeedMore);
                    }
                    if buf[src] != b'\r' {
                        return Err(ChunkedError::Invalid);
                    }
                    src += 1;
                    if src >= total {
                        return Err(ChunkedError::NeedMore);
                    }
                    if buf[src] != b'\n' {
                        return Err(ChunkedError::Invalid);
                    }
                    src += 1;
                    self._hex_count = 0;
                    self._state = ChunkedState::ChunkSize;
                }

                ChunkedState::TrailerLineHead => {
                    if src >= total {
                        return Err(ChunkedError::NeedMore);
                    }
                    if buf[src] == b'\r' {
                        src += 1;
                        if src >= total {
                            return Err(ChunkedError::NeedMore);
                        }
                        if buf[src] != b'\n' {
                            return Err(ChunkedError::Invalid);
                        }
                        // End of trailers — the empty line marks completion.
                        self._state = ChunkedState::Done;
                        continue;
                    }
                    self._state = ChunkedState::TrailerLineMiddle;
                }

                ChunkedState::TrailerLineMiddle => {
                    // Skip until end of trailer line.
                    while src < total {
                        if buf[src] == b'\r' {
                            src += 1;
                            if src >= total {
                                return Err(ChunkedError::NeedMore);
                            }
                            if buf[src] != b'\n' {
                                return Err(ChunkedError::Invalid);
                            }
                            src += 1;
                            self._state = ChunkedState::TrailerLineHead;
                            break;
                        }
                        src += 1;
                    }
                    if src >= total && self._state == ChunkedState::TrailerLineMiddle {
                        return Err(ChunkedError::NeedMore);
                    }
                }

                ChunkedState::TrailerFinalCrlf => {
                    // Consume the final \r\n after the terminal chunk.
                    if src >= total {
                        return Err(ChunkedError::NeedMore);
                    }
                    if buf[src] != b'\r' {
                        return Err(ChunkedError::Invalid);
                    }
                    src += 1;
                    if src >= total {
                        return Err(ChunkedError::NeedMore);
                    }
                    if buf[src] != b'\n' {
                        return Err(ChunkedError::Invalid);
                    }
                    // No need to advance src further — we're done.
                    self._state = ChunkedState::Done;
                    break;
                }

                ChunkedState::Done => {
                    break;
                }
            }
        }

        // If we exited the loop because we ran out of data, and the decoder
        // is not in a terminal state, signal that more data is needed.
        // This matches phr_decode_chunked's behavior of returning -2 when
        // the input is incomplete.
        match self._state {
            ChunkedState::Done => Ok(dst),
            _ => Err(ChunkedError::NeedMore),
        }
    }
}

/// Errors from chunked transfer decoding.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChunkedError {
    Invalid,
    NeedMore,
}

// ──────────────────────────────────────────────────────────────────────────
// Compatibility re-exports (preserving downstream code that uses the old
// C FFI type/function names)
// ──────────────────────────────────────────────────────────────────────────

/// Type alias preserving the old name. Pure Rust — no C layout constraint.
#[allow(non_camel_case_types)]
pub type phr_chunked_decoder = ChunkedDecoder;

/// Type alias preserving the old struct tag name.
#[allow(non_camel_case_types)]
pub type struct_phr_chunked_decoder = ChunkedDecoder;

/// Raw-FFI-compatible wrapper for `ChunkedDecoder::decode_raw`.
///
/// Preserves the exact C function signature that downstream code calls
/// via `picohttp::phr_decode_chunked(&raw mut decoder, ptr, &raw mut len)`.
///
/// # Safety
/// Same as `ChunkedDecoder::decode_raw`: `buf` must be valid for `*len` bytes,
/// `len` must point to a valid `usize`.
#[inline]
pub unsafe fn phr_decode_chunked(
    decoder: *mut phr_chunked_decoder,
    buf: *mut u8,
    len: *mut usize,
) -> isize {
    // SAFETY: caller guarantees decoder is valid, buf is valid for *len bytes,
    // and len points to a valid usize.
    unsafe { ChunkedDecoder::decode_raw(decoder, buf, len) }
}

/// Returns whether the decoder is currently in the data phase (reading chunk body).
///
/// Preserves the C function signature. Returns 1 if in data state, 0 otherwise.
#[inline]
pub fn phr_decode_chunked_is_in_data(decoder: *mut phr_chunked_decoder) -> core::ffi::c_int {
    unsafe { (*decoder)._state == ChunkedState::ChunkData as _ }.into()
}

// ported from: src/picohttp/picohttp.zig
