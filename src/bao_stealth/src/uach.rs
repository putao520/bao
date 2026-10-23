// REQ-STL-004 + REQ-STL-007: navigator.userAgentData(UA-CH)注入形状派生
// @trace REQ-STL-004 [req:REQ-STL-004] [level:unit]
//
// 真源:WICG UA-CH spec(https://wicg.github.io/ua-client-hints/,2026-10-21 live
// 核对)——`NavigatorUAData` [Exposed=(Window,Worker)](window + worker 全暴露);
// `userAgentData` 属性 [SecureContext](非安全 context 整属性缺席);brands =
// FrozenArray<NavigatorUABrandVersion>;`toJSON()` = UALowEntropyJSON
// {brands, mobile, platform};`getHighEntropyValues()` = Promise<UADataValues>,
// 决议**恒含低熵三元**(brands/mobile/platform),高熵键只含「请求 ∩ 支持」。
//
// 设计裁决(为什么从 NavigatorProfile 既有字段派生,而不是给 NavigatorProfile
// 加 UA-CH 字段):NavigatorProfile 在本任务清单外文件(engine_props.rs 的
// RealmProfile 平面字段往返 ×2 + tests/suite 两处)有结构体字面量构造,加字段 =
// 炸清单外编译面;且 UA-CH 全部形状可从既有字段(user_agent + platform)
// **确定性**派生 —— brands 版本直接取注入 UA 里的 Chrome major(与 UA 天然同步,
// 连「桶表 CHROME_MAJOR 与用户自定义 UA 不同步」的漂移面都消掉),uaData.platform
// 从 navigator.platform 纯函数映射(「uaData.platform === navigator.platform 桶
// 同源」不变量由构造保证:两脸同源,只能同真同假)。派生比平行数据管道更防漂移。
//
// GREASE brands 形状取自桶表策展真源 `derive_tables::UA_DATA_BRANDS`(结构 +
// GREASE 名/版本);Chromium/Google Chrome 两槽版本按 UA major 重填。
//
// fail-closed 判别(命中即不注入 —— 缺席是真 Chrome 形态里可接受的「无 UA-CH」,
// 拼错的 UA-CH 才是主动矛盾脸):
//  - UA 无 `Chrome/` major(Firefox 族等):真 Firefox 本就无 userAgentData;
//  - UA 含 `Edg/`:真 Edge 报自己的 brands 表,Chrome brands 配 Edge UA = 族间
//    矛盾(bao 桶不产 Edge UA,用户自定义 UA 面 fail-closed);
//  - navigator.platform 不在三桌面档(Win32/MacIntel/Linux*):无桶可映射;
//  - UA 平台 token 与 navigator.platform 档位矛盾(自定义 profile 拼装面):
//    注入会造出「Windows UA + macOS platformVersion」这类我们要封的死穴。
//
// 高熵策展值(任务指定档,「近似就行」口径):platformVersion Windows="15.0.0"
// (Win11 24H2 形)/ macOS="14.1.2" / Linux="6.5.0";architecture="x86"、
// bitness="64"(桌面 x86-64 桶,navigator_platform 的 x86_64 token 同源);
// wow64=false;model=""(真 Chrome 桌面就报空串,不是占位)。formFactors 键
// **不做**(树内/任务均无策展值,禁发明)—— 请求该键时决议缺键 = 「本 UA 不
// 支持 formFactors」的诚实形态。uaFullVersion/fullVersionList 版本串 = 自洽
// 派生(短 major 补 ".0.0.0"),不发明真实 build 号。

use crate::derive_tables::UA_DATA_BRANDS;
use crate::navigator::NavigatorProfile;
use serde::Serialize;

/// navigator.platform → 档位(与 derive_tables 三桌面桶同值域)。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Tier {
    Windows,
    Macos,
    Linux,
}

impl Tier {
    /// 档位判别:精确匹配桶表产出值("Win32"/"MacIntel"),Linux 档按前缀
    /// (navigator.rs 出 "Linux x86_64";自定义 "Linux x86" 同族放行)。
    fn from_navigator_platform(platform: &str) -> Option<Self> {
        match platform {
            "Win32" => Some(Tier::Windows), // @evidence .spec/10-REQUIREMENTS.html:1072
            "MacIntel" => Some(Tier::Macos), // @evidence .spec/10-REQUIREMENTS.html:1072
            p if p.starts_with("Linux") => Some(Tier::Linux), // @evidence .spec/10-REQUIREMENTS.html:1072
            _ => None, // @evidence .spec/10-REQUIREMENTS.html:1072
        }
    }

    /// userAgentData.platform(WICG 值域与 navigator.platform 不同值:
    /// Windows / macOS / Linux —— 桶表 UserAgentDataShape.platform 同形)。
    fn ua_data_platform(self) -> &'static str {
        match self {
            Tier::Windows => "Windows", // @evidence .spec/10-REQUIREMENTS.html:1072
            Tier::Macos => "macOS", // @evidence .spec/10-REQUIREMENTS.html:1072
            Tier::Linux => "Linux", // @evidence .spec/10-REQUIREMENTS.html:1072
        }
    }

    /// UA 平台 token 与档位的跨面一致性守卫(防线:自定义 profile 的 UA 与
    /// navigator.platform 拼装矛盾)。
    fn ua_token_present(self, ua: &str) -> bool {
        match self {
            Tier::Windows => ua.contains("Windows NT"), // @evidence .spec/10-REQUIREMENTS.html:1072
            Tier::Macos => ua.contains("Macintosh"), // @evidence .spec/10-REQUIREMENTS.html:1072
            Tier::Linux => ua.contains("Linux"), // @evidence .spec/10-REQUIREMENTS.html:1072
        }
    }

    /// 高熵 platformVersion 策展档(任务指定值;真实观测形)。
    fn platform_version(self) -> &'static str {
        match self {
            Tier::Windows => "15.0.0", // @evidence .spec/10-REQUIREMENTS.html:1072
            Tier::Macos => "14.1.2", // @evidence .spec/10-REQUIREMENTS.html:1072
            Tier::Linux => "6.5.0", // @evidence .spec/10-REQUIREMENTS.html:1072
        }
    }
}

#[derive(Serialize)]
struct BrandValue {
    brand: String,
    version: String,
}

/// 短 major 版本 → fullVersionList/uaFullVersion 的自洽 full 形("141" →
/// "141.0.0.0";GREASE "8" → "8.0.0.0" —— 真 Chrome fullVersionList 的 GREASE
/// 槽即 "8.0.0.0" 形)。已含点号的版本原样保留。
fn full_version(version: &str) -> String {
    if version.contains('.') {
        version.to_string()
    } else {
        format!("{version}.0.0.0")
    }
}

/// 从注入 UA 提取 Chrome major(如 "Chrome/141.0.0.0" → "141")。
/// 无 Chrome/ token 或 token 后非数字 → None。
fn chrome_major(ua: &str) -> Option<String> {
    let rest = ua.split("Chrome/").nth(1)?;
    let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
    if digits.is_empty() {
        None
    } else {
        Some(digits)
    }
}

/// UA-CH 形状(注入所需的全部 Rust 侧数据)。
struct UachShape {
    platform: &'static str,
    mobile: bool,
    brands_json: String,
    full_version_list_json: String,
    ua_full_version: String,
    platform_version: &'static str,
}

/// 形状派生(见模块头 fail-closed 判别;任何判别不过 → None → 不注入)。
fn derive_shape(nav: &NavigatorProfile) -> Option<UachShape> {
    let ua = nav.user_agent.as_str();
    // 真 Edge 有自己的 brands 表;Chrome brands 配 Edge UA = 族间矛盾。
    if ua.contains("Edg/") {
        return None;
    }
    let major = chrome_major(ua)?;
    let tier = Tier::from_navigator_platform(&nav.platform)?;
    if !tier.ua_token_present(ua) {
        return None;
    }

    // brands:GREASE 槽取桶表策展形,真品牌槽版本按 UA major 重填(与注入 UA
    // 天然同步)。JSON 字段序 brand-first(真 Chrome JSON.stringify 形)。
    let brands: Vec<BrandValue> = UA_DATA_BRANDS
        .iter()
        .map(|(brand, version)| BrandValue {
            brand: (*brand).to_string(),
            version: if *brand == "Chromium" || *brand == "Google Chrome" {
                major.clone()
            } else {
                (*version).to_string()
            },
        })
        .collect();
    let full_version_list: Vec<BrandValue> = brands
        .iter()
        .map(|b| BrandValue {
            brand: b.brand.clone(),
            version: full_version(&b.version),
        })
        .collect();
    let brands_json = serde_json::to_string(&brands).ok()?;
    let full_version_list_json = serde_json::to_string(&full_version_list).ok()?;

    Some(UachShape {
        platform: tier.ua_data_platform(),
        mobile: false,
        brands_json,
        full_version_list_json,
        ua_full_version: full_version(&major),
        platform_version: tier.platform_version(),
    })
}

/// 生成 navigator.userAgentData 注入 JS 块(空串 = 本 profile 不注入)。
///
/// # 契约(调用方 = hooks::build_navigator_js 的 IIFE 内部)
/// - 只能在 navigator IIFE 内展开:依赖先行定义的 `__bao_def`(refused-redefine
///   吞没包装,本 crate error.rs:74 纪律)与 `__nav_t`(e148 Chromium-parity
///   原型定位目标)。
/// - 展开点在 `if (__scr_t)` 段之前(WebGL 早退 `return` 之后会跳过本块)。
/// - 门:`__nav_t && globalThis.isSecureContext` —— spec 的 [SecureContext] 属性
///   门(servo WindowOrWorkerGlobalScope.webidl:46 实现了 isSecureContext;
///   worker realm 的 globalThis 即 WorkerGlobalScope,同 mixin)。非安全 context /
///   裸 realm(nav 为 null)整块跳过 = 真 Chrome 形态。
/// - e148 parity:成员全部挂原型(`__uad_p`),实例零 own names;brands 数组
///   Object.freeze(FrozenArray 语义);单例实例(属性访问同引用,真 Chrome
///   属性缓存形)。
pub(crate) fn build_uach_js(nav: &NavigatorProfile) -> String {
    let Some(shape) = derive_shape(nav) else {
        return String::new();
    };
    let platform = shape.platform;
    let mobile = shape.mobile;
    format!(
        r#"  // navigator.userAgentData (UA-CH): Chrome-family [SecureContext] interface
  // (WICG ua-client-hints: NavigatorUAData [Exposed=(Window,Worker)] — absence
  // in insecure realms / non-Chrome families is real-engine parity, not a gap).
  // e148 parity: members live on the prototype, instance carries zero own names.
  if (__nav_t && globalThis.isSecureContext) {{
  var __uad_brands = Object.freeze({brands});
  var __uad_high = {{ platformVersion: {pv:?}, architecture: "x86", bitness: "64", model: "", uaFullVersion: {fv:?}, fullVersionList: Object.freeze({fvl}), wow64: false }};
  var __uad_p = Object.create(Object.prototype);
  try {{ Object.defineProperty(__uad_p, Symbol.toStringTag, {{ value: 'NavigatorUAData', configurable: true }}); }} catch (e) {{}}
  __bao_def(__uad_p, 'brands', {{ get: function() {{ return __uad_brands; }}, enumerable: true, configurable: true }});
  __bao_def(__uad_p, 'mobile', {{ get: function() {{ return {mobile}; }}, enumerable: true, configurable: true }});
  __bao_def(__uad_p, 'platform', {{ get: function() {{ return {platform:?}; }}, enumerable: true, configurable: true }});
  __bao_def(__uad_p, 'getHighEntropyValues', {{ value: function(hints) {{
    // Spec: resolution ALWAYS carries the low-entropy triple; requested
    // high-entropy keys are added iff supported (unknown hints ignored).
    var out = {{ brands: __uad_brands, mobile: {mobile}, platform: {platform:?} }};
    try {{
      var req = (hints && typeof hints.length === 'number') ? hints : [];
      for (var i = 0; i < req.length; i++) {{
        var k = String(req[i]);
        if (Object.prototype.hasOwnProperty.call(__uad_high, k)) out[k] = __uad_high[k];
      }}
    }} catch (e) {{ /* malformed hint list — resolve the low-entropy triple */ }}
    return Promise.resolve(out);
  }}, writable: true, enumerable: true, configurable: true }});
  __bao_def(__uad_p, 'toJSON', {{ value: function() {{ return {{ brands: __uad_brands, mobile: {mobile}, platform: {platform:?} }}; }}, writable: true, enumerable: true, configurable: true }});
  var __uad = Object.create(__uad_p);
  __bao_def(__nav_t, 'userAgentData', {{ get: function() {{ return __uad; }}, enumerable: true, configurable: true }});
  }}
"#,
        brands = shape.brands_json,
        fvl = shape.full_version_list_json,
        pv = shape.platform_version,
        fv = shape.ua_full_version,
        platform = platform,
        mobile = mobile,
    )
}

// @trace REQ-STL-004 [req:REQ-STL-004] [level:unit]
#[cfg(test)]
mod tests {
    use super::*;

    /// 桶形 Chrome profile(Windows 档 UA + 指定 navigator.platform)。
    fn chrome_nav(platform: &str) -> NavigatorProfile {
        NavigatorProfile {
            user_agent: format!(
                "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/{}.0.0.0 Safari/537.36",
                crate::derive_tables::CHROME_MAJOR
            ),
            platform: platform.into(),
            language: "en-US".into(),
            languages: vec!["en-US".into(), "en".into()],
            hardware_concurrency: 8,
            max_touch_points: 0,
            vendor: "Google Inc.".into(),
            app_version: String::new(),
            oscpu: None,
            build_id: None,
            product_sub: "20030107".into(),
            device_memory: 8.0,
        }
    }

    fn firefox_nav() -> NavigatorProfile {
        NavigatorProfile::firefox()
    }

    fn mac_nav() -> NavigatorProfile {
        let mut nav = chrome_nav("MacIntel");
        nav.user_agent = format!(
            "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/{}.0.0.0 Safari/537.36",
            crate::derive_tables::CHROME_MAJOR
        );
        nav
    }

    fn linux_nav() -> NavigatorProfile {
        let mut nav = chrome_nav("Linux x86_64");
        nav.user_agent = format!(
            "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/{}.0.0.0 Safari/537.36",
            crate::derive_tables::CHROME_MAJOR
        );
        nav
    }

    // ── Chrome major 提取 ────────────────────────────────────────────────

    #[test]
    fn chrome_major_extracts_from_standard_ua() {
        assert_eq!(
            chrome_major(
                "Mozilla/5.0 (X11) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/141.0.0.0 Safari/537.36"
            )
            .as_deref(),
            Some("141")
        );
    }

    #[test]
    fn chrome_major_absent_for_firefox_and_tokenless_ua() {
        assert_eq!(
            chrome_major(NavigatorProfile::firefox().user_agent.as_str()),
            None
        );
        assert_eq!(chrome_major("Mozilla/5.0 (X11; Linux x86_64) Gecko/"), None);
        assert_eq!(chrome_major("Mozilla/5.0 Chrome/x.0.0.0"), None);
    }

    // ── fail-closed 判别 ─────────────────────────────────────────────────

    #[test]
    fn firefox_family_yields_empty_block() {
        assert_eq!(
            build_uach_js(&firefox_nav()),
            "",
            "Firefox 族无 UA-CH(真引擎 parity)"
        );
    }

    #[test]
    fn edge_ua_is_fail_closed() {
        let mut nav = chrome_nav("Win32");
        nav.user_agent = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/141.0.0.0 Safari/537.36 Edg/141.0.0.0".into();
        assert_eq!(
            build_uach_js(&nav),
            "",
            "Edge UA 禁配 Chrome brands(fail-closed)"
        );
    }

    #[test]
    fn cross_face_platform_mismatch_is_fail_closed() {
        // Windows UA + MacIntel platform = 拼装矛盾脸,禁注入。
        assert_eq!(build_uach_js(&chrome_nav("MacIntel")), "");
    }

    #[test]
    fn unmapped_platform_yields_empty_block() {
        assert_eq!(build_uach_js(&chrome_nav("iPhone")), "");
    }

    // ── 档位映射(与 derive_tables 桶表同值域)──────────────────────────

    #[test]
    fn tier_ladder_matches_bucket_value_domains() {
        assert_eq!(
            Tier::from_navigator_platform("Win32").map(Tier::ua_data_platform),
            Some("Windows")
        );
        assert_eq!(
            Tier::from_navigator_platform("MacIntel").map(Tier::ua_data_platform),
            Some("macOS")
        );
        assert_eq!(
            Tier::from_navigator_platform("Linux x86_64").map(Tier::ua_data_platform),
            Some("Linux")
        );
        // 高熵 platformVersion 策展档(任务指定值)。
        assert_eq!(Tier::Windows.platform_version(), "15.0.0");
        assert_eq!(Tier::Macos.platform_version(), "14.1.2");
        assert_eq!(Tier::Linux.platform_version(), "6.5.0");
    }

    #[test]
    fn ua_data_platform_is_pure_function_of_navigator_platform() {
        // 任务钉的联动不变量:uaData.platform === navigator.platform 桶同源 ——
        // 映射是纯函数,两脸只能同真同假。
        for (nav_platform, ua_data_platform) in [
            ("Win32", "Windows"),
            ("MacIntel", "macOS"),
            ("Linux x86_64", "Linux"),
        ] {
            let nav = if nav_platform == "Win32" {
                chrome_nav(nav_platform)
            } else if nav_platform == "MacIntel" {
                mac_nav()
            } else {
                linux_nav()
            };
            let shape = derive_shape(&nav).expect("桶形 profile 必须可派生");
            assert_eq!(shape.platform, ua_data_platform);
            let js = build_uach_js(&nav);
            assert!(
                js.contains("'userAgentData'"),
                "{nav_platform}: 注入块必须定义 userAgentData"
            );
            assert!(js.contains(&format!("return \"{ua_data_platform}\";")));
        }
    }

    // ── 形状断言(brands / 键集 / parity 细节)──────────────────────────

    #[test]
    fn brands_are_nonempty_track_ua_major_and_grease_shaped() {
        let nav = chrome_nav("Win32");
        let js = build_uach_js(&nav);
        let major = chrome_major(&nav.user_agent).unwrap();
        assert!(js.contains(&format!(
            "\"brand\":\"Google Chrome\",\"version\":\"{major}\""
        )));
        assert!(js.contains(&format!("\"brand\":\"Chromium\",\"version\":\"{major}\"")));
        // GREASE 槽取桶表策展形(fullVersionList 槽为 "8.0.0.0" full 形)。
        assert!(js.contains("\"brand\":\"Not_A Brand\",\"version\":\"8\""));
        assert!(js.contains("\"brand\":\"Not_A Brand\",\"version\":\"8.0.0.0\""));
    }

    #[test]
    fn high_entropy_block_carries_task_key_set() {
        let js = build_uach_js(&chrome_nav("Win32"));
        for key in [
            "platformVersion",
            "model",
            "uaFullVersion",
            "fullVersionList",
            "architecture",
            "bitness",
            "wow64",
        ] {
            assert!(js.contains(key), "高熵块必须携带任务键集: {key}");
        }
        // 低熵面 + spec 方法面。
        for face in [
            "'brands'",
            "'mobile'",
            "'platform'",
            "getHighEntropyValues",
            "toJSON",
            "Promise.resolve",
            "isSecureContext",
        ] {
            assert!(js.contains(face), "注入块必须携带 spec 面: {face}");
        }
    }

    #[test]
    fn frozen_brands_and_prototype_targeting_parity() {
        let js = build_uach_js(&chrome_nav("Win32"));
        assert!(js.contains("Object.freeze"), "brands=FrozenArray 语义");
        assert!(
            js.contains("Object.create(__uad_p)"),
            "原型承载成员,实例零 own names"
        );
        assert!(js.contains("NavigatorUAData"), "toStringTag parity");
    }

    #[test]
    fn desktop_shape_carries_empty_model_and_no_wow64() {
        let js = build_uach_js(&chrome_nav("Win32"));
        assert!(
            js.contains("model: \"\""),
            "真 Chrome 桌面 model=空串(语义值,非占位)"
        );
        assert!(js.contains("wow64: false"));
        assert!(js.contains("mobile: false"), "桌面桶 mobile=false");
    }

    #[test]
    fn block_is_deterministic_per_profile() {
        let nav = linux_nav();
        assert_eq!(build_uach_js(&nav), build_uach_js(&nav));
    }

    #[test]
    fn full_version_derivation_is_self_consistent() {
        assert_eq!(full_version("141"), "141.0.0.0");
        assert_eq!(full_version("8"), "8.0.0.0");
        assert_eq!(
            full_version("141.0.7390.54"),
            "141.0.7390.54",
            "已 full 形原样保留"
        );
    }

    // ── from_seed 派生 profile 端到端(桶形三档全可注入)─────────────────

    #[test]
    fn from_seed_profiles_yield_coherent_uach_across_tiers() {
        for seed in 1..=64u64 {
            let profile = crate::profile::StealthProfile::from_seed(seed);
            let js = build_uach_js(&profile.navigator);
            let expected_platform = match profile.navigator.platform.as_str() {
                "Win32" => "Windows", // @evidence .spec/10-REQUIREMENTS.html:1072
                "MacIntel" => "macOS", // @evidence .spec/10-REQUIREMENTS.html:1072
                "Linux x86_64" => "Linux", // @evidence .spec/10-REQUIREMENTS.html:1072
                other => panic!("seed {seed}: 未预期档位 {other}"), // @evidence .spec/10-REQUIREMENTS.html:1072
            };
            assert!(
                js.contains("'userAgentData'"),
                "seed {seed}: 桶形 profile 必须注入"
            );
            assert!(js.contains(&format!("return \"{expected_platform}\";")));
        }
    }
}
