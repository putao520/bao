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

/// Plugin/MimeType spoofing configuration.
/// Overrides navigator.plugins and navigator.mimeTypes with realistic values.
#[derive(Debug, Clone)]
pub struct PluginConfig {
    /// Number of plugins to spoof (3-5 common PDF viewer plugins).
    pub plugin_count: u32,
    /// Plugin names in order.
    pub plugins: Vec<String>,
    /// MIME type descriptions corresponding to plugins.
    pub mime_types: Vec<String>,
}

impl Default for PluginConfig {
    fn default() -> Self {
        PluginConfig {
            plugin_count: 5,
            plugins: vec![
                "PDF Viewer".into(),
                "Chrome PDF Viewer".into(),
                "Chromium PDF Viewer".into(),
                "Microsoft Edge PDF Viewer".into(),
                "WebKit built-in PDF".into(),
            ],
            mime_types: vec![
                "application/pdf".into(),
                "text/pdf".into(),
            ],
        }
    }
}

/// SpeechSynthesis voices spoofing configuration.
#[derive(Debug, Clone)]
pub struct SpeechConfig {
    /// Whether to spoof speechSynthesis.getVoices().
    pub enabled: bool,
    /// Voice names to return from getVoices().
    pub voices: Vec<String>,
}

impl Default for SpeechConfig {
    fn default() -> Self {
        SpeechConfig {
            enabled: true,
            voices: vec![
                "Google US English".into(),
                "Google UK English Female".into(),
                "Google UK English Male".into(),
                "Google Deutsch".into(),
                "Google Français".into(),
                "Google Español".into(),
                "Google Italiano".into(),
                "Google Japanese".into(),
                "Google Nederlands".into(),
                "Google Polski".into(),
                "Google Português do Brasil".into(),
                "Google Pútonghuà".into(),
            ],
        }
    }
}

/// MediaDevices enumeration spoofing configuration.
#[derive(Debug, Clone)]
pub struct MediaDevicesConfig {
    /// Whether to spoof navigator.mediaDevices.enumerateDevices().
    pub enabled: bool,
    /// Number of audioinput devices.
    pub audio_input_count: u32,
    /// Number of videoinput devices.
    pub video_input_count: u32,
    /// Number of audiooutput devices.
    pub audio_output_count: u32,
}

impl Default for MediaDevicesConfig {
    fn default() -> Self {
        MediaDevicesConfig {
            enabled: true,
            audio_input_count: 1,
            video_input_count: 1,
            audio_output_count: 1,
        }
    }
}

/// Permissions API spoofing configuration.
#[derive(Debug, Clone)]
pub struct PermissionsConfig {
    /// Whether to spoof navigator.permissions.query().
    pub enabled: bool,
    /// Map of permission name -> state string (e.g. "notifications" -> "prompt").
    pub states: Vec<(String, String)>,
}

impl Default for PermissionsConfig {
    fn default() -> Self {
        PermissionsConfig {
            enabled: true,
            states: vec![
                ("notifications".into(), "prompt".into()),
                ("geolocation".into(), "prompt".into()),
                ("camera".into(), "prompt".into()),
                ("microphone".into(), "prompt".into()),
                ("midi".into(), "prompt".into()),
            ],
        }
    }
}

/// WebGL context attributes spoofing configuration.
#[derive(Debug, Clone)]
pub struct WebGLContextConfig {
    /// Whether to override getContextAttributes().
    pub enabled: bool,
    pub antialias: bool,
    pub depth: bool,
    pub stencil: bool,
    pub alpha: bool,
    pub premultiplied_alpha: bool,
    pub preserve_drawing_buffer: bool,
    pub power_preference: String,
    pub fail_if_major_performance_caveat: bool,
}

impl Default for WebGLContextConfig {
    fn default() -> Self {
        WebGLContextConfig {
            enabled: true,
            antialias: true,
            depth: true,
            stencil: false,
            alpha: true,
            premultiplied_alpha: true,
            preserve_drawing_buffer: false,
            power_preference: "default".into(),
            fail_if_major_performance_caveat: false,
        }
    }
}

/// navigator.connection (NetworkInformation) spoofing configuration.
#[derive(Debug, Clone)]
pub struct ConnectionConfig {
    /// Whether to spoof navigator.connection.
    pub enabled: bool,
    pub effective_type: String,
    pub downlink: f64,
    pub rtt: u32,
    pub save_data: bool,
}

impl Default for ConnectionConfig {
    fn default() -> Self {
        ConnectionConfig {
            enabled: true,
            effective_type: "4g".into(),
            downlink: 10.0,
            rtt: 50,
            save_data: false,
        }
    }
}

/// iframe contentWindow normalization configuration.
#[derive(Debug, Clone)]
pub struct IframeConfig {
    /// Whether to normalize iframe.contentWindow behavior.
    pub enabled: bool,
}

impl Default for IframeConfig {
    fn default() -> Self {
        IframeConfig { enabled: true }
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
    // Missing dimensions
    pub plugin: PluginConfig,
    pub speech: SpeechConfig,
    pub media_devices: MediaDevicesConfig,
    pub permissions: PermissionsConfig,
    pub webgl_context: WebGLContextConfig,
    pub connection: ConnectionConfig,
    pub iframe: IframeConfig,
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
            plugin: PluginConfig::default(),
            speech: SpeechConfig::default(),
            media_devices: MediaDevicesConfig::default(),
            permissions: PermissionsConfig::default(),
            webgl_context: WebGLContextConfig::default(),
            connection: ConnectionConfig::default(),
            iframe: IframeConfig::default(),
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
            plugin: PluginConfig::default(),
            speech: SpeechConfig::default(),
            media_devices: MediaDevicesConfig::default(),
            permissions: PermissionsConfig::default(),
            webgl_context: WebGLContextConfig::default(),
            connection: ConnectionConfig::default(),
            iframe: IframeConfig::default(),
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

    #[test]
    fn default_plugin_config_has_5_plugins() {
        let profile = StealthProfile::firefox_default();
        assert_eq!(profile.plugin.plugin_count, 5);
        assert_eq!(profile.plugin.plugins.len(), 5);
        assert!(profile.plugin.plugins.contains(&"PDF Viewer".to_string()));
    }

    #[test]
    fn default_speech_config_enabled() {
        let profile = StealthProfile::firefox_default();
        assert!(profile.speech.enabled);
        assert!(!profile.speech.voices.is_empty());
    }

    #[test]
    fn default_media_devices_config() {
        let profile = StealthProfile::firefox_default();
        assert!(profile.media_devices.enabled);
        assert_eq!(profile.media_devices.audio_input_count, 1);
        assert_eq!(profile.media_devices.video_input_count, 1);
        assert_eq!(profile.media_devices.audio_output_count, 1);
    }

    #[test]
    fn default_permissions_config() {
        let profile = StealthProfile::firefox_default();
        assert!(profile.permissions.enabled);
        assert_eq!(profile.permissions.states.len(), 5);
    }

    #[test]
    fn default_webgl_context_config() {
        let profile = StealthProfile::firefox_default();
        assert!(profile.webgl_context.enabled);
        assert!(profile.webgl_context.antialias);
        assert!(profile.webgl_context.depth);
        assert!(!profile.webgl_context.stencil);
    }

    #[test]
    fn default_connection_config() {
        let profile = StealthProfile::firefox_default();
        assert!(profile.connection.enabled);
        assert_eq!(profile.connection.effective_type, "4g");
        assert!((profile.connection.downlink - 10.0).abs() < f64::EPSILON);
        assert_eq!(profile.connection.rtt, 50);
        assert!(!profile.connection.save_data);
    }

    #[test]
    fn default_iframe_config_enabled() {
        let profile = StealthProfile::firefox_default();
        assert!(profile.iframe.enabled);
    }
}
