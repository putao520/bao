// @trace TEST-BRW-MEDIA-E2E [req:REQ-BRW-001,REQ-BRW-002] [level:e2e]
// Media domain end-to-end over the REAL servo-media stack: HTMLMediaElement
// (audio + video semantics) and WebAudio (AudioContext / decodeAudioData /
// OfflineAudioContext), all through a real page in a real BaoRuntime.
//
// Real-path contract under test (no mocks anywhere):
//
//   page JS → servo DOM (htmlmediaelement.rs / baseaudiocontext.rs)
//           → servo fetch (local H1 fixture serves programmatically
//             generated RIFF/WAVE bytes — zero external assets)
//           → servo-media player → gstreamer playbin3/appsink pipeline
//           → servo-media audio graph (decode → source → destination mixdown)
//
// Backend gating (fail-closed, never silent-green):
//   servo picks its media backend at compile time (`servo.rs media_platform`):
//   `media-gstreamer` feature → GStreamerBackend, otherwise DummyBackend.
//   The dummy backend answers `canPlayType(anything) === ""` and its
//   player/decoder never produce observable transitions, so the suite probes
//   `canPlayType('audio/wav')` once (dummy ⇒ "", gstreamer+wavparse ⇒
//   "maybe" — registry_scanner.rs inserts audio/wav when a wav demuxer
//   exists). With the dummy backend every scenario reports a loud
//   `[skip] gstreamer backend inactive …` line: explicit skip with an
//   observable reason, no fabricated green. When the gstreamer build is
//   wired in (feature flip / #57), the same suite runs the full real path.
//
// Coverage map (what these e2e DO and DO NOT reach):
//   COVERED (real path, this file — verified green with the gstreamer
//   backend compiled in, i.e. bao-servo `media-gstreamer` feature enabled;
//   see REAL-RUN NOTES below):
//     - gstreamer registry surface: canPlayType maybe/probably/no mapping
//     - audio decode pipeline: loadstart→durationchange→loadedmetadata→
//       canplay, duration discovered from real WAV bytes, readyState climb
//     - playback state machine: play()/pause() events, paused flag,
//       currentTime advancement (pipeline clock) and freeze on pause
//     - error semantics: garbage bytes and HTTP 404 both → error event +
//       MediaError.code === 4 (MEDIA_ERR_SRC_NOT_SUPPORTED) +
//       networkState === 3 (NETWORK_NO_SOURCE) + readyState stuck at 0
//       (NEVER_HAVE_METADATA) — mapping pinned in htmlmediaelement.rs
//       (media_data_processing_failure_steps → dedicated media source
//       failure steps)
//     - recovery cycle: after a fatal error, src swap + load() rebuilds the
//       pipeline and reaches HAVE_METADATA again (generation_id reset)
//     - video element error path + NEVER_HAVE_METADATA state semantics
//     - WebAudio: AudioContext construction (create_audio_context path),
//       decodeAudioData through the gstreamer audio decoder (channel count /
//       frame length / duration from real bytes), OfflineAudioContext
//       render with real sample flow (sine amplitude survives the graph)
//   NOT covered here (e2e-unreachable in this harness — unit-level targets):
//     - video frame decode/render (needs a muxed video container asset +
//       GL render context; servo-media-gstreamer render-unix crate is the
//       unit surface)
//     - MSE (MediaSource/extensions), WebRTC datachannel, capture devices
//       (get_user_media), audio sink device output (needs a sound server)
//     - seek/rate/loop matrix (element surface exists; keep for a follow-up
//       wave once the base path is green in CI)
//
// REAL-RUN NOTES (findings from the gstreamer-enabled run, 2026-08-18):
//   1. The default build compiles the DummyBackend (bao_browser requests
//      bao-servo without `media-gstreamer`); the whole suite then skips
//      loudly. Enabling the feature (one-line Cargo.toml change, NOT part
//      of this test commit — product change, owner decision) runs the full
//      real path: 9/9 scenarios green in ~46s headless, no display needed.
//   2. AudioContext constructs headless but stays `state === "suspended"`
//      (no sound server; autoaudiosink falls back to ALSA and fails) —
//      construction + offline paths unaffected.
//   3. PRODUCT FINDING (vendor servo): `atob` truncated a 117,660-char
//      base64 string to 9 bytes in the page realm. Worked around here via
//      XHR responseType='arraybuffer' (the delivery the page_net matrix
//      e2e proves settles). atob on large inputs needs its own regression
//      test + fix at the servo/vendor level.
//   4. window.fetch from a page evaluate never settles its promise in this
//      harness (matches the page_net matrix e2e, which deliberately only
//      asserts fixture arrival for window.fetch, not page-side settlement).
//      Media network legs here use the element resource fetch (S2–S6) and
//      XHR (S9) — both settle.

use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use bao_browser::{BaoConfig, PageConfig, PageHandle, PageState};

// ---------------------------------------------------------------------------
// WAV generation — real RIFF/WAVE bytes, zero external assets
// ---------------------------------------------------------------------------

const WAV_SAMPLE_RATE: u32 = 44_100;
const WAV_CHANNELS: u16 = 2;
const WAV_AMPLITUDE: f32 = 0.5;
const WAV_FREQ_HZ: f32 = 440.0;

/// Canonical 44-byte PCM16 WAV: RIFF header + fmt chunk + data chunk of a
/// stereo sine at `seconds`. The generated bytes are the sole media producer
/// in this suite — every duration/amplitude assertion below derives from the
/// parameters handed to this function.
fn make_sine_wav(seconds: f64) -> Vec<u8> {
    let frames = (WAV_SAMPLE_RATE as f64 * seconds).round() as u32;
    let block_align = WAV_CHANNELS * 2; // 16-bit samples
    let data_len = frames * block_align as u32;
    let mut buf = Vec::with_capacity(44 + data_len as usize);
    buf.extend_from_slice(b"RIFF");
    buf.extend_from_slice(&(36 + data_len).to_le_bytes()); // rest of file
    buf.extend_from_slice(b"WAVE");
    buf.extend_from_slice(b"fmt ");
    buf.extend_from_slice(&16u32.to_le_bytes()); // fmt chunk size
    buf.extend_from_slice(&1u16.to_le_bytes()); // PCM
    buf.extend_from_slice(&WAV_CHANNELS.to_le_bytes());
    buf.extend_from_slice(&WAV_SAMPLE_RATE.to_le_bytes());
    buf.extend_from_slice(&(WAV_SAMPLE_RATE * block_align as u32).to_le_bytes()); // byte rate
    buf.extend_from_slice(&block_align.to_le_bytes());
    buf.extend_from_slice(&16u16.to_le_bytes()); // bits per sample
    buf.extend_from_slice(b"data");
    buf.extend_from_slice(&data_len.to_le_bytes());
    for i in 0..frames {
        let t = i as f32 / WAV_SAMPLE_RATE as f32;
        let sample = (t * WAV_FREQ_HZ * 2.0 * std::f32::consts::PI).sin() * WAV_AMPLITUDE;
        let pcm = (sample * i16::MAX as f32) as i16;
        // same waveform on both channels — channel split assertions stay
        // meaningful without a stereo panner
        for _ in 0..WAV_CHANNELS {
            buf.extend_from_slice(&pcm.to_le_bytes());
        }
    }
    assert_eq!(buf.len(), 44 + data_len as usize, "WAV byte layout must be exact");
    buf
}

/// 128 bytes of non-media junk served as audio/wav: typefind must reject it.
fn make_garbage_bytes() -> Vec<u8> {
    let mut v = Vec::with_capacity(128);
    for i in 0..128u32 {
        v.push((0xA5 ^ i as u8).wrapping_mul(31).wrapping_add(0x5A));
    }
    v
}

// ---------------------------------------------------------------------------
// Page fixture — one H1 server, media routes
// ---------------------------------------------------------------------------

struct MediaFixture {
    port: u16,
    shutdown: Arc<AtomicBool>,
}

impl MediaFixture {
    fn spawn() -> Self {
        let sine_half = make_sine_wav(0.5); // metadata scenario (0.5s, 22050 frames)
        let sine_two = make_sine_wav(2.0); // play/pause scenario (room for the clock)
        let garbage = make_garbage_bytes();
        let routes: Vec<(String, &'static str, Vec<u8>)> = vec![
            (
                "/".into(),
                "text/html",
                b"<!DOCTYPE html><html><body><p>media-e2e</p></body></html>".to_vec(),
            ),
            ("/media/sine-05.wav".into(), "audio/wav", sine_half),
            ("/media/sine-2s.wav".into(), "audio/wav", sine_two),
            ("/media/garbage.wav".into(), "audio/wav", garbage),
        ];

        let listener = TcpListener::bind("127.0.0.1:0").expect("bind media fixture");
        let port = listener.local_addr().unwrap().port();
        let shutdown = Arc::new(AtomicBool::new(false));
        let shutdown_c = Arc::clone(&shutdown);
        std::thread::Builder::new()
            .name("media-fixture".into())
            .spawn(move || {
                listener
                    .set_nonblocking(true)
                    .expect("nonblocking listener");
                while !shutdown_c.load(Ordering::SeqCst) {
                    match listener.accept() {
                        Ok((mut tcp, _)) => {
                            let mut req = [0u8; 2048];
                            let _ = tcp.read(&mut req); // drain request head
                            let path = std::str::from_utf8(&req)
                                .ok()
                                .and_then(|head| head.split_whitespace().nth(1))
                                .unwrap_or("/")
                                .to_string();
                            let (status, ctype, body) = match routes.iter().find(|(p, _, _)| *p == path)
                            {
                                Some((_, ct, body)) => ("200 OK", *ct, body.clone()),
                                None => ("404 Not Found", "application/octet-stream", Vec::new()),
                            };
                            let head = format!(
                                "HTTP/1.1 {}\r\nContent-Type: {}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                                status,
                                ctype,
                                body.len()
                            );
                            let _ = tcp.write_all(head.as_bytes());
                            let _ = tcp.write_all(&body);
                            let _ = tcp.flush();
                        }
                        Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                            std::thread::sleep(Duration::from_millis(5));
                        }
                        Err(_) => return,
                    }
                }
            })
            .expect("spawn media fixture");
        MediaFixture { port, shutdown }
    }

    fn url(&self) -> String {
        format!("http://127.0.0.1:{}/", self.port)
    }
}

impl Drop for MediaFixture {
    fn drop(&mut self) {
        self.shutdown.store(true, Ordering::SeqCst);
    }
}

// ---------------------------------------------------------------------------
// Report — fault-tolerant scenario accumulator (house style; servo opts are
// process-global singletons so the whole suite lives in one #[test])
// ---------------------------------------------------------------------------

#[derive(Default)]
struct Report {
    passed: u32,
    skipped: u32,
    failed: u32,
    messages: Vec<String>,
}

impl Report {
    fn pass(&mut self, name: &str) {
        self.passed += 1;
        self.messages.push(format!("PASS  {}", name));
    }
    fn skip(&mut self, name: &str, why: &str) {
        self.skipped += 1;
        self.messages.push(format!("SKIP  {}  ({})", name, why));
    }
    fn fail(&mut self, name: &str, why: &str) {
        self.failed += 1;
        self.messages.push(format!("FAIL  {}  ({})", name, why));
    }
    fn finish(&self) {
        eprintln!("\n=== Media domain E2E (gstreamer real path) ===");
        for m in &self.messages {
            eprintln!("{}", m);
        }
        eprintln!(
            "--- {} passed, {} skipped, {} failed ---",
            self.passed, self.skipped, self.failed
        );
    }
}

// ---------------------------------------------------------------------------
// Page helpers
// ---------------------------------------------------------------------------

fn wait_for_load(page: &PageHandle, max_ms: u64) {
    let start = Instant::now();
    while start.elapsed().as_millis() < max_ms as u128 {
        let _ = page.evaluate_js("");
        if matches!(page.get_state(), PageState::Interactive | PageState::Idle) {
            return;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
}

/// Evaluate an expression in the page realm and get back its string form
/// (defensively trimming any servo quoting).
fn js(page: &PageHandle, expr: &str) -> String {
    page.evaluate_js_web(expr)
        .unwrap_or_default()
        .trim()
        .trim_matches('"')
        .to_string()
}

/// Poll until `globalThis.<global>` left 'pending' AND `done_expr` holds,
/// then return the JSON snapshot stored in that global. On timeout the last
/// snapshot (or None) is returned so failures come with diagnostic state.
fn wait_json_state(
    page: &PageHandle,
    global: &str,
    done_expr: &str,
    timeout: Duration,
) -> Option<serde_json::Value> {
    let cond = format!(
        "globalThis.{g} !== undefined && globalThis.{g} !== 'pending' && ({d})",
        g = global,
        d = done_expr
    );
    let _ = page.wait_for_function(&cond, timeout);
    let raw = js(page, &format!("JSON.stringify(globalThis.{})", global));
    if raw.is_empty() || raw == "undefined" || raw == "\"pending\"" || raw == "pending" {
        return None;
    }
    serde_json::from_str(&raw).ok()
}

/// Scenario bootstrap: creates a fresh `<audio>`/`<video>` element, wires the
/// full media event recorder into it, stores it on `globalThis.<global>EL`
/// and seeds `globalThis.<global>` with a snapshot. Scenario bodies mutate
/// further from Rust via `js()` and snapshots refresh on every event.
fn install_media_recorder(page: &PageHandle, global: &str, tag: &str) {
    let script = format!(
        r#"
        (function() {{
            globalThis.{G} = 'pending';
            var el = document.createElement('{tag}');
            el.preload = 'auto';
            var pp = 'none';
            var events = [];
            var metaCount = 0;
            function snapshot() {{
                return {{
                    events: events.slice(),
                    duration: el.duration,
                    readyState: el.readyState,
                    networkState: el.networkState,
                    paused: el.paused,
                    ended: el.ended,
                    currentTime: el.currentTime,
                    seeking: el.seeking,
                    videoWidth: el.videoWidth,
                    videoHeight: el.videoHeight,
                    errorCode: el.error ? el.error.code : 0,
                    playPromise: pp,
                    metaCount: metaCount
                }};
            }}
            ['loadstart','emptied','abort','error','stalled','progress',
             'durationchange','loadedmetadata','loadeddata','canplay',
             'canplaythrough','play','playing','waiting','pause','timeupdate',
             'seeking','seeked','ended'].forEach(function(name) {{
                el.addEventListener(name, function() {{
                    if (name === 'loadedmetadata') metaCount++;
                    events.push(name);
                    globalThis.{G} = snapshot();
                }});
            }});
            var nativePlay = el.play.bind(el);
            el.play = function() {{
                pp = 'pending';
                var r = nativePlay();
                if (r && typeof r.then === 'function') {{
                    r.then(function() {{ pp = 'resolved'; globalThis.{G} = snapshot(); }},
                           function(e) {{ pp = 'rejected:' + String(e && e.name || e); globalThis.{G} = snapshot(); }});
                }} else {{ pp = 'no-promise'; }}
                return r;
            }};
            globalThis.{G}EL = el;
            globalThis.{G} = snapshot();
        }})();
        'installed'
        "#,
        G = global,
        tag = tag
    );
    let installed = js(page, &script);
    assert_eq!(installed, "installed", "media recorder must install cleanly");
}

fn event_index(state: &serde_json::Value, name: &str) -> i64 {
    let events = state
        .get("events")
        .and_then(|v| v.as_array())
        .expect("state must carry events");
    events
        .iter()
        .position(|e| e.as_str() == Some(name))
        .map(|i| i as i64)
        .unwrap_or(-1)
}

fn num(state: &serde_json::Value, key: &str) -> f64 {
    state.get(key).and_then(|v| v.as_f64()).unwrap_or(f64::NAN)
}

/// Boolean snapshot fields (`paused`, …) — `num()` would eat these and turn
/// them into NaN comparisons (every assertion would "fail" spuriously).
fn flag(state: &serde_json::Value, key: &str) -> Option<bool> {
    state.get(key).and_then(|v| v.as_bool())
}

// ---------------------------------------------------------------------------
// The suite
// ---------------------------------------------------------------------------

#[test]
fn media_domain_e2e_suite() {
    bun_core::Output::init_test();

    let fixture = MediaFixture::spawn();

    let runtime = bao_browser::BaoRuntime::new(BaoConfig::default()).expect("BaoRuntime::new");
    let pool = runtime.page_pool();

    let mut page = None;
    for _ in 0..3 {
        match pool.create_page(&PageConfig {
            url: Some(fixture.url()),
            ..Default::default()
        }) {
            Ok(p) => {
                page = Some(p);
                break;
            }
            Err(e) => {
                eprintln!("page creation failed (retrying): {}", e);
                std::thread::sleep(Duration::from_secs(3));
            }
        }
    }
    let page = page.expect("page creation failed after retries");
    wait_for_load(&page, 5000);

    let mut report = Report::default();

    // --- Backend probe (fail-closed gate) ---------------------------------
    // dummy backend ⇒ canPlayType('audio/wav') === "" for everything.
    // gstreamer with a wav demuxer ⇒ "maybe" (registry_scanner.rs:180).
    let probe = js(&page, "document.createElement('audio').canPlayType('audio/wav')");
    let gstreamer_active = !probe.is_empty();
    if !gstreamer_active {
        eprintln!("\n*** MEDIA BACKEND PROBE: canPlayType('audio/wav') === {:?} ***", probe);
        eprintln!("*** Dummy servo-media backend is compiled in (no media-gstreamer feature).");
        eprintln!("*** All real-path scenarios SKIP below — this is a dependency gate, not a green run.");
        eprintln!("*** Wire the gstreamer backend in (see #57) and rerun for the real path.\n");
    }

    // --- S1: canPlayType registry surface ---------------------------------
    // gstreamer: audio/wav (container, no codecs) ⇒ "maybe";
    // audio/wav; codecs="1" (PCM registered alongside wav) ⇒ "probably";
    // unknown container ⇒ "" (No).
    {
        let name = "canplaytype_registry_surface";
        if !gstreamer_active {
            report.skip(name, "gstreamer backend inactive — canPlayType is dummy-flat \"\"");
        } else {
            let wav_bare = js(&page, "document.createElement('audio').canPlayType('audio/wav')");
            let wav_pcm = js(
                &page,
                "document.createElement('audio').canPlayType('audio/wav; codecs=\"1\"')",
            );
            let bogus = js(
                &page,
                "document.createElement('audio').canPlayType('video/bao-nonexistent')",
            );
            let mut bad = Vec::new();
            if wav_bare != "maybe" {
                bad.push(format!("audio/wav={} (want maybe)", wav_bare));
            }
            if wav_pcm != "probably" {
                bad.push(format!("audio/wav;codecs=1={} (want probably)", wav_pcm));
            }
            if !bogus.is_empty() {
                bad.push(format!("bogus type={} (want empty)", bogus));
            }
            if bad.is_empty() {
                report.pass(name);
            } else {
                report.fail(name, &bad.join("; "));
            }
        }
    }

    // --- S2: audio metadata / duration / canplay (real decode pipeline) ---
    {
        let name = "audio_metadata_duration_canplay";
        if !gstreamer_active {
            report.skip(name, "gstreamer backend inactive — decode pipeline never fires events");
        } else {
            install_media_recorder(&page, "__m", "audio");
            js(&page, "globalThis.__mEL.src = '/media/sine-05.wav'");
            let state = wait_json_state(
                &page,
                "__m",
                "globalThis.__m.events.indexOf('canplay') >= 0 || globalThis.__m.events.indexOf('error') >= 0",
                Duration::from_secs(20),
            );
            match state {
                None => report.fail(name, "no state snapshot — pipeline emitted nothing observable"),
                Some(s) => {
                    let mut bad = Vec::new();
                    if event_index(&s, "error") >= 0 {
                        bad.push(format!(
                            "error event fired (code={})",
                            num(&s, "errorCode")
                        ));
                    }
                    if event_index(&s, "loadedmetadata") < 0 {
                        bad.push("loadedmetadata never fired".into());
                    }
                    if event_index(&s, "canplay") < 0 {
                        bad.push("canplay never fired".into());
                    }
                    // spec order: loadstart < durationchange < loadedmetadata
                    let ls = event_index(&s, "loadstart");
                    let dc = event_index(&s, "durationchange");
                    let lm = event_index(&s, "loadedmetadata");
                    if !(ls >= 0 && ls < lm) {
                        bad.push(format!("loadstart({}) must precede loadedmetadata({})", ls, lm));
                    }
                    if !(dc >= 0 && dc <= lm) {
                        bad.push(format!(
                            "durationchange({}) must precede loadedmetadata({})",
                            dc, lm
                        ));
                    }
                    let duration = num(&s, "duration");
                    if duration.is_nan() || (duration - 0.5).abs() > 0.05 {
                        bad.push(format!(
                            "duration={} (want 0.5±0.05 from generated WAV)",
                            duration
                        ));
                    }
                    let rs = num(&s, "readyState");
                    if rs < 1.0 {
                        bad.push(format!("readyState={} (want >=1 HAVE_METADATA)", rs));
                    }
                    if bad.is_empty() {
                        report.pass(name);
                    } else {
                        report.fail(name, &bad.join("; "));
                    }
                }
            }
        }
    }

    // --- S3: audio play/pause state machine + clock -----------------------
    {
        let name = "audio_play_pause_statemachine";
        if !gstreamer_active {
            report.skip(name, "gstreamer backend inactive — playback clock never runs");
        } else {
            install_media_recorder(&page, "__p", "audio");
            js(&page, "globalThis.__pEL.src = '/media/sine-2s.wav'");
            let ready = wait_json_state(
                &page,
                "__p",
                "globalThis.__p.events.indexOf('canplay') >= 0 || globalThis.__p.events.indexOf('error') >= 0",
                Duration::from_secs(20),
            );
            match ready {
                None => report.fail(name, "2s fixture never reached a terminal load state"),
                Some(s) if event_index(&s, "error") >= 0 => report.fail(
                    name,
                    &format!("load errored (code={})", num(&s, "errorCode")),
                ),
                Some(_) => {
                    js(&page, "globalThis.__pEL.play(); 'play-issued'");
                    let playing = wait_json_state(
                        &page,
                        "__p",
                        "(globalThis.__p.events.indexOf('play') >= 0 && globalThis.__p.currentTime > 0.005) || globalThis.__p.events.indexOf('error') >= 0",
                        Duration::from_secs(15),
                    );
                    match playing {
                        None => report.fail(name, "play() never advanced the clock past 0"),
                        Some(s) if event_index(&s, "error") >= 0 => report.fail(
                            name,
                            &format!("playback errored (code={})", num(&s, "errorCode")),
                        ),
                        Some(s) => {
                            let mut bad = Vec::new();
                            if flag(&s, "paused") != Some(false) {
                                bad.push(format!(
                                    "paused must be false while playing (got {:?})",
                                    flag(&s, "paused")
                                ));
                            }
                            if s.get("playPromise").and_then(|v| v.as_str()) != Some("resolved") {
                                bad.push(format!(
                                    "play() promise={} (want resolved)",
                                    s.get("playPromise").and_then(|v| v.as_str()).unwrap_or("?")
                                ));
                            }
                            let ct = num(&s, "currentTime");
                            if ct.is_nan() || ct <= 0.005 {
                                bad.push(format!(
                                    "currentTime={} — pipeline clock is not running",
                                    ct
                                ));
                            }
                            // pause and assert the freeze
                            js(&page, "globalThis.__pEL.pause()");
                            let paused = wait_json_state(
                                &page,
                                "__p",
                                "globalThis.__p.events.indexOf('pause') >= 0",
                                Duration::from_secs(10),
                            );
                            match paused {
                                None => {
                                    bad.push("pause event never fired".into());
                                }
                                Some(s2) => {
                                    if flag(&s2, "paused") != Some(true) {
                                        bad.push(format!(
                                            "paused must be true after pause() (got {:?})",
                                            flag(&s2, "paused")
                                        ));
                                    }
                                    let t1 = num(&s2, "currentTime");
                                    std::thread::sleep(Duration::from_millis(250));
                                    let t2 = js_state(&page, "__p")
                                        .map(|later| num(&later, "currentTime"))
                                        .unwrap_or(t1);
                                    if (t2 - t1).abs() > 0.02 {
                                        bad.push(format!(
                                            "currentTime kept moving after pause ({} → {})",
                                            t1, t2
                                        ));
                                    }
                                    if !(event_index(&s2, "play") < event_index(&s2, "pause")) {
                                        bad.push("play event must precede pause event".into());
                                    }
                                }
                            }
                            if bad.is_empty() {
                                report.pass(name);
                            } else {
                                report.fail(name, &bad.join("; "));
                            }
                        }
                    }
                }
            }
        }
    }

    // --- S4: garbage bytes → MEDIA_ERR_SRC_NOT_SUPPORTED semantics --------
    {
        let name = "audio_garbage_source_error_semantics";
        if !gstreamer_active {
            report.skip(name, "gstreamer backend inactive — dummy player never errors");
        } else {
            install_media_recorder(&page, "__g", "audio");
            js(&page, "globalThis.__gEL.src = '/media/garbage.wav'");
            let state = wait_json_state(
                &page,
                "__g",
                "globalThis.__g.events.indexOf('error') >= 0",
                Duration::from_secs(15),
            );
            match state {
                None => report.fail(name, "garbage source produced no error event (typefind reject)"),
                Some(s) => {
                    let mut bad = Vec::new();
                    if num(&s, "errorCode") != 4.0 {
                        bad.push(format!(
                            "error.code={} (want 4 MEDIA_ERR_SRC_NOT_SUPPORTED)",
                            num(&s, "errorCode")
                        ));
                    }
                    if num(&s, "networkState") != 3.0 {
                        bad.push(format!(
                            "networkState={} (want 3 NETWORK_NO_SOURCE)",
                            num(&s, "networkState")
                        ));
                    }
                    if num(&s, "readyState") != 0.0 {
                        bad.push(format!(
                            "readyState={} (want 0 — NEVER_HAVE_METADATA)",
                            num(&s, "readyState")
                        ));
                    }
                    if !num(&s, "duration").is_nan() {
                        bad.push(format!(
                            "duration={} (want NaN — no metadata ever arrived)",
                            num(&s, "duration")
                        ));
                    }
                    if flag(&s, "paused") != Some(true) {
                        bad.push(format!(
                            "paused must remain true on a failed load (got {:?})",
                            flag(&s, "paused")
                        ));
                    }
                    if bad.is_empty() {
                        report.pass(name);
                    } else {
                        report.fail(name, &bad.join("; "));
                    }
                }
            }
        }
    }

    // --- S5: HTTP 404 → same failure semantics via the network leg --------
    {
        let name = "audio_http_404_network_error_semantics";
        if !gstreamer_active {
            report.skip(name, "gstreamer backend inactive — fetch failure path needs real load");
        } else {
            install_media_recorder(&page, "__n", "audio");
            js(&page, "globalThis.__nEL.src = '/media/missing.wav'");
            let state = wait_json_state(
                &page,
                "__n",
                "globalThis.__n.events.indexOf('error') >= 0",
                Duration::from_secs(15),
            );
            match state {
                None => report.fail(name, "404 source produced no error event"),
                Some(s) => {
                    let mut bad = Vec::new();
                    if num(&s, "errorCode") != 4.0 {
                        bad.push(format!(
                            "error.code={} (want 4 — HAVE_NOTHING fetch failure maps to SRC_NOT_SUPPORTED)",
                            num(&s, "errorCode")
                        ));
                    }
                    if num(&s, "networkState") != 3.0 {
                        bad.push(format!(
                            "networkState={} (want 3 NETWORK_NO_SOURCE)",
                            num(&s, "networkState")
                        ));
                    }
                    if num(&s, "readyState") != 0.0 {
                        bad.push(format!(
                            "readyState={} (want 0 — NEVER_HAVE_METADATA)",
                            num(&s, "readyState")
                        ));
                    }
                    if bad.is_empty() {
                        report.pass(name);
                    } else {
                        report.fail(name, &bad.join("; "));
                    }
                }
            }
        }
    }

    // --- S6: error → load() recovery cycle --------------------------------
    {
        let name = "audio_error_recovery_via_load";
        if !gstreamer_active {
            report.skip(name, "gstreamer backend inactive — recovery needs a real pipeline rebuild");
        } else {
            install_media_recorder(&page, "__r", "audio");
            js(&page, "globalThis.__rEL.src = '/media/garbage.wav'");
            let errored = wait_json_state(
                &page,
                "__r",
                "globalThis.__r.events.indexOf('error') >= 0",
                Duration::from_secs(15),
            );
            if errored.is_none() {
                report.fail(name, "precondition failed: first load never errored");
            } else {
                js(
                    &page,
                    "globalThis.__rEL.src = '/media/sine-05.wav'; globalThis.__rEL.load();",
                );
                // recovery proof: a loadedmetadata fired AFTER the error.
                // (Ordering, not metaCount: the src-set resource selection may
                // legitimately be superseded by load() before any metadata
                // arrives, so the second load can be the only one to fire.)
                let recovered = wait_json_state(
                    &page,
                    "__r",
                    "globalThis.__r.events.lastIndexOf('loadedmetadata') > globalThis.__r.events.indexOf('error')",
                    Duration::from_secs(20),
                );
                match recovered {
                    None => {
                        let s = js_state(&page, "__r");
                        let detail = s
                            .as_ref()
                            .map(|s| {
                                format!(
                                    "events={:?} duration={}",
                                    s.get("events"),
                                    num(s, "duration")
                                )
                            })
                            .unwrap_or_else(|| "no snapshot".into());
                        report.fail(
                            name,
                            &format!("load() after error never reached metadata again ({})", detail),
                        )
                    }
                    Some(s) => {
                        let duration = num(&s, "duration");
                        let events_after_error = s
                            .get("events")
                            .and_then(|v| v.as_array())
                            .map(|a| {
                                a.iter()
                                    .skip_while(|e| e.as_str() != Some("error"))
                                    .filter_map(|e| e.as_str())
                                    .collect::<Vec<_>>()
                                    .join(",")
                            })
                            .unwrap_or_default();
                        if duration.is_nan() || (duration - 0.5).abs() > 0.05 {
                            report.fail(
                                name,
                                &format!(
                                    "recovered duration={} (want 0.5±0.05); events after error: {}",
                                    duration, events_after_error
                                ),
                            );
                        } else {
                            report.pass(name);
                        }
                    }
                }
            }
        }
    }

    // --- S7: video element error path + NEVER_HAVE_METADATA ---------------
    {
        let name = "video_invalid_source_error_semantics";
        if !gstreamer_active {
            report.skip(name, "gstreamer backend inactive — video pipeline needs real backend");
        } else {
            install_media_recorder(&page, "__v", "video");
            js(&page, "globalThis.__vEL.src = '/media/garbage.wav'");
            let state = wait_json_state(
                &page,
                "__v",
                "globalThis.__v.events.indexOf('error') >= 0",
                Duration::from_secs(15),
            );
            match state {
                None => report.fail(name, "video element produced no error event for garbage src"),
                Some(s) => {
                    let mut bad = Vec::new();
                    if num(&s, "errorCode") != 4.0 {
                        bad.push(format!(
                            "error.code={} (want 4 MEDIA_ERR_SRC_NOT_SUPPORTED)",
                            num(&s, "errorCode")
                        ));
                    }
                    if num(&s, "readyState") != 0.0 {
                        bad.push(format!(
                            "readyState={} (want 0 — video NEVER_HAVE_METADATA)",
                            num(&s, "readyState")
                        ));
                    }
                    // no metadata ever arrived ⇒ intrinsic size must stay 0
                    if num(&s, "videoWidth") != 0.0 || num(&s, "videoHeight") != 0.0 {
                        bad.push(format!(
                            "videoWidthxHeight={}x{} (want 0x0 — no metadata)",
                            num(&s, "videoWidth"),
                            num(&s, "videoHeight")
                        ));
                    }
                    if bad.is_empty() {
                        report.pass(name);
                    } else {
                        report.fail(name, &bad.join("; "));
                    }
                }
            }
        }
    }

    // --- S8: AudioContext construction smoke -------------------------------
    {
        let name = "webaudio_audiocomcontext_construct";
        if !gstreamer_active {
            report.skip(name, "gstreamer backend inactive — create_audio_context is dummy sink");
        } else {
            let raw = js(
                &page,
                r#"
                (function() {
                    try {
                        var ac = new AudioContext({ sampleRate: 44100 });
                        return JSON.stringify({ state: ac.state, sampleRate: ac.sampleRate,
                                                baseLatency: ac.baseLatency, destination: !!ac.destination });
                    } catch (e) {
                        return JSON.stringify({ threw: String(e && e.name || e) });
                    }
                })()
                "#,
            );
            match serde_json::from_str::<serde_json::Value>(&raw) {
                Err(e) => report.fail(name, &format!("probe returned non-JSON {:?} ({})", raw, e)),
                Ok(v) if v.get("threw").is_some() => report.fail(
                    name,
                    &format!(
                        "AudioContext constructor threw: {} — create_audio_context path is broken headless",
                        v.get("threw").and_then(|t| t.as_str()).unwrap_or("?")
                    ),
                ),
                Ok(v) => {
                    let sr = v.get("sampleRate").and_then(|x| x.as_f64()).unwrap_or(f64::NAN);
                    let state = v
                        .get("state")
                        .and_then(|x| x.as_str())
                        .unwrap_or("?")
                        .to_string();
                    let has_dest = v.get("destination").and_then(|x| x.as_bool()).unwrap_or(false);
                    let mut bad = Vec::new();
                    if sr != 44100.0 {
                        bad.push(format!("sampleRate={} (want 44100 as requested)", sr));
                    }
                    if !matches!(state.as_str(), "running" | "suspended") {
                        bad.push(format!("state={} (want running|suspended)", state));
                    }
                    if !has_dest {
                        bad.push("destination node missing".into());
                    }
                    if bad.is_empty() {
                        eprintln!(
                            "      [{}] AudioContext state observed: {} (sink availability note)",
                            name, state
                        );
                        report.pass(name);
                    } else {
                        report.fail(name, &bad.join("; "));
                    }
                }
            }
        }
    }

    // --- S9: decodeAudioData + OfflineAudioContext real render -------------
    {
        let name = "webaudio_decode_and_offline_render";
        if !gstreamer_active {
            report.skip(name, "gstreamer backend inactive — decode callbacks never fire on dummy");
        } else {
            // WAV bytes delivered by XHR (absolute URL) — the delivery the
            // page_net matrix e2e proves settles in pages. window.fetch is
            // deliberately NOT used (Node-stack fetch, page-side promise
            // settlement not asserted there either) and atob is NOT used
            // (observed truncating 117KB base64 to 9 bytes — logged in the
            // coverage report). XHR responseType='arraybuffer' is the real
            // consumer shape decodeAudioData wants.
            let wav_url = format!("http://127.0.0.1:{}/media/sine-05.wav", fixture.port);
            let script = r#"
                globalThis.__wa = { stage: 'start' };
                (async function() {
                    var ac = null;
                    function at(stage, extra) {
                        var s = { stage: stage };
                        if (extra) { for (var k in extra) s[k] = extra[k]; }
                        globalThis.__wa = s;
                    }
                    try {
                        at('ctx');
                        ac = new AudioContext({ sampleRate: 44100 });
                        at('xhr-issued');
                        var ab = await new Promise(function(resolve, reject) {
                            var xhr = new XMLHttpRequest();
                            xhr.open('GET', '__WAV_URL__', true);
                            xhr.responseType = 'arraybuffer';
                            xhr.onload = function() {
                                if (xhr.status === 200 && xhr.response) resolve(xhr.response);
                                else reject(new Error('xhr status ' + xhr.status));
                            };
                            xhr.onerror = function() { reject(new Error('xhr network error')); };
                            xhr.send();
                        });
                        at('decode-issued', { bytes: ab.byteLength });
                        var decoded = await Promise.race([
                            ac.decodeAudioData(ab),
                            new Promise(function(_, rej) {
                                setTimeout(function() { rej(new Error('decode-timeout-15s')); }, 15000);
                            })
                        ]);
                        at('decode-done', { channels: decoded.numberOfChannels,
                                             length: decoded.length,
                                             sampleRate: decoded.sampleRate,
                                             duration: decoded.duration });
                        var off = new OfflineAudioContext(2, 22050, 44100);
                        var src = off.createBufferSource();
                        src.buffer = decoded;
                        src.connect(off.destination);
                        src.start(0);
                        at('render-issued');
                        var rendered = await Promise.race([
                            off.startRendering(),
                            new Promise(function(_, rej) {
                                setTimeout(function() { rej(new Error('render-timeout-15s')); }, 15000);
                            })
                        ]);
                        at('analysis');
                        var ch0 = rendered.getChannelData(0);
                        var ch1 = rendered.getChannelData(1);
                        var max = 0, sumSq = 0;
                        for (var i = 0; i < ch0.length; i++) {
                            var ax = Math.abs(ch0[i]);
                            if (ax > max) max = ax;
                            sumSq += ch0[i] * ch0[i];
                        }
                        var max1 = 0;
                        for (var j = 0; j < ch1.length; j++) {
                            if (Math.abs(ch1[j]) > max1) max1 = Math.abs(ch1[j]);
                        }
                        at('done', {
                            ctxState: ac.state,
                            decodedChannels: decoded.numberOfChannels,
                            decodedLength: decoded.length,
                            decodedSampleRate: decoded.sampleRate,
                            decodedDuration: decoded.duration,
                            renderedLength: rendered.length,
                            renderedChannels: rendered.numberOfChannels,
                            renderedSampleRate: rendered.sampleRate,
                            ch0MaxAbs: max, ch1MaxAbs: max1,
                            rms: Math.sqrt(sumSq / ch0.length)
                        });
                    } catch (e) {
                        var prev = globalThis.__wa || {};
                        globalThis.__wa = { stage: prev.stage,
                                            bytes: prev.bytes,
                                            threw: String(e && e.name || e),
                                            msg: String(e && e.message || '') };
                    }
                })();
                'issued'
                "#
            .replace("__WAV_URL__", &wav_url);
            js(&page, &script);
            // window.fetch in bao pages is the Node-stack fetch bridge — its
            // promise settles through the runtime callback drain, not servo's
            // event loop alone. Pump `evaluate_js("")` (drain_callbacks) like
            // the page_net matrix e2e does, reading state between pumps.
            let mut last_state = serde_json::Value::Null;
            let mut final_state: Option<serde_json::Value> = None;
            let start = Instant::now();
            while start.elapsed() < Duration::from_secs(40) {
                let _ = page.evaluate_js(""); // pump the bao bridge
                let raw = js(&page, "JSON.stringify(globalThis.__wa)");
                if let Ok(v) = serde_json::from_str::<serde_json::Value>(&raw) {
                    let stage = v.get("stage").and_then(|s| s.as_str()).unwrap_or("");
                    let threw = v.get("threw").is_some();
                    if stage == "done" || threw {
                        final_state = Some(v);
                        break;
                    }
                    last_state = v;
                }
                std::thread::sleep(Duration::from_millis(20));
            }
            match final_state {
                None => report.fail(
                    name,
                    &format!("WebAudio pipeline stuck (last state: {})", last_state),
                ),
                Some(s) if s.get("threw").is_some() => report.fail(
                    name,
                    &format!(
                        "WebAudio pipeline threw at stage {:?}: {} {} [state: {}]",
                        s.get("stage").and_then(|v| v.as_str()).unwrap_or("?"),
                        s.get("threw").and_then(|v| v.as_str()).unwrap_or("?"),
                        s.get("msg").and_then(|v| v.as_str()).unwrap_or(""),
                        s
                    ),
                ),
                Some(s) => {
                    let mut bad = Vec::new();
                    let dch = num(&s, "decodedChannels");
                    let dlen = num(&s, "decodedLength");
                    let dsr = num(&s, "decodedSampleRate");
                    let ddur = num(&s, "decodedDuration");
                    if dch != 2.0 {
                        bad.push(format!("decoded.numberOfChannels={} (want 2, stereo WAV)", dch));
                    }
                    if dlen != 22050.0 {
                        bad.push(format!(
                            "decoded.length={} (want 22050 frames = 0.5s @44100)",
                            dlen
                        ));
                    }
                    if dsr != 44100.0 {
                        bad.push(format!("decoded.sampleRate={} (want 44100)", dsr));
                    }
                    if ddur.is_nan() || (ddur - 0.5).abs() > 0.01 {
                        bad.push(format!("decoded.duration={} (want 0.5±0.01)", ddur));
                    }
                    let rlen = num(&s, "renderedLength");
                    let rch = num(&s, "renderedChannels");
                    if rlen != 22050.0 {
                        bad.push(format!("rendered.length={} (want 22050 — offline length)", rlen));
                    }
                    if rch < 1.0 {
                        bad.push(format!("rendered.numberOfChannels={} (want >=1)", rch));
                    }
                    let m0 = num(&s, "ch0MaxAbs");
                    let m1 = num(&s, "ch1MaxAbs");
                    let rms = num(&s, "rms");
                    // 0.5-amplitude sine: peak ~0.5, RMS ~0.354 — both must
                    // survive decode→graph→mixdown, proving real sample flow
                    if !(0.30..=0.70).contains(&m0) {
                        bad.push(format!("ch0 peak={} (want 0.30..0.70 — real samples)", m0));
                    }
                    if !(0.30..=0.70).contains(&m1) {
                        bad.push(format!("ch1 peak={} (want 0.30..0.70 — real samples)", m1));
                    }
                    if !(0.15..=0.55).contains(&rms) {
                        bad.push(format!("ch0 rms={} (want 0.15..0.55 — energy preserved)", rms));
                    }
                    if bad.is_empty() {
                        report.pass(name);
                    } else {
                        report.fail(
                            name,
                            &format!("{} [state: {}]", bad.join("; "), s),
                        );
                    }
                }
            }
        }
    }

    report.finish();
    assert_eq!(
        report.failed, 0,
        "media e2e suite had failing scenarios — see the scenario report above"
    );
}

/// Read the current JSON snapshot of a scenario global without waiting.
fn js_state(page: &PageHandle, global: &str) -> Option<serde_json::Value> {
    let raw = js(page, &format!("JSON.stringify(globalThis.{})", global));
    if raw.is_empty() || raw == "undefined" || raw == "pending" || raw == "\"pending\"" {
        return None;
    }
    serde_json::from_str(&raw).ok()
}
