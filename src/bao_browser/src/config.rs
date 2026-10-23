// @trace REQ-BRW-001  REQ-CDP-008: CDP and browser configuration structs
use std::time::Duration;

use bao_stealth::StealthProfile;

use crate::permission::Permission;

#[derive(Debug, Clone)]
pub struct BaoConfig {
    pub cdp_port: Option<u16>,
    pub max_pages: usize,
    pub idle_ttl: Duration,
    pub default_viewport_width: u32,
    pub default_viewport_height: u32,
    /// Runtime-level explicit stealth choice (e.g. CLI `bao browser
    /// --stealth` → firefox preset). Sits above the process-wide random
    /// default identity and below page/context-level choices in the
    /// `PagePool::create_page` resolution chain (user ruling 2026-10-21:
    /// 默认 = 随机身份; explicit choice wins). `None` = no runtime-level
    /// choice → default pages share `StealthProfile::default_identity()`.
    pub stealth_profile: Option<StealthProfile>,
    /// Servo `opts::ignore_certificate_errors` (WPT posture): skip peer
    /// certificate verification failures for page/network fetches. Production
    /// keeps `false` (fail closed); tests driving local self-signed TLS
    /// fixtures (e.g. the U2 h2 e2e matrix) enable it. Process-global
    /// (first `BrowserRuntime::new` wins — same first-writer-wins semantics as
    /// every other servo opt).
    pub ignore_certificate_errors: bool,
    /// Servo `opts::certificate_path` passthrough (`--certificate-path`) —
    /// the WPT tooling CA for strict TLS verification. `None` keeps servo's
    /// default trust roots.
    pub certificate_path: Option<String>,
    /// Raw pref overrides (`--pref=K=V` vocabulary, user ruling 2026-10-08
    /// ruling A): applied on top of the curated `ServoBuilder::preferences`
    /// flips inside `BrowserRuntime::new`. Without this the CLI pref surface
    /// is dead — `Servo::new` ends with `prefs::set(builder_preferences)`,
    /// which overwrites any global pref state the CLI had applied earlier.
    /// Empty = curated flips only (no behavioral delta for library callers).
    pub pref_overrides: Vec<(String, String)>,
}

impl Default for BaoConfig {
    fn default() -> Self {
        BaoConfig {
            cdp_port: None,
            max_pages: 50,
            idle_ttl: Duration::from_secs(60),
            default_viewport_width: 1920,
            default_viewport_height: 1080,
            stealth_profile: None,
            ignore_certificate_errors: false,
            certificate_path: None,
            pref_overrides: Vec::new(),
        }
    }
}

impl BaoConfig {
    pub fn validate(&self) -> Result<(), String> {
        if self.max_pages < 1 {
            return Err(format!("max_pages must be >= 1, got {}", self.max_pages));
        }
        if self.default_viewport_width < 800 {
            return Err(format!(
                "viewport_width must be >= 800, got {}",
                self.default_viewport_width
            ));
        }
        if self.default_viewport_height < 600 {
            return Err(format!(
                "viewport_height must be >= 600, got {}",
                self.default_viewport_height
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Default)]
pub struct PageConfig {
    pub url: Option<String>,
    pub viewport_width: Option<u32>,
    pub viewport_height: Option<u32>,
    /// Explicit stealth identity for this page. Priority chain in
    /// `PagePool::create_page` (user ruling 2026-10-21: 默认 = 随机身份):
    /// this `Some(profile)` > [`Self::stealth_disabled`] > the page's browser
    /// context identity (`browser_context_id`) > the runtime-level
    /// `BaoConfig::stealth_profile` > the process-wide default identity
    /// (`StealthProfile::default_identity()`, one random draw per process).
    pub stealth_profile: Option<StealthProfile>,
    /// Explicit stealth-free opt-out (显式退出面, user ruling 2026-10-21).
    /// A page created with `stealth_disabled: true` and no explicit profile
    /// carries NO stealth chain at all (keyed registry entries are written as
    /// explicit `None` — R53-A semantics unchanged). Without this flag the
    /// no-profile default is the process-wide random identity, NOT
    /// stealth-free.
    pub stealth_disabled: bool,
    pub permission: Option<Permission>,
    /// REQ-CDS-009: the browser context this page belongs to. `None` = the
    /// implicit default context. `create_page` validates the id against the
    /// pool's context registry BEFORE building the WebView — unknown ids
    /// fail closed (Chrome's "Failed to find browser context" error), never
    /// a page silently parked in the wrong context.
    pub browser_context_id: Option<String>,
}

#[derive(Debug, Clone)]
pub struct BrowserConfig {
    pub url: Option<String>,
    pub cdp_port: u16,
    pub viewport_width: u32,
    pub viewport_height: u32,
    pub headless: bool,
    pub stealth_profile: Option<StealthProfile>,
    /// WPT official-toolchain face (REQ-BRW-002): serve the WebDriver
    /// protocol on this port (upstream servoshell `--webdriver[=port]`).
    /// When set, `run_browser` drives the webdriver_server host instead of
    /// the CDP bridge and runs until a WebDriver Shutdown.
    pub webdriver_port: Option<u16>,
    /// Servo `opts::ignore_certificate_errors` passthrough for the WebDriver
    /// entry (`--ignore-certificate-errors`). The CDP entry keeps the
    /// fail-closed default; wptrunner always passes the flag (upstream issue
    /// servo#30080 — rustls rejects the WPT tooling certificate).
    pub ignore_certificate_errors: bool,
    /// Servo `opts::certificate_path` passthrough (`--certificate-path`) —
    /// the WPT tooling CA for strict TLS verification. `None` keeps servo's
    /// default trust roots.
    pub certificate_path: Option<String>,
    /// Raw pref overrides for the WebDriver/WPT entry (`--pref=K=V` plus the
    /// `--enable-experimental-web-platform-features` expansion, done in
    /// `bao_cli`): carried into `BaoConfig::pref_overrides` so they survive
    /// `Servo::new`'s preferences reset (see `BaoConfig::pref_overrides`).
    pub pref_overrides: Vec<(String, String)>,
}

impl Default for BrowserConfig {
    fn default() -> Self {
        BrowserConfig {
            url: None,
            cdp_port: 9222,
            viewport_width: 1920,
            viewport_height: 1080,
            headless: true,
            stealth_profile: None,
            webdriver_port: None,
            ignore_certificate_errors: false,
            certificate_path: None,
            pref_overrides: Vec::new(),
        }
    }
}

impl From<BrowserConfig> for BaoConfig {
    fn from(bc: BrowserConfig) -> Self {
        BaoConfig {
            cdp_port: Some(bc.cdp_port),
            max_pages: 50,
            idle_ttl: Duration::from_secs(60),
            default_viewport_width: bc.viewport_width,
            default_viewport_height: bc.viewport_height,
            stealth_profile: bc.stealth_profile,
            // WebDriver entry (`--ignore-certificate-errors`) is the one
            // caller that relaxes verification; the interactive/CDP entry
            // keeps the fail-closed default.
            ignore_certificate_errors: bc.ignore_certificate_errors,
            certificate_path: bc.certificate_path,
            pref_overrides: bc.pref_overrides,
        }
    }
}

#[cfg(test)]
mod tests {
    // @trace REQ-BRW-001 [req:REQ-BRW-001,REQ-CDP-008] [level:unit]
    use super::*;

    #[test]
    fn bao_config_default_values() {
        let cfg = BaoConfig::default();
        assert_eq!(cfg.cdp_port, None);
        assert_eq!(cfg.max_pages, 50);
        assert_eq!(cfg.idle_ttl, Duration::from_secs(60));
        assert_eq!(cfg.default_viewport_width, 1920);
        assert_eq!(cfg.default_viewport_height, 1080);
        assert!(cfg.stealth_profile.is_none());
    }

    #[test]
    fn validate_ok_with_defaults() {
        assert!(BaoConfig::default().validate().is_ok());
    }

    #[test]
    fn validate_fails_max_pages_zero() {
        let mut cfg = BaoConfig::default();
        cfg.max_pages = 0;
        let err = cfg.validate().unwrap_err();
        assert!(
            err.contains("max_pages must be >= 1"),
            "unexpected error: {err}"
        );
        assert!(err.contains("0"), "error should report the value 0: {err}");
    }

    #[test]
    fn validate_fails_viewport_width_799() {
        let mut cfg = BaoConfig::default();
        cfg.default_viewport_width = 799;
        let err = cfg.validate().unwrap_err();
        assert!(
            err.contains("viewport_width must be >= 800"),
            "unexpected error: {err}"
        );
        assert!(
            err.contains("799"),
            "error should report the value 799: {err}"
        );
    }

    #[test]
    fn validate_fails_viewport_height_599() {
        let mut cfg = BaoConfig::default();
        cfg.default_viewport_height = 599;
        let err = cfg.validate().unwrap_err();
        assert!(
            err.contains("viewport_height must be >= 600"),
            "unexpected error: {err}"
        );
        assert!(
            err.contains("599"),
            "error should report the value 599: {err}"
        );
    }

    #[test]
    fn browser_config_default_values() {
        let cfg = BrowserConfig::default();
        assert_eq!(cfg.url, None);
        assert_eq!(cfg.cdp_port, 9222);
        assert_eq!(cfg.viewport_width, 1920);
        assert_eq!(cfg.viewport_height, 1080);
        assert!(cfg.headless);
        assert!(cfg.stealth_profile.is_none());
        assert_eq!(cfg.webdriver_port, None);
        // Fail-closed default: only the WebDriver entry relaxes verification,
        // and only when `--ignore-certificate-errors` is passed.
        assert!(!cfg.ignore_certificate_errors);
    }

    #[test]
    fn from_browser_config_preserves_cdp_port() {
        let bc = BrowserConfig {
            cdp_port: 9333,
            ..Default::default()
        };
        let bao: BaoConfig = bc.into();
        assert_eq!(bao.cdp_port, Some(9333));
    }

    #[test]
    fn from_browser_config_preserves_stealth_profile_none() {
        let bc = BrowserConfig::default();
        let bao: BaoConfig = bc.into();
        assert!(bao.stealth_profile.is_none());
    }

    #[test]
    fn from_browser_config_preserves_webdriver_faces() {
        let bc = BrowserConfig {
            webdriver_port: Some(4444),
            ignore_certificate_errors: true,
            ..Default::default()
        };
        let bao: BaoConfig = bc.into();
        // The WPT posture relaxations land in the engine config — this is
        // what wptrunner's servo product launches with.
        assert!(bao.ignore_certificate_errors);
    }

    #[test]
    fn from_browser_config_maps_fields_correctly() {
        let bc = BrowserConfig {
            url: Some("https://example.com".into()),
            cdp_port: 1234,
            viewport_width: 1280,
            viewport_height: 720,
            headless: false,
            stealth_profile: None,
            ..Default::default()
        };
        let bao: BaoConfig = bc.into();
        assert_eq!(bao.cdp_port, Some(1234));
        assert_eq!(bao.max_pages, 50);
        assert_eq!(bao.idle_ttl, Duration::from_secs(60));
        assert_eq!(bao.default_viewport_width, 1280);
        assert_eq!(bao.default_viewport_height, 720);
        assert!(bao.stealth_profile.is_none());
    }
}
