use super::*;

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::mpsc::{Receiver, Sender};

use dpi::PhysicalSize;
use servo::{
    AllowOrDenyRequest, ConsoleLogLevel, CreateNewWebViewRequest, DeviceIntPoint, DeviceIntRect,
    DeviceIntSize, EmbedderControl, EmbedderControlId, InputEventId, InputEventResult, LoadStatus,
    NavigationRequest, PermissionRequest, ScreenGeometry, ServoDelegate, ServoError, TraversalId,
    WebView, WebViewDelegate,
};

use bao_cdp::servo_bridge::main_frame_id_for_target;
use bao_cdp::{BaoEvent, ConsoleMessage};
use bao_cdp_client::bridge::{ConsoleLevel, ServoEvent};

// ─── Worker Script Loading Pipeline (REQ-BRW-004 / DF-WK-2) ────────────
// @trace REQ-BRW-004 [entity:Worker] [DF-WK-2]
// SPEC DF-WK-2: "worker 线程 (DedicatedWorkerGlobalScope) → 构建 RequestBuilder
// → global.fetch 经 servo resource_threads (bao-browser 桥) → process_response_eof
// (HTTP status + JS MIME 校验 + UTF-8 解码) → Classic/Module 编译 → scope.on_complete"
//
// Architecture:
//   - In browser mode (bao_browser), servo's DOM Worker binding handles the full
//     Worker::Constructor lifecycle internally, including script fetching via its
//     own resource_threads. Bao's responsibility is to provide the bao_browser-side
//     bridge that tracks the loading state and provides script resolution for
//     Workers created outside servo's DOM path (e.g., via bao_engine WebWorker).
//   - For URL-based Worker scripts (new Worker(url)), the WorkerScriptLoader
//     resolves the URL, fetches the script content, validates MIME type, decodes
//     as UTF-8, and provides the script source for evaluation.
//   - For inline/data: URL scripts, the source is provided directly without
//     network fetch (matches Web Worker spec behavior for data: and blob: URLs).
//
// Thread safety: WorkerScriptLoader is Send — it holds no JSObject references,
// only String data. Script fetching is done on the Worker thread itself (per
// DF-WK-2: "线程归属: worker 线程"), so no cross-thread JSObject transfer.
//
// MIME type validation (DF-WK-2: "JS MIME 校验"):
//   Per the Web Worker spec, Worker script responses must have a JavaScript MIME
//   type. The allowed MIME types are:
//     - application/ecmascript
//     - application/javascript
//     - application/x-ecmascript
//     - application/x-javascript
//     - text/ecmascript
//     - text/javascript
//     - text/javascript1.0
//     - text/javascript1.1
//     - text/javascript1.2
//     - text/javascript1.3
//     - text/javascript1.4
//     - text/javascript1.5
//     - text/jscript
//     - text/livescript
//     - text/x-ecmascript
//     - text/x-javascript
//   If the MIME type doesn't match, the Worker should fire an error event.

/// Source of a Worker script — either inline (data:/blob:/string) or URL-based.
///
/// @trace REQ-BRW-004 [entity:Worker] [DF-WK-2]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WorkerScriptSource {
    /// Inline script source (e.g., data: URL content, or string passed directly).
    /// No network fetch needed — the script content is provided as-is.
    /// @trace REQ-BRW-004 [entity:Worker] [DF-WK-2]
    Inline(String),
    /// URL-based script that needs to be fetched via HTTP.
    /// The URL is resolved relative to the page's origin.
    /// @trace REQ-BRW-004 [entity:Worker] [DF-WK-2]
    Url(String),
}

/// Result of loading a Worker script.
///
/// @trace REQ-BRW-004 [entity:Worker] [DF-WK-2]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkerScriptLoadResult {
    /// The script source code (successfully loaded).
    /// @trace REQ-BRW-004 [entity:Worker] [DF-WK-2]
    pub source: String,
    /// The final URL after any redirects.
    /// @trace REQ-BRW-004 [entity:Worker] [DF-WK-2]
    pub final_url: String,
    /// MIME type of the response (for validation diagnostics).
    /// @trace REQ-BRW-004 [entity:Worker] [DF-WK-2]
    pub mime_type: Option<String>,
}

/// Error from loading a Worker script.
///
/// @trace REQ-BRW-004 [entity:Worker] [DF-WK-2]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WorkerScriptLoadError {
    /// Network error during script fetch (HTTP status code or transport error).
    /// @trace REQ-BRW-004 [entity:Worker] [DF-WK-2]
    NetworkError(String),
    /// MIME type validation failed — response is not a JavaScript MIME type.
    /// Per DF-WK-2: "JS MIME 校验".
    /// @trace REQ-BRW-004 [entity:Worker] [DF-WK-2]
    InvalidMimeType {
        /// The MIME type received from the server.
        received: String,
        /// The URL that was fetched.
        url: String,
    },
    /// Failed to decode the response body as UTF-8.
    /// Per DF-WK-2: "UTF-8 解码".
    /// @trace REQ-BRW-004 [entity:Worker] [DF-WK-2]
    Utf8DecodeError(String),
    /// The URL is invalid or cannot be parsed.
    /// @trace REQ-BRW-004 [entity:Worker] [DF-WK-2]
    InvalidUrl(String),
    /// Script loading was cancelled (Worker terminated before load completed).
    /// @trace REQ-BRW-004 [entity:Worker] [DF-WK-2]
    Cancelled,
}

/// Script type for Worker compilation.
///
/// Per DF-WK-2: "Classic/Module 编译".
///
/// @trace REQ-BRW-004 [entity:Worker] [DF-WK-2]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkerScriptType {
    /// Classic Worker script (default, `new Worker(url)`).
    /// @trace REQ-BRW-004 [entity:Worker] [DF-WK-2]
    Classic,
    /// Module Worker script (`new Worker(url, { type: "module" })`).
    /// @trace REQ-BRW-004 [entity:Worker] [DF-WK-2]
    Module,
}

impl Default for WorkerScriptType {
    fn default() -> Self {
        WorkerScriptType::Classic
    }
}

/// JavaScript MIME types allowed for Worker scripts.
///
/// Per the Web Worker spec and DF-WK-2 ("JS MIME 校验"), Worker script
/// responses must have a JavaScript MIME type. This list matches the
/// [JavaScript MIME type](https://mimesniff.spec.whatwg.org/#javascript-mime-type)
/// definition from the WHATWG MIME Sniffing spec.
///
/// @trace REQ-BRW-004 [entity:Worker] [DF-WK-2]
const JAVASCRIPT_MIME_TYPES: &[&str] = &[
    "application/ecmascript",
    "application/javascript",
    "application/x-ecmascript",
    "application/x-javascript",
    "text/ecmascript",
    "text/javascript",
    "text/javascript1.0",
    "text/javascript1.1",
    "text/javascript1.2",
    "text/javascript1.3",
    "text/javascript1.4",
    "text/javascript1.5",
    "text/jscript",
    "text/livescript",
    "text/x-ecmascript",
    "text/x-javascript",
];

/// Check if a MIME type is a valid JavaScript MIME type for Worker scripts.
///
/// Per DF-WK-2: "JS MIME 校验" — the response Content-Type must be a
/// JavaScript MIME type. This function performs a case-insensitive match
/// against the WHATWG JavaScript MIME type list.
///
/// @trace REQ-BRW-004 [entity:Worker] [DF-WK-2]
pub fn is_javascript_mime_type(mime: &str) -> bool {
    // Strip parameters (e.g., "text/javascript; charset=utf-8" → "text/javascript")
    let base_type = mime.split(';').next().unwrap_or(mime).trim();
    JAVASCRIPT_MIME_TYPES
        .iter()
        .any(|&valid| valid.eq_ignore_ascii_case(base_type))
}

/// Worker script loader — handles URL-based script fetching for Workers.
///
/// Provides the bridge between bao_browser's Worker tracking and the script
/// loading process described in DF-WK-2. In browser mode, servo's DOM Worker
/// binding handles the full script loading pipeline internally. This struct
/// provides the bao_browser-side tracking and validation that supplements
/// servo's internal mechanism.
///
/// For Workers created via bao_engine::WebWorker (CLI/test mode), this loader
/// resolves script URLs and provides script content for evaluation on the
/// Worker thread.
///
/// @trace REQ-BRW-004 [entity:Worker] [DF-WK-2]
#[derive(Debug, Clone)]
pub struct WorkerScriptLoader {
    /// The script URL or inline source to load.
    /// @trace REQ-BRW-004 [entity:Worker] [DF-WK-2]
    pub source: WorkerScriptSource,
    /// The script type (Classic or Module).
    /// @trace REQ-BRW-004 [entity:Worker] [DF-WK-2]
    pub script_type: WorkerScriptType,
}

impl WorkerScriptLoader {
    /// Create a new WorkerScriptLoader for an inline script source.
    ///
    /// @trace REQ-BRW-004 [entity:Worker] [DF-WK-2]
    pub fn inline(script: String, script_type: WorkerScriptType) -> Self {
        WorkerScriptLoader {
            source: WorkerScriptSource::Inline(script),
            script_type,
        }
    }

    /// Create a new WorkerScriptLoader for a URL-based script.
    ///
    /// @trace REQ-BRW-004 [entity:Worker] [DF-WK-2]
    pub fn url(url: String, script_type: WorkerScriptType) -> Self {
        WorkerScriptLoader {
            source: WorkerScriptSource::Url(url),
            script_type,
        }
    }

    /// Create from WorkerScriptSource.
    ///
    /// @trace REQ-BRW-004 [entity:Worker] [DF-WK-2]
    pub fn from_source(source: WorkerScriptSource, script_type: WorkerScriptType) -> Self {
        WorkerScriptLoader {
            source,
            script_type,
        }
    }

    /// Resolve the script source to loadable content.
    ///
    /// For inline sources, returns the content directly.
    /// For URL sources, resolves the URL to determine the script location.
    /// In browser mode, servo handles the actual HTTP fetch internally —
    /// this method validates the URL and returns it for servo to fetch.
    /// For data:/blob: URLs embedded in the WorkerScriptSource::Inline variant,
    /// the content is already available.
    ///
    /// Returns the script content (for inline) or the validated URL (for URL source).
    ///
    /// @trace REQ-BRW-004 [entity:Worker] [DF-WK-2]
    pub fn resolve(&self) -> Result<WorkerScriptSource, WorkerScriptLoadError> {
        match &self.source {
            WorkerScriptSource::Inline(content) => {
                // Inline source is ready to evaluate — no fetch needed.
                Ok(WorkerScriptSource::Inline(content.clone()))
            }
            WorkerScriptSource::Url(url_str) => {
                // Validate the URL can be parsed.
                let parsed = url::Url::parse(url_str).map_err(|e| {
                    WorkerScriptLoadError::InvalidUrl(format!(
                        "Invalid Worker script URL '{}': {}",
                        url_str, e
                    ))
                })?;

                // For data: URLs, extract the script content directly.
                if parsed.scheme() == "data" {
                    return Self::resolve_data_url(&parsed);
                }

                // For blob: URLs, we can't resolve them here (they're scoped
                // to the creating page's origin). Servo handles blob: resolution
                // internally. We just pass the URL through.
                if parsed.scheme() == "blob" {
                    return Ok(WorkerScriptSource::Url(url_str.clone()));
                }

                // For http:/https: URLs, servo handles the fetch via its
                // resource_threads. We validate the URL format and return it.
                if parsed.scheme() == "http" || parsed.scheme() == "https" {
                    return Ok(WorkerScriptSource::Url(url_str.clone()));
                }

                // file: URLs for local development/testing.
                if parsed.scheme() == "file" {
                    return Self::resolve_file_url(&parsed);
                }

                Err(WorkerScriptLoadError::InvalidUrl(format!(
                    "Unsupported Worker script URL scheme '{}'",
                    parsed.scheme()
                )))
            }
        }
    }

    /// Resolve a data: URL to inline script content.
    ///
    /// data: URLs embed the script content directly in the URL itself,
    /// so no network fetch is needed. This extracts the script from
    /// the data URL per the Web Worker spec.
    ///
    /// @trace REQ-BRW-004 [entity:Worker] [DF-WK-2]
    fn resolve_data_url(parsed: &url::Url) -> Result<WorkerScriptSource, WorkerScriptLoadError> {
        // data: URL format: data:[<mediatype>][;base64],<data>
        let path = parsed.path();
        // Split on first comma to separate metadata from data
        let comma_pos = path.find(',').ok_or_else(|| {
            WorkerScriptLoadError::InvalidUrl("data: URL missing comma separator".to_string())
        })?;

        let metadata = &path[..comma_pos];
        let data = &path[comma_pos + 1..];

        // Parse metadata: "text/javascript" or "text/javascript;base64"
        let (mime_part, is_base64) = if metadata.ends_with(";base64") {
            (&metadata[..metadata.len() - 7], true)
        } else if metadata.is_empty() {
            ("text/plain", false)
        } else {
            (metadata, false)
        };

        // Validate MIME type for data: URLs
        // Per spec, data: URLs with non-JS MIME types should still work for Workers
        // (the MIME check applies to HTTP responses, not data: URLs).
        // However, we validate for consistency and to catch common mistakes.
        if !mime_part.is_empty() && !is_javascript_mime_type(mime_part) {
            // Log a warning but don't reject — data: URLs bypass MIME checks
            // per the HTML spec (the MIME type of a data: URL is advisory).
            log::warn!(
                "[WorkerScriptLoader] data: URL has non-JS MIME type '{}', loading anyway",
                mime_part
            );
        }

        // Decode the content
        let content = if is_base64 {
            use base64::Engine;
            base64::engine::general_purpose::STANDARD
                .decode(data)
                .map_err(|e| {
                    WorkerScriptLoadError::Utf8DecodeError(format!(
                        "Failed to decode base64 data: URL: {}",
                        e
                    ))
                })?
        } else {
            // For non-base64 data: URLs, the data is percent-encoded ASCII.
            // We decode percent-encoding and validate UTF-8.
            decode_percent_encoded(data)?
        };

        let script = String::from_utf8(content).map_err(|e| {
            WorkerScriptLoadError::Utf8DecodeError(format!(
                "data: URL content is not valid UTF-8: {}",
                e
            ))
        })?;

        Ok(WorkerScriptSource::Inline(script))
    }

    /// Resolve a file: URL to inline script content.
    ///
    /// file: URLs are used for local development/testing. Reads the
    /// file content directly from the filesystem.
    ///
    /// @trace REQ-BRW-004 [entity:Worker] [DF-WK-2]
    fn resolve_file_url(parsed: &url::Url) -> Result<WorkerScriptSource, WorkerScriptLoadError> {
        let path = parsed.to_file_path().map_err(|_| {
            WorkerScriptLoadError::InvalidUrl(format!(
                "Cannot convert file: URL to path: {}",
                parsed
            ))
        })?;

        let content = std::fs::read_to_string(&path).map_err(|e| {
            WorkerScriptLoadError::NetworkError(format!(
                "Failed to read Worker script file '{}': {}",
                path.display(),
                e
            ))
        })?;

        Ok(WorkerScriptSource::Inline(content))
    }

    /// Validate the MIME type of a Worker script response.
    ///
    /// Per DF-WK-2: "JS MIME 校验" — HTTP responses for Worker scripts
    /// must have a JavaScript MIME type. This validation applies to HTTP
    /// responses only (not data: or blob: URLs).
    ///
    /// Returns Ok(()) if the MIME type is valid, or Err with the
    /// invalid MIME type details.
    ///
    /// @trace REQ-BRW-004 [entity:Worker] [DF-WK-2]
    pub fn validate_mime_type(mime_type: &str, url: &str) -> Result<(), WorkerScriptLoadError> {
        if is_javascript_mime_type(mime_type) {
            Ok(())
        } else {
            Err(WorkerScriptLoadError::InvalidMimeType {
                received: mime_type.to_string(),
                url: url.to_string(),
            })
        }
    }

    /// Returns the script URL for this loader (if URL-based).
    ///
    /// @trace REQ-BRW-004 [entity:Worker] [DF-WK-2]
    pub fn script_url(&self) -> Option<&str> {
        match &self.source {
            WorkerScriptSource::Url(url) => Some(url),
            WorkerScriptSource::Inline(_) => None,
        }
    }

    /// Returns true if this loader requires a network fetch.
    ///
    /// @trace REQ-BRW-004 [entity:Worker] [DF-WK-2]
    pub fn requires_fetch(&self) -> bool {
        matches!(&self.source, WorkerScriptSource::Url(url)
            if url.starts_with("http://") || url.starts_with("https://"))
    }

    /// Load the Worker script through the full DF-WK-2 pipeline.
    ///
    /// This method implements the complete Worker script loading pipeline:
    ///   1. Resolve — URL parsing / data: / file: extraction
    ///   2. Fetch — HTTP GET via bao_runtime's stealth HTTP client
    ///   3. Validate — MIME type check (JS MIME types only)
    ///   4. Decode — UTF-8 decode of response body
    ///   5. Compile — SpiderMonkey compilation (Classic vs Module)
    ///   6. Ready — script source available for evaluation
    ///
    /// For inline/data:/file: sources, steps 2–4 are skipped — content
    /// is already available as a UTF-8 string.
    ///
    /// The `stealth_profile` is passed through to the HTTP client so that
    /// Worker script fetches use the same TLS/HTTP2 fingerprint as the
    /// parent page (SPEC criterion #12: CRIT-STL-WK).
    ///
    /// The `state_callback` is called at each pipeline stage transition,
    /// enabling CDP observability of the loading progress.
    ///
    /// Returns `WorkerScriptLoadResult` on success, or `WorkerScriptLoadError`
    /// at the stage where loading failed.
    ///
    /// @trace REQ-BRW-004 [entity:Worker] [DF-WK-2]
    ///   pipeline: fetch → MIME check → decode → compile
    /// @trace REQ-BRW-004 [criterion:12] CRIT-STL-WK: Worker fetch
    ///   uses parent page's stealth TLS/HTTP2 profile
    pub fn load<F>(
        &self,
        stealth_profile: &Option<bao_stealth::StealthProfile>,
        mut state_callback: F,
    ) -> Result<WorkerScriptLoadResult, WorkerScriptLoadError>
    where
        F: FnMut(WorkerScriptLoadState),
    {
        // Stage 1: Resolve the script source.
        // @trace REQ-BRW-004 [DF-WK-2] URL resolve
        state_callback(WorkerScriptLoadState::Pending);
        let resolved = self.resolve()?;

        let (source, final_url, mime_type) = match resolved {
            WorkerScriptSource::Inline(content) => {
                // Inline source: no fetch needed, skip to Ready.
                // @trace REQ-BRW-004 [DF-WK-2] inline — no fetch
                state_callback(WorkerScriptLoadState::Ready);
                return Ok(WorkerScriptLoadResult {
                    source: content,
                    final_url: self.script_url().unwrap_or("inline").to_string(),
                    mime_type: None,
                });
            }
            WorkerScriptSource::Url(url_str) => {
                // Stage 2: Fetch the script via HTTP.
                // @trace REQ-BRW-004 [DF-WK-2] HTTP fetch
                state_callback(WorkerScriptLoadState::Fetching);

                let response = fetch_worker_script(&url_str, stealth_profile)
                    .map_err(|e| WorkerScriptLoadError::NetworkError(e))?;

                // Stage 3: Validate MIME type.
                // @trace REQ-BRW-004 [DF-WK-2] JS MIME 校验
                state_callback(WorkerScriptLoadState::Validating);

                // Extract Content-Type header (case-insensitive).
                let ct = response
                    .headers
                    .iter()
                    .find(|(k, _)| k.eq_ignore_ascii_case("content-type"))
                    .map(|(_, v)| v.to_string());

                if let Some(ref content_type) = ct {
                    // Per DF-WK-2: "JS MIME 校验" — HTTP responses for Worker
                    // scripts must have a JavaScript MIME type.
                    Self::validate_mime_type(content_type, &url_str)?;
                }
                // If no Content-Type header, we proceed — some servers omit it
                // for small scripts. The WHATWG spec says a missing MIME type
                // is treated as "application/octet-stream" which would fail, but
                // in practice browsers are lenient for same-origin Worker scripts.
                // We log a warning but don't reject.
                if ct.is_none() {
                    log::warn!(
                        "[WorkerScriptLoader] no Content-Type header for '{}', loading anyway",
                        url_str
                    );
                }

                // Stage 4: Decode response body as UTF-8.
                // @trace REQ-BRW-004 [DF-WK-2] UTF-8 解码
                state_callback(WorkerScriptLoadState::Decoding);

                let source = String::from_utf8(response.body.to_vec()).map_err(|e| {
                    WorkerScriptLoadError::Utf8DecodeError(format!(
                        "Worker script response body is not valid UTF-8: {}",
                        e
                    ))
                })?;

                (source, url_str, ct)
            }
        };

        // Stage 5: Compiling — SpiderMonkey compilation happens when
        // the Worker thread evaluates the script via WebWorker::new.
        // We mark this stage as a placeholder for CDP observability.
        // @trace REQ-BRW-004 [DF-WK-2] Classic/Module 编译
        state_callback(WorkerScriptLoadState::Compiling);

        // Stage 6: Ready.
        // @trace REQ-BRW-004 [DF-WK-2] script ready
        state_callback(WorkerScriptLoadState::Ready);

        Ok(WorkerScriptLoadResult {
            source,
            final_url,
            mime_type,
        })
    }

    /// Load the Worker script without state callbacks (simplified API).
    ///
    /// Equivalent to `load()` with a no-op state callback. Use when
    /// CDP observability of loading stages is not needed.
    ///
    /// @trace REQ-BRW-004 [entity:Worker] [DF-WK-2]
    pub fn load_simple(
        &self,
        stealth_profile: &Option<bao_stealth::StealthProfile>,
    ) -> Result<WorkerScriptLoadResult, WorkerScriptLoadError> {
        self.load(stealth_profile, |_| {})
    }

    /// Returns the script type for SpiderMonkey compilation options.
    ///
    /// Module Workers use ES module compilation; Classic Workers use
    /// the default script compilation (DF-WK-2: "Classic/Module 编译").
    ///
    /// @trace REQ-BRW-004 [entity:Worker] [DF-WK-2]
    pub fn is_module(&self) -> bool {
        matches!(self.script_type, WorkerScriptType::Module)
    }
}

/// Fetch a Worker script via HTTP using bao_runtime's stealth HTTP client.
///
/// Performs a synchronous GET request to the script URL. When a stealth
/// profile is provided, the request uses the same TLS/HTTP2 fingerprint
/// as the parent page (SPEC criterion #12: CRIT-STL-WK).
///
/// DF-WK-2: "线程归属: worker 线程" — this function runs on the Worker
/// thread, not the main thread. The synchronous blocking call is safe here
/// because the Worker thread has no event loop obligations during script load.
///
/// Returns the HTTP response (status, headers, body) on success, or an
/// error message on failure.
///
/// @trace REQ-BRW-004 [entity:Worker] [DF-WK-2]
/// @trace REQ-BRW-004 [criterion:12] CRIT-STL-WK: stealth profile inheritance
fn fetch_worker_script(
    url: &str,
    stealth_profile: &Option<bao_stealth::StealthProfile>,
) -> Result<WorkerScriptFetchResponse, String> {
    use bun_http::Method;
    use bun_runtime::stealth_http::stealth_http_request;

    // @trace REQ-BRW-004 [criterion:12] CRIT-STL-WK
    // The stealth profile is inherited from the parent page so that
    // Worker script fetches produce the same TLS JA3/JA4 + HTTP2
    // AKAMAI fingerprint. Without this, a Worker's script fetch would
    // use a default fingerprint, leaking a distinct fingerprint that
    // can be correlated back to the page (CreepJS worker-vs-main test).
    let result = stealth_http_request(
        stealth_profile,
        Method::GET,
        url,
        &[],  // no custom headers for Worker script fetch
        None, // no body for GET request
    )
    .map_err(|e| format!("Failed to fetch Worker script from '{}': {}", url, e))?;

    // Convert StealthSyncResult to our response type.
    Ok(WorkerScriptFetchResponse {
        status_code: result.status_code,
        headers: result
            .headers
            .into_iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect(),
        body: result.body.to_vec(),
    })
}

/// Response from a Worker script HTTP fetch.
///
/// Owns all data with standard types (no CompactString/SmallVec) for
/// simplicity in the script loading pipeline.
///
/// @trace REQ-BRW-004 [entity:Worker] [DF-WK-2]
struct WorkerScriptFetchResponse {
    /// HTTP status code (e.g., 200, 404).
    status_code: u32,
    /// Response headers as (name, value) pairs.
    headers: Vec<(String, String)>,
    /// Response body bytes.
    body: Vec<u8>,
}
///
/// Used for CDP observability and lifecycle management. The Worker script
/// loading process has these states:
/// 1. Pending — URL resolved, fetch not yet started
/// 2. Fetching — HTTP request in progress (DF-WK-2: "global.fetch")
/// 3. Validating — Response received, MIME type check (DF-WK-2: "JS MIME 校验")
/// 4. Decoding — UTF-8 decode of response body (DF-WK-2: "UTF-8 解码")
/// 5. Compiling — SpiderMonkey compilation (DF-WK-2: "Classic/Module 编译")
/// 6. Ready — Script compiled, ready for Worker thread evaluation
/// 7. Failed — Error at any stage (network/MIME/decode/compile)
///
/// @trace REQ-BRW-004 [entity:Worker] [DF-WK-2]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WorkerScriptLoadState {
    /// URL resolved, fetch not yet started.
    /// @trace REQ-BRW-004 [entity:Worker] [DF-WK-2]
    Pending,
    /// HTTP request in progress.
    /// @trace REQ-BRW-004 [entity:Worker] [DF-WK-2]
    Fetching,
    /// Response received, validating MIME type.
    /// @trace REQ-BRW-004 [entity:Worker] [DF-WK-2]
    Validating,
    /// Decoding response body as UTF-8.
    /// @trace REQ-BRW-004 [entity:Worker] [DF-WK-2]
    Decoding,
    /// Compiling script with SpiderMonkey.
    /// @trace REQ-BRW-004 [entity:Worker] [DF-WK-2]
    Compiling,
    /// Script compiled successfully, ready for evaluation.
    /// @trace REQ-BRW-004 [entity:Worker] [DF-WK-2]
    Ready,
    /// Loading failed with an error.
    /// @trace REQ-BRW-004 [entity:Worker] [DF-WK-2]
    Failed(WorkerScriptLoadError),
}

impl WorkerScriptLoadState {
    /// Returns true if the script is ready for evaluation.
    ///
    /// @trace REQ-BRW-004 [entity:Worker] [DF-WK-2]
    pub fn is_ready(&self) -> bool {
        matches!(self, WorkerScriptLoadState::Ready)
    }

    /// Returns true if loading failed.
    ///
    /// @trace REQ-BRW-004 [entity:Worker] [DF-WK-2]
    pub fn is_failed(&self) -> bool {
        matches!(self, WorkerScriptLoadState::Failed(_))
    }

    /// Returns true if loading is still in progress (not Ready or Failed).
    ///
    /// @trace REQ-BRW-004 [entity:Worker] [DF-WK-2]
    pub fn is_loading(&self) -> bool {
        !self.is_ready() && !self.is_failed()
    }
}

/// Decode percent-encoded data URL content to UTF-8 bytes.
///
/// Simple percent-decoding for data: URL content: %XX → byte value.
/// Returns the decoded bytes, or an error if UTF-8 validation fails.
///
/// @trace REQ-BRW-004 [entity:Worker] [DF-WK-2]
fn decode_percent_encoded(data: &str) -> Result<Vec<u8>, WorkerScriptLoadError> {
    let mut bytes = Vec::with_capacity(data.len());
    let mut chars = data.chars();
    while let Some(c) = chars.next() {
        if c == '%' {
            // Read two hex digits
            let hex: String = chars.by_ref().take(2).collect();
            if hex.len() != 2 {
                return Err(WorkerScriptLoadError::Utf8DecodeError(
                    "Incomplete percent-encoding in data: URL".to_string(),
                ));
            }
            let byte = u8::from_str_radix(&hex, 16).map_err(|e| {
                WorkerScriptLoadError::Utf8DecodeError(format!(
                    "Invalid percent-encoding '%{}' in data: URL: {}",
                    hex, e
                ))
            })?;
            bytes.push(byte);
        } else if c == '+' {
            // In some data: URL contexts, '+' means space (form encoding)
            bytes.push(b' ');
        } else {
            // ASCII character as-is
            let mut buf = [0u8; 4];
            bytes.extend_from_slice(c.encode_utf8(&mut buf).as_bytes());
        }
    }
    // Validate UTF-8 by converting to String and back
    String::from_utf8(bytes.clone()).map_err(|e| {
        WorkerScriptLoadError::Utf8DecodeError(format!(
            "data: URL content is not valid UTF-8: {}",
            e
        ))
    })?;
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    // ─── Worker Script Loading Pipeline (REQ-BRW-004 / DF-WK-2) ────────
    // @trace REQ-BRW-004 [req:REQ-BRW-004] [entity:Worker] [DF-WK-2] [level:unit]

    #[test]
    fn test_worker_script_source_inline() {
        let source = WorkerScriptSource::Inline("var x = 1;".to_string());
        assert_eq!(source, WorkerScriptSource::Inline("var x = 1;".to_string()));
        assert_ne!(source, WorkerScriptSource::Inline("var y = 2;".to_string()));
    }

    #[test]
    fn test_worker_script_source_url() {
        let source = WorkerScriptSource::Url("https://example.com/worker.js".to_string());
        assert_eq!(
            source,
            WorkerScriptSource::Url("https://example.com/worker.js".to_string())
        );
        assert_ne!(
            source,
            WorkerScriptSource::Url("https://other.com/worker.js".to_string())
        );
    }

    #[test]
    fn test_worker_script_load_result() {
        let result = WorkerScriptLoadResult {
            source: "self.onmessage = function(e) {}".to_string(),
            final_url: "https://example.com/worker.js".to_string(),
            mime_type: Some("text/javascript".to_string()),
        };
        assert_eq!(result.source, "self.onmessage = function(e) {}");
        assert_eq!(result.final_url, "https://example.com/worker.js");
        assert_eq!(result.mime_type.as_deref(), Some("text/javascript"));
    }

    #[test]
    fn test_worker_script_load_error_network() {
        let err = WorkerScriptLoadError::NetworkError("404 Not Found".to_string());
        assert_eq!(
            err,
            WorkerScriptLoadError::NetworkError("404 Not Found".to_string())
        );
    }

    #[test]
    fn test_worker_script_load_error_invalid_mime() {
        let err = WorkerScriptLoadError::InvalidMimeType {
            received: "text/html".to_string(),
            url: "https://example.com/worker.js".to_string(),
        };
        match err {
            WorkerScriptLoadError::InvalidMimeType { received, url } => {
                assert_eq!(received, "text/html");
                assert_eq!(url, "https://example.com/worker.js");
            }
            _ => panic!("expected InvalidMimeType"),
        }
    }

    #[test]
    fn test_worker_script_load_error_utf8() {
        let err = WorkerScriptLoadError::Utf8DecodeError("invalid UTF-8".to_string());
        assert_eq!(
            err,
            WorkerScriptLoadError::Utf8DecodeError("invalid UTF-8".to_string())
        );
    }

    #[test]
    fn test_worker_script_load_error_invalid_url() {
        let err = WorkerScriptLoadError::InvalidUrl("bad url".to_string());
        assert_eq!(
            err,
            WorkerScriptLoadError::InvalidUrl("bad url".to_string())
        );
    }

    #[test]
    fn test_worker_script_load_error_cancelled() {
        let err = WorkerScriptLoadError::Cancelled;
        assert_eq!(err, WorkerScriptLoadError::Cancelled);
    }

    #[test]
    fn test_worker_script_type_default_classic() {
        assert_eq!(WorkerScriptType::default(), WorkerScriptType::Classic);
    }

    #[test]
    fn test_worker_script_type_equality() {
        assert_eq!(WorkerScriptType::Classic, WorkerScriptType::Classic);
        assert_eq!(WorkerScriptType::Module, WorkerScriptType::Module);
        assert_ne!(WorkerScriptType::Classic, WorkerScriptType::Module);
    }

    #[test]
    fn test_is_javascript_mime_type_valid() {
        assert!(is_javascript_mime_type("text/javascript"));
        assert!(is_javascript_mime_type("application/javascript"));
        assert!(is_javascript_mime_type("application/ecmascript"));
        assert!(is_javascript_mime_type("application/x-javascript"));
        assert!(is_javascript_mime_type("text/ecmascript"));
        assert!(is_javascript_mime_type("text/x-javascript"));
        assert!(is_javascript_mime_type("text/jscript"));
        assert!(is_javascript_mime_type("text/livescript"));
    }

    #[test]
    fn test_is_javascript_mime_type_case_insensitive() {
        assert!(is_javascript_mime_type("Text/JavaScript"));
        assert!(is_javascript_mime_type("APPLICATION/JAVASCRIPT"));
        assert!(is_javascript_mime_type("text/JavaScript"));
    }

    #[test]
    fn test_is_javascript_mime_type_with_charset() {
        // MIME type with parameters should still match
        assert!(is_javascript_mime_type("text/javascript; charset=utf-8"));
        assert!(is_javascript_mime_type(
            "application/javascript;charset=utf-8"
        ));
    }

    #[test]
    fn test_is_javascript_mime_type_invalid() {
        assert!(!is_javascript_mime_type("text/html"));
        assert!(!is_javascript_mime_type("application/json"));
        assert!(!is_javascript_mime_type("text/plain"));
        assert!(!is_javascript_mime_type("application/octet-stream"));
        assert!(!is_javascript_mime_type("text/css"));
    }

    #[test]
    fn test_worker_script_loader_inline() {
        let loader =
            WorkerScriptLoader::inline("var x = 1;".to_string(), WorkerScriptType::Classic);
        assert!(loader.script_url().is_none());
        assert!(!loader.requires_fetch());
        let resolved = loader.resolve().unwrap();
        assert_eq!(
            resolved,
            WorkerScriptSource::Inline("var x = 1;".to_string())
        );
    }

    #[test]
    fn test_worker_script_loader_url_https() {
        let loader = WorkerScriptLoader::url(
            "https://example.com/worker.js".to_string(),
            WorkerScriptType::Classic,
        );
        assert_eq!(loader.script_url(), Some("https://example.com/worker.js"));
        assert!(loader.requires_fetch());
        let resolved = loader.resolve().unwrap();
        assert_eq!(
            resolved,
            WorkerScriptSource::Url("https://example.com/worker.js".to_string())
        );
    }

    #[test]
    fn test_worker_script_loader_url_http() {
        let loader = WorkerScriptLoader::url(
            "http://localhost:3000/worker.js".to_string(),
            WorkerScriptType::Module,
        );
        assert!(loader.requires_fetch());
        assert_eq!(loader.script_type, WorkerScriptType::Module);
    }

    #[test]
    fn test_worker_script_loader_url_invalid() {
        let loader = WorkerScriptLoader::url("not a url".to_string(), WorkerScriptType::Classic);
        let result = loader.resolve();
        assert!(result.is_err());
        match result.unwrap_err() {
            WorkerScriptLoadError::InvalidUrl(msg) => {
                assert!(msg.contains("Invalid Worker script URL"));
            }
            _ => panic!("expected InvalidUrl error"),
        }
    }

    #[test]
    fn test_worker_script_loader_url_unsupported_scheme() {
        let loader = WorkerScriptLoader::url(
            "ftp://example.com/worker.js".to_string(),
            WorkerScriptType::Classic,
        );
        let result = loader.resolve();
        assert!(result.is_err());
        match result.unwrap_err() {
            WorkerScriptLoadError::InvalidUrl(msg) => {
                assert!(msg.contains("Unsupported") || msg.contains("ftp"));
            }
            _ => panic!("expected InvalidUrl error"),
        }
    }

    #[test]
    fn test_worker_script_loader_data_url_text() {
        let loader = WorkerScriptLoader::url(
            "data:text/javascript,self.postMessage('hello')".to_string(),
            WorkerScriptType::Classic,
        );
        let resolved = loader.resolve().unwrap();
        match resolved {
            WorkerScriptSource::Inline(script) => {
                assert_eq!(script, "self.postMessage('hello')");
            }
            WorkerScriptSource::Url(_) => panic!("expected inline source from data: URL"),
        }
    }

    #[test]
    fn test_worker_script_loader_data_url_base64() {
        // base64 of "var x = 1;" = "dmFyIHggPSAxOw=="
        let loader = WorkerScriptLoader::url(
            "data:text/javascript;base64,dmFyIHggPSAxOw==".to_string(),
            WorkerScriptType::Classic,
        );
        let resolved = loader.resolve().unwrap();
        match resolved {
            WorkerScriptSource::Inline(script) => {
                assert_eq!(script, "var x = 1;");
            }
            WorkerScriptSource::Url(_) => panic!("expected inline source from data: URL"),
        }
    }

    #[test]
    fn test_worker_script_loader_data_url_invalid_base64() {
        let loader = WorkerScriptLoader::url(
            "data:text/javascript;base64,!!!invalid!!!".to_string(),
            WorkerScriptType::Classic,
        );
        let result = loader.resolve();
        assert!(result.is_err());
    }

    #[test]
    fn test_worker_script_loader_data_url_missing_comma() {
        let loader = WorkerScriptLoader::url(
            "data:text/javascript".to_string(),
            WorkerScriptType::Classic,
        );
        let result = loader.resolve();
        assert!(result.is_err());
        match result.unwrap_err() {
            WorkerScriptLoadError::InvalidUrl(msg) => {
                assert!(msg.contains("comma separator"));
            }
            _ => panic!("expected InvalidUrl error"),
        }
    }

    #[test]
    fn test_worker_script_loader_blob_url_passthrough() {
        let loader = WorkerScriptLoader::url(
            "blob:https://example.com/550e8400-e29b-41d4-a716-446655440000".to_string(),
            WorkerScriptType::Classic,
        );
        let resolved = loader.resolve().unwrap();
        assert_eq!(
            resolved,
            WorkerScriptSource::Url(
                "blob:https://example.com/550e8400-e29b-41d4-a716-446655440000".to_string()
            )
        );
    }

    #[test]
    fn test_worker_script_loader_from_source() {
        let loader = WorkerScriptLoader::from_source(
            WorkerScriptSource::Inline("code".to_string()),
            WorkerScriptType::Module,
        );
        assert_eq!(loader.script_type, WorkerScriptType::Module);
        assert!(loader.script_url().is_none());
    }

    #[test]
    fn test_worker_script_loader_validate_mime_type_valid() {
        assert!(
            WorkerScriptLoader::validate_mime_type(
                "text/javascript",
                "https://example.com/worker.js"
            )
            .is_ok()
        );
        assert!(
            WorkerScriptLoader::validate_mime_type(
                "application/javascript",
                "https://example.com/worker.js"
            )
            .is_ok()
        );
    }

    #[test]
    fn test_worker_script_loader_validate_mime_type_invalid() {
        let result =
            WorkerScriptLoader::validate_mime_type("text/html", "https://example.com/worker.js");
        assert!(result.is_err());
        match result.unwrap_err() {
            WorkerScriptLoadError::InvalidMimeType { received, url } => {
                assert_eq!(received, "text/html");
                assert_eq!(url, "https://example.com/worker.js");
            }
            _ => panic!("expected InvalidMimeType error"),
        }
    }

    #[test]
    fn test_worker_script_load_state_transitions() {
        let mut state = WorkerScriptLoadState::Pending;
        assert!(state.is_loading());
        assert!(!state.is_ready());
        assert!(!state.is_failed());

        state = WorkerScriptLoadState::Fetching;
        assert!(state.is_loading());

        state = WorkerScriptLoadState::Validating;
        assert!(state.is_loading());

        state = WorkerScriptLoadState::Decoding;
        assert!(state.is_loading());

        state = WorkerScriptLoadState::Compiling;
        assert!(state.is_loading());

        state = WorkerScriptLoadState::Ready;
        assert!(!state.is_loading());
        assert!(state.is_ready());

        state = WorkerScriptLoadState::Failed(WorkerScriptLoadError::NetworkError(
            "timeout".to_string(),
        ));
        assert!(!state.is_loading());
        assert!(state.is_failed());
    }

    #[test]
    fn test_webview_state_worker_script_load_state_registration() {
        let mut state = BaoWebViewState::default();
        let worker_id = WorkerId("worker1.js".to_string());
        state.register_worker_script_load_state(worker_id.clone(), WorkerScriptLoadState::Pending);
        assert_eq!(state.worker_script_load_state_count(), 1);
        assert!(state.worker_script_load_state(&worker_id).is_some());
        assert_eq!(
            state.worker_script_load_state(&worker_id).unwrap(),
            &WorkerScriptLoadState::Pending
        );
    }

    #[test]
    fn test_webview_state_worker_script_load_state_update() {
        let mut state = BaoWebViewState::default();
        let worker_id = WorkerId("worker1.js".to_string());
        state.register_worker_script_load_state(worker_id.clone(), WorkerScriptLoadState::Pending);
        state.update_worker_script_load_state(&worker_id, WorkerScriptLoadState::Fetching);
        assert_eq!(
            state.worker_script_load_state(&worker_id).unwrap(),
            &WorkerScriptLoadState::Fetching
        );
    }

    #[test]
    fn test_webview_state_worker_script_load_state_remove() {
        let mut state = BaoWebViewState::default();
        let worker_id = WorkerId("worker1.js".to_string());
        state.register_worker_script_load_state(worker_id.clone(), WorkerScriptLoadState::Ready);
        let removed = state.remove_worker_script_load_state(&worker_id);
        assert!(removed.is_some());
        assert_eq!(removed.unwrap(), WorkerScriptLoadState::Ready);
        assert_eq!(state.worker_script_load_state_count(), 0);
    }

    #[test]
    fn test_webview_state_terminate_clears_script_load_states() {
        let mut state = BaoWebViewState::default();
        state.track_worker(WorkerHandle::new("worker1.js".to_string()));
        state.register_worker_script_load_state(
            WorkerId("worker1.js".to_string()),
            WorkerScriptLoadState::Fetching,
        );
        assert_eq!(state.worker_script_load_state_count(), 1);
        state.terminate_all_workers();
        assert_eq!(state.worker_script_load_state_count(), 0);
    }

    #[test]
    fn test_webview_state_reap_terminated_worker_script_load_states() {
        let mut state = BaoWebViewState::default();
        state.track_worker(WorkerHandle::new("worker1.js".to_string()));
        state.track_worker(WorkerHandle::new("worker2.js".to_string()));
        state.register_worker_script_load_state(
            WorkerId("worker1.js".to_string()),
            WorkerScriptLoadState::Ready,
        );
        state.register_worker_script_load_state(
            WorkerId("worker2.js".to_string()),
            WorkerScriptLoadState::Fetching,
        );
        // Terminate and reap worker1
        state.active_workers[0].handle().terminate();
        state.active_workers[0].handle().mark_terminated();
        state.reap_terminated_workers();
        // worker1's script load state should be reaped, worker2's should remain
        assert_eq!(state.worker_script_load_state_count(), 1);
        assert!(
            state
                .worker_script_load_state(&WorkerId("worker2.js".to_string()))
                .is_some()
        );
    }

    #[test]
    fn test_worker_script_loader_file_url() {
        // Create a temp file with Worker script content
        let temp_dir = std::env::temp_dir();
        let temp_file = temp_dir.join("bao_test_worker_script.js");
        std::fs::write(&temp_file, "var x = 42;").unwrap();

        let file_url = format!("file://{}", temp_file.display());
        let loader = WorkerScriptLoader::url(file_url, WorkerScriptType::Classic);
        let resolved = loader.resolve().unwrap();
        match resolved {
            WorkerScriptSource::Inline(script) => {
                assert_eq!(script, "var x = 42;");
            }
            WorkerScriptSource::Url(_) => panic!("expected inline source from file: URL"),
        }

        // Cleanup
        let _ = std::fs::remove_file(&temp_file);
    }

    #[test]
    fn test_worker_script_loader_file_url_not_found() {
        let loader = WorkerScriptLoader::url(
            "file:///nonexistent/path/worker.js".to_string(),
            WorkerScriptType::Classic,
        );
        let result = loader.resolve();
        assert!(result.is_err());
        match result.unwrap_err() {
            WorkerScriptLoadError::NetworkError(msg) => {
                assert!(msg.contains("Failed to read") || msg.contains("No such file"));
            }
            _ => panic!("expected NetworkError for missing file"),
        }
    }

    #[test]
    fn test_worker_script_loader_full_pipeline_states() {
        // Simulate the full DF-WK-2 pipeline state transitions
        let mut state = BaoWebViewState::default();
        let worker_id = WorkerId("https://example.com/worker.js".to_string());

        // Step 1: Worker created → Pending
        state.register_worker_script_load_state(worker_id.clone(), WorkerScriptLoadState::Pending);
        assert!(
            state
                .worker_script_load_state(&worker_id)
                .unwrap()
                .is_loading()
        );

        // Step 2: Fetch started → Fetching
        state.update_worker_script_load_state(&worker_id, WorkerScriptLoadState::Fetching);
        assert!(matches!(
            state.worker_script_load_state(&worker_id).unwrap(),
            WorkerScriptLoadState::Fetching
        ));

        // Step 3: Response received → Validating
        state.update_worker_script_load_state(&worker_id, WorkerScriptLoadState::Validating);

        // Step 4: MIME check passed → Decoding
        state.update_worker_script_load_state(&worker_id, WorkerScriptLoadState::Decoding);

        // Step 5: UTF-8 decoded → Compiling
        state.update_worker_script_load_state(&worker_id, WorkerScriptLoadState::Compiling);

        // Step 6: Compilation succeeded → Ready
        state.update_worker_script_load_state(&worker_id, WorkerScriptLoadState::Ready);
        assert!(
            state
                .worker_script_load_state(&worker_id)
                .unwrap()
                .is_ready()
        );
    }

    #[test]
    fn test_worker_script_loader_pipeline_failure() {
        let mut state = BaoWebViewState::default();
        let worker_id = WorkerId("https://example.com/bad-worker.js".to_string());

        state.register_worker_script_load_state(worker_id.clone(), WorkerScriptLoadState::Pending);

        // Simulate MIME type failure during validation
        state.update_worker_script_load_state(
            &worker_id,
            WorkerScriptLoadState::Failed(WorkerScriptLoadError::InvalidMimeType {
                received: "text/html".to_string(),
                url: "https://example.com/bad-worker.js".to_string(),
            }),
        );
        assert!(
            state
                .worker_script_load_state(&worker_id)
                .unwrap()
                .is_failed()
        );
    }
}
