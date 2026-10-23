#![allow(dead_code, unused_imports)]
// REQ-STL-007: Stealth engine integration and CDP stealth  @trace REQ-STL-001 [entity:TlsProfile] [entity:StealthProfile]
//
// ── from_seed 随机指纹模型(用户裁决 2026-10-21)────────────────────────────
//
// 反指纹 = 随机指纹,非固定预设:一个 u64 种子(`StealthProfile::from_seed`)
// 确定性派生一整套档内连贯的指纹 — 同种子跨重载/会话逐字段复现(登录态不破),
// 异种子互不可关联(域分离子种子构造性单射)。强联动维度(UA ⇔ platform ⇔
// userAgentData ⇔ WebGL renderer ⇔ screen)钉进平台档位桶(`derive_tables`:
// Windows/macOS/Linux,档内 GPU/屏幕/UA-CH 按子种子确定性选型),TLS/H2 只从
// 树内真实抓取形态池选型 — 逐字段随机会造出地球不存在的组合,反成指纹。
// 默认身份 = 进程级单例 `StealthProfile::default_identity()`(每次启动随机一次,
// 无显式配置的 page 全共享 —— 同启动内指纹稳定、禁 per-page 各自随机);显式
// browser context 以 `from_random_seed()` 派生该 context 的共享身份(解析链见
// page_pool.rs 注释锚;显式 stealth-free 退出面 = `PageConfig::stealth_disabled`)。
// firefox_default()/chrome_default() 保留为显式选择面。
mod behavior;
mod canvas;
mod derive;
mod derive_tables;
pub mod engine_props;
pub mod hooks;
mod http2;
mod navigator;
mod profile;
pub mod stealth_wire;
mod tls;
mod webgl_audio;

pub use behavior::{BehaviorConfig, BehaviorSimulator, ClickEvent, ClickEventType, TypingEvent};
pub use canvas::CanvasNoise;
pub use hooks::StealthHooks;
pub use http2::Http2Fingerprint;
pub use http2::{PriorityFrame, PriorityFrameMode};
pub use http2::{global_http2_fingerprint, set_global_http2_fingerprint};
pub use navigator::{NavigatorProfile, ScreenProfile};
pub use profile::StealthProfile;
pub use profile::{
    BatteryConfig, ClientRectsConfig, FontConfig, ScreenDisplayConfig, TimingConfig, WebRtcMode,
};
pub use stealth_wire::StealthTlsWireConfig;
pub use tls::TlsFingerprint;
pub use tls::TlsFingerprintConfig;
// IANA → OpenSSL/BoringSSL name mapping — single source of truth shared by
// every TLS stack in Bao (servo `net::connector`, `bao_runtime`/`bun_http`).
pub use tls::boringssl_cipher_list_string;
pub use tls::boringssl_curves_list_string;
pub use tls::boringssl_sigalgs_list_string;
pub use tls::cipher_suite_boringssl_supported;
pub use tls::cipher_suite_openssl_name;
pub use tls::group_boringssl_supported;
pub use tls::group_openssl_name;
pub use tls::sigalg_openssl_name;
pub use webgl_audio::{AudioProfile, WebGLProfile};

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
        assert_eq!(
            engine.profile().navigator.user_agent,
            profile.navigator.user_agent
        );
    }

    #[test]
    fn default_engine_is_firefox() {
        let engine = StealthEngine::default_engine();
        let firefox = StealthProfile::firefox_default();
        assert_eq!(
            engine.profile().navigator.user_agent,
            firefox.navigator.user_agent
        );
    }

    #[test]
    fn tls_config_matches_profile() {
        let engine = StealthEngine::default_engine();
        assert_eq!(engine.tls_config().ja3_hash, engine.profile().tls.ja3_hash);
    }

    #[test]
    fn http2_config_matches_profile() {
        let engine = StealthEngine::default_engine();
        assert_eq!(
            engine.http2_config().header_table_size,
            engine.profile().http2.header_table_size
        );
    }

    #[test]
    fn canvas_noise_matches_profile() {
        let engine = StealthEngine::default_engine();
        assert_eq!(engine.canvas_noise().seed(), engine.profile().canvas.seed());
    }

    #[test]
    fn navigator_matches_profile() {
        let engine = StealthEngine::default_engine();
        assert_eq!(
            engine.navigator().user_agent,
            engine.profile().navigator.user_agent
        );
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
        assert!(
            (engine.audio().noise_amplitude() - engine.profile().audio.noise_amplitude()).abs()
                < f64::EPSILON
        );
    }

    #[test]
    fn behavior_matches_profile() {
        let engine = StealthEngine::default_engine();
        assert_eq!(engine.behavior().seed(), engine.profile().behavior.seed());
    }
}

// Higher-tier soft-link providers are dev-deps that nothing in this crate's
// code path `use`s, so cargo never passes them to the linker — GNU ld
// tolerates the undefined faces in test executables, lld-link/COFF does not.
// This test forces them onto the lib-test link line (bao_native_stubs
// force_link precedent in bao_engine's dispatch_sm_tests).
#[cfg(test)]
mod test_link_seams {
    #[test]
    fn link_higher_tier_seams() {
        bao_bundler::force_link_test_seams();
        bun_runtime::webcore_faces::force_link_webcore_faces();
    }
}
