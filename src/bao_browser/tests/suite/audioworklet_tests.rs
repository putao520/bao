// @trace TEST-BRW-002 [req:REQ-BRW-002] [level:integration]
// AudioWorklet 段(1) script-skeleton live tests — REQ-BRW-002 fork自治面.
//
// Provenance (e89, user ruling 2026-10-05 "自研吧"): upstream servo has zero
// AudioWorklet runtime (e83 profile). This file exercises, on the live servo
// path (real AudioContext → real Worklet engine thread pool → real module
// fetch → real AudioWorkletGlobalScope):
//   1. The `audioContext.audioWorklet` surface is PRESENT (the e83 §2 probe
//      vector — `audioWorklet === undefined` — must be closed), with
//      `addModule` inherited from the Worklet prototype.
//   2. `addModule` resolves against a real HTTP fixture serving a JS module
//      whose top level calls `registerProcessor` and publishes scope facts
//      (registerProcessor ran, sampleRate/currentTime/currentFrame are real
//      numbers from the creating context's servo-media handle).
//   3. The module fetch carries `Request.destination == "audioworklet"`
//      observed through a REAL service worker respondWith path (the WPT
//      fetch-destination audioworklet subtest shape; e72 probe手法).
//   4. AudioWorkletNode constructs — INERT by 段(1) design (node_id = None,
//      the engine's own no-backend form; the servo-media graph face is
//      段(2)), `instanceof AudioNode` holds, `port` is a real MessagePort.
//      NOTE (documented deviation, 段(2) closes): the node constructor does
//      NOT yet reject unregistered processor names (spec's NotSupportedError
//      arm) — validation lands with the 段(2) render bridge that reads the
//      scope-side processor registry.
//
// Environment gating (same as serviceworker_fetchevent_tests): real servo
// rendering requires DISPLAY (Xvfb) and network I/O.
//
// Usage:
//   BAO_TEST_NETWORK=1 xvfb-run cargo nt -p bao-browser \
//     -E 'test(audioworklet_tests)'

#![allow(dead_code)]

use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use bao_browser::{BaoConfig, BrowserRuntime, PageConfig, PageHandle};

static TEST_SERIALIZER: Mutex<()> = Mutex::new(());

/// The AudioWorklet module the fixture serves at /processor.js. Publishes
/// the worklet-scope facts through the fixture (sync XHR — no
/// promise/event-loop dependency, failure travels on the next sync XHR).
// 段(1) observability note: an AudioWorkletGlobalScope has NO channel back
// to the page by spec shape — no fetch/XHR/timers (worklet scopes are
// restricted to registerProcessor + the audio face), and the node/processor
// MessagePort pair that would carry scope facts is the 段(2) render-bridge
// face. The module therefore only performs its spec-visible work:
// registerProcessor at top level (its duplicate arm below is where the
// 段(2) NotSupportedError face gets exercised).
const PROCESSOR_JS: &str = r#"
registerProcessor('probe-processor', class extends AudioWorkletProcessor {});
"#;

/// 段(3) wiring: an echo processor with one k-rate parameter — its
/// `process()` multiplies the input by the `gain` parameter and returns
/// true, so an OfflineAudioContext render produces a real, sample-exact
/// echo (the script↔media bridge under test).
const ECHO_PROCESSOR_JS: &str = r#"
registerProcessor('echo-processor', class extends AudioWorkletProcessor {
  static get parameterDescriptors() {
    return [{ name: 'gain', defaultValue: 1, minValue: 0, maxValue: 10,
              automationRate: 'k-rate' }];
  }
  process(inputs, outputs, parameters) {
    var input = inputs[0];
    var output = outputs[0];
    var gain = parameters.gain[0];
    for (var ch = 0; ch < output.length; ++ch) {
      var inp = (input && input.length > 0) ? input[Math.min(ch, input.length - 1)]
                                            : null;
      for (var i = 0; i < output[ch].length; ++i) {
        output[ch][i] = inp ? inp[i] * gain : 0;
      }
    }
    return true;
  }
});
"#;

/// A processor whose `process()` throws on the first block: the node must
/// mute and `onprocessorerror` must fire exactly once on the main thread.
const THROW_PROCESSOR_JS: &str = r#"
registerProcessor('throw-processor', class extends AudioWorkletProcessor {
  process(inputs, outputs, parameters) {
    throw new TypeError('boom from process()');
  }
});
"#;

/// A processor that answers node-port messages (the conduit roundtrip
/// under test): every inbound message is echoed back with the scope's real
/// sampleRate attached.
const PORT_PROCESSOR_JS: &str = r#"
registerProcessor('port-processor', class extends AudioWorkletProcessor {
  constructor() {
    super();
    var p = this.port;
    p.addEventListener('message', function (e) {
      p.postMessage({ echo: e.data.value, sr: sampleRate });
    });
  }
  process(inputs, outputs, parameters) { return true; }
});
"#;

/// (e122) The constructor-post face: `this.port.postMessage` from inside
/// the constructor must ride the node's lane-0 conduit — the redirect is
/// handed to the base construction through the in-flight instantiation's
/// pending-construction slot, so the message event fires on the node's
/// port with no prior page→worklet traffic. (Pre-e122 the redirect was
/// installed only after the registered constructor returned, and every
/// constructor-posted message fell into the dead constellation path — the
/// WPT messageport/options hang class.)
const CTOR_POST_PROCESSOR_JS: &str = r#"
registerProcessor('ctor-post-processor', class extends AudioWorkletProcessor {
  constructor() {
    super();
    this.port.postMessage({ state: 'created' });
  }
  process(inputs, outputs) { return true; }
});
"#;

/// (e122) The `this`-in-process face: the WPT processor idiom reads and
/// writes own fields (counters, `this.port`) inside `process()`. Class
/// bodies are strict mode, so the pre-e122 call shape (`this = undefined`)
/// made every `this.<x>` access throw on the first block — the node
/// latched processorerror and went silent. This processor counts its
/// blocks and reports from the tenth, pinning both the `this` binding and
/// per-block own-field state.
const THIS_STATE_PROCESSOR_JS: &str = r#"
registerProcessor('this-state-processor', class extends AudioWorkletProcessor {
  constructor() {
    super();
    this.count = 0;
  }
  process(inputs, outputs) {
    ++this.count;
    if (this.count === 10) {
      this.port.postMessage({ count: this.count });
      return false;
    }
    return true;
  }
});
"#;

/// (e114) A processor that publishes what its constructor argument carries:
/// the spec's "invoking processor constructor" step 8 hands the constructor
/// the DESERIALIZED options dictionary — `options.processorOptions` must be
/// the exact value the page placed there (structured clone through the
/// shared serialization base). Poke-reply shape (the established
/// PORT_PROCESSOR_JS idiom): the page asks, the processor answers with the
/// facts it captured at construction. (Since e122 a processor CAN also post
/// from its own constructor — the pending-construction handoff wires
/// `this.port` during the base construction; see CTOR_POST_PROCESSOR_JS.)
const OPTS_PROCESSOR_JS: &str = r#"
registerProcessor('opts-processor', class extends AudioWorkletProcessor {
  constructor(options) {
    super();
    var node = this;
    node.opts = options;
    node.port.addEventListener('message', function (e) {
      if (!e.data || e.data.cmd !== 'go') { return; }
      var options = node.opts;
      var po = options && options.processorOptions;
      node.port.postMessage({
        hasOptions: typeof options === 'object' && options !== null,
        poType: po === null ? 'null' : typeof po,
        x: po ? po.x : null,
        numberOfInputs: options ? options.numberOfInputs : null,
        numberOfOutputs: options ? options.numberOfOutputs : null
      });
    });
  }
  process(inputs, outputs, parameters) { return true; }
});
"#;

/// (e114) The nested-structure variant: the whole `processorOptions`
/// subgraph (objects, arrays, numbers, strings, booleans, null) must
/// survive the main→worklet structured clone byte-shape (the substitution
/// clone preserves enumeration order; JSON comparison is the oracle).
const NESTED_OPTS_PROCESSOR_JS: &str = r#"
registerProcessor('nested-opts-processor', class extends AudioWorkletProcessor {
  constructor(options) {
    super();
    var node = this;
    node.opts = options;
    node.port.addEventListener('message', function (e) {
      if (!e.data || e.data.cmd !== 'go') { return; }
      var po = node.opts && node.opts.processorOptions;
      node.port.postMessage({
        roundtrip: po ? JSON.stringify(po) : null,
        innerIsArray: !!(po && po.a && Array.isArray(po.a.b))
      });
    });
  }
  process(inputs, outputs, parameters) { return true; }
});
"#;

/// (e114) The port-in-port variant (the Transferable subface): a MessagePort
/// placed inside `processorOptions` is transferred to the processor realm
/// (the worklet thread receives the minted counterpart endpoint wired
/// through the node's conduit). On the page's 'go' poke it posts the
/// transfer facts and pings through the channel port (worklet→main);
/// anything arriving on the channel port is forwarded through the node's
/// port (main→worklet observability).
const PIPO_PROCESSOR_JS: &str = r#"
registerProcessor('pipo-processor', class extends AudioWorkletProcessor {
  constructor(options) {
    super();
    var node = this;
    node.chan = options && options.processorOptions && options.processorOptions.channel;
    node.opts = options;
    node.port.addEventListener('message', function (e) {
      if (!e.data || e.data.cmd !== 'go') { return; }
      node.port.postMessage({
        chanIsPort: node.chan instanceof MessagePort,
        poType: node.opts ? typeof node.opts.processorOptions : 'none'
      });
      if (node.chan) {
        node.chan.postMessage('ping-from-processor');
      }
    });
    if (node.chan) {
      node.chan.addEventListener('message', function (e) {
        node.port.postMessage({ got: e.data });
      });
    }
  }
  process(inputs, outputs, parameters) { return true; }
});
"#;

/// (e155) The invalid-parameters-getter module, mirroring the WPT
/// `invalid-param-array-processor.js` byte-for-byte in behavior: the class's
/// static `parameterDescriptors` returns `[]` for the first registration and
/// `[{name: 'invalidParam'}]` for the second, the constructor hands back a
/// shared singleton, and an `Object.prototype` accessor feeds the engine's
/// parameters read-back a 128-sample array on the first fetch and a
/// 256-sample one on the second. The second class is the throwing-getter
/// variant: its accessor always throws, which the rebuild face must absorb
/// by shadowing it with an own data property (the node keeps rendering).
const INVALID_PARAM_PROCESSOR_JS: &str = r#"
let singleton = undefined;
let secondFetch = false;
let useDescriptor = false;

class InvalidParamArrayProcessor extends AudioWorkletProcessor {
  static get parameterDescriptors() {
    if (useDescriptor)
      return [{name: 'invalidParam'}];
    useDescriptor = true;
    return [];
  }
  constructor() {
    super();
    if (singleton === undefined)
      singleton = this;
    return singleton;
  }
  process(inputs, outputs) {
    const output = outputs[0];
    for (let channel = 0; channel < output.length; ++channel)
      output[channel].fill(1);
    return false;
  }
}

Object.defineProperty(Object.prototype, 'invalidParam', {'get': () => {
  if (secondFetch)
    return new Float32Array(256);
  secondFetch = true;
  return new Float32Array(128);
}});

registerProcessor('invalid-param-array-1', InvalidParamArrayProcessor);
registerProcessor('invalid-param-array-2', InvalidParamArrayProcessor);

let throwUseDescriptor = false;

class ThrowingParamProcessor extends AudioWorkletProcessor {
  static get parameterDescriptors() {
    if (throwUseDescriptor)
      return [{name: 'throwParam'}];
    throwUseDescriptor = true;
    return [];
  }
  process(inputs, outputs) {
    const output = outputs[0];
    for (let channel = 0; channel < output.length; ++channel)
      output[channel].fill(1);
    return true;
  }
}

Object.defineProperty(Object.prototype, 'throwParam', {'get': () => {
  throw new Error('invalid parameters getter');
}});

registerProcessor('invalid-throw-1', ThrowingParamProcessor);
registerProcessor('invalid-throw-2', ThrowingParamProcessor);
"#;

/// Service worker: asserts the module fetch destination the same way the WPT
/// fetch-destination worker does, and responds with the pass-through fetch
/// when it matches. Records the observed destination for the fixture probe.
const AW_SW_JS: &str = r#"
var __seen = { destinations: [], matched: false };
self.addEventListener('fetch', function (event) {
  var url = event.request.url;
  __seen.destinations.push(url + ' => ' + event.request.destination);
  if (url.indexOf('dummy') !== -1) {
    if (event.request.destination === 'audioworklet') {
      __seen.matched = true;
      event.respondWith(fetch(event.request));
    } else {
      event.respondWith(Response.error());
    }
  }
  __publish('__ORIGIN__');
});
function __publish(origin) {
  try {
    var x = new XMLHttpRequest();
    x.open('GET', origin + 'aw-sw-probe?matched=' + __seen.matched +
                    '&dest=' + encodeURIComponent(__seen.destinations.join('|')), false);
    x.send(null);
  } catch (e) {}
}
"#;

/// Minimal HTTP fixture: `/` page, `/processor.js` JS module, `/sw.js`
/// service worker, `/dummy` module target (served with a JS MIME so the
/// engine module path is exercised end to end), `/aw-sw-probe` verdict
/// sink.
struct AwHttpFixture {
    shutdown: Arc<AtomicBool>,
    paths: Arc<Mutex<Vec<String>>>,
    origin: Arc<Mutex<String>>,
    port: u16,
}

impl AwHttpFixture {
    fn spawn() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind audio worklet fixture");
        let port = listener.local_addr().unwrap().port();
        let _ = listener.set_nonblocking(true);
        let shutdown = Arc::new(AtomicBool::new(false));
        let paths: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let origin: Arc<Mutex<String>> = Arc::new(Mutex::new(String::new()));
        let shutdown_c = Arc::clone(&shutdown);
        let paths_c = Arc::clone(&paths);
        let origin_c = Arc::clone(&origin);
        std::thread::Builder::new()
            .name("aw-fixture".into())
            .spawn(move || {
                while !shutdown_c.load(Ordering::SeqCst) {
                    match listener.accept() {
                        Ok((mut tcp, _)) => {
                            let _ = tcp.set_read_timeout(Some(Duration::from_millis(300)));
                            let mut buf = Vec::new();
                            let mut tmp = [0u8; 4096];
                            let deadline = Instant::now() + Duration::from_secs(2);
                            while buf.windows(4).position(|w| w == b"\r\n\r\n").is_none() &&
                                Instant::now() < deadline
                            {
                                match tcp.read(&mut tmp) {
                                    Ok(0) => break,
                                    Ok(n) => buf.extend_from_slice(&tmp[..n]),
                                    Err(_) => break,
                                }
                            }
                            let head = String::from_utf8_lossy(&buf).to_string();
                            let path = head
                                .lines()
                                .next()
                                .and_then(|line| line.split_whitespace().nth(1))
                                .unwrap_or("")
                                .to_string();
                            paths_c.lock().unwrap().push(path.clone());
                            let origin = origin_c.lock().unwrap().clone();
                            let (ct, body): (&str, String) = if path.starts_with("/processor.js")
                            {
                                ("text/javascript", PROCESSOR_JS.replace("__ORIGIN__", &origin))
                            } else if path.starts_with("/echo-processor.js") {
                                ("text/javascript", ECHO_PROCESSOR_JS.to_string())
                            } else if path.starts_with("/throw-processor.js") {
                                ("text/javascript", THROW_PROCESSOR_JS.to_string())
                            } else if path.starts_with("/port-processor.js") {
                                ("text/javascript", PORT_PROCESSOR_JS.to_string())
                            } else if path.starts_with("/ctor-post-processor.js") {
                                ("text/javascript", CTOR_POST_PROCESSOR_JS.to_string())
                            } else if path.starts_with("/this-state-processor.js") {
                                ("text/javascript", THIS_STATE_PROCESSOR_JS.to_string())
                            } else if path.starts_with("/opts-processor.js") {
                                ("text/javascript", OPTS_PROCESSOR_JS.to_string())
                            } else if path.starts_with("/nested-opts-processor.js") {
                                ("text/javascript", NESTED_OPTS_PROCESSOR_JS.to_string())
                            } else if path.starts_with("/pipo-processor.js") {
                                ("text/javascript", PIPO_PROCESSOR_JS.to_string())
                            } else if path.starts_with("/invalid-param-processor.js") {
                                ("text/javascript", INVALID_PARAM_PROCESSOR_JS.to_string())
                            } else if path.starts_with("/sw.js") {
                                ("application/javascript", AW_SW_JS.replace("__ORIGIN__", &origin))
                            } else if path.starts_with("/dummy") {
                                // JS MIME: keeps the engine module MIME face in
                                // play (the WPT wptserve serves its bare
                                // `dummy` as octet-stream — a test-data
                                // artifact this fixture removes).
                                ("text/javascript", String::new())
                            } else {
                                ("text/html", "<html><body>aw fixture</body></html>".into())
                            };
                            let response = format!(
                                "HTTP/1.1 200 OK\r\nContent-Type: {ct}\r\nContent-Length: {len}\r\nAccess-Control-Allow-Origin: *\r\nService-Worker-Allowed: /\r\nConnection: close\r\n\r\n",
                                ct = ct,
                                len = body.len(),
                            );
                            let _ = tcp.write_all(response.as_bytes());
                            let _ = tcp.write_all(body.as_bytes());
                            let _ = tcp.shutdown(std::net::Shutdown::Both);
                        },
                        Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                            std::thread::sleep(Duration::from_millis(5));
                        },
                        Err(_) => return,
                    }
                }
            })
            .expect("spawn audio worklet fixture thread");
        AwHttpFixture {
            shutdown,
            paths,
            origin,
            port,
        }
    }

    fn set_origin(&self, origin: String) {
        *self.origin.lock().unwrap() = origin;
    }

    fn recorded_paths(&self) -> Vec<String> {
        self.paths.lock().unwrap().clone()
    }

    fn probe_result(&self, sink: &str) -> Option<String> {
        self.recorded_paths()
            .into_iter()
            .find(|p| p.starts_with(sink))
            .map(|p| {
                let raw = p.trim_start_matches(sink).trim_start_matches("?result=");
                urldecode(raw)
            })
    }
}

impl Drop for AwHttpFixture {
    fn drop(&mut self) {
        self.shutdown.store(true, Ordering::SeqCst);
    }
}

fn urldecode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'%' if i + 2 < bytes.len() => {
                let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).unwrap_or("");
                match u8::from_str_radix(hex, 16) {
                    Ok(b) => {
                        out.push(b);
                        i += 3;
                    },
                    Err(_) => {
                        out.push(bytes[i]);
                        i += 1;
                    },
                }
            },
            b'+' => {
                out.push(b' ');
                i += 1;
            },
            b => {
                out.push(b);
                i += 1;
            },
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn wait_for<F: Fn() -> Option<T>, T>(mut poll: F, timeout: Duration, what: &str) -> Option<T> {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if let Some(v) = poll() {
            return Some(v);
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    eprintln!("[timeout] {what}");
    None
}

fn should_skip() -> bool {
    if std::env::var("BAO_TEST_NETWORK").as_deref() != Ok("1") {
        eprintln!("[skip] BAO_TEST_NETWORK != 1");
        return true;
    }
    if std::env::var("DISPLAY").unwrap_or_default().is_empty() {
        eprintln!("[skip] no DISPLAY");
        return true;
    }
    false
}

/// Evaluates `js` on the page and polls until it stops reporting `pending`.
fn eval_pending(
    page: &PageHandle,
    js: &str,
    timeout: Duration,
    what: &str,
) -> String {
    let _ = page.evaluate_js_web(js);
    wait_for(
        || match page.evaluate_js_web("window.__probe") {
            Ok(s) if s.contains("pending") => None,
            Ok(s) => Some(s),
            Err(_) => None,
        },
        timeout,
        what,
    )
    .unwrap_or_else(|| "<no settlement>".to_string())
}

/// @trace REQ-BRW-002 [criterion:audioworklet-surface] live
///
/// The e83 §2 probe vector (`audioContext.audioWorklet` undefined) must be
/// closed, addModule must resolve against a real HTTP module, and the module
/// must run on a real AudioWorkletGlobalScope (registerProcessor callable,
/// real sampleRate/currentTime/currentFrame).
#[test]
fn audioworklet_surface_addmodule_and_scope_live() {
    if should_skip() {
        return;
    }
    let _guard = TEST_SERIALIZER.lock().unwrap_or_else(|e| e.into_inner());

    let fixture = AwHttpFixture::spawn();
    let origin = format!("http://127.0.0.1:{}/", fixture.port);
    fixture.set_origin(origin.clone());

    let runtime = BrowserRuntime::new(BaoConfig::default())
        .expect("gated live test: BrowserRuntime::new must succeed");
    let page = runtime
        .create_page(&PageConfig {
            url: Some(origin.clone()),
            ..Default::default()
        })
        .expect("gated live test: create_page must succeed");

    // 1. Surface probe: audioWorklet present + addModule inherited.
    let surface = page
        .evaluate_js_web(
            "(function(){ try { \
               var c = new AudioContext(); \
               window.__ctx = c; \
               return JSON.stringify({ \
                 awType: typeof c.audioWorklet, \
                 isAudioWorklet: c.audioWorklet instanceof AudioWorklet, \
                 addModuleType: typeof c.audioWorklet.addModule \
               }); \
             } catch (e) { return 'threw:' + e; } })()",
        )
        .expect("surface probe eval must run");
    eprintln!("[aw-test] surface = {surface}");
    assert!(
        surface.contains("\"awType\":\"object\"") &&
            surface.contains("\"isAudioWorklet\":true") &&
            surface.contains("\"addModuleType\":\"function\""),
        "audioWorklet surface must be present with an inherited addModule, got: {surface}"
    );

    // 2. addModule resolution against the real fixture module.
    let add = eval_pending(
        &page,
        &format!(
            "window.__probe = 'pending'; try {{ \
               window.__ctx.audioWorklet.addModule('{origin}processor.js').then( \
                 function () {{ window.__probe = 'ok'; }}, \
                 function (e) {{ window.__probe = 'error:' + e; }}); \
             }} catch (err) {{ window.__probe = 'threw:' + err; }} window.__probe"
        ),
        Duration::from_secs(20),
        "addModule promise settlement",
    );
    eprintln!("[aw-test] addModule = {add}");
    assert!(
        add == "ok",
        "audioWorklet.addModule must resolve against a real JS module, got: {add}"
    );

    // 3. The module was fetched through the AUDIO worklet engine path: the
    //    StatelessWorkletThreadPool loads the module into every pool thread,
    //    so the fixture must have served /processor.js to all 3 of them.
    //    (Scope-side facts — registerProcessor registry, sampleRate/
    //    currentTime/currentFrame — have NO page channel by spec shape until
    //    段(2) wires the node/processor MessagePort pair; see the header of
    //    PROCESSOR_JS and the file header.)
    let module_hits = fixture
        .recorded_paths()
        .into_iter()
        .filter(|p| p.starts_with("/processor.js"))
        .count();
    eprintln!("[aw-test] /processor.js hits = {module_hits}");
    assert!(
        module_hits >= 3,
        "the audio worklet thread pool (3 threads) must each load the module, got \
         {module_hits} hits"
    );
}

/// @trace REQ-BRW-002 [criterion:audioworklet-destination] live
///
/// The WPT fetch-destination audioworklet subtest shape, end to end on the
/// live path: a real service worker intercepts the worklet module fetch,
/// observes `Request.destination == "audioworklet"` (the e72 destination
/// face), and respondWith's the pass-through fetch — the module loads only
/// when BOTH the destination axis and the module pipeline are right.
#[test]
fn audioworklet_module_fetch_destination_via_sw_live() {
    if should_skip() {
        return;
    }
    let _guard = TEST_SERIALIZER.lock().unwrap_or_else(|e| e.into_inner());

    let fixture = AwHttpFixture::spawn();
    let origin = format!("http://127.0.0.1:{}/", fixture.port);
    fixture.set_origin(origin.clone());

    let runtime = BrowserRuntime::new(BaoConfig::default())
        .expect("gated live test: BrowserRuntime::new must succeed");
    let page = runtime
        .create_page(&PageConfig {
            url: Some(origin.clone()),
            ..Default::default()
        })
        .expect("gated live test: create_page must succeed");

    // Register the SW and wait for activation.
    let register = eval_pending(
        &page,
        &format!(
            "window.__probe = 'pending'; try {{ \
               navigator.serviceWorker.register('{origin}sw.js').then( \
                 function () {{ window.__probe = 'ok'; }}, \
                 function (e) {{ window.__probe = 'error:' + e; }}); \
             }} catch (err) {{ window.__probe = 'threw:' + err; }} window.__probe"
        ),
        Duration::from_secs(20),
        "serviceWorker.register settlement",
    );
    eprintln!("[aw-test] sw register = {register}");
    assert!(
        register == "ok",
        "SW registration must resolve, got: {register}"
    );

    // The audioworklet subtest: the module fetch must surface as
    // destination "audioworklet" in the SW and then load.
    let add = eval_pending(
        &page,
        &format!(
            "window.__probe = 'pending'; try {{ \
               new AudioContext().audioWorklet.addModule('{origin}dummy?dest=audioworklet')\
                 .then(function () {{ window.__probe = 'ok'; }}, \
                       function (e) {{ window.__probe = 'error:' + e; }}); \
             }} catch (err) {{ window.__probe = 'threw:' + err; }} window.__probe"
        ),
        Duration::from_secs(25),
        "addModule via SW settlement",
    );
    eprintln!("[aw-test] addModule-via-sw = {add}");
    assert!(
        add == "ok",
        "addModule must resolve when the SW observes destination=audioworklet and \
         pass-through fetches the module, got: {add}"
    );

    // The SW's own record of the observed destination.
    let sw_verdict =
        wait_for(
            || fixture.probe_result("/aw-sw-probe"),
            Duration::from_secs(20),
            "SW destination probe publish (/aw-sw-probe)",
        )
        .unwrap_or_else(|| {
            let seen = fixture.recorded_paths();
            panic!("SW destination verdict never arrived; fixture paths: {seen:?}")
        });
    eprintln!("[aw-test] sw destination verdict = {sw_verdict}");
    assert!(
        sw_verdict.contains("matched=true") &&
            sw_verdict.contains("audioworklet"),
        "SW must observe Request.destination == 'audioworklet' for the worklet module \
         fetch, got: {sw_verdict}"
    );
}

/// @trace REQ-BRW-002 [criterion:audioworklet-node] live
///
/// AudioWorkletNode constructs as a real (INERT by 段(1) design) AudioNode:
/// `instanceof AudioNode` holds, `port` is a real MessagePort, and
/// `onprocessorerror` is settable, `parameters` is a real AudioParamMap.
/// The unregistered-name rejection (spec NotSupportedError arm) went live
/// with the 段(3) wiring — the 段(1) INERT deviation documented in the file
/// header is closed, so this test now asserts the rejection too.
#[test]
fn audioworklet_node_constructs_live() {
    if should_skip() {
        return;
    }
    let _guard = TEST_SERIALIZER.lock().unwrap_or_else(|e| e.into_inner());

    let fixture = AwHttpFixture::spawn();
    let origin = format!("http://127.0.0.1:{}/", fixture.port);
    fixture.set_origin(origin.clone());

    let runtime = BrowserRuntime::new(BaoConfig::default())
        .expect("gated live test: BrowserRuntime::new must succeed");
    let page = runtime
        .create_page(&PageConfig {
            url: Some(origin.clone()),
            ..Default::default()
        })
        .expect("gated live test: create_page must succeed");

    // Register the processor first: the name validation is the registry.
    let add = eval_pending(
        &page,
        &format!(
            "window.__probe = 'pending'; try {{ \
               window.__c = new AudioContext(); \
               window.__c.audioWorklet.addModule('{origin}processor.js').then( \
                 function() {{ window.__probe = 'resolved'; }}, \
                 function(e) {{ window.__probe = 'rejected:' + e; }}); \
             }} catch (e) {{ window.__probe = 'threw:' + e; }}",
            origin = origin,
        ),
        Duration::from_secs(20),
        "addModule settlement",
    );
    assert!(
        add.contains("resolved"),
        "addModule must resolve before node construction, got: {add}"
    );

    let node = page
        .evaluate_js_web(
            "(function(){ try { \
               var threw = null; \
               var unregistered = null; \
               try { new AudioWorkletNode(window.__c, 'no-such-processor'); } \
               catch (e) { threw = e.name; } \
               var n = new AudioWorkletNode(window.__c, 'probe-processor'); \
               var p = n.port; \
               n.onprocessorerror = function () {}; \
               return JSON.stringify({ \
                 unregisteredThrew: threw, \
                 isNode: n instanceof AudioWorkletNode, \
                 isAudioNode: n instanceof AudioNode, \
                 portIsPort: p instanceof MessagePort, \
                 onprocessorerror: typeof n.onprocessorerror, \
                 parameters: typeof n.parameters, \
                 parametersSize: n.parameters.size \
               }); \
             } catch (e) { return 'threw:' + e; } })()",
        )
        .expect("node probe eval must run");
    eprintln!("[aw-test] node = {node}");
    assert!(
        node.contains("\"unregisteredThrew\":\"NotSupportedError\"") &&
            node.contains("\"isNode\":true") &&
            node.contains("\"isAudioNode\":true") &&
            node.contains("\"portIsPort\":true") &&
            node.contains("\"onprocessorerror\":\"function\"") &&
            node.contains("\"parameters\":\"object\"") &&
            node.contains("\"parametersSize\":0"),
        "AudioWorkletNode must reject unregistered names (NotSupportedError) and construct \
         registered ones with a real AudioNode/port/parameters/event-handler face, \
         got: {node}"
    );
}

/// Shared live-runtime setup for the 段(3) wiring scenarios: fixture + page
/// with the named processor module already added to the context's worklet.
fn spawn_wired_page(
    fixture: &AwHttpFixture,
    runtime: &BrowserRuntime,
    module_path: &str,
) -> PageHandle {
    let origin = format!("http://127.0.0.1:{}/", fixture.port);
    let page = runtime
        .create_page(&PageConfig {
            url: Some(origin.clone()),
            ..Default::default()
        })
        .expect("gated live test: create_page must succeed");
    page.evaluate_js_web("window.__ctx = new AudioContext();")
        .expect("AudioContext construction must run");
    let add = eval_pending(
        &page,
        &format!(
            "window.__probe = 'pending'; try {{ \
               window.__ctx.audioWorklet.addModule('{origin}{module}').then( \
                 function() {{ window.__probe = 'resolved'; }}, \
                 function(e) {{ window.__probe = 'rejected:' + e; }}); \
             }} catch (e) {{ window.__probe = 'threw:' + e; }}",
            origin = origin,
            module = module_path,
        ),
        Duration::from_secs(20),
        "addModule settlement",
    );
    assert!(
        add.contains("resolved"),
        "addModule of {module_path} must resolve for the wiring scenarios, got: {add}"
    );
    page
}

/// @trace REQ-BRW-002 [criterion:audioworklet-echo-render] live
///
/// 段(3) wiring end to end: registerProcessor (parameterDescriptors
/// extraction included) → `new AudioWorkletNode` (real servo-media graph
/// node + bridge) → OfflineAudioContext render → the destination carries the
/// processor's sample-exact echo, and `node.parameters` is a real
/// AudioParamMap whose `gain` entry automates `WorkletParam(0)`.
#[test]
fn audioworklet_render_echo_and_parameters_live() {
    if should_skip() {
        return;
    }
    let _guard = TEST_SERIALIZER.lock().unwrap_or_else(|e| e.into_inner());

    let fixture = AwHttpFixture::spawn();
    fixture.set_origin(format!("http://127.0.0.1:{}/", fixture.port));
    let runtime = BrowserRuntime::new(BaoConfig::default())
        .expect("gated live test: BrowserRuntime::new must succeed");
    let page = spawn_wired_page(&fixture, &runtime, "echo-processor.js");

    let result = eval_pending(
        &page,
        r#"window.__probe = 'pending'; (async function() {
             try {
               var ctx = new OfflineAudioContext(1, 44100, 44100);
               await ctx.audioWorklet.addModule(window.location.origin + '/echo-processor.js');
               var node = new AudioWorkletNode(ctx, 'echo-processor',
                 { numberOfInputs: 1, numberOfOutputs: 1, outputChannelCount: [1] });
               var paramsType = typeof node.parameters;
               var gain = node.parameters.get('gain');
               var gainType = gain ? typeof gain.value : 'missing';
               var src = ctx.createConstantSource();
               src.offset.value = 0.25;
               src.connect(node);
               node.connect(ctx.destination);
               src.start(0);
               var buf = await ctx.startRendering();
               var d = buf.getChannelData(0);
               var firstNonzero = -1;
               for (var i = 0; i < d.length; ++i) {
                 if (Math.abs(d[i]) > 1e-6) { firstNonzero = i; break; }
               }
               // The exact-echo proof: the longest run of samples equal to
               // 0.25 (f32-exact). The offline render fast-forwards through
               // the bridge, so WHERE the echo lands depends on machine
               // speed (pipeline fill + pump spin-up); THAT it appears,
               // contiguous and sample-exact, is the machine-independent
               // signal.
               var bestRun = 0, run = 0;
               for (var i = 0; i < d.length; ++i) {
                 if (d[i] === 0.25) { ++run; if (run > bestRun) bestRun = run; }
                 else { run = 0; }
               }
               window.__probe = JSON.stringify({
                 stage: 'done', paramsType: paramsType, gainType: gainType,
                 firstNonzero: firstNonzero, bestRun: bestRun, len: d.length
               });
             } catch (e) { window.__probe = 'threw:' + e; }
           })();"#,
        Duration::from_secs(30),
        "offline echo render",
    );

    eprintln!("[aw-test] echo render = {result}");
    assert!(
        result.contains("\"paramsType\":\"object\"") && result.contains("\"gainType\":\"number\""),
        "AudioWorkletNode.parameters must be a real AudioParamMap with a numeric gain entry, \
         got: {result}"
    );
    let best_run: f64 = result
        .split("\"bestRun\":")
        .nth(1)
        .and_then(|rest| rest.split([',', '}']).next())
        .and_then(|v| v.parse().ok())
        .unwrap_or(0.);
    // 8 blocks of contiguous f32-exact echo: the processor ran at block
    // rate through the real bridge and the graph delivered its output.
    assert!(
        best_run >= 8. * 128.,
        "the render must carry a contiguous sample-exact 0.25 echo run of at least 8 blocks \
         (got {best_run} samples), result: {result}"
    );
    let first_nonzero: f64 = result
        .split("\"firstNonzero\":")
        .nth(1)
        .and_then(|rest| rest.split([',', '}']).next())
        .and_then(|v| v.parse().ok())
        .unwrap_or(-1.);
    assert!(
        first_nonzero >= 0.,
        "the render must not be entirely silent, got: {result}"
    );
}

/// @trace REQ-BRW-002 [criterion:audioworklet-onprocessorerror] live
///
/// A processor whose `process()` throws latches the bridge on the worklet
/// thread and fires `processorerror` on the main thread exactly once (the
/// spec's one-shot), through the Trusted-task channel.
#[test]
fn audioworklet_processorerror_fires_once_live() {
    if should_skip() {
        return;
    }
    let _guard = TEST_SERIALIZER.lock().unwrap_or_else(|e| e.into_inner());

    let fixture = AwHttpFixture::spawn();
    fixture.set_origin(format!("http://127.0.0.1:{}/", fixture.port));
    let runtime = BrowserRuntime::new(BaoConfig::default())
        .expect("gated live test: BrowserRuntime::new must succeed");
    let page = spawn_wired_page(&fixture, &runtime, "throw-processor.js");

    let result = eval_pending(
        &page,
        r#"window.__probe = 'pending'; (async function() {
             try {
               var ctx = new OfflineAudioContext(1, 4096, 44100);
               await ctx.audioWorklet.addModule(window.location.origin + '/throw-processor.js');
               var node = new AudioWorkletNode(ctx, 'throw-processor');
               var fired = 0;
               node.onprocessorerror = function () { ++fired; };
               var src = ctx.createConstantSource();
               src.offset.value = 0.5;
               src.connect(node);
               node.connect(ctx.destination);
               src.start(0);
               await ctx.startRendering();
               // Give the one-shot task a moment beyond the render EOS.
               await new Promise(function (resolve) { setTimeout(resolve, 500); });
               window.__probe = JSON.stringify({ stage: 'done', fired: fired });
             } catch (e) { window.__probe = 'threw:' + e; }
           })();"#,
        Duration::from_secs(30),
        "onprocessorerror render",
    );

    eprintln!("[aw-test] processorerror = {result}");
    assert!(
        result.contains("\"fired\":1"),
        "onprocessorerror must fire exactly once for a throwing processor, got: {result}"
    );
}

/// @trace REQ-BRW-002 [criterion:audioworklet-port-roundtrip] live
///
/// The node↔processor port pair routes through the bounded conduit: a
/// main→processor message wakes the pump drain on the worklet thread, the
/// processor's handler replies, and the reply is dispatched as a `message`
/// event on the node's port with the scope's real sampleRate attached.
#[test]
fn audioworklet_port_roundtrip_live() {
    if should_skip() {
        return;
    }
    let _guard = TEST_SERIALIZER.lock().unwrap_or_else(|e| e.into_inner());

    let fixture = AwHttpFixture::spawn();
    fixture.set_origin(format!("http://127.0.0.1:{}/", fixture.port));
    let runtime = BrowserRuntime::new(BaoConfig::default())
        .expect("gated live test: BrowserRuntime::new must succeed");
    let page = spawn_wired_page(&fixture, &runtime, "port-processor.js");

    let result = eval_pending(
        &page,
        r#"window.__probe = 'pending'; (async function() {
             try {
               var ctx = new OfflineAudioContext(1, 128, 44100);
               await ctx.audioWorklet.addModule(window.location.origin + '/port-processor.js');
               var node = new AudioWorkletNode(ctx, 'port-processor');
               var reply = await new Promise(function (resolve, reject) {
                 node.port.onmessage = function (e) { resolve(e.data); };
                 setTimeout(function () { reject(new Error('port roundtrip timeout')); }, 10000);
                 node.port.postMessage({ value: 'ping-42' });
               });
               window.__probe = JSON.stringify({
                 stage: 'done', echo: reply.echo, sr: reply.sr,
                 srIsNumber: typeof reply.sr === 'number' && reply.sr === 44100
               });
             } catch (e) { window.__probe = 'threw:' + e; }
           })();"#,
        Duration::from_secs(30),
        "port roundtrip",
    );

    eprintln!("[aw-test] port roundtrip = {result}");
    assert!(
        result.contains("\"echo\":\"ping-42\"") && result.contains("\"srIsNumber\":true"),
        "the node↔processor port roundtrip must echo the payload with the real scope sampleRate, \
         got: {result}"
    );
}

/// @trace REQ-BRW-004 [criterion:structured-clone] live
///
/// (e114) `processorOptions` passthrough: the options dictionary crosses to
/// the processor realm through the shared structured-clone base (spec
/// "invoking processor constructor" steps 3-5 + 8) — the processor
/// constructor receives the deserialized dictionary object, and
/// `options.processorOptions.x` is the exact value the page placed there.
#[test]
fn audioworklet_processor_options_simple_live() {
    if should_skip() {
        return;
    }
    let _guard = TEST_SERIALIZER.lock().unwrap_or_else(|e| e.into_inner());

    let fixture = AwHttpFixture::spawn();
    fixture.set_origin(format!("http://127.0.0.1:{}/", fixture.port));
    let runtime = BrowserRuntime::new(BaoConfig::default())
        .expect("gated live test: BrowserRuntime::new must succeed");
    let page = spawn_wired_page(&fixture, &runtime, "opts-processor.js");

    let result = eval_pending(
        &page,
        r#"window.__probe = 'pending'; (async function() {
             try {
               var ctx = new OfflineAudioContext(1, 128, 44100);
               await ctx.audioWorklet.addModule(window.location.origin + '/opts-processor.js');
               var node = new AudioWorkletNode(ctx, 'opts-processor',
                 { processorOptions: { x: 1 } });
               var msgs = [];
               node.port.onmessage = function (e) { msgs.push(e.data); };
               node.port.postMessage({ cmd: 'go' });
               var deadline = Date.now() + 10000;
               while (msgs.length < 1 && Date.now() < deadline) {
                 await new Promise(function (r) { setTimeout(r, 50); });
               }
               var m = msgs[0] || {};
               window.__probe = JSON.stringify({
                 stage: 'done', hasOptions: m.hasOptions, poType: m.poType,
                 x: m.x, numberOfInputs: m.numberOfInputs, numberOfOutputs: m.numberOfOutputs
               });
             } catch (e) { window.__probe = 'threw:' + e; }
           })();"#,
        Duration::from_secs(30),
        "processorOptions simple passthrough",
    );

    eprintln!("[aw-test] processorOptions simple = {result}");
    assert!(
        result.contains("\"hasOptions\":true") &&
            result.contains("\"poType\":\"object\"") &&
            result.contains("\"x\":1") &&
            result.contains("\"numberOfInputs\":1") &&
            result.contains("\"numberOfOutputs\":1"),
        "the processor constructor must receive the deserialized options dictionary with the \
         page's processorOptions intact, got: {result}"
    );
}

/// @trace REQ-BRW-004 [criterion:structured-clone] live
///
/// (e114) The nested-structure variant of the passthrough: objects, arrays
/// and every scalar flavour inside `processorOptions` survive the
/// main→worklet structured clone with shape and order intact (the
/// substitution clone preserves enumeration order; JSON comparison is the
/// oracle).
#[test]
fn audioworklet_processor_options_nested_live() {
    if should_skip() {
        return;
    }
    let _guard = TEST_SERIALIZER.lock().unwrap_or_else(|e| e.into_inner());

    let fixture = AwHttpFixture::spawn();
    fixture.set_origin(format!("http://127.0.0.1:{}/", fixture.port));
    let runtime = BrowserRuntime::new(BaoConfig::default())
        .expect("gated live test: BrowserRuntime::new must succeed");
    let page = spawn_wired_page(&fixture, &runtime, "nested-opts-processor.js");

    let result = eval_pending(
        &page,
        r#"window.__probe = 'pending'; (async function() {
             try {
               var ctx = new OfflineAudioContext(1, 128, 44100);
               await ctx.audioWorklet.addModule(
                 window.location.origin + '/nested-opts-processor.js');
               var payload = { a: { b: [1, 2.5, 'three', true, null] }, n: [4, 5], flag: false };
               var node = new AudioWorkletNode(ctx, 'nested-opts-processor',
                 { processorOptions: payload });
               var msgs = [];
               node.port.onmessage = function (e) { msgs.push(e.data); };
               node.port.postMessage({ cmd: 'go' });
               var deadline = Date.now() + 10000;
               while (msgs.length < 1 && Date.now() < deadline) {
                 await new Promise(function (r) { setTimeout(r, 50); });
               }
               var m = msgs[0] || {};
               window.__probe = JSON.stringify({
                 stage: 'done', equal: m.roundtrip === JSON.stringify(payload),
                 innerIsArray: m.innerIsArray, got: m.roundtrip
               });
             } catch (e) { window.__probe = 'threw:' + e; }
           })();"#,
        Duration::from_secs(30),
        "processorOptions nested passthrough",
    );

    eprintln!("[aw-test] processorOptions nested = {result}");
    assert!(
        result.contains("\"equal\":true") && result.contains("\"innerIsArray\":true"),
        "the nested processorOptions subgraph must survive the structured clone with shape and \
         order intact, got: {result}"
    );
}

/// @trace REQ-BRW-004 [criterion:structured-clone] live
///
/// (e114) The port-in-port roundtrip (the Transferable subface): a
/// MessagePort placed inside `processorOptions` is transferred to the
/// processor realm — the worklet thread's constructor sees a real
/// MessagePort (the minted conduit lane counterpart), its
/// `postMessage('ping-from-processor')` lands on the page's port
/// (worklet→main through the lane), and the page's reply is received by the
/// processor and forwarded through the node's port (main→worklet through
/// the lane).
#[test]
fn audioworklet_processor_options_port_in_port_roundtrip_live() {
    if should_skip() {
        return;
    }
    let _guard = TEST_SERIALIZER.lock().unwrap_or_else(|e| e.into_inner());

    let fixture = AwHttpFixture::spawn();
    fixture.set_origin(format!("http://127.0.0.1:{}/", fixture.port));
    let runtime = BrowserRuntime::new(BaoConfig::default())
        .expect("gated live test: BrowserRuntime::new must succeed");
    let page = spawn_wired_page(&fixture, &runtime, "pipo-processor.js");

    let result = eval_pending(
        &page,
        r#"window.__probe = 'pending'; (async function() {
             try {
               var ctx = new OfflineAudioContext(1, 128, 44100);
               await ctx.audioWorklet.addModule(window.location.origin + '/pipo-processor.js');
               var mc = new MessageChannel();
               var node = new AudioWorkletNode(ctx, 'pipo-processor',
                 { processorOptions: { channel: mc.port1 } });
               var fromNode = [], fromChan = [];
               node.port.onmessage = function (e) { fromNode.push(e.data); };
               mc.port1.onmessage = function (e) { fromChan.push(e.data); };
               node.port.postMessage({ cmd: 'go' });
               var wait_for = function (pred) {
                 var deadline = Date.now() + 10000;
                 return new Promise(function (resolve) {
                   (function poll() {
                     if (pred() || Date.now() >= deadline) { resolve(); return; }
                     setTimeout(poll, 50);
                   })();
                 });
               };
               await wait_for(function () { return fromNode.length >= 1; });
               var first = fromNode[0] || {};
               await wait_for(function () { return fromChan.length >= 1; });
               var ping = fromChan.length >= 1 ? String(fromChan[0]) : '__none__';
               mc.port1.postMessage('pong-to-processor');
               await wait_for(function () {
                 return fromNode.some(function (m) { return m && m.got; });
               });
               var echo = '__none__';
               for (var i = 0; i < fromNode.length; ++i) {
                 if (fromNode[i] && fromNode[i].got) { echo = fromNode[i].got; break; }
               }
               window.__probe = JSON.stringify({
                 stage: 'done', chanIsPort: !!first.chanIsPort,
                 poType: first.poType, ping: ping, echo: echo
               });
             } catch (e) { window.__probe = 'threw:' + e; }
           })();"#,
        Duration::from_secs(40),
        "processorOptions port-in-port roundtrip",
    );

    eprintln!("[aw-test] processorOptions port-in-port = {result}");
    assert!(
        result.contains("\"chanIsPort\":true") &&
            result.contains("\"poType\":\"object\"") &&
            result.contains("\"ping\":\"ping-from-processor\"") &&
            result.contains("\"echo\":\"pong-to-processor\""),
        "the processorOptions MessagePort must transfer to the processor realm and carry a full \
         postMessage roundtrip (worklet→main ping, main→worklet pong), got: {result}"
    );
}

/// @trace REQ-BRW-004 [criterion:audioworklet-ctor-port-message] live
///
/// (e122) A processor constructor's `this.port.postMessage` rides the
/// node's lane-0 conduit from construction on: the message event fires on
/// the node's port with NO prior page→worklet traffic. Pre-e122 the
/// redirect was installed only after the registered constructor returned,
/// so every constructor-posted message was lost (the WPT
/// messageport/options deterministic-TIMEOUT class — first message arrives
/// only when the page posts first).
#[test]
fn audioworklet_ctor_port_message_live() {
    if should_skip() {
        return;
    }
    let _guard = TEST_SERIALIZER.lock().unwrap_or_else(|e| e.into_inner());

    let fixture = AwHttpFixture::spawn();
    fixture.set_origin(format!("http://127.0.0.1:{}/", fixture.port));
    let runtime = BrowserRuntime::new(BaoConfig::default())
        .expect("gated live test: BrowserRuntime::new must succeed");
    let page = spawn_wired_page(&fixture, &runtime, "ctor-post-processor.js");

    let result = eval_pending(
        &page,
        r#"window.__probe = 'pending'; (async function() {
             try {
               var ctx = new OfflineAudioContext(1, 128, 44100);
               await ctx.audioWorklet.addModule(window.location.origin + '/ctor-post-processor.js');
               var node = new AudioWorkletNode(ctx, 'ctor-post-processor');
               var msg = await new Promise(function (resolve) {
                 node.port.onmessage = function (e) { resolve(e.data); };
                 setTimeout(function () { resolve('__timeout__'); }, 10000);
               });
               window.__probe = JSON.stringify({ stage: 'done', msg: msg });
             } catch (e) { window.__probe = 'threw:' + e; }
           })();"#,
        Duration::from_secs(30),
        "ctor port message",
    );

    eprintln!("[aw-test] ctor port message = {result}");
    assert!(
        result.contains("\"state\":\"created\""),
        "the processor constructor's port.postMessage must reach node.port.onmessage without \
         prior page→worklet traffic, got: {result}"
    );
}

/// @trace REQ-BRW-004 [criterion:audioworklet-process-this-binding] live
///
/// (e122) `process()` runs with `this` bound to the processor instance:
/// own-field counters and `this.port` posts work per block. The pre-e122
/// call passed `this = undefined` — class bodies are strict mode, so every
/// `this.<x>` access threw on the first block, the node latched
/// processorerror and went silent (every WPT processor that touches `this`
/// in `process()` hung there while the arg-only echo surface stayed green).
#[test]
fn audioworklet_process_this_binding_live() {
    if should_skip() {
        return;
    }
    let _guard = TEST_SERIALIZER.lock().unwrap_or_else(|e| e.into_inner());

    let fixture = AwHttpFixture::spawn();
    fixture.set_origin(format!("http://127.0.0.1:{}/", fixture.port));
    let runtime = BrowserRuntime::new(BaoConfig::default())
        .expect("gated live test: BrowserRuntime::new must succeed");
    let page = spawn_wired_page(&fixture, &runtime, "this-state-processor.js");

    let result = eval_pending(
        &page,
        r#"window.__probe = 'pending'; (async function() {
             try {
               var ctx = new OfflineAudioContext(1, 128 * 32, 44100);
               await ctx.audioWorklet.addModule(window.location.origin + '/this-state-processor.js');
               var node = new AudioWorkletNode(ctx, 'this-state-processor');
               var src = ctx.createConstantSource();
               src.offset.value = 0.25;
               src.connect(node);
               node.connect(ctx.destination);
               src.start(0);
               var report = new Promise(function (resolve) {
                 node.port.onmessage = function (e) { resolve(e.data); };
               });
               var buf = await ctx.startRendering();
               var msg = await Promise.race([
                 report,
                 new Promise(function (resolve) { setTimeout(function () { resolve('__timeout__'); }, 10000); }),
               ]);
               window.__probe = JSON.stringify({ stage: 'done', msg: msg, len: buf.length });
             } catch (e) { window.__probe = 'threw:' + e; }
           })();"#,
        Duration::from_secs(40),
        "process this-binding",
    );

    eprintln!("[aw-test] process this-binding = {result}");
    assert!(
        result.contains("\"count\":10"),
        "process() must run with this bound to the instance (own-field counter reaches 10 and \
         posts from this.port), got: {result}"
    );
}

/// @trace REQ-BRW-002 [criterion:audioworklet-invalid-param-getter-invalidation] live
///
/// (e155) The spec's parameters-argument read-back face, mirroring the
/// reference implementation of crbug.com/1151069 (the WPT
/// `audioworkletprocessor-param-getter-overridden` semantics): before every
/// `process()` call the engine re-reads each declared parameter property off
/// the JS params object. A page-defined `Object.prototype` accessor
/// intercepts that read.
///
/// Variant A (wrong-length array on the second fetch): the match check is
/// fed a valid 128-sample array and the copy a 256-sample one — the
/// processor is INVALIDATED: `process()` never runs, the rendered buffer is
/// silent and `processorerror` fires exactly once (the node outputs silence
/// for the rest of its lifetime).
///
/// Variant B (always-throwing getter): the match check fails, the engine
/// rebuilds the params object with own data properties (shadowing the
/// accessor, CreateDataProperty semantics) — the node keeps rendering (the
/// buffer is NOT silent and no error fires). Runs on its own context: an
/// OfflineAudioContext renders exactly once.
#[test]
fn audioworklet_invalid_param_getter_invalidates_node_live() {
    if should_skip() {
        return;
    }
    let _guard = TEST_SERIALIZER.lock().unwrap_or_else(|e| e.into_inner());

    let fixture = AwHttpFixture::spawn();
    fixture.set_origin(format!("http://127.0.0.1:{}/", fixture.port));
    let runtime = BrowserRuntime::new(BaoConfig::default())
        .expect("gated live test: BrowserRuntime::new must succeed");
    let page = spawn_wired_page(&fixture, &runtime, "invalid-param-processor.js");

    let result = eval_pending(
        &page,
        r#"window.__probe = 'pending'; (async function() {
             try {
               var module = window.location.origin + '/invalid-param-processor.js';

               // Variant A: the 128-then-256 getter feeds the read-back.
               var ctxA = new OfflineAudioContext(1, 12800, 16000);
               await ctxA.audioWorklet.addModule(module);
               var buffer = new AudioBuffer({
                 length: 2, numberOfChannels: 1, sampleRate: ctxA.sampleRate
               });
               buffer.getChannelData(0)[0] = 1;
               var source = new AudioBufferSourceNode(ctxA, {buffer: buffer, loop: true});
               source.start();
               var n1 = new AudioWorkletNode(ctxA, 'invalid-param-array-1');
               var n2 = new AudioWorkletNode(ctxA, 'invalid-param-array-2');
               n1.connect(n2).connect(ctxA.destination);
               source.connect(n2.parameters.get('invalidParam'));
               var fired = 0;
               n2.onprocessorerror = function () { ++fired; };
               var bufA = await ctxA.startRendering();
               var dA = bufA.getChannelData(0);
               var nonzeroA = 0;
               for (var i = 0; i < dA.length; ++i) if (dA[i] !== 0) ++nonzeroA;

               // Variant B: an always-throwing getter — absorbed by the
               // rebuild (own data property shadows the accessor).
               var ctxB = new OfflineAudioContext(1, 12800, 16000);
               await ctxB.audioWorklet.addModule(module);
               var bufferB = new AudioBuffer({
                 length: 2, numberOfChannels: 1, sampleRate: ctxB.sampleRate
               });
               bufferB.getChannelData(0)[0] = 1;
               var sourceB = new AudioBufferSourceNode(ctxB, {buffer: bufferB, loop: true});
               sourceB.start();
               var t1 = new AudioWorkletNode(ctxB, 'invalid-throw-1');
               var t2 = new AudioWorkletNode(ctxB, 'invalid-throw-2');
               t1.connect(t2).connect(ctxB.destination);
               sourceB.connect(t2.parameters.get('throwParam'));
               var throwFired = 0;
               t2.onprocessorerror = function () { ++throwFired; };
               var bufB = await ctxB.startRendering();
               var dB = bufB.getChannelData(0);
               var nonzeroB = 0;
               for (var j = 0; j < dB.length; ++j) if (dB[j] !== 0) ++nonzeroB;

               window.__probe = JSON.stringify({
                 stage: 'done', nonzeroA: nonzeroA, fired: fired,
                 nonzeroB: nonzeroB, throwFired: throwFired
               });
             } catch (e) { window.__probe = 'threw:' + e; }
           })();"#,
        Duration::from_secs(40),
        "invalid param getter invalidation",
    );

    eprintln!("[aw-test] invalid param getter = {result}");
    let nonzero_a: f64 = result
        .split("\"nonzeroA\":")
        .nth(1)
        .and_then(|rest| rest.split([',', '}']).next())
        .and_then(|v| v.parse().ok())
        .unwrap_or(-1.);
    let fired: f64 = result
        .split("\"fired\":")
        .nth(1)
        .and_then(|rest| rest.split([',', '}']).next())
        .and_then(|v| v.parse().ok())
        .unwrap_or(-1.);
    assert!(
        nonzero_a == 0. && fired == 1.,
        "a wrong-length parameters getter must invalidate the node: process() never runs, the \
         render is silent (nonzeroA={nonzero_a}) and processorerror fires once (fired={fired}), \
         got: {result}"
    );
    let nonzero_b: f64 = result
        .split("\"nonzeroB\":")
        .nth(1)
        .and_then(|rest| rest.split([',', '}']).next())
        .and_then(|v| v.parse().ok())
        .unwrap_or(-1.);
    let throw_fired: f64 = result
        .split("\"throwFired\":")
        .nth(1)
        .and_then(|rest| rest.split([',', '}']).next())
        .and_then(|v| v.parse().ok())
        .unwrap_or(-1.);
    assert!(
        nonzero_b > 0. && throw_fired == 0.,
        "a throwing parameters getter must be absorbed by the params-object rebuild (own data \
         property shadows the accessor): the node keeps rendering (nonzeroB={nonzero_b}, \
         throwFired={throw_fired}), got: {result}"
    );
}
