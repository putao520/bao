// REQ-STL-004: Navigator/Screen property construction  @trace REQ-STL-004
#[derive(Debug, Clone)]
pub struct NavigatorProfile {
    pub user_agent: String,
    pub platform: String,
    pub language: String,
    pub hardware_concurrency: u32,
    pub max_touch_points: u32,
    pub vendor: String,
    pub app_version: String,
    pub oscpu: Option<String>,
    pub build_id: Option<String>,
    pub product_sub: String,
}

impl NavigatorProfile {
    pub fn firefox() -> Self {
        NavigatorProfile {
            user_agent: "Mozilla/5.0 (X11; Linux x86_64; rv:128.0) Gecko/20100101 Firefox/128.0".into(),
            platform: "Linux x86_64".into(),
            language: "en-US".into(),
            hardware_concurrency: 8,
            max_touch_points: 0,
            vendor: "".into(),
            app_version: "5.0 (X11)".into(),
            oscpu: Some("Linux x86_64".into()),
            build_id: Some("20240701150000".into()),
            product_sub: "20100101".into(),
        }
    }

    pub fn chrome() -> Self {
        NavigatorProfile {
            user_agent: "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/128.0.0.0 Safari/537.36".into(),
            platform: "Linux x86_64".into(),
            language: "en-US".into(),
            hardware_concurrency: 8,
            max_touch_points: 0,
            vendor: "Google Inc.".into(),
            app_version: "5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/128.0.0.0 Safari/537.36".into(),
            oscpu: None,
            build_id: None,
            product_sub: "20030107".into(),
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
