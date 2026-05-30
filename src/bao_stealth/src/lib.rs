#![allow(dead_code, unused_imports)]
// REQ-STL-007: Stealth engine integration and CDP stealth  @trace REQ-STL-001
mod profile;
mod tls;
mod http2;
mod canvas;
mod navigator;
mod webgl_audio;
mod behavior;

pub use profile::StealthProfile;
pub use tls::TlsFingerprint;
pub use http2::Http2Fingerprint;
pub use canvas::CanvasNoise;
pub use navigator::{NavigatorProfile, ScreenProfile};
pub use webgl_audio::{WebGLProfile, AudioProfile};
pub use behavior::BehaviorSimulator;


pub struct StealthEngine {
    profile: StealthProfile,
}

impl StealthEngine {
    pub fn new(profile: StealthProfile) -> Self {
        StealthEngine { profile }
    }

    #[allow(clippy::new_ret_no_self)]
    pub fn default_engine() -> Self {
        Self::new(StealthProfile::firefox_default())
    }

    pub fn profile(&self) -> &StealthProfile {
        &self.profile
    }

    pub fn tls_config(&self) -> &TlsFingerprint {
        &self.profile.tls
    }

    pub fn http2_config(&self) -> &Http2Fingerprint {
        &self.profile.http2
    }

    pub fn canvas_noise(&self) -> &CanvasNoise {
        &self.profile.canvas
    }

    pub fn navigator(&self) -> &NavigatorProfile {
        &self.profile.navigator
    }

    pub fn screen(&self) -> &ScreenProfile {
        &self.profile.screen
    }

    pub fn webgl(&self) -> &WebGLProfile {
        &self.profile.webgl
    }

    pub fn audio(&self) -> &AudioProfile {
        &self.profile.audio
    }

    pub fn behavior(&self) -> &BehaviorSimulator {
        &self.profile.behavior
    }

    pub fn inject_navigator_js(&self) -> String {
        let nav = &self.profile.navigator;
        let scr = &self.profile.screen;
        format!(
            r#"
Object.defineProperty(navigator, 'userAgent', {{ get: () => '{ua}' }});
Object.defineProperty(navigator, 'platform', {{ get: () => '{platform}' }});
Object.defineProperty(navigator, 'language', {{ get: () => '{language}' }});
Object.defineProperty(navigator, 'languages', {{ get: () => ['{language}'] }});
Object.defineProperty(navigator, 'hardwareConcurrency', {{ get: () => {cores} }});
Object.defineProperty(navigator, 'webdriver', {{ get: () => false }});
Object.defineProperty(navigator, 'maxTouchPoints', {{ get: () => {touch} }});
Object.defineProperty(screen, 'width', {{ get: () => {w} }});
Object.defineProperty(screen, 'height', {{ get: () => {h} }});
Object.defineProperty(screen, 'availWidth', {{ get: () => {w} }});
Object.defineProperty(screen, 'availHeight', {{ get: () => {h} }});
Object.defineProperty(window, 'devicePixelRatio', {{ get: () => {dpr} }});

// CDP stealth: remove automation markers
delete window.cdc_adoQpoasnfa76pfcZLmcfl_Array;
delete window.cdc_adoQpoasnfa76pfcZLmcfl_Promise;
delete window.cdc_adoQpoasnfa76pfcZLmcfl_Symbol;
if (window.chrome) {{ delete window.chrome.runtime; }}

// WebGL vendor/renderer override
const getParameter = WebGLRenderingContext.prototype.getParameter;
WebGLRenderingContext.prototype.getParameter = function(param) {{
    if (param === 0x1F00) return '{vendor}';
    if (param === 0x1F01) return '{renderer}';
    return getParameter.call(this, param);
}};
"#,
            ua = nav.user_agent,
            platform = nav.platform,
            language = nav.language,
            cores = nav.hardware_concurrency,
            touch = nav.max_touch_points,
            w = scr.width,
            h = scr.height,
            dpr = scr.device_pixel_ratio,
            vendor = self.profile.webgl.vendor,
            renderer = self.profile.webgl.renderer,
        )
    }
}
