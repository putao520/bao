// REQ-STL-007: 连贯性桶表 — StealthProfile::from_seed 的档位数据  @trace REQ-STL-007
//
// 「近似就行」的正确实现(用户裁决 2026-10-21「按某个随机值固定指纹,近似就行」):
// 强联动维度(UA ⇔ navigator.platform ⇔ userAgentData ⇔ WebGL renderer ⇔ screen 形)
// 钉进同一 platform 档,档内按子种子确定性选型。逐字段独立随机会造出「Windows UA +
// Metal renderer」这类地球不存在的组合,反而成为最强指纹。
//
// 档位族 = Chrome 形(桌面真实占优;UA-CH/ANGLE renderer 形均 Chrome 形)。Firefox 列
// 保留在显式选择面(StealthProfile::firefox_default),不入随机派生桶。
//
// 数据来源(策展级「近似」,同日裁决明示不需权威数据库):
//  - GPU 串:常见真实 ANGLE 形(Windows=D3D11 后端 / macOS=Metal 后端 / Linux=Mesa
//    GL 后端);设备名/DeviceID 取当期主流机器的常见观测形。
//  - 屏幕:StatCounter 级全球桌面分辨率常见档(Win/Linux 以 dpr=1 为主;1536x864 即
//    Windows 125% 缩放形,配 dpr=1.25 保持 Chrome 观测一致性;mac 档 Retina CSS 形
//    dpr=2 为主 + 外接非 Retina 形)。
//  - 版本:CHROME_MAJOR 集中常量(当期主流),更新只动这里;UA-CH brands 版本由
//    tests 钉死与该常量同步。
//  - UA-CH(userAgentData)形状:GREASE 品牌名随 milestone 轮换,此为当期主流形。
//    注入面已接(2026-10-21,hooks::uach):从 NavigatorProfile 派生注入,brands
//    结构/GREASE 槽取自本表 UA_DATA_BRANDS,真品牌槽版本按注入 UA 的 Chrome
//    major 重填(与 UA 天然同步);platform 值已经不变量测试与 navigator.platform
//    钉死一致。

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum PlatformTier {
    Windows,
    Macos,
    Linux,
}

/// navigator.userAgentData 形状(Chrome 形)。
pub(crate) struct UserAgentDataShape {
    /// userAgentData.platform(注意与 navigator.platform 不同值域:
    /// Windows / macOS / Linux vs Win32 / MacIntel / Linux x86_64)。
    pub platform: &'static str,
    pub mobile: bool,
    /// userAgentData.brands(GREASE 名随 milestone 轮换的当期主流形;Chromium/
    /// Google Chrome 版本串必须与 [`CHROME_MAJOR`] 同步,tests 钉死)。
    pub brands: &'static [(&'static str, &'static str)],
}

/// 一档屏幕形(真实统计常见档)。
pub(crate) struct ScreenShape {
    pub width: u32,
    pub height: u32,
    pub device_pixel_ratio: f64,
}

/// 一档 GPU:UNMASKED_VENDOR_WEBGL 的 "Google Inc. (...)" 括号后缀 + 全 renderer 串。
/// vendor 与 renderer 成对策展,禁止跨档/跨厂商拼装。
pub(crate) struct GpuShape {
    pub vendor_suffix: &'static str,
    pub renderer: &'static str,
}

/// 一个 platform 档位的全部联动面。
pub(crate) struct PlatformCohort {
    pub tier: PlatformTier,
    /// navigator.platform(Win32 / MacIntel / Linux x86_64)。
    pub navigator_platform: &'static str,
    /// UA 圆括号内平台 token。
    pub ua_platform_token: &'static str,
    pub ua_data: UserAgentDataShape,
    pub gpus: &'static [GpuShape],
    pub screens: &'static [ScreenShape],
}

/// 当期主流 Chrome 大版本(策展值,更新只动此常量)。
pub(crate) const CHROME_MAJOR: &str = "141";

pub(crate) const UA_DATA_BRANDS: &[(&str, &str)] = &[
    ("Not_A Brand", "8"),
    ("Chromium", CHROME_MAJOR),
    ("Google Chrome", CHROME_MAJOR),
];

// ── GPU 池(Windows=D3D11 / macOS=Metal / Linux=Mesa GL 的真实 ANGLE 串形)────────

const WINDOWS_GPUS: &[GpuShape] = &[
    GpuShape {
        vendor_suffix: "Intel",
        renderer: "ANGLE (Intel, Intel(R) UHD Graphics 620 (0x00003EA0) Direct3D11 vs_5_0 ps_5_0, D3D11)",
    },
    GpuShape {
        vendor_suffix: "Intel",
        renderer: "ANGLE (Intel, Intel(R) UHD Graphics 630 (0x00003E92) Direct3D11 vs_5_0 ps_5_0, D3D11)",
    },
    GpuShape {
        vendor_suffix: "Intel",
        renderer: "ANGLE (Intel, Intel(R) Iris(R) Xe Graphics (0x00009A49) Direct3D11 vs_5_0 ps_5_0, D3D11)",
    },
    GpuShape {
        vendor_suffix: "NVIDIA",
        renderer: "ANGLE (NVIDIA, NVIDIA GeForce GTX 1060 6GB (0x00001C03) Direct3D11 vs_5_0 ps_5_0, D3D11)",
    },
    GpuShape {
        vendor_suffix: "NVIDIA",
        renderer: "ANGLE (NVIDIA, NVIDIA GeForce RTX 3060 (0x00002503) Direct3D11 vs_5_0 ps_5_0, D3D11)",
    },
    GpuShape {
        vendor_suffix: "NVIDIA",
        renderer: "ANGLE (NVIDIA, NVIDIA GeForce RTX 4060 (0x00002882) Direct3D11 vs_5_0 ps_5_0, D3D11)",
    },
    GpuShape {
        vendor_suffix: "AMD",
        renderer: "ANGLE (AMD, AMD Radeon(TM) Graphics (0x0000164E) Direct3D11 vs_5_0 ps_5_0, D3D11)",
    },
    GpuShape {
        vendor_suffix: "AMD",
        renderer: "ANGLE (AMD, AMD Radeon RX 580 Series (0x000067DF) Direct3D11 vs_5_0 ps_5_0, D3D11)",
    },
    GpuShape {
        vendor_suffix: "AMD",
        renderer: "ANGLE (AMD, AMD Radeon RX 6600 (0x000073FF) Direct3D11 vs_5_0 ps_5_0, D3D11)",
    },
];

/// macOS 真实形态:Chrome/Metal 后端,Apple Silicon/Intel/AMD 旧款(当期 macOS
/// 无 NVIDIA Metal 面 — 池子如实反映平台现实,不硬凑三厂商)。
const MACOS_GPUS: &[GpuShape] = &[
    GpuShape {
        vendor_suffix: "Apple",
        renderer: "ANGLE (Apple, ANGLE Metal Renderer: Apple M1, Unspecified Version)",
    },
    GpuShape {
        vendor_suffix: "Apple",
        renderer: "ANGLE (Apple, ANGLE Metal Renderer: Apple M2, Unspecified Version)",
    },
    GpuShape {
        vendor_suffix: "Intel",
        renderer:
            "ANGLE (Apple, ANGLE Metal Renderer: Intel(R) UHD Graphics 630, Unspecified Version)",
    },
    GpuShape {
        vendor_suffix: "AMD",
        renderer: "ANGLE (Apple, ANGLE Metal Renderer: AMD Radeon Pro 5500M, Unspecified Version)",
    },
];

const LINUX_GPUS: &[GpuShape] = &[
    GpuShape {
        vendor_suffix: "Intel",
        renderer: "ANGLE (Intel, Mesa Intel(R) UHD Graphics 630 (CFL GT2), OpenGL 4.6)",
    },
    GpuShape {
        vendor_suffix: "Intel",
        renderer: "ANGLE (Intel, Mesa Intel(R) Graphics (RPL-S), OpenGL 4.6)",
    },
    GpuShape {
        vendor_suffix: "AMD",
        renderer: "ANGLE (AMD, Mesa AMD Radeon(TM) Graphics (raphael-mendocino), OpenGL 4.6)",
    },
    GpuShape {
        vendor_suffix: "AMD",
        renderer: "ANGLE (AMD, Mesa AMD Radeon RX 6600 (navi23), OpenGL 4.6)",
    },
    GpuShape {
        vendor_suffix: "NVIDIA",
        renderer: "ANGLE (NVIDIA, NVIDIA GeForce RTX 3060/PCIe/SSE2, OpenGL 4.6)",
    },
    GpuShape {
        vendor_suffix: "NVIDIA",
        renderer: "ANGLE (NVIDIA, NVIDIA GeForce GTX 1650/PCIe/SSE2, OpenGL 4.6)",
    },
];

// ── 屏幕档(真实统计常见形;mac 档 Retina CSS 形 dpr=2 为主)────────────────────

const WINDOWS_SCREENS: &[ScreenShape] = &[
    ScreenShape {
        width: 1920,
        height: 1080,
        device_pixel_ratio: 1.0,
    },
    ScreenShape {
        width: 1536,
        height: 864,
        device_pixel_ratio: 1.25,
    },
    ScreenShape {
        width: 1366,
        height: 768,
        device_pixel_ratio: 1.0,
    },
    ScreenShape {
        width: 2560,
        height: 1440,
        device_pixel_ratio: 1.0,
    },
    ScreenShape {
        width: 1280,
        height: 720,
        device_pixel_ratio: 1.0,
    },
    ScreenShape {
        width: 1680,
        height: 1050,
        device_pixel_ratio: 1.0,
    },
];

const MACOS_SCREENS: &[ScreenShape] = &[
    ScreenShape {
        width: 1440,
        height: 900,
        device_pixel_ratio: 2.0,
    },
    ScreenShape {
        width: 1512,
        height: 982,
        device_pixel_ratio: 2.0,
    },
    ScreenShape {
        width: 1728,
        height: 1117,
        device_pixel_ratio: 2.0,
    },
    ScreenShape {
        width: 1536,
        height: 960,
        device_pixel_ratio: 2.0,
    },
    ScreenShape {
        width: 1920,
        height: 1080,
        device_pixel_ratio: 1.0,
    },
    ScreenShape {
        width: 1680,
        height: 1050,
        device_pixel_ratio: 2.0,
    },
];

const LINUX_SCREENS: &[ScreenShape] = &[
    ScreenShape {
        width: 1920,
        height: 1080,
        device_pixel_ratio: 1.0,
    },
    ScreenShape {
        width: 2560,
        height: 1440,
        device_pixel_ratio: 1.0,
    },
    ScreenShape {
        width: 1366,
        height: 768,
        device_pixel_ratio: 1.0,
    },
    ScreenShape {
        width: 1600,
        height: 900,
        device_pixel_ratio: 1.0,
    },
    ScreenShape {
        width: 3840,
        height: 2160,
        device_pixel_ratio: 1.0,
    },
    ScreenShape {
        width: 1280,
        height: 1024,
        device_pixel_ratio: 1.0,
    },
];

pub(crate) static WINDOWS_COHORT: PlatformCohort = PlatformCohort {
    tier: PlatformTier::Windows,
    navigator_platform: "Win32",
    ua_platform_token: "Windows NT 10.0; Win64; x64",
    ua_data: UserAgentDataShape {
        platform: "Windows",
        mobile: false,
        brands: UA_DATA_BRANDS,
    },
    gpus: WINDOWS_GPUS,
    screens: WINDOWS_SCREENS,
};

pub(crate) static MACOS_COHORT: PlatformCohort = PlatformCohort {
    tier: PlatformTier::Macos,
    navigator_platform: "MacIntel",
    ua_platform_token: "Macintosh; Intel Mac OS X 10_15_7",
    ua_data: UserAgentDataShape {
        platform: "macOS",
        mobile: false,
        brands: UA_DATA_BRANDS,
    },
    gpus: MACOS_GPUS,
    screens: MACOS_SCREENS,
};

pub(crate) static LINUX_COHORT: PlatformCohort = PlatformCohort {
    tier: PlatformTier::Linux,
    navigator_platform: "Linux x86_64",
    ua_platform_token: "X11; Linux x86_64",
    ua_data: UserAgentDataShape {
        platform: "Linux",
        mobile: false,
        brands: UA_DATA_BRANDS,
    },
    gpus: LINUX_GPUS,
    screens: LINUX_SCREENS,
};

/// platform 子种子 → 档位(确定性;档内所有联动面共享同档)。
pub(crate) fn cohort_for_seed(platform_sub_seed: u64) -> &'static PlatformCohort {
    match platform_sub_seed % 3 {
        0 => &WINDOWS_COHORT, // @evidence .spec/10-REQUIREMENTS.html:1184
        1 => &MACOS_COHORT, // @evidence .spec/10-REQUIREMENTS.html:1184
        _ => &LINUX_COHORT, // @evidence .spec/10-REQUIREMENTS.html:1184
    }
}

/// deviceMemory 档(任务指定档位;GB,Chrome 只报 2 的幂档)。
pub(crate) const DEVICE_MEMORY_POOL: &[f64] = &[4.0, 8.0, 16.0];

/// hardwareConcurrency 档(任务指定档位)。
pub(crate) const HARDWARE_CONCURRENCY_POOL: &[u32] = &[4, 6, 8, 12, 16];

// @trace REQ-STL-007 [req:REQ-STL-007] [level:unit]
#[cfg(test)]
mod tests {
    use super::*;

    fn all_cohorts() -> Vec<&'static PlatformCohort> {
        vec![&WINDOWS_COHORT, &MACOS_COHORT, &LINUX_COHORT]
    }

    #[test]
    fn windows_pool_covers_intel_nvidia_amd_three_each() {
        let count = |suffix: &str| {
            WINDOWS_GPUS
                .iter()
                .filter(|g| g.vendor_suffix == suffix)
                .count()
        };
        assert!(count("Intel") >= 3, "Windows 档 Intel ≥3 条");
        assert!(count("NVIDIA") >= 3, "Windows 档 NVIDIA ≥3 条");
        assert!(count("AMD") >= 3, "Windows 档 AMD ≥3 条");
    }

    #[test]
    fn every_cohort_screen_pool_has_at_least_six_shapes() {
        for cohort in all_cohorts() {
            assert!(
                cohort.screens.len() >= 6,
                "{:?} 档 screen 池 ≥6 条,实际 {}",
                cohort.tier,
                cohort.screens.len()
            );
            for s in cohort.screens {
                assert!(s.width > 0 && s.height > 0 && s.device_pixel_ratio > 0.0);
            }
        }
    }

    #[test]
    fn ua_data_brands_versions_track_chrome_major() {
        for cohort in all_cohorts() {
            assert!(!cohort.ua_data.brands.is_empty());
            assert!(cohort
                .ua_data
                .brands
                .iter()
                .any(|(n, _)| *n == "Google Chrome"));
            for (name, version) in cohort.ua_data.brands {
                if *name == "Chromium" || *name == "Google Chrome" {
                    assert_eq!(
                        *version, CHROME_MAJOR,
                        "{name} 版本串必须与 CHROME_MAJOR 同步"
                    );
                }
            }
        }
    }

    #[test]
    fn tier_markers_are_mutually_exclusive_across_pools() {
        // renderer 平台标记与档位一一对应:Direct3D11 只在 Windows 池,Metal 只在
        // macOS 池 — 跨档拼装在数据层结构性不可能。
        for cohort in all_cohorts() {
            for g in cohort.gpus {
                let d3d = g.renderer.contains("Direct3D11");
                let metal = g.renderer.contains("Metal");
                match cohort.tier {
                    PlatformTier::Windows => assert!(d3d && !metal), // @evidence .spec/10-REQUIREMENTS.html:1184
                    PlatformTier::Macos => assert!(metal && !d3d), // @evidence .spec/10-REQUIREMENTS.html:1184
                    PlatformTier::Linux => assert!(!d3d && !metal), // @evidence .spec/10-REQUIREMENTS.html:1184
                }
            }
        }
    }

    #[test]
    fn navigator_platform_and_ua_data_agree_per_tier() {
        let expect = [
            (PlatformTier::Windows, "Win32", "Windows"),
            (PlatformTier::Macos, "MacIntel", "macOS"),
            (PlatformTier::Linux, "Linux x86_64", "Linux"),
        ];
        for cohort in all_cohorts() {
            let (nav, ua_data) = expect
                .iter()
                .find(|(t, _, _)| *t == cohort.tier)
                .map(|(_, n, u)| (*n, *u))
                .unwrap();
            assert_eq!(cohort.navigator_platform, nav);
            assert_eq!(cohort.ua_data.platform, ua_data);
            assert!(!cohort.ua_data.mobile, "桌面档 mobile=false");
        }
    }

    #[test]
    fn cohort_for_seed_is_deterministic_and_total() {
        for seed in 0..64u64 {
            let a = cohort_for_seed(seed);
            let b = cohort_for_seed(seed);
            assert!(std::ptr::eq(a, b), "同子种子必回同档");
        }
    }

    #[test]
    fn pools_match_task_specified_tiers() {
        assert_eq!(DEVICE_MEMORY_POOL, &[4.0, 8.0, 16.0][..]);
        assert_eq!(HARDWARE_CONCURRENCY_POOL, &[4u32, 6, 8, 12, 16][..]);
    }
}
