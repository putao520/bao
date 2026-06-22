// @trace REQ-STL-007
use crate::tls::TlsFingerprint;
use crate::http2::Http2Fingerprint;
use crate::canvas::CanvasNoise;
use crate::navigator::{NavigatorProfile, ScreenProfile};
use crate::webgl_audio::{WebGLProfile, AudioProfile};
use crate::behavior::{BehaviorConfig, BehaviorSimulator};

/// WebRTC leak protection mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WebRtcMode {
    /// Default: filter ICE candidates, only allow mDNS/relay.
    Default,
    /// Strict: filter ICE candidates aggressively.
    Strict,
    /// None: completely disable WebRTC (throw NotAllowedError).
    None,
}

impl Default for WebRtcMode {
    fn default() -> Self {
        WebRtcMode::Default
    }
}

/// Font fingerprint protection configuration.
#[derive(Debug, Clone)]
pub struct FontConfig {
    pub seed: u64,
    pub extra_font_count: u32,
    pub hidden_fonts: Vec<String>,
}

impl FontConfig {
    pub fn new(seed: u64) -> Self {
        FontConfig {
            seed,
            extra_font_count: 0,
            hidden_fonts: Vec::new(),
        }
    }
}

impl Default for FontConfig {
    fn default() -> Self {
        FontConfig::new(42)
    }
}

/// Battery API simulation configuration.
#[derive(Debug, Clone)]
pub struct BatteryConfig {
    pub charging: bool,
    pub level: f64,
    pub charging_time: f64,
    pub discharging_time: f64,
}

impl Default for BatteryConfig {
    fn default() -> Self {
        BatteryConfig {
            charging: true,
            level: 1.0,
            charging_time: 0.0,
            discharging_time: f64::INFINITY,
        }
    }
}

/// Performance timing precision configuration.
#[derive(Debug, Clone)]
pub struct TimingConfig {
    /// Precision in microseconds. Default 100 = 0.1ms rounding.
    pub precision_us: u64,
}

impl Default for TimingConfig {
    fn default() -> Self {
        TimingConfig { precision_us: 100 }
    }
}

/// ClientRects noise configuration.
#[derive(Debug, Clone)]
pub struct ClientRectsConfig {
    /// Noise delta (±). Default 0.5 pixels.
    pub noise_delta: f64,
    /// Seed for deterministic noise.
    pub seed: u64,
}

impl Default for ClientRectsConfig {
    fn default() -> Self {
        ClientRectsConfig {
            noise_delta: 0.5,
            seed: 42,
        }
    }
}

/// Screen/Display fingerprint configuration.
/// Overrides screen dimensions, color depth, and device pixel ratio
/// beyond what ScreenProfile already provides (this adds the JS injection
/// that runs at the prototype level to resist dynamic queries).
#[derive(Debug, Clone)]
pub struct ScreenDisplayConfig {
    pub width: u32,
    pub height: u32,
    pub color_depth: u32,
    pub device_pixel_ratio: f64,
}

impl Default for ScreenDisplayConfig {
    fn default() -> Self {
        ScreenDisplayConfig {
            width: 1920,
            height: 1080,
            color_depth: 24,
            device_pixel_ratio: 1.0,
        }
    }
}

#[derive(Debug, Clone)]
pub struct StealthProfile {
    pub tls: TlsFingerprint,
    pub http2: Http2Fingerprint,
    pub canvas: CanvasNoise,
    pub navigator: NavigatorProfile,
    pub screen: ScreenProfile,
    pub webgl: WebGLProfile,
    pub audio: AudioProfile,
    pub behavior: BehaviorSimulator,
    // New dimensions
    pub font: FontConfig,
    pub battery: BatteryConfig,
    pub webrtc_mode: WebRtcMode,
    pub timing: TimingConfig,
    pub clientrects: ClientRectsConfig,
    pub screen_display: ScreenDisplayConfig,
}

impl StealthProfile {
    pub fn firefox_default() -> Self {
        StealthProfile {
            tls: TlsFingerprint::firefox(),
            http2: Http2Fingerprint::firefox(),
            canvas: CanvasNoise::new(42),
            navigator: NavigatorProfile::firefox(),
            screen: ScreenProfile::default(),
            webgl: WebGLProfile::firefox(),
            audio: AudioProfile::new(42),
            behavior: BehaviorSimulator::with_config(42, BehaviorConfig::firefox()),
            font: FontConfig::new(42),
            battery: BatteryConfig::default(),
            webrtc_mode: WebRtcMode::Default,
            timing: TimingConfig::default(),
            clientrects: ClientRectsConfig { noise_delta: 0.5, seed: 42 },
            screen_display: ScreenDisplayConfig::default(),
        }
    }

    pub fn chrome_default() -> Self {
        StealthProfile {
            tls: TlsFingerprint::chrome(),
            http2: Http2Fingerprint::chrome(),
            canvas: CanvasNoise::new(137),
            navigator: NavigatorProfile::chrome(),
            screen: ScreenProfile::default(),
            webgl: WebGLProfile::chrome(),
            audio: AudioProfile::new(137),
            behavior: BehaviorSimulator::with_config(137, BehaviorConfig::chrome()),
            font: FontConfig::new(137),
            battery: BatteryConfig::default(),
            webrtc_mode: WebRtcMode::Default,
            timing: TimingConfig::default(),
            clientrects: ClientRectsConfig { noise_delta: 0.5, seed: 137 },
            screen_display: ScreenDisplayConfig::default(),
        }
    }
}

// @trace REQ-STL-007 [req:REQ-STL-007] [level:unit]
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn firefox_default_creates_without_panic() {
        let _ = StealthProfile::firefox_default();
    }

    #[test]
    fn chrome_default_creates_without_panic() {
        let _ = StealthProfile::chrome_default();
    }

    #[test]
    fn firefox_user_agent_contains_firefox() {
        let profile = StealthProfile::firefox_default();
        assert!(
            profile.navigator.user_agent.contains("Firefox"),
            "Firefox profile user_agent should contain 'Firefox', got: {}",
            profile.navigator.user_agent
        );
    }

    #[test]
    fn chrome_user_agent_contains_chrome() {
        let profile = StealthProfile::chrome_default();
        assert!(
            profile.navigator.user_agent.contains("Chrome"),
            "Chrome profile user_agent should contain 'Chrome', got: {}",
            profile.navigator.user_agent
        );
    }

    #[test]
    fn firefox_and_chrome_have_different_user_agents() {
        let ff = StealthProfile::firefox_default();
        let ch = StealthProfile::chrome_default();
        assert_ne!(
            ff.navigator.user_agent, ch.navigator.user_agent,
            "Firefox and Chrome profiles should have different user agents"
        );
    }

    #[test]
    fn firefox_and_chrome_have_different_navigator_vendor() {
        let ff = StealthProfile::firefox_default();
        let ch = StealthProfile::chrome_default();
        assert_ne!(
            ff.navigator.vendor, ch.navigator.vendor,
            "Firefox and Chrome profiles should have different navigator.vendor"
        );
    }

    #[test]
    fn clone_works() {
        let original = StealthProfile::firefox_default();
        let cloned = original.clone();
        assert_eq!(
            original.navigator.user_agent, cloned.navigator.user_agent,
            "Cloned profile should have the same user agent"
        );
    }

    #[test]
    fn debug_format_contains_stealth_profile() {
        let profile = StealthProfile::firefox_default();
        let debug_str = format!("{:?}", profile);
        assert!(
            debug_str.contains("StealthProfile"),
            "Debug output should contain 'StealthProfile', got: {}",
            debug_str
        );
    }

    #[test]
    fn firefox_screen_is_default_width() {
        let profile = StealthProfile::firefox_default();
        assert_eq!(profile.screen.width, 1920);
    }

    #[test]
    fn chrome_screen_is_default_width() {
        let profile = StealthProfile::chrome_default();
        assert_eq!(profile.screen.width, 1920);
    }

    #[test]
    fn firefox_and_chrome_behavior_produce_different_mouse_paths() {
        let ff = StealthProfile::firefox_default();
        let ch = StealthProfile::chrome_default();
        let ff_path = ff.behavior.generate_human_mouse_path((0.0, 0.0), (200.0, 200.0), 20.0);
        let ch_path = ch.behavior.generate_human_mouse_path((0.0, 0.0), (200.0, 200.0), 20.0);
        assert_ne!(ff_path, ch_path, "Firefox and Chrome should produce different mouse paths");
    }

    #[test]
    fn firefox_and_chrome_have_different_font_seeds() {
        let ff = StealthProfile::firefox_default();
        let ch = StealthProfile::chrome_default();
        assert_ne!(
            ff.font.seed, ch.font.seed,
            "Firefox and Chrome should have different font seeds"
        );
    }

    #[test]
    fn default_webrtc_mode_is_default() {
        let ff = StealthProfile::firefox_default();
        assert_eq!(ff.webrtc_mode, WebRtcMode::Default);
    }

    #[test]
    fn default_battery_is_fully_charged() {
        let profile = StealthProfile::firefox_default();
        assert!(profile.battery.charging);
        assert!((profile.battery.level - 1.0).abs() < f64::EPSILON);
        assert!((profile.battery.charging_time - 0.0).abs() < f64::EPSILON);
        assert!(profile.battery.discharging_time.is_infinite());
    }

    #[test]
    fn default_timing_precision_is_100us() {
        let profile = StealthProfile::firefox_default();
        assert_eq!(profile.timing.precision_us, 100);
    }

    #[test]
    fn default_clientrects_noise_delta() {
        let profile = StealthProfile::firefox_default();
        assert!((profile.clientrects.noise_delta - 0.5).abs() < f64::EPSILON);
    }

    #[test]
    fn default_screen_display_is_1920x1080() {
        let profile = StealthProfile::firefox_default();
        assert_eq!(profile.screen_display.width, 1920);
        assert_eq!(profile.screen_display.height, 1080);
        assert_eq!(profile.screen_display.color_depth, 24);
        assert!((profile.screen_display.device_pixel_ratio - 1.0).abs() < f64::EPSILON);
    }
}
