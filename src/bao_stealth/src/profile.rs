// @trace REQ-STL-007
use crate::tls::TlsFingerprint;
use crate::http2::Http2Fingerprint;
use crate::canvas::CanvasNoise;
use crate::navigator::{NavigatorProfile, ScreenProfile};
use crate::webgl_audio::{WebGLProfile, AudioProfile};
use crate::behavior::BehaviorSimulator;

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
            behavior: BehaviorSimulator::new(42),
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
            behavior: BehaviorSimulator::new(137),
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
}
