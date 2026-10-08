// REQ-CDP-009: CDP screencast — change-driven frame stream (putao520/bao #55).
//
// Page.startScreencast / Page.stopScreencast / Page.screencastFrameAck plus
// the Page.screencastFrame event flow, implemented on bao's own headless
// frame-production contract (the pump loops' repaint-latch composites):
//
// - **Command faces**: both real CDP dispatch routes intercept the three
//   methods BEFORE `bao_cdp::handle_command` — the memory:// host bridge
//   (`cdp_memory::MemoryCdpBridge`) and the WS registry
//   (`ws_registry::BaoWsRegistry`). Registration only touches this
//   process-global manager (plain data, Mutex-guarded) so the dispatch
//   threads never touch servo state (`WebView` is `!Send`).
// - **Frame production**: the pump loops (`run` / `pump_cdp` /
//   `run_with_bridge`) call [`drive`] BEFORE `paint_pages_needing_repaint`
//   each iteration. Change detection is the servo repaint latch itself
//   (`notify_new_frame_ready` — servo's own "content changed" signal, not a
//   timer) plus a pixel-digest gate on the captured frame, so a frame is
//   emitted only when the page content actually changed.
// - **Event delivery**: memory-origin sessions push translated CdpEvents
//   through the client crate's process event channel (the documented
//   InMemoryTransport event-sender pattern); WS-origin sessions ride the
//   same target-scoped routing as servo events
//   (`BaoWsRegistry::broadcast_for_target` / page-endpoint delivery).
//
// Gates per session (Chrome screencast semantics):
// - `maxFrameRate` → minimum interval between EMITTED frames (rate cap).
// - `Page.screencastFrameAck` → at most ONE un-acked frame in flight; newer
//   captures replace a single held slot (bounded memory, latest state).
// - `everyNthFrame` → emit only every n-th changed frame.
// - pixel digest → suppress frames whose pixels are identical to the last
//   EMITTED frame.
//
// @trace REQ-CDP-009 [level:library]

use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, Instant};

use base64::Engine;
use image::RgbaImage;
use serde_json::{json, Value};

use crate::page_pool::PagePool;
use crate::screenshot::{encode_image_with_quality, ScreenshotFormat};

/// CDP method names (interception keys for both dispatch faces).
const START: &str = "Page.startScreencast";
const STOP: &str = "Page.stopScreencast";
const ACK: &str = "Page.screencastFrameAck";

/// ScreencastFrame event method name.
const FRAME_EVENT: &str = "Page.screencastFrame";

/// Default JPEG quality when `startScreencast` omits `quality` (CDP:
/// "Compression quality from range [0..100]"; Chrome's internal default).
const DEFAULT_JPEG_QUALITY: u8 = 80;

/// JSON-RPC error code mirroring the WS registry's target-resolution
/// failures ("No target with given id found" family).
const WS_ERR_TARGET_NOT_FOUND: i64 = -32000;

/// Which dispatch face owns a screencast session. Stop is face-scoped (the
/// memory client stops its own stream, a WS session stops its own); ack is
/// session-id-addressed regardless of face (Chrome parity).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Origin {
    /// `memory://bao` client (process-registry bridge).
    Memory,
    /// WebSocket CDP session (`ws://127.0.0.1:<port>/devtools/...`).
    Ws,
}

/// Process-global screencast registry. Production is pool-scoped: each pump
/// only drives sessions whose target resolves to a page in ITS pool, so a
/// process hosting multiple runtimes never crosses page-identity domains.
pub struct ScreencastManager {
    /// target_id (decimal page id) → active sessions.
    sessions: Mutex<HashMap<String, Vec<Session>>>,
    next_session_id: AtomicU64,
    /// Monotonic epoch for the screencastFrame metadata timestamp.
    started_at: Instant,
}

struct Session {
    id: u64,
    origin: Origin,
    format: ScreenshotFormat,
    quality: u8,
    max_width: Option<u32>,
    max_height: Option<u32>,
    /// From `maxFrameRate` — minimum interval between emitted frames.
    min_interval: Option<Duration>,
    /// From `everyNthFrame` (1 = every frame).
    every_nth: u32,
    /// Counts captured change-frames (feeds `everyNthFrame`).
    frame_no: u32,
    /// The first poll after start captures regardless of the repaint latch
    /// (the consumer's "current state" frame; Chrome sends it immediately).
    first_frame_pending: bool,
    /// Ack gate: a frame was emitted and not yet acked.
    in_flight: bool,
    /// Latest captured frame held while rate-limited or un-acked.
    held: Option<Frame>,
    last_emit: Option<Instant>,
    /// Pixel digest of the last EMITTED frame (change gate).
    last_digest: Option<u64>,
}

/// A captured frame ready for emission (encoded + base64).
struct Frame {
    base64: String,
    digest: u64,
    device_width: u32,
    device_height: u32,
    timestamp: f64,
}

impl ScreencastManager {
    fn new() -> Self {
        ScreencastManager {
            sessions: Mutex::new(HashMap::new()),
            next_session_id: AtomicU64::new(1),
            started_at: Instant::now(),
        }
    }

    /// Active session count (test observability + C5 release probe).
    pub fn active_session_count(&self) -> usize {
        self.sessions
            .lock()
            .map(|m| m.values().map(|v| v.len()).sum())
            .unwrap_or(0)
    }

    /// Drop every session (test isolation face: `cargo test
    /// --test-threads=1` runs the whole suite in one process, so a test that
    /// fails before its stop must not leak sessions into the next test's
    /// page-identity domain — every screencast test calls this first).
    pub fn drop_all_sessions(&self) {
        if let Ok(mut m) = self.sessions.lock() {
            m.clear();
        }
    }

    /// Page.startScreencast — register (or replace, same target + face) a
    /// change-driven frame stream. Fails closed on an unknown target.
    ///
    /// @trace REQ-CDP-009 [criterion:Page.startScreencast 建立 change-driven 帧流会话]
    fn start(&self, target_id: &str, origin: Origin, params: &Value) -> Result<Value, String> {
        let page_id = target_id
            .parse::<usize>()
            .map_err(|_| format!("No target with given id found: {target_id}"))?;
        if crate::page::webview_id_for_page(page_id).is_none() {
            return Err(format!("No target with given id found: {target_id}"));
        }
        let cfg = SessionConfig::from_params(params)?;

        let id = self.next_session_id.fetch_add(1, Ordering::Relaxed);
        let mut map = self.sessions.lock().map_err(|_| "screencast state poisoned")?;
        let entry = map.entry(target_id.to_string()).or_default();
        // Same-face restart replaces the previous stream (Chrome: a second
        // startScreencast on the same session restarts the stream).
        entry.retain(|s| s.origin != origin);
        entry.push(Session {
            id,
            origin,
            format: cfg.format,
            quality: cfg.quality,
            max_width: cfg.max_width,
            max_height: cfg.max_height,
            min_interval: cfg.min_interval,
            every_nth: cfg.every_nth,
            frame_no: 0,
            first_frame_pending: true,
            in_flight: false,
            held: None,
            last_emit: None,
            last_digest: None,
        });
        Ok(json!({}))
    }

    /// Page.stopScreencast — terminate this face's stream(s) for the target
    /// and release the held frame. Idempotent (stop of a non-started target
    /// is a no-op ok, Chrome parity).
    ///
    /// @trace REQ-CDP-009 [criterion:Page.stopScreencast 终止会话停止帧生产,资源正常释放]
    fn stop(&self, target_id: &str, origin: Origin) -> Result<Value, String> {
        if let Ok(mut map) = self.sessions.lock() {
            if let Some(list) = map.get_mut(target_id) {
                list.retain(|s| s.origin != origin);
                if list.is_empty() {
                    map.remove(target_id);
                }
            }
        }
        Ok(json!({}))
    }

    /// Page.screencastFrameAck — clear the in-flight gate for the frame's
    /// screencast session. Unknown session ids are accepted silently
    /// (a stale ack after stop must not error, Chrome parity).
    ///
    /// @trace REQ-CDP-009 [criterion:Page.screencastFrameAck 确认参与出帧节流]
    fn ack(&self, session_id: u64) -> Result<Value, String> {
        if let Ok(mut map) = self.sessions.lock() {
            for list in map.values_mut() {
                for s in list.iter_mut() {
                    if s.id == session_id {
                        s.in_flight = false;
                        return Ok(json!({}));
                    }
                }
            }
        }
        Ok(json!({}))
    }
}

/// Parsed `Page.startScreencast` parameters.
struct SessionConfig {
    format: ScreenshotFormat,
    quality: u8,
    max_width: Option<u32>,
    max_height: Option<u32>,
    min_interval: Option<Duration>,
    every_nth: u32,
}

impl SessionConfig {
    /// Protocol shape per the CDP spec: format ("jpeg" | "png"),
    /// quality (0..100), maxWidth, maxHeight, maxFrameRate, everyNthFrame.
    fn from_params(params: &Value) -> Result<Self, String> {
        let format = match params.get("format").and_then(|v| v.as_str()) {
            None | Some("png") => ScreenshotFormat::Png,
            Some("jpeg") => ScreenshotFormat::Jpeg,
            Some(other) => return Err(format!("Invalid format '{other}' (jpeg | png)")),
        };
        let quality = params
            .get("quality")
            .and_then(|v| v.as_u64())
            .map(|q| q.clamp(1, 100) as u8)
            .unwrap_or(DEFAULT_JPEG_QUALITY);
        let dim = |name: &str| -> Option<u32> {
            params
                .get(name)
                .and_then(|v| v.as_u64())
                .filter(|v| *v > 0)
                .map(|v| v.min(u32::MAX as u64) as u32)
        };
        let min_interval = params
            .get("maxFrameRate")
            .and_then(|v| v.as_f64())
            .filter(|r| *r > 0.0)
            .map(|r| Duration::from_secs_f64(1.0 / r));
        let every_nth = params
            .get("everyNthFrame")
            .and_then(|v| v.as_u64())
            .map(|n| n.clamp(1, u32::MAX as u64) as u32)
            .unwrap_or(1);
        Ok(SessionConfig {
            format,
            quality,
            max_width: dim("maxWidth"),
            max_height: dim("maxHeight"),
            min_interval,
            every_nth,
        })
    }
}

/// Process-global manager (single live CDP screencast domain per process —
/// same single-slot shape as the memory bridge registry in
/// `bao_cdp_client::browser`, with pool-scoped production).
static MANAGER: LazyLock<ScreencastManager> = LazyLock::new(ScreencastManager::new);

/// Active screencast session count (test observability).
pub fn active_session_count() -> usize {
    MANAGER.active_session_count()
}

/// Drop every screencast session (test isolation face — see
/// [`ScreencastManager::drop_all_sessions`]).
pub fn drop_all_sessions() {
    MANAGER.drop_all_sessions();
}

/// Dispatch the three screencast methods for the memory:// face.
/// Returns `None` when `method` is not a screencast command (the caller
/// falls through to `bao_cdp::handle_command`).
pub fn dispatch_memory_command(
    method: &str,
    params: &Value,
    target_id: &str,
) -> Option<bao_cdp_client::transport::in_memory::InMemoryBridgeResponse> {
    use bao_cdp_client::transport::in_memory::InMemoryBridgeResponse;
    dispatch(method, params, target_id, Origin::Memory).map(|r| match r {
        Ok(v) => InMemoryBridgeResponse::Ok(v),
        Err(msg) => InMemoryBridgeResponse::Err(msg),
    })
}

/// Dispatch the three screencast methods for the WS face. Returns `None`
/// when `method` is not a screencast command.
pub fn dispatch_ws_command(
    method: &str,
    params: &Value,
    target_id: &str,
) -> Option<Result<Value, cdp_server::CdpError>> {
    dispatch(method, params, target_id, Origin::Ws).map(|r| {
        r.map_err(|message| cdp_server::CdpError {
            code: WS_ERR_TARGET_NOT_FOUND,
            message,
        })
    })
}

fn dispatch(
    method: &str,
    params: &Value,
    target_id: &str,
    origin: Origin,
) -> Option<Result<Value, String>> {
    match method {
        START => Some(MANAGER.start(target_id, origin, params)),
        STOP => Some(MANAGER.stop(target_id, origin)),
        ACK => Some(match params.get("sessionId").and_then(|v| v.as_u64()) {
            Some(session_id) => MANAGER.ack(session_id),
            None => Err("'Page.screencastFrameAck' requires a numeric sessionId param".to_string()),
        }),
        _ => None,
    }
}

/// Drive one pump tick: capture changed frames for every active screencast
/// target in this pool and emit the gated ones.
///
/// Called from the pump loops BEFORE `paint_pages_needing_repaint` — the
/// capture path composites on demand (`PageHandle::capture_frame` paints),
/// so polling first lets the capture consume the repaint latch and the
/// sweep below becomes a no-op for that page. `ws_sink` routes WS-origin
/// frames (target-scoped, same routing as servo events); `None` means this
/// pump has no WS event path — a WS-origin session is then undeliverable
/// and is dropped (fail-closed, never silently swallowed).
///
/// @trace REQ-CDP-009 [criterion:Page.startScreencast 建立 change-driven 帧流会话]
pub fn drive(pool: &PagePool, ws_sink: Option<&dyn Fn(&str, &str, Value)>) {
    // Phase 1 (locked, cheap): snapshot the target list. Empty → the fast
    // path (zero servo traffic when no screencast is active anywhere).
    let targets: Vec<String> = {
        let Ok(map) = MANAGER.sessions.lock() else {
            return;
        };
        if map.is_empty() {
            return;
        }
        map.keys().cloned().collect()
    };

    for target in targets {
        // Phase 2 (unlocked, servo-touching): resolve the page in THIS pool
        // (pool-scoped production) and capture once when anything changed.
        let Some(page_id) = target.parse::<usize>().ok() else {
            continue;
        };
        let Some(page) = pool.get_page(page_id) else {
            // Page closed / foreign pool: no production possible — release.
            drop_target_sessions(&target);
            continue;
        };
        let needs_capture = {
            let Ok(map) = MANAGER.sessions.lock() else {
                return;
            };
            map.get(&target).is_some_and(|list| {
                list.iter()
                    .any(|s| s.first_frame_pending || page.repaint_pending())
            })
        };
        if !needs_capture {
            // No latch and no first frame — but a held frame may now be due
            // (rate cap expiry without new changes is still change-driven:
            // the held frame exists only because a change produced it).
            emit_due_held(&target, ws_sink);
            continue;
        }
        match page.capture_frame() {
            Ok(image) => process_capture(&target, &image, ws_sink),
            Err(e) => {
                // Capture failure is observable and loud; the session stays
                // (transient pipeline states exist mid-navigation) but no
                // frame is fabricated.
                log::warn!("[screencast] target {target} capture failed: {e:?}");
            }
        }
    }
}

/// Drop every session registered for `target` (page gone / client gone).
fn drop_target_sessions(target: &str) {
    if let Ok(mut map) = MANAGER.sessions.lock() {
        map.remove(target);
    }
}

/// Apply the per-session gates to one captured frame and emit what passes.
fn process_capture(target: &str, image: &RgbaImage, ws_sink: Option<&dyn Fn(&str, &str, Value)>) {
    let digest = pixel_digest(image);
    let timestamp = MANAGER.started_at.elapsed().as_secs_f64();
    let mut emissions: Vec<(Origin, u64, Value)> = Vec::new();
    let mut dead_origins: Vec<Origin> = Vec::new();
    {
        let Ok(mut map) = MANAGER.sessions.lock() else {
            return;
        };
        let Some(list) = map.get_mut(target) else {
            return;
        };
        for s in list.iter_mut() {
            s.first_frame_pending = false;
            // Change gate: identical pixels to the last emitted frame.
            if s.last_digest == Some(digest) {
                continue;
            }
            s.frame_no += 1;
            if s.every_nth > 1 && ((s.frame_no - 1) % s.every_nth) != 0 {
                continue;
            }
            let frame = match build_frame(s, image, digest, timestamp) {
                Some(f) => f,
                None => continue,
            };
            // Ack gate then rate gate: while a frame is un-acked or the rate
            // cap is not due, hold the LATEST frame (bounded single slot).
            if s.in_flight || !rate_due(s) {
                s.held = Some(frame);
                continue;
            }
            emit_locked(s, frame, &mut emissions);
        }
    }
    deliver(target, emissions, ws_sink, &mut dead_origins);
    for origin in dead_origins {
        retire_origin(target, origin);
    }
}

/// Emit any held frame whose rate cap has expired (no capture this tick).
fn emit_due_held(target: &str, ws_sink: Option<&dyn Fn(&str, &str, Value)>) {
    let mut emissions: Vec<(Origin, u64, Value)> = Vec::new();
    let mut dead_origins: Vec<Origin> = Vec::new();
    {
        let Ok(mut map) = MANAGER.sessions.lock() else {
            return;
        };
        let Some(list) = map.get_mut(target) else {
            return;
        };
        for s in list.iter_mut() {
            if s.in_flight || s.held.is_none() || !rate_due(s) {
                continue;
            }
            let frame = s.held.take().expect("held checked above");
            emit_locked(s, frame, &mut emissions);
        }
    }
    deliver(target, emissions, ws_sink, &mut dead_origins);
    for origin in dead_origins {
        retire_origin(target, origin);
    }
}

/// Rate-cap check: enough time since the last emitted frame (no cap → due).
fn rate_due(s: &Session) -> bool {
    match (s.min_interval, s.last_emit) {
        (Some(min), Some(last)) => last.elapsed() >= min,
        _ => true,
    }
}

/// Mark `s` as having emitted `frame` (caller delivers after unlock).
///
/// The metadata fields state facts about the delivered frame: full viewport,
/// scale 1, origin-anchored (offsetTop/scroll 0) — the exact capture bao
/// produces, never fabricated scroll state.
fn emit_locked(s: &mut Session, frame: Frame, emissions: &mut Vec<(Origin, u64, Value)>) {
    let params = json!({
        "data": frame.base64,
        "metadata": {
            "offsetTop": 0,
            "pageScaleFactor": 1.0,
            "deviceWidth": frame.device_width,
            "deviceHeight": frame.device_height,
            "scrollOffsetX": 0.0,
            "scrollOffsetY": 0.0,
            "timestamp": frame.timestamp,
        },
        "sessionId": s.id,
    });
    s.in_flight = true;
    s.last_emit = Some(Instant::now());
    s.last_digest = Some(frame.digest);
    s.held = None;
    emissions.push((s.origin, s.id, params));
}

/// Deliver emitted frames to their face sink. Memory delivery failures (no
/// client / client gone) retire the session — fail-closed, no silent drop.
fn deliver(
    target: &str,
    emissions: Vec<(Origin, u64, Value)>,
    ws_sink: Option<&dyn Fn(&str, &str, Value)>,
    dead_origins: &mut Vec<Origin>,
) {
    for (origin, _session_id, params) in emissions {
        match origin {
            Origin::Memory => {
                let Some(sender) = bao_cdp_client::browser::process_memory_event_sender() else {
                    log::warn!(
                        "[screencast] memory client vanished — retiring its session on {target}"
                    );
                    dead_origins.push(origin);
                    continue;
                };
                if sender
                    .send(bao_cdp_client::transport::CdpEvent::new(FRAME_EVENT, params))
                    .is_err()
                {
                    dead_origins.push(origin);
                }
            }
            Origin::Ws => {
                let Some(sink) = ws_sink else {
                    log::warn!(
                        "[screencast] WS session on {target} has no event path in this pump — \
                         retiring (fail-closed)"
                    );
                    dead_origins.push(origin);
                    continue;
                };
                sink(target, FRAME_EVENT, params);
            }
        }
    }
}

/// Remove a face's sessions for `target` (delivery path is gone).
fn retire_origin(target: &str, origin: Origin) {
    if let Ok(mut map) = MANAGER.sessions.lock() {
        if let Some(list) = map.get_mut(target) {
            list.retain(|s| s.origin != origin);
            if list.is_empty() {
                map.remove(target);
            }
        }
    }
}

/// Encode one captured frame for a session: downscale to maxWidth/maxHeight
/// (aspect-preserving, never upscaled), encode with the session format and
/// quality, base64. `None` only when encoding fails (loud at the caller).
fn build_frame(s: &Session, image: &RgbaImage, digest: u64, timestamp: f64) -> Option<Frame> {
    let scaled = downscale(image, s.max_width, s.max_height);
    let bytes = encode_image_with_quality(&scaled, s.format, s.quality).ok()?;
    Some(Frame {
        base64: base64::engine::general_purpose::STANDARD.encode(bytes),
        digest,
        device_width: image.width(),
        device_height: image.height(),
        timestamp,
    })
}

/// Aspect-preserving downscale when the frame exceeds the requested caps
/// (Chrome screencast semantics; never upscales).
fn downscale(image: &RgbaImage, max_width: Option<u32>, max_height: Option<u32>) -> RgbaImage {
    let (w, h) = image.dimensions();
    if w == 0 || h == 0 {
        return image.clone();
    }
    let mut scale = 1.0f64;
    if let Some(max_w) = max_width.filter(|m| *m > 0 && w > *m) {
        scale = scale.min(max_w as f64 / w as f64);
    }
    if let Some(max_h) = max_height.filter(|m| *m > 0 && h > *m) {
        scale = scale.min(max_h as f64 / h as f64);
    }
    if scale >= 1.0 {
        return image.clone();
    }
    let nw = ((w as f64 * scale).round() as u32).max(1);
    let nh = ((h as f64 * scale).round() as u32).max(1);
    image::imageops::resize(image, nw, nh, image::imageops::FilterType::Triangle)
}

/// Exact pixel digest of a captured frame (change gate — hash the RAW
/// capture, not the encoded/scaled form, so downsampling cannot hide a
/// content change from the gate).
fn pixel_digest(image: &RgbaImage) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    image.as_raw().hash(&mut hasher);
    hasher.finish()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn red_image(w: u32, h: u32) -> RgbaImage {
        RgbaImage::from_pixel(w, h, image::Rgba([255, 0, 0, 255]))
    }

    // ── SessionConfig::from_params (protocol shape) ─────────────────────

    #[test]
    fn config_defaults_png_uncapped_every_frame() {
        let c = SessionConfig::from_params(&json!({})).unwrap();
        assert!(matches!(c.format, ScreenshotFormat::Png));
        assert_eq!(c.quality, DEFAULT_JPEG_QUALITY);
        assert_eq!(c.max_width, None);
        assert_eq!(c.max_height, None);
        assert_eq!(c.min_interval, None);
        assert_eq!(c.every_nth, 1);
    }

    // @trace REQ-CDP-009 [criterion:format 参数支持 jpeg 与 png]
    #[test]
    fn config_jpeg_and_png_formats() {
        let jpeg = SessionConfig::from_params(&json!({"format": "jpeg"})).unwrap();
        assert!(matches!(jpeg.format, ScreenshotFormat::Jpeg));
        let png = SessionConfig::from_params(&json!({"format": "png"})).unwrap();
        assert!(matches!(png.format, ScreenshotFormat::Png));
    }

    #[test]
    fn config_unknown_format_fails_closed() {
        assert!(SessionConfig::from_params(&json!({"format": "webp"})).is_err());
    }

    // @trace REQ-CDP-009 [criterion:maxFrameRate 上限可设且生效(出帧频率不超过上限)]
    #[test]
    fn config_max_frame_rate_maps_to_min_interval() {
        let c = SessionConfig::from_params(&json!({"maxFrameRate": 4})).unwrap();
        assert_eq!(c.min_interval, Some(Duration::from_millis(250)));
        // Absent / non-positive → uncapped (change+ack driven only).
        assert_eq!(
            SessionConfig::from_params(&json!({})).unwrap().min_interval,
            None
        );
        assert_eq!(
            SessionConfig::from_params(&json!({"maxFrameRate": 0}))
                .unwrap()
                .min_interval,
            None
        );
    }

    #[test]
    fn config_quality_clamped_and_dims_guarded() {
        let c =
            SessionConfig::from_params(&json!({"quality": 250, "maxWidth": 640, "maxHeight": 0}))
                .unwrap();
        assert_eq!(c.quality, 100);
        assert_eq!(c.max_width, Some(640));
        assert_eq!(c.max_height, None, "zero maxHeight means absent");
    }

    #[test]
    fn config_every_nth_frame_clamped() {
        assert_eq!(
            SessionConfig::from_params(&json!({"everyNthFrame": 3}))
                .unwrap()
                .every_nth,
            3
        );
        assert_eq!(
            SessionConfig::from_params(&json!({"everyNthFrame": 0}))
                .unwrap()
                .every_nth,
            1
        );
    }

    // ── pixel digest / downscale (change gate exactness) ────────────────

    #[test]
    fn pixel_digest_identical_images_equal_distinct_images_differ() {
        let a = red_image(8, 8);
        let b = red_image(8, 8);
        let c = RgbaImage::from_pixel(8, 8, image::Rgba([0, 255, 0, 255]));
        assert_eq!(pixel_digest(&a), pixel_digest(&b));
        assert_ne!(pixel_digest(&a), pixel_digest(&c));
    }

    #[test]
    fn downscale_never_upscales_and_preserves_aspect() {
        let img = red_image(100, 50);
        let out = downscale(&img, Some(50), None);
        assert_eq!(out.dimensions(), (50, 25));
        // Fits → identical copy.
        let same = downscale(&img, Some(200), Some(200));
        assert_eq!(same.dimensions(), (100, 50));
    }

    // ── dispatch routing (face-agnostic core) ───────────────────────────

    #[test]
    fn dispatch_routes_only_the_three_methods() {
        assert!(dispatch(START, &json!({}), "404", Origin::Memory).is_some());
        assert!(dispatch(STOP, &json!({}), "404", Origin::Memory).is_some());
        assert!(dispatch(ACK, &json!({"sessionId": 1}), "404", Origin::Memory).is_some());
        assert!(dispatch("Page.navigate", &json!({}), "1", Origin::Memory).is_none());
    }

    #[test]
    fn ack_requires_numeric_session_id() {
        let r = dispatch(ACK, &json!({}), "1", Origin::Memory).unwrap();
        assert!(r.is_err());
    }

    // @trace REQ-CDP-009 [criterion:Page.startScreencast 建立 change-driven 帧流会话]
    #[test]
    fn start_fails_closed_on_unknown_target() {
        // No page with id 999_999 exists in this process's registry.
        let r = MANAGER
            .start("999999", Origin::Memory, &json!({}))
            .unwrap_err();
        assert!(r.contains("No target with given id found"));
        let r = MANAGER
            .start("not-a-number", Origin::Memory, &json!({}))
            .unwrap_err();
        assert!(r.contains("No target with given id found"));
    }

    #[test]
    fn stop_is_idempotent_ok() {
        assert!(MANAGER.stop("404", Origin::Memory).is_ok());
        assert!(MANAGER.stop("404", Origin::Memory).is_ok());
    }

    #[test]
    fn ack_unknown_session_is_silent_ok() {
        assert!(MANAGER.ack(u64::MAX).is_ok());
    }
}
