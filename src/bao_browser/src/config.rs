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
    pub stealth_profile: Option<StealthProfile>,
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
        }
    }
}

impl BaoConfig {
    pub fn validate(&self) -> Result<(), String> {
        if self.max_pages < 1 {
            return Err(format!("max_pages must be >= 1, got {}", self.max_pages));
        }
        if self.default_viewport_width < 800 {
            return Err(format!("viewport_width must be >= 800, got {}", self.default_viewport_width));
        }
        if self.default_viewport_height < 600 {
            return Err(format!("viewport_height must be >= 600, got {}", self.default_viewport_height));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Default)]
pub struct PageConfig {
    pub url: Option<String>,
    pub viewport_width: Option<u32>,
    pub viewport_height: Option<u32>,
    pub stealth_profile: Option<StealthProfile>,
    pub permission: Option<Permission>,
}

#[derive(Debug, Clone)]
pub struct BrowserConfig {
    pub url: Option<String>,
    pub cdp_port: u16,
    pub viewport_width: u32,
    pub viewport_height: u32,
    pub headless: bool,
    pub stealth_profile: Option<StealthProfile>,
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
        assert!(err.contains("max_pages must be >= 1"), "unexpected error: {err}");
        assert!(err.contains("0"), "error should report the value 0: {err}");
    }

    #[test]
    fn validate_fails_viewport_width_799() {
        let mut cfg = BaoConfig::default();
        cfg.default_viewport_width = 799;
        let err = cfg.validate().unwrap_err();
        assert!(err.contains("viewport_width must be >= 800"), "unexpected error: {err}");
        assert!(err.contains("799"), "error should report the value 799: {err}");
    }

    #[test]
    fn validate_fails_viewport_height_599() {
        let mut cfg = BaoConfig::default();
        cfg.default_viewport_height = 599;
        let err = cfg.validate().unwrap_err();
        assert!(err.contains("viewport_height must be >= 600"), "unexpected error: {err}");
        assert!(err.contains("599"), "error should report the value 599: {err}");
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
    fn from_browser_config_maps_fields_correctly() {
        let bc = BrowserConfig {
            url: Some("https://example.com".into()),
            cdp_port: 1234,
            viewport_width: 1280,
            viewport_height: 720,
            headless: false,
            stealth_profile: None,
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
