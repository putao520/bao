// REQ-CDP-008: CDP and browser configuration structs
use std::time::Duration;

use crate::permission::Permission;

#[derive(Debug, Clone)]
pub struct BaoConfig {
    pub cdp_port: Option<u16>,
    pub max_pages: usize,
    pub idle_ttl: Duration,
    pub default_viewport_width: u32,
    pub default_viewport_height: u32,
}

impl Default for BaoConfig {
    fn default() -> Self {
        BaoConfig {
            cdp_port: None,
            max_pages: 50,
            idle_ttl: Duration::from_secs(60),
            default_viewport_width: 1920,
            default_viewport_height: 1080,
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

#[derive(Debug, Clone)]
pub struct PageConfig {
    pub url: Option<String>,
    pub viewport_width: Option<u32>,
    pub viewport_height: Option<u32>,
    pub stealth: bool,
    pub permission: Option<Permission>,
}

impl Default for PageConfig {
    fn default() -> Self {
        PageConfig {
            url: None,
            viewport_width: None,
            viewport_height: None,
            stealth: false,
            permission: None,
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct BrowserConfig {
    pub url: Option<String>,
    pub cdp_port: u16,
    pub viewport_width: u32,
    pub viewport_height: u32,
    pub headless: bool,
    pub stealth: bool,
}

impl From<BrowserConfig> for BaoConfig {
    fn from(bc: BrowserConfig) -> Self {
        BaoConfig {
            cdp_port: Some(bc.cdp_port),
            max_pages: 50,
            idle_ttl: Duration::from_secs(60),
            default_viewport_width: bc.viewport_width,
            default_viewport_height: bc.viewport_height,
        }
    }
}
