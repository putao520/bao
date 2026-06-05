#![allow(dead_code, unused_imports)]
// REQ-STL-007: Stealth engine integration and CDP stealth  @trace REQ-STL-001
mod profile;
mod tls;
mod http2;
mod canvas;
mod navigator;
mod webgl_audio;
mod behavior;
pub mod engine_props;

pub use profile::StealthProfile;
pub use tls::TlsFingerprint;
pub use tls::TlsFingerprintConfig;
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

}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_stores_profile() {
        let profile = StealthProfile::firefox_default();
        let engine = StealthEngine::new(profile.clone());
        assert_eq!(engine.profile().navigator.user_agent, profile.navigator.user_agent);
    }

    #[test]
    fn default_engine_is_firefox() {
        let engine = StealthEngine::default_engine();
        let firefox = StealthProfile::firefox_default();
        assert_eq!(engine.profile().navigator.user_agent, firefox.navigator.user_agent);
    }

    #[test]
    fn tls_config_matches_profile() {
        let engine = StealthEngine::default_engine();
        assert_eq!(engine.tls_config().ja3_hash, engine.profile().tls.ja3_hash);
    }

    #[test]
    fn http2_config_matches_profile() {
        let engine = StealthEngine::default_engine();
        assert_eq!(engine.http2_config().header_table_size, engine.profile().http2.header_table_size);
    }

    #[test]
    fn canvas_noise_matches_profile() {
        let engine = StealthEngine::default_engine();
        assert_eq!(engine.canvas_noise().seed(), engine.profile().canvas.seed());
    }

    #[test]
    fn navigator_matches_profile() {
        let engine = StealthEngine::default_engine();
        assert_eq!(engine.navigator().user_agent, engine.profile().navigator.user_agent);
    }

    #[test]
    fn screen_matches_profile() {
        let engine = StealthEngine::default_engine();
        assert_eq!(engine.screen().width, engine.profile().screen.width);
        assert_eq!(engine.screen().height, engine.profile().screen.height);
    }

    #[test]
    fn webgl_matches_profile() {
        let engine = StealthEngine::default_engine();
        assert_eq!(engine.webgl().vendor, engine.profile().webgl.vendor);
        assert_eq!(engine.webgl().renderer, engine.profile().webgl.renderer);
    }

    #[test]
    fn audio_matches_profile() {
        let engine = StealthEngine::default_engine();
        assert!((engine.audio().noise_amplitude() - engine.profile().audio.noise_amplitude()).abs() < f64::EPSILON);
    }

    #[test]
    fn behavior_matches_profile() {
        let engine = StealthEngine::default_engine();
        assert_eq!(engine.behavior().seed(), engine.profile().behavior.seed());
    }

}
