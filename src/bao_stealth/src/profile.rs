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
