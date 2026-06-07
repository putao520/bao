// REQ-STL-004: Navigator/Screen property construction  @trace REQ-STL-004
#[derive(Debug, Clone)]
pub struct NavigatorProfile {
    pub user_agent: String,
    pub platform: String,
    pub language: String,
    pub languages: Vec<String>,
    pub hardware_concurrency: u32,
    pub max_touch_points: u32,
    pub vendor: String,
    pub app_version: String,
    pub oscpu: Option<String>,
    pub build_id: Option<String>,
    pub product_sub: String,
    pub device_memory: f64,
}

impl NavigatorProfile {
    pub fn firefox() -> Self {
        NavigatorProfile {
            user_agent: "Mozilla/5.0 (X11; Linux x86_64; rv:128.0) Gecko/20100101 Firefox/128.0".into(),
            platform: "Linux x86_64".into(),
            language: "en-US".into(),
            languages: vec!["en-US".into(), "en".into()],
            hardware_concurrency: 8,
            max_touch_points: 0,
            vendor: "".into(),
            app_version: "5.0 (X11)".into(),
            oscpu: Some("Linux x86_64".into()),
            build_id: Some("20240701150000".into()),
            product_sub: "20100101".into(),
            device_memory: 8.0,
        }
    }

    pub fn chrome() -> Self {
        NavigatorProfile {
            user_agent: "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/128.0.0.0 Safari/537.36".into(),
            platform: "Linux x86_64".into(),
            language: "en-US".into(),
            languages: vec!["en-US".into(), "en".into()],
            hardware_concurrency: 8,
            max_touch_points: 0,
            vendor: "Google Inc.".into(),
            app_version: "5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/128.0.0.0 Safari/537.36".into(),
            oscpu: None,
            build_id: None,
            product_sub: "20030107".into(),
            device_memory: 8.0,
        }
    }
}

#[derive(Debug, Clone)]
pub struct ScreenProfile {
    pub width: u32,
    pub height: u32,
    pub avail_width: u32,
    pub avail_height: u32,
    pub color_depth: u32,
    pub pixel_depth: u32,
    pub device_pixel_ratio: f64,
}

impl Default for ScreenProfile {
    fn default() -> Self {
        ScreenProfile {
            width: 1920,
            height: 1080,
            avail_width: 1920,
            avail_height: 1040,
            color_depth: 24,
            pixel_depth: 24,
            device_pixel_ratio: 1.0,
        }
    }
}

impl ScreenProfile {
    pub fn new(width: u32, height: u32, dpr: f64) -> Self {
        ScreenProfile {
            width,
            height,
            avail_width: width,
            avail_height: height - 40,
            color_depth: 24,
            pixel_depth: 24,
            device_pixel_ratio: dpr,
        }
    }
}

// @trace REQ-STL-004 [req:REQ-STL-004] [level:unit]
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn firefox_user_agent_contains_firefox() {
        let profile = NavigatorProfile::firefox();
        assert!(profile.user_agent.contains("Firefox"));
    }

    #[test]
    fn firefox_platform_is_linux_x86_64() {
        let profile = NavigatorProfile::firefox();
        assert_eq!(profile.platform, "Linux x86_64");
    }

    #[test]
    fn firefox_oscpu_is_some() {
        let profile = NavigatorProfile::firefox();
        assert!(profile.oscpu.is_some());
    }

    #[test]
    fn firefox_vendor_is_empty() {
        let profile = NavigatorProfile::firefox();
        assert_eq!(profile.vendor, "");
    }

    #[test]
    fn chrome_user_agent_contains_chrome() {
        let profile = NavigatorProfile::chrome();
        assert!(profile.user_agent.contains("Chrome"));
    }

    #[test]
    fn chrome_vendor_is_google_inc() {
        let profile = NavigatorProfile::chrome();
        assert_eq!(profile.vendor, "Google Inc.");
    }

    #[test]
    fn chrome_oscpu_is_none() {
        let profile = NavigatorProfile::chrome();
        assert!(profile.oscpu.is_none());
    }

    #[test]
    fn chrome_build_id_is_none() {
        let profile = NavigatorProfile::chrome();
        assert!(profile.build_id.is_none());
    }

    #[test]
    fn chrome_product_sub_is_20030107() {
        let profile = NavigatorProfile::chrome();
        assert_eq!(profile.product_sub, "20030107");
    }

    #[test]
    fn screen_profile_default_has_1920x1080() {
        let screen = ScreenProfile::default();
        assert_eq!(screen.width, 1920);
        assert_eq!(screen.height, 1080);
    }

    #[test]
    fn screen_profile_default_avail_height_is_1040() {
        let screen = ScreenProfile::default();
        assert_eq!(screen.avail_height, 1040);
    }

    #[test]
    fn screen_profile_new_custom_values() {
        let screen = ScreenProfile::new(800, 600, 2.0);
        assert_eq!(screen.width, 800);
        assert_eq!(screen.height, 600);
        assert_eq!(screen.device_pixel_ratio, 2.0);
        assert_eq!(screen.color_depth, 24);
        assert_eq!(screen.pixel_depth, 24);
    }

    #[test]
    fn screen_profile_new_avail_height_minus_40() {
        let screen = ScreenProfile::new(800, 600, 2.0);
        assert_eq!(screen.avail_height, 560);
    }

    #[test]
    fn navigator_profile_clone_preserves_user_agent() {
        let profile = NavigatorProfile::firefox();
        let cloned = profile.clone();
        assert_eq!(cloned.user_agent, profile.user_agent);
    }

    #[test]
    fn screen_profile_clone_works() {
        let screen = ScreenProfile::default();
        let cloned = screen.clone();
        assert_eq!(cloned.width, screen.width);
        assert_eq!(cloned.height, screen.height);
        assert_eq!(cloned.device_pixel_ratio, screen.device_pixel_ratio);
    }

    #[test]
    fn firefox_languages_contains_en_us() {
        let profile = NavigatorProfile::firefox();
        assert!(profile.languages.contains(&"en-US".to_string()));
        assert!(!profile.languages.is_empty());
    }

    #[test]
    fn chrome_languages_contains_en_us() {
        let profile = NavigatorProfile::chrome();
        assert!(profile.languages.contains(&"en-US".to_string()));
    }

    #[test]
    fn firefox_device_memory_is_positive() {
        let profile = NavigatorProfile::firefox();
        assert!(profile.device_memory > 0.0);
    }

    #[test]
    fn chrome_device_memory_is_positive() {
        let profile = NavigatorProfile::chrome();
        assert!(profile.device_memory > 0.0);
    }

    #[test]
    fn firefox_language_matches_languages_first() {
        let profile = NavigatorProfile::firefox();
        assert_eq!(profile.language, profile.languages[0]);
    }

    #[test]
    fn chrome_language_matches_languages_first() {
        let profile = NavigatorProfile::chrome();
        assert_eq!(profile.language, profile.languages[0]);
    }
}
