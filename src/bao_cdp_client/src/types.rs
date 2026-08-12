//! 公共类型 — 用户面向的语义化类型集合。
//!
//! 这些类型是 bao_cdp_client 公共 API 表面的一部分,设计原则:
//! - **稳定**:语义与 Playwright / Puppeteer 对齐,降低用户学习成本
//! - **可序列化**:Cookie / DeviceDescriptor / Viewport 都派生 `Serialize/Deserialize`
//!   以便通过 JSON 持久化或跨进程传输
//! - **零依赖**:不引入 CDP protocol 类型直接出现在公共 API 中,内部用 cdp-protocol crate
//!
//! # 与内部类型的关系
//!
//! - [`ScreenshotFormat`] 与 `bridge::BridgeScreenshotFormat` 等价,本类型作为公共别名。
//!   bridge 类型保留 `Bridge` 前缀以区分内/外表面。
//! - [`Viewport`] 与 `api::page::Viewport` 等价,本模块 re-export 统一类型避免歧义。
//!
//! @trace REQ-BAO-API-008 [level:library]

use serde::{Deserialize, Serialize};
use std::fmt;

// 直接复用 api::page::Viewport(单一真源),避免重复定义造成两份"真相"。
pub use crate::api::page::Viewport;

/// 截图格式(对应 CDP `Page.captureScreenshot.format`)。
///
/// # 示例
///
/// ```
/// use bao_cdp_client::types::ScreenshotFormat;
///
/// let fmt = ScreenshotFormat::Png;
/// assert_eq!(fmt.as_cdp_str(), "png");
/// ```
///
/// @trace REQ-BAO-API-008 [level:library]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ScreenshotFormat {
    /// JPEG 格式(有损压缩,文件小)。
    Jpeg,
    /// PNG 格式(无损,适合 UI 截图)。
    Png,
    /// WebP 格式(现代浏览器支持,压缩率高)。
    Webp,
}

impl ScreenshotFormat {
    /// 返回对应的 CDP format 字符串(`"png"` / `"jpeg"` / `"webp"`)。
    ///
    /// # 示例
    ///
    /// ```
    /// use bao_cdp_client::types::ScreenshotFormat;
    /// assert_eq!(ScreenshotFormat::Jpeg.as_cdp_str(), "jpeg");
    /// assert_eq!(ScreenshotFormat::Png.as_cdp_str(), "png");
    /// assert_eq!(ScreenshotFormat::Webp.as_cdp_str(), "webp");
    /// ```
    ///
    /// @trace REQ-BAO-API-008 [level:library]
    pub fn as_cdp_str(self) -> &'static str {
        match self {
            Self::Jpeg => "jpeg",
            Self::Png => "png",
            Self::Webp => "webp",
        }
    }

    /// 从 CDP format 字符串解析,未知值回退到 [`ScreenshotFormat::Png`]。
    ///
    /// # 示例
    ///
    /// ```
    /// use bao_cdp_client::types::ScreenshotFormat;
    /// assert_eq!(ScreenshotFormat::from_cdp(Some("jpeg")), ScreenshotFormat::Jpeg);
    /// assert_eq!(ScreenshotFormat::from_cdp(None), ScreenshotFormat::Png);
    /// ```
    ///
    /// @trace REQ-BAO-API-008 [level:library]
    pub fn from_cdp(s: Option<&str>) -> Self {
        match s {
            Some("jpeg") => Self::Jpeg,
            Some("webp") => Self::Webp,
            _ => Self::Png,
        }
    }
}

impl Default for ScreenshotFormat {
    fn default() -> Self {
        Self::Png
    }
}

impl fmt::Display for ScreenshotFormat {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_cdp_str())
    }
}

/// 等待页面就绪的状态(对应 Puppeteer `waitUntil` 选项)。
///
/// # 语义
///
/// - `Load`:等待 `window.load` 事件(全部资源加载完成)
/// - `DomContentLoaded`:等待 `document.DOMContentLoaded` 事件
/// - `NetworkIdle0`:网络静止 500ms(Puppeteer 默认)
/// - `NetworkIdle2`:至少 2 个网络请求持续 500ms
///
/// # 示例
///
/// ```
/// use bao_cdp_client::types::WaitUntilState;
///
/// let state = WaitUntilState::NetworkIdle0;
/// assert_eq!(state.as_str(), "networkidle0");
/// ```
///
/// @trace REQ-BAO-API-008 [level:library]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum WaitUntilState {
    /// `load` 事件。
    Load,
    /// `DOMContentLoaded` 事件。
    DomContentLoaded,
    /// 500ms 内无网络请求(严格)。
    NetworkIdle0,
    /// 500ms 内网络连接数 ≤ 2(宽松)。
    NetworkIdle2,
}

impl WaitUntilState {
    /// 返回 Puppeteer 风格的字符串标识。
    ///
    /// # 示例
    ///
    /// ```
    /// use bao_cdp_client::types::WaitUntilState;
    /// assert_eq!(WaitUntilState::Load.as_str(), "load");
    /// assert_eq!(WaitUntilState::NetworkIdle2.as_str(), "networkidle2");
    /// ```
    ///
    /// @trace REQ-BAO-API-008 [level:library]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Load => "load",
            Self::DomContentLoaded => "domcontentloaded",
            Self::NetworkIdle0 => "networkidle0",
            Self::NetworkIdle2 => "networkidle2",
        }
    }
}

impl Default for WaitUntilState {
    fn default() -> Self {
        Self::Load
    }
}

impl fmt::Display for WaitUntilState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// HTTP Cookie 表示(对应 CDP `Network.Cookie` / `Network.setCookie`)。
///
/// 所有字段都是 `Option` 以兼容不同来源(CDP / JS document.cookie / 持久化存储)。
/// `name` 和 `value` 是必填,其他可选。
///
/// # 示例
///
/// ```
/// use bao_cdp_client::types::Cookie;
///
/// let cookie = Cookie {
///     name: "session".to_string(),
///     value: "abc123".to_string(),
///     domain: Some("example.com".to_string()),
///     path: Some("/".to_string()),
///     secure: Some(true),
///     http_only: Some(true),
///     ..Default::default()
/// };
/// assert_eq!(cookie.name, "session");
/// ```
///
/// @trace REQ-BAO-API-008 [level:library]
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct Cookie {
    /// Cookie 名称(必填)。
    pub name: String,
    /// Cookie 值(必填)。
    pub value: String,
    /// 关联的 URL(可选 — 通常与 domain 二选一)。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    /// Cookie 域名(如 `example.com`)。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub domain: Option<String>,
    /// Cookie 路径(默认 `/`)。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    /// 过期 Unix 时间戳(秒,浮点支持小数部分)。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expires: Option<f64>,
    /// 是否仅 HTTP 不可见(JS `document.cookie` 读不到)。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub http_only: Option<bool>,
    /// 是否仅 HTTPS / Secure 连接发送。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub secure: Option<bool>,
    /// SameSite 策略(`"Strict"` / `"Lax"` / `"None"`)。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub same_site: Option<String>,
}

impl Cookie {
    /// 构造一个仅含 name/value 的 Cookie(其余字段为 None)。
    ///
    /// # 示例
    ///
    /// ```
    /// use bao_cdp_client::types::Cookie;
    /// let c = Cookie::new("k", "v");
    /// assert_eq!(c.name, "k");
    /// assert_eq!(c.value, "v");
    /// assert!(c.domain.is_none());
    /// ```
    ///
    /// @trace REQ-BAO-API-008 [level:library]
    pub fn new(name: impl Into<String>, value: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            value: value.into(),
            ..Default::default()
        }
    }

    /// 链式设置 domain。
    ///
    /// # 示例
    ///
    /// ```
    /// use bao_cdp_client::types::Cookie;
    /// let c = Cookie::new("k", "v").with_domain("example.com");
    /// assert_eq!(c.domain.as_deref(), Some("example.com"));
    /// ```
    ///
    /// @trace REQ-BAO-API-008 [level:library]
    pub fn with_domain(mut self, domain: impl Into<String>) -> Self {
        self.domain = Some(domain.into());
        self
    }

    /// 链式设置 path。
    pub fn with_path(mut self, path: impl Into<String>) -> Self {
        self.path = Some(path.into());
        self
    }

    /// 链式设置 secure。
    pub fn with_secure(mut self, secure: bool) -> Self {
        self.secure = Some(secure);
        self
    }

    /// 链式设置 http_only。
    pub fn with_http_only(mut self, http_only: bool) -> Self {
        self.http_only = Some(http_only);
        self
    }

    /// 链式设置 same_site。
    pub fn with_same_site(mut self, same_site: impl Into<String>) -> Self {
        self.same_site = Some(same_site.into());
        self
    }
}

/// 设备描述符(用于 `Page.emulate` 设备模拟)。
///
/// 封装 user_agent + viewport,可参考 Puppeteer `devices.ts`。
///
/// # 示例
///
/// ```
/// use bao_cdp_client::types::{DeviceDescriptor, Viewport};
///
/// let iphone = DeviceDescriptor {
///     name: "iPhone 13".to_string(),
///     user_agent: "Mozilla/5.0 (iPhone; CPU iPhone OS 15_0 like Mac OS X)".to_string(),
///     viewport: Viewport {
///         width: 390,
///         height: 844,
///         device_scale_factor: 3.0,
///         is_mobile: true,
///         has_touch: true,
///         is_landscape: false,
///     },
/// };
/// assert_eq!(iphone.viewport.width, 390);
/// ```
///
/// @trace REQ-BAO-API-008 [level:library]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeviceDescriptor {
    /// 设备显示名称(如 `"iPhone 13"`、`"Pixel 5"`)。
    pub name: String,
    /// User-Agent 字符串。
    pub user_agent: String,
    /// Viewport 配置。
    pub viewport: Viewport,
}

impl DeviceDescriptor {
    /// 构造设备描述符。
    ///
    /// # 示例
    ///
    /// ```
    /// use bao_cdp_client::types::{DeviceDescriptor, Viewport};
    ///
    /// let device = DeviceDescriptor::new(
    ///     "Test",
    ///     "Mozilla/5.0 Test",
    ///     Viewport { width: 100, height: 100, device_scale_factor: 1.0, is_mobile: false, has_touch: false, is_landscape: false },
    /// );
    /// assert_eq!(device.name, "Test");
    /// ```
    ///
    /// @trace REQ-BAO-API-008 [level:library]
    pub fn new(name: impl Into<String>, user_agent: impl Into<String>, viewport: Viewport) -> Self {
        Self {
            name: name.into(),
            user_agent: user_agent.into(),
            viewport,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn screenshot_format_default_is_png() {
        assert_eq!(ScreenshotFormat::default(), ScreenshotFormat::Png);
    }

    #[test]
    fn screenshot_format_round_trip() {
        for fmt in [
            ScreenshotFormat::Png,
            ScreenshotFormat::Jpeg,
            ScreenshotFormat::Webp,
        ] {
            let s = fmt.as_cdp_str();
            assert_eq!(ScreenshotFormat::from_cdp(Some(s)), fmt);
        }
    }

    #[test]
    fn screenshot_format_unknown_falls_back_to_png() {
        assert_eq!(
            ScreenshotFormat::from_cdp(Some("gif")),
            ScreenshotFormat::Png
        );
        assert_eq!(ScreenshotFormat::from_cdp(None), ScreenshotFormat::Png);
    }

    #[test]
    fn screenshot_format_display_matches_cdp_str() {
        assert_eq!(format!("{}", ScreenshotFormat::Webp), "webp");
    }

    #[test]
    fn wait_until_default_is_load() {
        assert_eq!(WaitUntilState::default(), WaitUntilState::Load);
    }

    #[test]
    fn wait_until_as_str_all_variants() {
        assert_eq!(WaitUntilState::Load.as_str(), "load");
        assert_eq!(
            WaitUntilState::DomContentLoaded.as_str(),
            "domcontentloaded"
        );
        assert_eq!(WaitUntilState::NetworkIdle0.as_str(), "networkidle0");
        assert_eq!(WaitUntilState::NetworkIdle2.as_str(), "networkidle2");
    }

    #[test]
    fn cookie_default_all_optional_none() {
        let c = Cookie {
            name: "k".into(),
            value: "v".into(),
            ..Default::default()
        };
        assert!(c.url.is_none());
        assert!(c.domain.is_none());
        assert!(c.path.is_none());
        assert!(c.expires.is_none());
        assert!(c.http_only.is_none());
        assert!(c.secure.is_none());
        assert!(c.same_site.is_none());
    }

    #[test]
    fn cookie_builder_chain() {
        let c = Cookie::new("k", "v")
            .with_domain("example.com")
            .with_path("/")
            .with_secure(true)
            .with_http_only(true)
            .with_same_site("Lax");
        assert_eq!(c.domain.as_deref(), Some("example.com"));
        assert_eq!(c.path.as_deref(), Some("/"));
        assert_eq!(c.secure, Some(true));
        assert_eq!(c.http_only, Some(true));
        assert_eq!(c.same_site.as_deref(), Some("Lax"));
    }

    #[test]
    fn cookie_serializes_with_only_required_fields() {
        let c = Cookie::new("k", "v");
        let json = serde_json::to_string(&c).unwrap();
        // 必须包含 name/value,可选字段为 None 时被 skip。
        assert!(json.contains("\"name\":\"k\""));
        assert!(json.contains("\"value\":\"v\""));
        assert!(!json.contains("domain"));
    }

    #[test]
    fn cookie_deserializes_back() {
        let c = Cookie::new("k", "v").with_domain("example.com");
        let json = serde_json::to_string(&c).unwrap();
        let parsed: Cookie = serde_json::from_str(&json).unwrap();
        assert_eq!(c, parsed);
    }

    #[test]
    fn device_descriptor_construction() {
        let vp = Viewport {
            width: 390,
            height: 844,
            device_scale_factor: 3.0,
            is_mobile: true,
            has_touch: true,
            is_landscape: false,
        };
        let dev = DeviceDescriptor::new("iPhone 13", "UA", vp);
        assert_eq!(dev.name, "iPhone 13");
        assert_eq!(dev.user_agent, "UA");
        assert_eq!(dev.viewport.width, 390);
        assert!(dev.viewport.is_mobile);
    }
}
