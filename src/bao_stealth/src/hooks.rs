// REQ-STL-004: Stealth runtime JS hook injection  @trace REQ-STL-004
use crate::canvas::CanvasNoise;
use crate::navigator::{NavigatorProfile, ScreenProfile};
use crate::webgl_audio::{AudioProfile, WebGLProfile};

pub struct StealthHooks {
    canvas_js: String,
    audio_js: String,
    navigator_js: String,
}

impl StealthHooks {
    pub fn from_profile(
        canvas: &CanvasNoise,
        audio: &AudioProfile,
        navigator: &NavigatorProfile,
        screen: &ScreenProfile,
        webgl: &WebGLProfile,
    ) -> Self {
        StealthHooks {
            canvas_js: Self::build_canvas_js(canvas),
            audio_js: Self::build_audio_js(audio),
            navigator_js: Self::build_navigator_js(navigator, screen, webgl),
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

    pub fn combined_js(&self) -> String {
        let mut out = String::with_capacity(
            self.canvas_js.len() + self.audio_js.len() + self.navigator_js.len() + 2,
        );
        out.push_str(&self.canvas_js);
        out.push('\n');
        out.push_str(&self.audio_js);
        out.push('\n');
        out.push_str(&self.navigator_js);
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
    fn combined_js_concatenates_all_three() {
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
        let hooks = StealthHooks::from_profile(&canvas, &audio, &nav, &screen, &webgl);
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
}
