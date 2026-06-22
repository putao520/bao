// REQ-STL-004: Stealth runtime JS hook injection  @trace REQ-STL-004
use crate::canvas::CanvasNoise;
use crate::navigator::{NavigatorProfile, ScreenProfile};
use crate::webgl_audio::{AudioProfile, WebGLProfile};
use crate::profile::{FontConfig, BatteryConfig, WebRtcMode, TimingConfig, ClientRectsConfig, ScreenDisplayConfig};

pub struct StealthHooks {
    canvas_js: String,
    audio_js: String,
    navigator_js: String,
    font_js: String,
    battery_js: String,
    webrtc_js: String,
    timing_js: String,
    clientrects_js: String,
    screen_display_js: String,
}

impl StealthHooks {
    pub fn from_profile(
        canvas: &CanvasNoise,
        audio: &AudioProfile,
        navigator: &NavigatorProfile,
        screen: &ScreenProfile,
        webgl: &WebGLProfile,
        font: &FontConfig,
        battery: &BatteryConfig,
        webrtc_mode: WebRtcMode,
        timing: &TimingConfig,
        clientrects: &ClientRectsConfig,
        screen_display: &ScreenDisplayConfig,
    ) -> Self {
        StealthHooks {
            canvas_js: Self::build_canvas_js(canvas),
            audio_js: Self::build_audio_js(audio),
            navigator_js: Self::build_navigator_js(navigator, screen, webgl),
            font_js: Self::build_font_js(font),
            battery_js: Self::build_battery_js(battery),
            webrtc_js: Self::build_webrtc_js(webrtc_mode),
            timing_js: Self::build_timing_js(timing),
            clientrects_js: Self::build_clientrects_js(clientrects),
            screen_display_js: Self::build_screen_display_js(screen_display),
        }
    }

    pub fn canvas_js(&self) -> &str {
        &self.canvas_js
    }

    pub fn audio_js(&self) -> &str {
        &self.audio_js
    }

    pub fn navigator_js(&self) -> &str {
        &self.navigator_js
    }

    pub fn font_js(&self) -> &str {
        &self.font_js
    }

    pub fn battery_js(&self) -> &str {
        &self.battery_js
    }

    pub fn webrtc_js(&self) -> &str {
        &self.webrtc_js
    }

    pub fn timing_js(&self) -> &str {
        &self.timing_js
    }

    pub fn clientrects_js(&self) -> &str {
        &self.clientrects_js
    }

    pub fn screen_display_js(&self) -> &str {
        &self.screen_display_js
    }

    pub fn combined_js(&self) -> String {
        let mut out = String::with_capacity(
            self.canvas_js.len()
                + self.audio_js.len()
                + self.navigator_js.len()
                + self.font_js.len()
                + self.battery_js.len()
                + self.webrtc_js.len()
                + self.timing_js.len()
                + self.clientrects_js.len()
                + self.screen_display_js.len()
                + 10,
        );
        out.push_str(&self.canvas_js);
        out.push('\n');
        out.push_str(&self.audio_js);
        out.push('\n');
        out.push_str(&self.navigator_js);
        out.push('\n');
        out.push_str(&self.font_js);
        out.push('\n');
        out.push_str(&self.battery_js);
        out.push('\n');
        out.push_str(&self.webrtc_js);
        out.push('\n');
        out.push_str(&self.timing_js);
        out.push('\n');
        out.push_str(&self.clientrects_js);
        out.push('\n');
        out.push_str(&self.screen_display_js);
        out
    }

    // ── Canvas hooks ──────────────────────────────────────────────

    fn build_canvas_js(canvas: &CanvasNoise) -> String {
        let seed = canvas.seed();
        let amplitude = canvas.noise_amplitude();

        format!(
            r#"(function() {{
  var seed = {seed}n;
  var amplitude = {amplitude};

  function detNoise(x, y) {{
    var state = seed;
    state ^= BigInt(x) * 0x517CC1B727220A95n;
    state ^= BigInt(y) * 0x6C62272E07BB0142n;
    state = BigInt.asUintN(64, state * 0x2545F4914F6CDD1Dn);
    state ^= state >> 33n;
    state = BigInt.asUintN(64, state * 0x27D4EB2D1659B4D6n);
    state ^= state >> 33n;
    return Number(BigInt.asUintN(64, state)) / 18446744073709551615 - 0.5;
  }}

  function addNoiseToImageData(imgData, width) {{
    for (var i = 0; i < imgData.data.length; i += 4) {{
      var x = (i / 4) % width;
      var y = Math.floor((i / 4) / width);
      var noise = detNoise(x, y);
      imgData.data[i]   = Math.max(0, Math.min(255, imgData.data[i]   + noise * amplitude * 255));
      imgData.data[i+1] = Math.max(0, Math.min(255, imgData.data[i+1] + noise * amplitude * 127));
      imgData.data[i+2] = Math.max(0, Math.min(255, imgData.data[i+2] + noise * amplitude * 63));
    }}
  }}

  var origToDataURL = HTMLCanvasElement.prototype.toDataURL;
  HTMLCanvasElement.prototype.toDataURL = function() {{
    var ctx = this.getContext('2d');
    if (ctx && seed > 0n) {{
      try {{
        var imgData = ctx.getImageData(0, 0, this.width, this.height);
        addNoiseToImageData(imgData, this.width);
        var temp = document.createElement('canvas');
        temp.width = this.width;
        temp.height = this.height;
        temp.getContext('2d').putImageData(imgData, 0, 0);
        return origToDataURL.apply(temp, arguments);
      }} catch(e) {{}}
    }}
    return origToDataURL.apply(this, arguments);
  }};

  var origToBlob = HTMLCanvasElement.prototype.toBlob;
  HTMLCanvasElement.prototype.toBlob = function(callback, mimeType, qualityArgument) {{
    var ctx = this.getContext('2d');
    if (ctx && seed > 0n) {{
      try {{
        var imgData = ctx.getImageData(0, 0, this.width, this.height);
        addNoiseToImageData(imgData, this.width);
        var temp = document.createElement('canvas');
        temp.width = this.width;
        temp.height = this.height;
        temp.getContext('2d').putImageData(imgData, 0, 0);
        return origToBlob.call(temp, callback, mimeType, qualityArgument);
      }} catch(e) {{}}
    }}
    return origToBlob.apply(this, arguments);
  }};

  var origGetImageData = CanvasRenderingContext2D.prototype.getImageData;
  CanvasRenderingContext2D.prototype.getImageData = function(sx, sy, sw, sh) {{
    var imgData = origGetImageData.call(this, sx, sy, sw, sh);
    if (seed > 0n) {{
      addNoiseToImageData(imgData, sw);
    }}
    return imgData;
  }};
}})();"#,
            seed = seed,
            amplitude = amplitude,
        )
    }

    // ── Audio hooks ───────────────────────────────────────────────

    fn build_audio_js(audio: &AudioProfile) -> String {
        let seed = audio.seed();
        let amplitude = audio.noise_amplitude();

        format!(
            r#"(function() {{
  var seed = {seed}n;
  var amplitude = {amplitude};

  function detNoise(index) {{
    var state = seed;
    state ^= BigInt(index) * 0x517CC1B727220A95n;
    state = BigInt.asUintN(64, state * 0x2545F4914F6CDD1Dn);
    state ^= state >> 33n;
    return Number(BigInt.asUintN(64, state)) / 18446744073709551615 - 0.5;
  }}

  var origGetChannelData = AudioBuffer.prototype.getChannelData;
  AudioBuffer.prototype.getChannelData = function(channel) {{
    var data = origGetChannelData.call(this, channel);
    if (seed > 0n && amplitude > 0) {{
      for (var i = 0; i < data.length; i++) {{
        data[i] += detNoise(i) * amplitude;
      }}
    }}
    return data;
  }};

  if (typeof OfflineAudioContext !== 'undefined') {{
    var origStartRendering = OfflineAudioContext.prototype.startRendering;
    OfflineAudioContext.prototype.startRendering = function() {{
      var self = this;
      return origStartRendering.call(this).then(function(buffer) {{
        if (seed > 0n && amplitude > 0) {{
          for (var ch = 0; ch < buffer.numberOfChannels; ch++) {{
            var data = buffer.getChannelData(ch);
            for (var i = 0; i < data.length; i++) {{
              data[i] += detNoise(ch * 1000000 + i) * amplitude;
            }}
          }}
        }}
        return buffer;
      }});
    }};
  }}
}})();"#,
            seed = seed,
            amplitude = amplitude,
        )
    }

    // ── Navigator/Screen/WebGL hooks ──────────────────────────────

    fn build_navigator_js(
        nav: &NavigatorProfile,
        screen: &ScreenProfile,
        webgl: &WebGLProfile,
    ) -> String {
        let languages_json = serde_json::to_string(&nav.languages)
            .unwrap_or_else(|_| r#"["en-US","en"]"#.into());
        let extensions_json = serde_json::to_string(&webgl.extensions)
            .unwrap_or_else(|_| "[]".into());

        format!(
            r#"(function() {{
  Object.defineProperty(navigator, 'userAgent', {{ get: function() {{ return {ua:?}; }}, configurable: false }});
  Object.defineProperty(navigator, 'platform', {{ get: function() {{ return {platform:?}; }}, configurable: false }});
  Object.defineProperty(navigator, 'hardwareConcurrency', {{ get: function() {{ return {hwc}; }}, configurable: false }});
  Object.defineProperty(navigator, 'language', {{ get: function() {{ return {lang:?}; }}, configurable: false }});
  Object.defineProperty(navigator, 'languages', {{ get: function() {{ return {langs}; }}, configurable: false }});
  Object.defineProperty(navigator, 'vendor', {{ get: function() {{ return {vendor:?}; }}, configurable: false }});
  Object.defineProperty(navigator, 'deviceMemory', {{ get: function() {{ return {dm}; }}, configurable: false }});
  Object.defineProperty(navigator, 'maxTouchPoints', {{ get: function() {{ return {mtp}; }}, configurable: false }});
  Object.defineProperty(navigator, 'webdriver', {{ get: function() {{ return false; }}, configurable: false }});

  Object.defineProperty(screen, 'width', {{ get: function() {{ return {sw}; }}, configurable: false }});
  Object.defineProperty(screen, 'height', {{ get: function() {{ return {sh}; }}, configurable: false }});
  Object.defineProperty(screen, 'availWidth', {{ get: function() {{ return {aw}; }}, configurable: false }});
  Object.defineProperty(screen, 'availHeight', {{ get: function() {{ return {ah}; }}, configurable: false }});
  Object.defineProperty(screen, 'colorDepth', {{ get: function() {{ return {cd}; }}, configurable: false }});
  Object.defineProperty(screen, 'pixelDepth', {{ get: function() {{ return {pd}; }}, configurable: false }});
  Object.defineProperty(window, 'devicePixelRatio', {{ get: function() {{ return {dpr}; }}, configurable: false }});

  var origGetParameter = WebGLRenderingContext.prototype.getParameter;
  WebGLRenderingContext.prototype.getParameter = function(param) {{
    var dbgRenderer = 0x9246;
    var dbgVendor = 0x9245;
    if (param === dbgRenderer) return {renderer:?};
    if (param === dbgVendor) return {wgl_vendor:?};
    var maxTexSize = 0x0D33;
    var maxRbSize = 0x84E8;
    var maxViewport = 0x0D3A;
    if (param === maxTexSize) return {mts};
    if (param === maxRbSize) return {mrbs};
    if (param === maxViewport) return new Int32Array([{vp0}, {vp1}]);
    return origGetParameter.call(this, param);
  }};

  var origGetSupportedExtensions = WebGLRenderingContext.prototype.getSupportedExtensions;
  WebGLRenderingContext.prototype.getSupportedExtensions = function() {{
    return {exts};
  }};

  if (typeof WebGL2RenderingContext !== 'undefined') {{
    var origGetParameter2 = WebGL2RenderingContext.prototype.getParameter;
    WebGL2RenderingContext.prototype.getParameter = function(param) {{
      var dbgRenderer = 0x9246;
      var dbgVendor = 0x9245;
      if (param === dbgRenderer) return {renderer:?};
      if (param === dbgVendor) return {wgl_vendor:?};
      var maxTexSize = 0x0D33;
      var maxRbSize = 0x84E8;
      var maxViewport = 0x0D3A;
      if (param === maxTexSize) return {mts};
      if (param === maxRbSize) return {mrbs};
      if (param === maxViewport) return new Int32Array([{vp0}, {vp1}]);
      return origGetParameter2.call(this, param);
    }};

    var origGetSupportedExtensions2 = WebGL2RenderingContext.prototype.getSupportedExtensions;
    WebGL2RenderingContext.prototype.getSupportedExtensions = function() {{
      return {exts};
    }};
  }}
}})();"#,
            ua = nav.user_agent,
            platform = nav.platform,
            hwc = nav.hardware_concurrency,
            lang = nav.language,
            langs = languages_json,
            vendor = nav.vendor,
            dm = nav.device_memory,
            mtp = nav.max_touch_points,
            sw = screen.width,
            sh = screen.height,
            aw = screen.avail_width,
            ah = screen.avail_height,
            cd = screen.color_depth,
            pd = screen.pixel_depth,
            dpr = screen.device_pixel_ratio,
            renderer = webgl.renderer,
            wgl_vendor = webgl.vendor,
            mts = webgl.max_texture_size,
            mrbs = webgl.max_renderbuffer_size,
            vp0 = webgl.max_viewport_dims[0],
            vp1 = webgl.max_viewport_dims[1],
            exts = extensions_json,
        )
    }

    // ── Font fingerprint protection ──────────────────────────────

    fn build_font_js(font: &FontConfig) -> String {
        let seed = font.seed;
        let extra_count = font.extra_font_count;
        let hidden_fonts_json = serde_json::to_string(&font.hidden_fonts)
            .unwrap_or_else(|_| "[]".into());

        format!(
            r#"(function() {{
  var SEED = {seed}n;
  var EXTRA_COUNT = {extra_count};
  var HIDDEN_FONTS = {hidden_fonts};

  // Deterministic xorshift64 PRNG
  function xorshift64(state) {{
    state = BigInt.asUintN(64, state);
    state ^= state << 13n;
    state = BigInt.asUintN(64, state);
    state ^= state >> 7n;
    state = BigInt.asUintN(64, state);
    state ^= state << 17n;
    return BigInt.asUintN(64, state);
  }}

  function detRand(index) {{
    var s = SEED ^ (BigInt(index) * 0x2545F4914F6CDD1Dn);
    s = xorshift64(s);
    return Number(s) / 18446744073709551615;
  }}

  if (typeof document !== 'undefined' && document.fonts) {{
    var origCheck = document.fonts.check.bind(document.fonts);
    document.fonts.check = function(font, text) {{
      for (var i = 0; i < HIDDEN_FONTS.length; i++) {{
        if (font.indexOf(HIDDEN_FONTS[i]) !== -1) return false;
      }}
      return origCheck(font, text);
    }};

    var origSize = Object.getOwnPropertyDescriptor(Document.prototype, 'fonts');
    if (origSize && origSize.get) {{
      var origGetter = origSize.get;
      Object.defineProperty(document, 'fonts', {{
        get: function() {{
          var fontsObj = origGetter.call(document);
          var origFontsSize = Object.getOwnPropertyDescriptor(FontData.prototype, 'size');
          if (origFontsSize && origFontsSize.get) {{
            var origSizeGetter = origFontsSize.get;
            Object.defineProperty(fontsObj, 'size', {{
              get: function() {{
                return origSizeGetter.call(fontsObj) + EXTRA_COUNT;
              }},
              configurable: true
            }});
          }}
          return fontsObj;
        }},
        configurable: true
      }});
    }}
  }}
}})();"#,
            seed = seed,
            extra_count = extra_count,
            hidden_fonts = hidden_fonts_json,
        )
    }

    // ── Battery API simulation ───────────────────────────────────

    fn build_battery_js(battery: &BatteryConfig) -> String {
        let charging = battery.charging;
        let level = battery.level;
        let charging_time = battery.charging_time;
        let discharging_time = if battery.discharging_time.is_infinite() {
            "Infinity".to_string()
        } else {
            format!("{}", battery.discharging_time)
        };

        format!(
            r#"(function() {{
  var fixedBattery = {{
    charging: {charging},
    chargingTime: {charging_time},
    dischargingTime: {discharging_time},
    level: {level},
    addEventListener: function() {{}},
    removeEventListener: function() {{}},
    dispatchEvent: function() {{ return true; }},
    onchargingchange: null,
    onchargingtimechange: null,
    ondischargingtimechange: null,
    onlevelchange: null
  }};

  if (typeof navigator !== 'undefined' && navigator.getBattery) {{
    navigator.getBattery = function() {{
      return Promise.resolve(fixedBattery);
    }};
  }}
}})();"#,
            charging = charging,
            charging_time = charging_time,
            discharging_time = discharging_time,
            level = level,
        )
    }

    // ── WebRTC leak protection ───────────────────────────────────

    fn build_webrtc_js(mode: WebRtcMode) -> String {
        match mode {
            WebRtcMode::None => r#"(function() {
  if (typeof RTCPeerConnection !== 'undefined') {
    window.RTCPeerConnection = function() {
      throw new DOMException('WebRTC is disabled', 'NotAllowedError');
    };
    Object.defineProperty(window, 'RTCPeerConnection', {
      value: function() {
        throw new DOMException('WebRTC is disabled', 'NotAllowedError');
      },
      configurable: false,
      writable: false
    });
  }
  if (typeof webkitRTCPeerConnection !== 'undefined') {
    window.webkitRTCPeerConnection = function() {
      throw new DOMException('WebRTC is disabled', 'NotAllowedError');
    };
  }
})();"#.to_string(),

            WebRtcMode::Default => r#"(function() {
  if (typeof RTCPeerConnection !== 'undefined') {
    var origAddEventListener = RTCPeerConnection.prototype.addEventListener;
    RTCPeerConnection.prototype.addEventListener = function(type, listener, options) {
      if (type === 'icecandidate') {
        var filteredListener = function(event) {
          if (event.candidate) {
            var c = event.candidate.candidate || '';
            // Allow mDNS candidates (end with .local) and relay candidates
            if (c.indexOf('.local') !== -1 || c.indexOf('relay') !== -1) {
              listener.call(this, event);
            }
            // Block srflx and host candidates to prevent IP leaks
          } else {
            listener.call(this, event);
          }
        };
        return origAddEventListener.call(this, type, filteredListener, options);
      }
      return origAddEventListener.call(this, type, listener, options);
    };
  }
})();"#.to_string(),

            WebRtcMode::Strict => r#"(function() {
  if (typeof RTCPeerConnection !== 'undefined') {
    var origAddEventListener = RTCPeerConnection.prototype.addEventListener;
    RTCPeerConnection.prototype.addEventListener = function(type, listener, options) {
      if (type === 'icecandidate') {
        var filteredListener = function(event) {
          if (event.candidate) {
            var c = event.candidate.candidate || '';
            // Strict: only allow relay candidates
            if (c.indexOf('relay') !== -1) {
              listener.call(this, event);
            }
          } else {
            listener.call(this, event);
          }
        };
        return origAddEventListener.call(this, type, filteredListener, options);
      }
      return origAddEventListener.call(this, type, listener, options);
    };

    // Also override createOffer/createAnswer to strip local IPs from SDP
    var origCreateOffer = RTCPeerConnection.prototype.createOffer;
    RTCPeerConnection.prototype.createOffer = function(options) {
      return origCreateOffer.call(this, options).then(function(desc) {
        desc.sdp = desc.sdp.replace(/a=candidate:.*typ\s+(srflx|host).*\r?\n/g, '');
        return desc;
      });
    };

    var origCreateAnswer = RTCPeerConnection.prototype.createAnswer;
    RTCPeerConnection.prototype.createAnswer = function(options) {
      return origCreateAnswer.call(this, options).then(function(desc) {
        desc.sdp = desc.sdp.replace(/a=candidate:.*typ\s+(srflx|host).*\r?\n/g, '');
        return desc;
      });
    };
  }
})();"#.to_string(),
        }
    }

    // ── Performance timing precision ─────────────────────────────

    fn build_timing_js(timing: &TimingConfig) -> String {
        let precision_us = timing.precision_us;
        let precision_ms = precision_us as f64 / 1000.0;

        format!(
            r#"(function() {{
  var PRECISION_MS = {precision_ms};

  function roundToPrecision(t) {{
    return Math.round(t / PRECISION_MS) * PRECISION_MS;
  }}

  if (typeof performance !== 'undefined') {{
    var origNow = performance.now.bind(performance);
    performance.now = function() {{
      return roundToPrecision(origNow());
    }};
  }}

  var origDateNow = Date.now;
  Date.now = function() {{
    return Math.round(roundToPrecision(origDateNow()));
  }};

  if (typeof Event !== 'undefined') {{
    var origTimeStamp = Object.getOwnPropertyDescriptor(Event.prototype, 'timeStamp');
    if (origTimeStamp && origTimeStamp.get) {{
      var origGetter = origTimeStamp.get;
      Object.defineProperty(Event.prototype, 'timeStamp', {{
        get: function() {{
          return roundToPrecision(origGetter.call(this));
        }},
        configurable: true
      }});
    }}
  }}
}})();"#,
            precision_ms = precision_ms,
        )
    }

    // ── ClientRects noise ────────────────────────────────────────

    fn build_clientrects_js(config: &ClientRectsConfig) -> String {
        let delta = config.noise_delta;
        let seed = config.seed;

        format!(
            r#"(function() {{
  var DELTA = {delta};
  var SEED = {seed}n;

  function xorshift64(state) {{
    state = BigInt.asUintN(64, state);
    state ^= state << 13n;
    state = BigInt.asUintN(64, state);
    state ^= state >> 7n;
    state = BigInt.asUintN(64, state);
    state ^= state << 17n;
    return BigInt.asUintN(64, state);
  }}

  function detNoise(element, field) {{
    var hash = SEED;
    if (typeof element === 'object' && element !== null) {{
      var id = element.id || element.tagName || '';
      for (var i = 0; i < id.length; i++) {{
        hash ^= BigInt(id.charCodeAt(i)) * BigInt(i + 1 + 0x517CC1B727220A95n);
        hash = xorshift64(hash);
      }}
    }}
    hash ^= BigInt(field) * 0x6C62272E07BB0142n;
    hash = xorshift64(hash);
    return (Number(hash) / 18446744073709551615 - 0.5) * 2 * DELTA;
  }}

  var cache = new WeakMap();
  var frameCounter = 0;

  function applyNoiseToRect(rect, element) {{
    if (!element || DELTA <= 0) return rect;
    var result = {{
      x: rect.x + detNoise(element, 0),
      y: rect.y + detNoise(element, 1),
      width: rect.width + detNoise(element, 2),
      height: rect.height + detNoise(element, 3),
      top: rect.top + detNoise(element, 4),
      right: rect.right + detNoise(element, 5),
      bottom: rect.bottom + detNoise(element, 6),
      left: rect.left + detNoise(element, 7),
      toJSON: function() {{
        return {{ x: this.x, y: this.y, width: this.width, height: this.height, top: this.top, right: this.right, bottom: this.bottom, left: this.left }};
      }}
    }};
    return result;
  }}

  if (typeof Element !== 'undefined') {{
    var origGetBoundingClientRect = Element.prototype.getBoundingClientRect;
    Element.prototype.getBoundingClientRect = function() {{
      var rect = origGetBoundingClientRect.call(this);
      return applyNoiseToRect(rect, this);
    }};

    var origGetClientRects = Element.prototype.getClientRects;
    Element.prototype.getClientRects = function() {{
      var rectList = origGetClientRects.call(this);
      var result = [];
      for (var i = 0; i < rectList.length; i++) {{
        result.push(applyNoiseToRect(rectList[i], this));
      }}
      return result;
    }};
  }}
}})();"#,
            delta = delta,
            seed = seed,
        )
    }

    // ── Screen/Display fingerprint ───────────────────────────────

    fn build_screen_display_js(config: &ScreenDisplayConfig) -> String {
        let width = config.width;
        let height = config.height;
        let color_depth = config.color_depth;
        let dpr = config.device_pixel_ratio;

        format!(
            r#"(function() {{
  var WIDTH = {width};
  var HEIGHT = {height};
  var COLOR_DEPTH = {color_depth};
  var DPR = {dpr};

  if (typeof screen !== 'undefined') {{
    Object.defineProperty(screen, 'width', {{ get: function() {{ return WIDTH; }}, configurable: true }});
    Object.defineProperty(screen, 'height', {{ get: function() {{ return HEIGHT; }}, configurable: true }});
    Object.defineProperty(screen, 'availWidth', {{ get: function() {{ return WIDTH; }}, configurable: true }});
    Object.defineProperty(screen, 'availHeight', {{ get: function() {{ return HEIGHT - 40; }}, configurable: true }});
    Object.defineProperty(screen, 'colorDepth', {{ get: function() {{ return COLOR_DEPTH; }}, configurable: true }});
    Object.defineProperty(screen, 'pixelDepth', {{ get: function() {{ return COLOR_DEPTH; }}, configurable: true }});
  }}

  if (typeof window !== 'undefined') {{
    Object.defineProperty(window, 'devicePixelRatio', {{ get: function() {{ return DPR; }}, configurable: true }});
    Object.defineProperty(window, 'outerWidth', {{ get: function() {{ return WIDTH; }}, configurable: true }});
    Object.defineProperty(window, 'outerHeight', {{ get: function() {{ return HEIGHT; }}, configurable: true }});
    Object.defineProperty(window, 'innerWidth', {{ get: function() {{ return WIDTH; }}, configurable: true }});
    Object.defineProperty(window, 'innerHeight', {{ get: function() {{ return HEIGHT - 80; }}, configurable: true }});
    Object.defineProperty(window, 'screenX', {{ get: function() {{ return 0; }}, configurable: true }});
    Object.defineProperty(window, 'screenY', {{ get: function() {{ return 0; }}, configurable: true }});
    Object.defineProperty(window, 'screenLeft', {{ get: function() {{ return 0; }}, configurable: true }});
    Object.defineProperty(window, 'screenTop', {{ get: function() {{ return 0; }}, configurable: true }});
  }}
}})();"#,
            width = width,
            height = height,
            color_depth = color_depth,
            dpr = dpr,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::canvas::CanvasNoise;
    use crate::navigator::{NavigatorProfile, ScreenProfile};
    use crate::profile::StealthProfile;
    use crate::webgl_audio::{AudioProfile, WebGLProfile};

    fn firefox_hooks() -> StealthHooks {
        let profile = StealthProfile::firefox_default();
        StealthHooks::from_profile(
            &profile.canvas,
            &profile.audio,
            &profile.navigator,
            &profile.screen,
            &profile.webgl,
            &profile.font,
            &profile.battery,
            profile.webrtc_mode,
            &profile.timing,
            &profile.clientrects,
            &profile.screen_display,
        )
    }

    fn chrome_hooks() -> StealthHooks {
        let profile = StealthProfile::chrome_default();
        StealthHooks::from_profile(
            &profile.canvas,
            &profile.audio,
            &profile.navigator,
            &profile.screen,
            &profile.webgl,
            &profile.font,
            &profile.battery,
            profile.webrtc_mode,
            &profile.timing,
            &profile.clientrects,
            &profile.screen_display,
        )
    }

    #[test]
    fn canvas_js_contains_todataurl_override() {
        let hooks = firefox_hooks();
        let js = hooks.canvas_js();
        assert!(
            js.contains("HTMLCanvasElement.prototype.toDataURL"),
            "canvas JS must override toDataURL"
        );
        assert!(
            js.contains("origToDataURL"),
            "canvas JS must store original toDataURL"
        );
    }

    #[test]
    fn canvas_js_contains_toblob_override() {
        let hooks = firefox_hooks();
        let js = hooks.canvas_js();
        assert!(
            js.contains("HTMLCanvasElement.prototype.toBlob"),
            "canvas JS must override toBlob"
        );
    }

    #[test]
    fn canvas_js_contains_getimagedata_override() {
        let hooks = firefox_hooks();
        let js = hooks.canvas_js();
        assert!(
            js.contains("CanvasRenderingContext2D.prototype.getImageData"),
            "canvas JS must override getImageData"
        );
    }

    #[test]
    fn canvas_js_contains_noise_injection() {
        let hooks = firefox_hooks();
        let js = hooks.canvas_js();
        assert!(
            js.contains("addNoiseToImageData"),
            "canvas JS must have noise injection function"
        );
        assert!(
            js.contains("detNoise"),
            "canvas JS must have deterministic noise function"
        );
    }

    #[test]
    fn canvas_js_contains_seed_value() {
        let hooks = firefox_hooks();
        let js = hooks.canvas_js();
        assert!(
            js.contains("42n"),
            "canvas JS must contain the seed as BigInt"
        );
    }

    #[test]
    fn canvas_js_contains_deterministic_noise_constants() {
        let hooks = firefox_hooks();
        let js = hooks.canvas_js();
        assert!(
            js.contains("0x517CC1B727220A95n"),
            "canvas JS must contain the x-multiply constant"
        );
        assert!(
            js.contains("0x6C62272E07BB0142n"),
            "canvas JS must contain the y-multiply constant"
        );
        assert!(
            js.contains("0x2545F4914F6CDD1Dn"),
            "canvas JS must contain the first multiply constant"
        );
        assert!(
            js.contains("0x27D4EB2D1659B4D6n"),
            "canvas JS must contain the second multiply constant"
        );
    }

    #[test]
    fn audio_js_contains_getchanneldata_override() {
        let hooks = firefox_hooks();
        let js = hooks.audio_js();
        assert!(
            js.contains("AudioBuffer.prototype.getChannelData"),
            "audio JS must override getChannelData"
        );
        assert!(
            js.contains("origGetChannelData"),
            "audio JS must store original getChannelData"
        );
    }

    #[test]
    fn audio_js_contains_startrendering_override() {
        let hooks = firefox_hooks();
        let js = hooks.audio_js();
        assert!(
            js.contains("OfflineAudioContext.prototype.startRendering"),
            "audio JS must override startRendering"
        );
    }

    #[test]
    fn audio_js_contains_noise_algorithm() {
        let hooks = firefox_hooks();
        let js = hooks.audio_js();
        assert!(
            js.contains("detNoise"),
            "audio JS must have deterministic noise function"
        );
        assert!(
            js.contains("0x517CC1B727220A95n"),
            "audio JS must contain the index-multiply constant"
        );
    }

    #[test]
    fn navigator_js_contains_user_agent_defineproperty() {
        let hooks = firefox_hooks();
        let js = hooks.navigator_js();
        assert!(
            js.contains("Object.defineProperty(navigator, 'userAgent'"),
            "navigator JS must define userAgent"
        );
    }

    #[test]
    fn navigator_js_contains_platform_defineproperty() {
        let hooks = firefox_hooks();
        let js = hooks.navigator_js();
        assert!(
            js.contains("Object.defineProperty(navigator, 'platform'"),
            "navigator JS must define platform"
        );
    }

    #[test]
    fn navigator_js_contains_hardware_concurrency_defineproperty() {
        let hooks = firefox_hooks();
        let js = hooks.navigator_js();
        assert!(
            js.contains("Object.defineProperty(navigator, 'hardwareConcurrency'"),
            "navigator JS must define hardwareConcurrency"
        );
    }

    #[test]
    fn navigator_js_contains_webdriver_false() {
        let hooks = firefox_hooks();
        let js = hooks.navigator_js();
        assert!(
            js.contains("navigator, 'webdriver'") && js.contains("return false"),
            "navigator JS must set webdriver to false"
        );
    }

    #[test]
    fn navigator_js_contains_screen_overrides() {
        let hooks = firefox_hooks();
        let js = hooks.navigator_js();
        assert!(
            js.contains("Object.defineProperty(screen, 'width'"),
            "navigator JS must define screen.width"
        );
        assert!(
            js.contains("Object.defineProperty(screen, 'height'"),
            "navigator JS must define screen.height"
        );
        assert!(
            js.contains("Object.defineProperty(window, 'devicePixelRatio'"),
            "navigator JS must define window.devicePixelRatio"
        );
    }

    #[test]
    fn navigator_js_contains_webgl_getparameter_override() {
        let hooks = firefox_hooks();
        let js = hooks.navigator_js();
        assert!(
            js.contains("WebGLRenderingContext.prototype.getParameter"),
            "navigator JS must override WebGL getParameter"
        );
    }

    #[test]
    fn navigator_js_contains_webgl_getsupportedextensions_override() {
        let hooks = firefox_hooks();
        let js = hooks.navigator_js();
        assert!(
            js.contains("WebGLRenderingContext.prototype.getSupportedExtensions"),
            "navigator JS must override getSupportedExtensions"
        );
    }

    #[test]
    fn navigator_js_contains_webgl2_overrides() {
        let hooks = firefox_hooks();
        let js = hooks.navigator_js();
        assert!(
            js.contains("WebGL2RenderingContext.prototype.getParameter"),
            "navigator JS must override WebGL2 getParameter"
        );
        assert!(
            js.contains("WebGL2RenderingContext.prototype.getSupportedExtensions"),
            "navigator JS must override WebGL2 getSupportedExtensions"
        );
    }

    #[test]
    fn combined_js_concatenates_all() {
        let hooks = firefox_hooks();
        let combined = hooks.combined_js();
        assert!(
            combined.contains("HTMLCanvasElement.prototype.toDataURL"),
            "combined JS must contain canvas hooks"
        );
        assert!(
            combined.contains("AudioBuffer.prototype.getChannelData"),
            "combined JS must contain audio hooks"
        );
        assert!(
            combined.contains("Object.defineProperty(navigator, 'userAgent'"),
            "combined JS must contain navigator hooks"
        );
        assert!(
            combined.contains("document.fonts.check"),
            "combined JS must contain font hooks"
        );
        assert!(
            combined.contains("navigator.getBattery"),
            "combined JS must contain battery hooks"
        );
        assert!(
            combined.contains("RTCPeerConnection"),
            "combined JS must contain WebRTC hooks"
        );
        assert!(
            combined.contains("performance.now"),
            "combined JS must contain timing hooks"
        );
        assert!(
            combined.contains("getBoundingClientRect"),
            "combined JS must contain clientrects hooks"
        );
        assert!(
            combined.contains("screen_display_js") || combined.contains("screen, 'width'"),
            "combined JS must contain screen/display hooks"
        );
    }

    #[test]
    fn different_profiles_produce_different_canvas_js() {
        let ff = firefox_hooks();
        let ch = chrome_hooks();
        assert_ne!(
            ff.canvas_js(),
            ch.canvas_js(),
            "Firefox and Chrome should produce different canvas JS (different seeds)"
        );
    }

    #[test]
    fn different_profiles_produce_different_audio_js() {
        let ff = firefox_hooks();
        let ch = chrome_hooks();
        assert_ne!(
            ff.audio_js(),
            ch.audio_js(),
            "Firefox and Chrome should produce different audio JS (different seeds)"
        );
    }

    #[test]
    fn different_profiles_produce_different_navigator_js() {
        let ff = firefox_hooks();
        let ch = chrome_hooks();
        assert_ne!(
            ff.navigator_js(),
            ch.navigator_js(),
            "Firefox and Chrome should produce different navigator JS (different UA/vendor)"
        );
    }

    #[test]
    fn firefox_profile_user_agent_in_js() {
        let hooks = firefox_hooks();
        let js = hooks.navigator_js();
        assert!(
            js.contains("Firefox/128.0"),
            "Firefox navigator JS must contain Firefox user agent string"
        );
    }

    #[test]
    fn chrome_profile_user_agent_in_js() {
        let hooks = chrome_hooks();
        let js = hooks.navigator_js();
        assert!(
            js.contains("Chrome/128.0.0.0"),
            "Chrome navigator JS must contain Chrome user agent string"
        );
    }

    #[test]
    fn firefox_vendor_empty_in_js() {
        let hooks = firefox_hooks();
        let js = hooks.navigator_js();
        assert!(
            js.contains("return \"\"") || js.contains("return ''"),
            "Firefox navigator JS must have empty vendor"
        );
    }

    #[test]
    fn chrome_vendor_google_in_js() {
        let hooks = chrome_hooks();
        let js = hooks.navigator_js();
        assert!(
            js.contains("Google Inc."),
            "Chrome navigator JS must have Google Inc. vendor"
        );
    }

    #[test]
    fn canvas_noise_algorithm_matches_rust_constants() {
        let hooks = firefox_hooks();
        let js = hooks.canvas_js();
        assert!(
            js.contains("0x517CC1B727220A95n"),
            "JS must use same x-multiply constant as Rust deterministic_noise"
        );
        assert!(
            js.contains("0x6C62272E07BB0142n"),
            "JS must use same y-multiply constant as Rust deterministic_noise"
        );
        assert!(
            js.contains("0x2545F4914F6CDD1Dn"),
            "JS must use same first multiply constant as Rust deterministic_noise"
        );
        assert!(
            js.contains("0x27D4EB2D1659B4D6n"),
            "JS must use same second multiply constant as Rust deterministic_noise"
        );
    }

    #[test]
    fn audio_noise_algorithm_matches_rust_constants() {
        let hooks = firefox_hooks();
        let js = hooks.audio_js();
        assert!(
            js.contains("0x517CC1B727220A95n"),
            "Audio JS must use same index-multiply constant as Rust AudioProfile"
        );
        assert!(
            js.contains("0x2545F4914F6CDD1Dn"),
            "Audio JS must use same multiply constant as Rust AudioProfile"
        );
    }

    #[test]
    fn canvas_js_amplitude_matches_profile() {
        let canvas = CanvasNoise::new(42);
        let js = StealthHooks::build_canvas_js(&canvas);
        assert!(
            js.contains("0.001"),
            "Canvas JS must contain the default noise amplitude 0.001"
        );
    }

    #[test]
    fn audio_js_amplitude_matches_profile() {
        let audio = AudioProfile::new(42);
        let js = StealthHooks::build_audio_js(&audio);
        assert!(
            js.contains("1e-7") || js.contains("0.0000001"),
            "Audio JS must contain the default noise amplitude 1e-7"
        );
    }

    #[test]
    fn custom_canvas_seed_appears_in_js() {
        let canvas = CanvasNoise::new(9999);
        let js = StealthHooks::build_canvas_js(&canvas);
        assert!(
            js.contains("9999n"),
            "Canvas JS must contain the custom seed as BigInt"
        );
    }

    #[test]
    fn custom_audio_seed_appears_in_js() {
        let audio = AudioProfile::new(7777);
        let js = StealthHooks::build_audio_js(&audio);
        assert!(
            js.contains("7777n"),
            "Audio JS must contain the custom seed as BigInt"
        );
    }

    #[test]
    fn navigator_js_firefox_screen_values() {
        let hooks = firefox_hooks();
        let js = hooks.navigator_js();
        assert!(
            js.contains("1920") && js.contains("1080"),
            "Navigator JS must contain default screen dimensions"
        );
    }

    #[test]
    fn navigator_js_firefox_webgl_vendor() {
        let hooks = firefox_hooks();
        let js = hooks.navigator_js();
        assert!(
            js.contains("Mozilla"),
            "Firefox navigator JS must contain WebGL vendor 'Mozilla'"
        );
    }

    #[test]
    fn navigator_js_chrome_webgl_vendor() {
        let hooks = chrome_hooks();
        let js = hooks.navigator_js();
        assert!(
            js.contains("Google Inc. (NVIDIA)"),
            "Chrome navigator JS must contain WebGL vendor"
        );
    }

    #[test]
    fn hooks_from_profile_custom_screen() {
        let canvas = CanvasNoise::new(42);
        let audio = AudioProfile::new(42);
        let nav = NavigatorProfile::firefox();
        let screen = ScreenProfile::new(800, 600, 2.0);
        let webgl = WebGLProfile::firefox();
        let font = FontConfig::new(42);
        let battery = BatteryConfig::default();
        let timing = TimingConfig::default();
        let clientrects = ClientRectsConfig::default();
        let screen_display = ScreenDisplayConfig::default();
        let hooks = StealthHooks::from_profile(&canvas, &audio, &nav, &screen, &webgl, &font, &battery, WebRtcMode::Default, &timing, &clientrects, &screen_display);
        let js = hooks.navigator_js();
        assert!(
            js.contains("800") && js.contains("600"),
            "Custom screen dimensions must appear in navigator JS"
        );
        assert!(
            js.contains("2"),
            "Custom device pixel ratio must appear in navigator JS"
        );
    }

    #[test]
    fn canvas_js_is_valid_iife() {
        let hooks = firefox_hooks();
        let js = hooks.canvas_js();
        assert!(
            js.starts_with("(function() {") || js.starts_with("(function(){{"),
            "Canvas JS must be an IIFE"
        );
        assert!(
            js.ends_with("})();"),
            "Canvas JS must end with IIFE closure"
        );
    }

    #[test]
    fn audio_js_is_valid_iife() {
        let hooks = firefox_hooks();
        let js = hooks.audio_js();
        assert!(
            js.starts_with("(function() {") || js.starts_with("(function(){{"),
            "Audio JS must be an IIFE"
        );
        assert!(
            js.ends_with("})();"),
            "Audio JS must end with IIFE closure"
        );
    }

    #[test]
    fn navigator_js_is_valid_iife() {
        let hooks = firefox_hooks();
        let js = hooks.navigator_js();
        assert!(
            js.starts_with("(function() {") || js.starts_with("(function(){{"),
            "Navigator JS must be an IIFE"
        );
        assert!(
            js.ends_with("})();"),
            "Navigator JS must end with IIFE closure"
        );
    }

    // ── New dimension tests ──────────────────────────────────────

    #[test]
    fn font_js_contains_fonts_check_override() {
        let hooks = firefox_hooks();
        let js = hooks.font_js();
        assert!(
            js.contains("document.fonts.check"),
            "font JS must override document.fonts.check"
        );
    }

    #[test]
    fn font_js_contains_seed() {
        let hooks = firefox_hooks();
        let js = hooks.font_js();
        assert!(
            js.contains("42n"),
            "font JS must contain the seed as BigInt"
        );
    }

    #[test]
    fn font_js_is_valid_iife() {
        let hooks = firefox_hooks();
        let js = hooks.font_js();
        assert!(
            js.starts_with("(function() {") || js.starts_with("(function(){{"),
            "Font JS must be an IIFE"
        );
        assert!(
            js.ends_with("})();"),
            "Font JS must end with IIFE closure"
        );
    }

    #[test]
    fn battery_js_contains_getbattery_override() {
        let hooks = firefox_hooks();
        let js = hooks.battery_js();
        assert!(
            js.contains("navigator.getBattery"),
            "battery JS must override navigator.getBattery"
        );
        assert!(
            js.contains("Promise.resolve"),
            "battery JS must return a resolved Promise"
        );
    }

    #[test]
    fn battery_js_default_values() {
        let hooks = firefox_hooks();
        let js = hooks.battery_js();
        assert!(
            js.contains("charging: true"),
            "battery JS must have charging=true by default"
        );
        assert!(
            js.contains("level: 1"),
            "battery JS must have level=1 by default"
        );
        assert!(
            js.contains("Infinity"),
            "battery JS must have dischargingTime=Infinity by default"
        );
    }

    #[test]
    fn battery_js_is_valid_iife() {
        let hooks = firefox_hooks();
        let js = hooks.battery_js();
        assert!(
            js.starts_with("(function() {") || js.starts_with("(function(){{"),
            "Battery JS must be an IIFE"
        );
        assert!(
            js.ends_with("})();"),
            "Battery JS must end with IIFE closure"
        );
    }

    #[test]
    fn webrtc_js_default_mode_filters_candidates() {
        let hooks = firefox_hooks();
        let js = hooks.webrtc_js();
        assert!(
            js.contains("RTCPeerConnection"),
            "WebRTC JS must reference RTCPeerConnection"
        );
        assert!(
            js.contains("icecandidate"),
            "WebRTC Default mode must filter ICE candidates"
        );
    }

    #[test]
    fn webrtc_js_none_mode_throws_error() {
        let js = StealthHooks::build_webrtc_js(WebRtcMode::None);
        assert!(
            js.contains("NotAllowedError"),
            "WebRTC None mode must throw NotAllowedError"
        );
        assert!(
            js.contains("WebRTC is disabled"),
            "WebRTC None mode must have descriptive error message"
        );
    }

    #[test]
    fn webrtc_js_strict_mode_strips_sdp() {
        let js = StealthHooks::build_webrtc_js(WebRtcMode::Strict);
        assert!(
            js.contains("createOffer"),
            "WebRTC Strict mode must override createOffer"
        );
        assert!(
            js.contains("createAnswer"),
            "WebRTC Strict mode must override createAnswer"
        );
        assert!(
            js.contains("srflx"),
            "WebRTC Strict mode must strip srflx candidates from SDP"
        );
    }

    #[test]
    fn webrtc_js_is_valid_iife() {
        let hooks = firefox_hooks();
        let js = hooks.webrtc_js();
        assert!(
            js.starts_with("(function() {") || js.starts_with("(function(){{"),
            "WebRTC JS must be an IIFE"
        );
        assert!(
            js.ends_with("})();"),
            "WebRTC JS must end with IIFE closure"
        );
    }

    #[test]
    fn timing_js_contains_performance_now_override() {
        let hooks = firefox_hooks();
        let js = hooks.timing_js();
        assert!(
            js.contains("performance.now"),
            "timing JS must override performance.now"
        );
        assert!(
            js.contains("Date.now"),
            "timing JS must override Date.now"
        );
        assert!(
            js.contains("timeStamp"),
            "timing JS must override Event.timeStamp"
        );
    }

    #[test]
    fn timing_js_default_precision() {
        let hooks = firefox_hooks();
        let js = hooks.timing_js();
        assert!(
            js.contains("0.1"),
            "timing JS must contain default precision 0.1ms"
        );
    }

    #[test]
    fn timing_js_is_valid_iife() {
        let hooks = firefox_hooks();
        let js = hooks.timing_js();
        assert!(
            js.starts_with("(function() {") || js.starts_with("(function(){{"),
            "Timing JS must be an IIFE"
        );
        assert!(
            js.ends_with("})();"),
            "Timing JS must end with IIFE closure"
        );
    }

    #[test]
    fn clientrects_js_contains_getboundingclientrect_override() {
        let hooks = firefox_hooks();
        let js = hooks.clientrects_js();
        assert!(
            js.contains("getBoundingClientRect"),
            "clientrects JS must override getBoundingClientRect"
        );
        assert!(
            js.contains("getClientRects"),
            "clientrects JS must override getClientRects"
        );
    }

    #[test]
    fn clientrects_js_contains_noise_delta() {
        let hooks = firefox_hooks();
        let js = hooks.clientrects_js();
        assert!(
            js.contains("0.5"),
            "clientrects JS must contain default noise delta 0.5"
        );
    }

    #[test]
    fn clientrects_js_is_valid_iife() {
        let hooks = firefox_hooks();
        let js = hooks.clientrects_js();
        assert!(
            js.starts_with("(function() {") || js.starts_with("(function(){{"),
            "ClientRects JS must be an IIFE"
        );
        assert!(
            js.ends_with("})();"),
            "ClientRects JS must end with IIFE closure"
        );
    }

    #[test]
    fn screen_display_js_contains_screen_overrides() {
        let hooks = firefox_hooks();
        let js = hooks.screen_display_js();
        assert!(
            js.contains("screen, 'width'"),
            "screen display JS must override screen.width"
        );
        assert!(
            js.contains("screen, 'height'"),
            "screen display JS must override screen.height"
        );
        assert!(
            js.contains("devicePixelRatio"),
            "screen display JS must override devicePixelRatio"
        );
    }

    #[test]
    fn screen_display_js_default_values() {
        let hooks = firefox_hooks();
        let js = hooks.screen_display_js();
        assert!(
            js.contains("1920") && js.contains("1080"),
            "screen display JS must contain default 1920x1080"
        );
        assert!(
            js.contains("24"),
            "screen display JS must contain default colorDepth 24"
        );
    }

    #[test]
    fn screen_display_js_is_valid_iife() {
        let hooks = firefox_hooks();
        let js = hooks.screen_display_js();
        assert!(
            js.starts_with("(function() {") || js.starts_with("(function(){{"),
            "Screen display JS must be an IIFE"
        );
        assert!(
            js.ends_with("})();"),
            "Screen display JS must end with IIFE closure"
        );
    }

    #[test]
    fn different_profiles_produce_different_font_js() {
        let ff = firefox_hooks();
        let ch = chrome_hooks();
        assert_ne!(
            ff.font_js(), ch.font_js(),
            "Firefox and Chrome should produce different font JS (different seeds)"
        );
    }

    #[test]
    fn different_profiles_produce_different_clientrects_js() {
        let ff = firefox_hooks();
        let ch = chrome_hooks();
        assert_ne!(
            ff.clientrects_js(), ch.clientrects_js(),
            "Firefox and Chrome should produce different clientrects JS (different seeds)"
        );
    }
}
