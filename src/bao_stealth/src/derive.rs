// REQ-STL-007: StealthProfile::from_seed — 单随机种子确定性派生整套指纹  @trace REQ-STL-007
//
// 随机指纹模型(用户裁决 2026-10-21「反指纹=随机指纹;按某个随机值固定指纹,近似
// 就行」):一个 u64 种子 → 一整套互不可关联、但档内连贯的指纹。同种子跨重载/会话
// 逐字段复现(登录态不破);异种子在任一域上派生不同子种子(构造性单射,见 [`sub`]),
// 无共享可关联量。
//
// 派生面:canvas/webgl/audio/font/behavior/clientrects 六个既有 seed 字段 + 档内联动
// 的 navigator/screen/webgl/tls/h2 形。timezone/locale/timing/battery 等用户可配或
// 非指纹主轴面不派生(保持出厂默认形,注释锚在 [`StealthProfile::from_seed`])。
//
// 默认身份翻转点:bao_browser 显式 browser context 创建时以 OS 熵种子调用
// [`StealthProfile::from_random_seed`] 并在 context 内共享(page_pool.rs 注释锚)。
// 出厂固定预设 firefox_default()/chrome_default() 原样保留(显式选择面)。

use crate::behavior::{BehaviorConfig, BehaviorSimulator};
use crate::canvas::CanvasNoise;
use crate::derive_tables::{
    cohort_for_seed, GpuShape, PlatformCohort, CHROME_MAJOR, DEVICE_MEMORY_POOL,
    HARDWARE_CONCURRENCY_POOL,
};
use crate::http2::Http2Fingerprint;
use crate::navigator::{NavigatorProfile, ScreenProfile};
use crate::profile::{ClientRectsConfig, FontConfig, ScreenDisplayConfig, StealthProfile};
use crate::tls::TlsFingerprint;
use crate::webgl_audio::{AudioProfile, WebGLProfile};
use std::sync::OnceLock;

/// splitmix64 终结混洗(公域算法,12 行):uint64 → uint64 双射。本 crate 各面
/// 确定性噪声的同一原语族(canvas.rs/behavior.rs 的 finalizer 常量同源),不引入
/// 新依赖。
pub(crate) fn splitmix64(state: &mut u64) -> u64 {
    *state = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
    let mut z = *state;
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// FNV-1a 64:域标签 → 固定常量(域分离用)。
fn fnv1a64(bytes: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for &b in bytes {
        hash ^= b as u64;
        hash = hash.wrapping_mul(0x0000_0100_0000_01B3);
    }
    hash
}

/// 域分离子种子:对固定域标签是 master seed 的**单射**(XOR 常量 + 两轮
/// splitmix64 皆双射,见 tests)——异种子在任一域必派生不同子种子,发散性是
/// 构造性保证,不靠抽样概率。
pub(crate) fn sub(seed: u64, domain: &str) -> u64 {
    let mut state = seed ^ fnv1a64(domain.as_bytes());
    let _first = splitmix64(&mut state);
    splitmix64(&mut state)
}

/// 从非空池确定性选型(「近似就行」标准下均匀取模足够)。
fn pick<'a, T>(sub_seed: u64, pool: &'a [T]) -> &'a T {
    &pool[(sub_seed % pool.len() as u64) as usize]
}

/// CanvasNoise::new 的 SPEC 断言要求 seed>0;splitmix64 输出可为 0(2^-64),
/// 0 映射到黄金比例常数 — 仍确定性,分布不变。
fn nonzero(seed: u64) -> u64 {
    if seed == 0 {
        0x9E37_79B9_7F4A_7C15
    } else {
        seed
    }
}

/// UA 族(TLS 池选型的族约束轴:TLS 形只能配同族 UA —— Chrome BoringSSL 形与
/// Firefox 形在 groups/sigalgs/ALPN/H2 pseudo 序/PRIORITY 面互斥,跨族拼装
/// (Chrome UA + Firefox TLS)是经典检测向量,数据层结构性禁止)。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum UaFamily {
    Chrome,
    Firefox,
}

/// Chrome 族真实形池(构造器即真源;显式 fn 指针类型,不同 fn item 共型进池)。
const CHROME_TLS_POOL: &[fn() -> TlsFingerprint] =
    &[TlsFingerprint::chrome_120, TlsFingerprint::chrome_latest];

/// Firefox 族真实形池(firefox 形真实串只入本池 —— 与 Chrome UA 同发 = 族间
/// 矛盾,见 [`UaFamily`])。
const FIREFOX_TLS_POOL: &[fn() -> TlsFingerprint] = &[TlsFingerprint::firefox];

/// 「UA 族 → TLS 池子集」映射(只收树内/资料库有据可查的真实抓取形态,禁发明
/// 不存在的 cipher 组合)。
///
/// 池浅缺口登记(2026-10-21):Chrome 族树内真实面 = 2 形(chrome_120 /
/// chrome_latest,见 tls.rs);firefox 形真实串树内亦有但只入 Firefox 池。
/// edge/safari 形在树内与资料库(.gsc/domain-knowledge/tls-fingerprinting.md,
/// 其基线锚仅 Firefox/Chrome 两族)零据 —— 任务「禁发明」下不造假形入池;
/// 扩池须先以真机 oracle 取证(tls.peet.ws 配方,资料库「解决问题时参考 1」)
/// 再补构造器 + 更新 tls_pool_gap_registration_pins_honest_count 测试钉。
pub(crate) fn tls_pool(family: UaFamily) -> &'static [fn() -> TlsFingerprint] {
    match family {
        UaFamily::Chrome => CHROME_TLS_POOL, // @evidence .spec/10-REQUIREMENTS.html:1184
        UaFamily::Firefox => FIREFOX_TLS_POOL, // @evidence .spec/10-REQUIREMENTS.html:1184
    }
}

/// TLS 池选型:只从**树内真实抓取形态**中确定性选(逐字段随机 = 造出地球不存在的
/// JA3,反成指纹)。from_seed 全部档位 = Chrome 族 UA ⇒ 恒走 Chrome 池
/// (族约束由 [`tls_pool`] 的「UA 族→池子集」映射承载)。
pub(crate) fn tls_candidate(tls_sub_seed: u64) -> TlsFingerprint {
    let pool = tls_pool(UaFamily::Chrome);
    pool[(tls_sub_seed % pool.len() as u64) as usize]()
}

/// Chrome 形 UA 串(档内平台 token + 当期主流版本常量)。
fn ua_string(cohort: &PlatformCohort) -> String {
    format!(
        "Mozilla/5.0 ({}) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/{}.0.0.0 Safari/537.36",
        cohort.ua_platform_token, CHROME_MAJOR
    )
}

/// Chrome 约定的 navigator.appVersion = UA 去掉 "Mozilla/" 前缀。
fn app_version_string(cohort: &PlatformCohort) -> String {
    format!(
        "5.0 ({}) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/{}.0.0.0 Safari/537.36",
        cohort.ua_platform_token, CHROME_MAJOR
    )
}

impl StealthProfile {
    /// 单种子确定性派生:同 seed 两次调用逐字段相等;异种子互不可关联(域分离
    /// 子种子单射)+ 档内连贯(平台联动桶,见 derive_tables)。
    ///
    /// 不派生面(保持出厂默认形,与 shipped 预设一致):timezone(用户可配,任务
    /// 明示不派生)、locale(与 navigator.language 同为 en-US,双面一致既有不变量)、
    /// timing/battery/webrtc/plugin/speech/media_devices/permissions/webgl_context/
    /// connection/iframe。
    pub fn from_seed(seed: u64) -> Self {
        let cohort = cohort_for_seed(sub(seed, "platform"));
        let gpu: &GpuShape = pick(sub(seed, "webgl"), cohort.gpus);
        let screen_shape = pick(sub(seed, "screen"), cohort.screens);
        let nav_sub = sub(seed, "navigator");
        // deviceMemory 与 hardwareConcurrency 同属 navigator 面但物理独立
        // (核数≠内存),splitmix64 再推进一轮解关联,不做同子种子取模双用。
        let mut nav_mem_state = nav_sub;
        let nav_mem = splitmix64(&mut nav_mem_state);

        StealthProfile {
            tls: tls_candidate(sub(seed, "tls")),
            // H2 与 TLS 同族(Chrome 形;树内唯一 Chrome 真实形,firefox 形与
            // Chrome UA 同发即族间矛盾)。
            http2: Http2Fingerprint::chrome(),
            canvas: CanvasNoise::new(nonzero(sub(seed, "canvas"))),
            navigator: NavigatorProfile {
                user_agent: ua_string(cohort),
                platform: cohort.navigator_platform.into(),
                // locale 双面一致不变量:navigator.language ≡ LocaleConfig.locale
                // (profile.rs 既有测试钉的形;两处都不派生)。
                language: "en-US".into(),
                languages: vec!["en-US".into(), "en".into()],
                hardware_concurrency: *pick(nav_sub, HARDWARE_CONCURRENCY_POOL),
                max_touch_points: 0,
                vendor: "Google Inc.".into(),
                app_version: app_version_string(cohort),
                oscpu: None,
                build_id: None,
                product_sub: "20030107".into(),
                device_memory: *pick(nav_mem, DEVICE_MEMORY_POOL),
            },
            screen: ScreenProfile::new(
                screen_shape.width,
                screen_shape.height,
                screen_shape.device_pixel_ratio,
            ),
            webgl: WebGLProfile {
                vendor: format!("Google Inc. ({})", gpu.vendor_suffix),
                renderer: gpu.renderer.into(),
                extensions: WebGLProfile::chrome().extensions,
                max_texture_size: 16384,
                max_renderbuffer_size: 16384,
                max_viewport_dims: [16384, 16384],
            },
            audio: AudioProfile::new(sub(seed, "audio")),
            behavior: BehaviorSimulator::with_config(
                sub(seed, "behavior"),
                BehaviorConfig::chrome(),
            ),
            font: FontConfig::new(sub(seed, "font")),
            battery: Default::default(),
            webrtc_mode: Default::default(),
            timing: Default::default(),
            clientrects: ClientRectsConfig {
                noise_delta: 0.5,
                seed: sub(seed, "clientrects"),
            },
            screen_display: ScreenDisplayConfig {
                width: screen_shape.width,
                height: screen_shape.height,
                color_depth: 24,
                device_pixel_ratio: screen_shape.device_pixel_ratio,
            },
            plugin: Default::default(),
            speech: Default::default(),
            media_devices: Default::default(),
            permissions: Default::default(),
            webgl_context: Default::default(),
            connection: Default::default(),
            iframe: Default::default(),
            locale: Default::default(),
            timezone: Default::default(),
        }
    }

    /// OS 熵源单次采样 → [`StealthProfile::from_seed`]。bao_browser 显式 browser
    /// context 创建时的默认身份入口(每次调用产互异身份,~2^64 空间)。
    pub fn from_random_seed() -> Self {
        Self::from_seed(random_seed_from_os_entropy())
    }

    /// 进程级默认身份单例(用户裁决 2026-10-21:默认 = 随机身份,不可妥协点)。
    ///
    /// 懒初始化一次 `from_random_seed()`:每次**进程启动**随机(落实「随机非
    /// 固定」),进程内全部无显式配置的 page 共享同一套指纹(同启动内指纹稳定:
    /// 登录态/重载不破,popup 自然连贯)。**禁 per-page 各自随机** —— 站点内
    /// 指纹漂移本身就是检测向量。显式退出面见 `PageConfig::stealth_disabled`。
    pub fn default_identity() -> &'static StealthProfile {
        static DEFAULT_IDENTITY: OnceLock<StealthProfile> = OnceLock::new();
        DEFAULT_IDENTITY.get_or_init(StealthProfile::from_random_seed)
    }
}

/// OS 熵源随机 u64(std-only):`RandomState` 的 key 由进程启动期 OS 熵初始化、
/// 实例间单调步进 — 非密码学承诺,防关联场景(2^64 空间)足够。
pub(crate) fn random_seed_from_os_entropy() -> u64 {
    use std::hash::{BuildHasher, Hasher};
    let mut hasher = std::collections::hash_map::RandomState::new().build_hasher();
    hasher.write_u64(0xBA0F_00D5_EED0_0001); // 固定输入,避免零写入 finish 形态
    hasher.finish()
}

// @trace REQ-STL-007 [req:REQ-STL-007] [level:unit]
#[cfg(test)]
mod tests {
    use super::*;

    // ── 确定性(用户裁决:同种子跨重载/会话同指纹)──────────────────────────

    #[test]
    fn from_seed_is_deterministic_per_seed() {
        for seed in [0u64, 1, 42, 137, 0xDEAD_BEEF, u64::MAX] {
            let a = StealthProfile::from_seed(seed);
            let b = StealthProfile::from_seed(seed);
            // 深等:全字段 Debug 形等值(所有 config 均派生 Debug)。
            assert_eq!(
                format!("{a:?}"),
                format!("{b:?}"),
                "seed {seed} 必须逐字段确定"
            );
            // 关键字段点名(不依赖 Debug 形态细节)。
            assert_eq!(a.navigator.user_agent, b.navigator.user_agent);
            assert_eq!(a.navigator.platform, b.navigator.platform);
            assert_eq!(
                a.navigator.hardware_concurrency,
                b.navigator.hardware_concurrency
            );
            assert_eq!(a.navigator.device_memory, b.navigator.device_memory);
            assert_eq!(a.canvas.seed(), b.canvas.seed());
            assert_eq!(a.tls.ja3_hash, b.tls.ja3_hash);
            assert_eq!(a.tls.cipher_suites, b.tls.cipher_suites);
            assert_eq!(a.http2.akamai_fingerprint(), b.http2.akamai_fingerprint());
            assert_eq!(a.screen.width, b.screen.width);
            assert_eq!(a.screen.height, b.screen.height);
            assert_eq!(a.webgl.vendor, b.webgl.vendor);
            assert_eq!(a.webgl.renderer, b.webgl.renderer);
            assert_eq!(a.behavior.seed(), b.behavior.seed());
            assert_eq!(a.font.seed, b.font.seed);
            assert_eq!(a.clientrects.seed, b.clientrects.seed);
        }
    }

    // ── 发散(异种子互不可关联;域分离子种子构造性单射)────────────────────────

    #[test]
    fn sub_seed_is_injective_in_master_seed_per_domain() {
        for domain in [
            "canvas",
            "webgl",
            "audio",
            "font",
            "behavior",
            "clientrects",
            "platform",
            "screen",
            "navigator",
            "tls",
        ] {
            assert_ne!(sub(1, domain), sub(2, domain), "domain {domain}");
            assert_ne!(
                sub(u64::MAX, domain),
                sub(u64::MAX - 1, domain),
                "domain {domain}"
            );
            // 域间分离:同一 master 在不同域派生不同子种子(域标签混入)。
            assert_ne!(sub(7, domain), sub(7, "another-domain"));
        }
    }

    #[test]
    fn different_seeds_diverge_on_all_seed_domains() {
        let a = StealthProfile::from_seed(1);
        let b = StealthProfile::from_seed(2);
        assert_ne!(a.canvas.seed(), b.canvas.seed());
        assert_ne!(a.audio.seed(), b.audio.seed());
        assert_ne!(a.behavior.seed(), b.behavior.seed());
        assert_ne!(a.font.seed, b.font.seed);
        assert_ne!(a.clientrects.seed, b.clientrects.seed);
    }

    #[test]
    fn canvas_seed_is_never_zero() {
        // CanvasNoise::new 的 SPEC 断言;0 子种子必须被 nonzero 守卫接住。
        for seed in 0..=64u64 {
            assert!(
                StealthProfile::from_seed(seed).canvas.seed() > 0,
                "seed {seed} 派生 canvas seed 必须 > 0"
            );
        }
    }

    // ── 档内联动不变量(「近似就行」的正确性核心)────────────────────────────

    #[test]
    fn coherence_bucket_invariants_hold_across_seeds() {
        use crate::derive_tables::PlatformTier;
        let mut tiers_seen = std::collections::HashSet::new();
        for seed in 1..=300u64 {
            let profile = StealthProfile::from_seed(seed);
            let cohort = cohort_for_seed(sub(seed, "platform"));
            tiers_seen.insert(cohort.tier);

            let ua = profile.navigator.user_agent.as_str();
            let platform = profile.navigator.platform.as_str();
            let renderer = profile.webgl.renderer.as_str();
            let vendor = profile.webgl.vendor.as_str();

            // 档内三元组:UA token ⇔ navigator.platform ⇔ ua_data.platform。
            match cohort.tier {
                PlatformTier::Windows => { // @evidence .spec/10-REQUIREMENTS.html:1184
                    assert_eq!(platform, "Win32");
                    assert!(ua.contains("Windows NT"), "seed {seed}: {ua}");
                    assert_eq!(cohort.ua_data.platform, "Windows");
                }
                PlatformTier::Macos => { // @evidence .spec/10-REQUIREMENTS.html:1184
                    assert_eq!(platform, "MacIntel");
                    assert!(
                        ua.contains("Macintosh; Intel Mac OS X"),
                        "seed {seed}: {ua}"
                    );
                    assert_eq!(cohort.ua_data.platform, "macOS");
                }
                PlatformTier::Linux => { // @evidence .spec/10-REQUIREMENTS.html:1184
                    assert_eq!(platform, "Linux x86_64");
                    assert!(ua.contains("X11; Linux x86_64"), "seed {seed}: {ua}");
                    assert_eq!(cohort.ua_data.platform, "Linux");
                }
            }
            // 双向一致(不能只测单向,否则档位翻转漏检)。
            assert_eq!(ua.contains("Windows NT"), platform == "Win32");
            assert_eq!(renderer.contains("Direct3D11"), platform == "Win32");
            assert_eq!(renderer.contains("Metal"), platform == "MacIntel");
            assert_eq!(ua.contains("Chrome"), ua.contains("AppleWebKit"));

            // WebGL vendor/renderer 必须出自同档 GPU 池的同一对策展条目。
            let gpu = cohort
                .gpus
                .iter()
                .find(|g| g.renderer == renderer)
                .unwrap_or_else(|| panic!("seed {seed}: renderer 不在档内 GPU 池: {renderer}"));
            assert_eq!(vendor, format!("Google Inc. ({})", gpu.vendor_suffix));

            // screen 双源(ScreenProfile/ScreenDisplayConfig)一致且出自档内池。
            assert_eq!(profile.screen.width, profile.screen_display.width);
            assert_eq!(profile.screen.height, profile.screen_display.height);
            assert!(
                (profile.screen.device_pixel_ratio - profile.screen_display.device_pixel_ratio)
                    .abs()
                    < f64::EPSILON
            );
            assert!(
                cohort
                    .screens
                    .iter()
                    .any(|s| s.width == profile.screen.width && s.height == profile.screen.height),
                "seed {seed}: {}x{} 不在档内 screen 池",
                profile.screen.width,
                profile.screen.height
            );

            // navigator 档位约束 + locale 双面一致。
            assert!(DEVICE_MEMORY_POOL.contains(&profile.navigator.device_memory));
            assert!(HARDWARE_CONCURRENCY_POOL.contains(&profile.navigator.hardware_concurrency));
            assert_eq!(profile.navigator.language, profile.locale.locale);
        }
        assert_eq!(
            tiers_seen.len(),
            3,
            "300 个种子必须三档全命中(实际 {tiers_seen:?})"
        );
    }

    // ── TLS/H2 面:池内真实形选型(禁止逐字段随机的缝合怪)────────────────────

    #[test]
    fn tls_face_is_pooled_real_chrome_shape() {
        let mut saw_120 = false;
        let mut saw_latest = false;
        for seed in 1..=100u64 {
            let profile = StealthProfile::from_seed(seed);
            let real_120 = TlsFingerprint::chrome_120();
            let real_latest = TlsFingerprint::chrome_latest();
            let is_120 = profile.tls.ja3_hash == real_120.ja3_hash
                && profile.tls.cipher_suites == real_120.cipher_suites
                && profile.tls.extensions == real_120.extensions
                && profile.tls.signature_algorithms == real_120.signature_algorithms
                && profile.tls.supported_groups == real_120.supported_groups
                && profile.tls.record_size_limit == real_120.record_size_limit
                && profile.tls.compress_certificate_algos == real_120.compress_certificate_algos
                && profile.tls.application_settings_protocol
                    == real_120.application_settings_protocol;
            let is_latest = profile.tls.ja3_hash == real_latest.ja3_hash
                && profile.tls.cipher_suites == real_latest.cipher_suites
                && profile.tls.extensions == real_latest.extensions
                && profile.tls.record_size_limit == real_latest.record_size_limit
                && profile.tls.compress_certificate_algos == real_latest.compress_certificate_algos
                && profile.tls.application_settings_protocol
                    == real_latest.application_settings_protocol;
            assert!(
                is_120 ^ is_latest,
                "seed {seed}: TLS 必须恰为池内一条真实形"
            );
            saw_120 |= is_120;
            saw_latest |= is_latest;

            // H2 与 TLS 同族(Chrome 形,无 PRIORITY 帧)。
            assert_eq!(
                profile.http2.pseudo_header_order,
                Http2Fingerprint::chrome().pseudo_header_order
            );
            assert!(!profile.http2.sends_priority_frames());
        }
        assert!(saw_120 && saw_latest, "池内两条真实形都要被选到");
    }

    #[test]
    fn tls_pool_is_family_constrained_real_shapes_only() {
        // 族标记判别(资料库「确证性锚点」节):Chrome 形 = 首 3 序
        // 4865-4866-4867 且无 FF-only DHE-GCM 158;Firefox 形 = 首 3 序
        // 4865-4867-4866 且 158 在场。跨族拼装在数据层结构性不可能。
        let chrome_markers = |fp: &TlsFingerprint| {
            fp.cipher_suites[..3] == [0x1301, 0x1302, 0x1303] && !fp.cipher_suites.contains(&0x009E)
        };
        let firefox_markers = |fp: &TlsFingerprint| {
            fp.cipher_suites[..3] == [0x1301, 0x1303, 0x1302] && fp.cipher_suites.contains(&0x009E)
        };
        for ctor in tls_pool(UaFamily::Chrome) {
            assert!(chrome_markers(&ctor()), "Chrome 池成员必须带 Chrome 族标记");
        }
        for ctor in tls_pool(UaFamily::Firefox) {
            assert!(
                firefox_markers(&ctor()),
                "Firefox 池成员必须带 Firefox 族标记"
            );
        }
        // 族间零交集:同一条真实形不得同时出现在两个族池。
        for chrome in tls_pool(UaFamily::Chrome) {
            for firefox in tls_pool(UaFamily::Firefox) {
                assert_ne!(chrome().ja3_hash, firefox().ja3_hash);
            }
        }
    }

    #[test]
    fn tls_pool_gap_registration_pins_honest_count() {
        // 池浅缺口钉(2026-10-21):Chrome 族真实形 = 2(扩池目标 ≥6,缺口 =
        // 树内/资料库无 edge/safari 据可查,禁发明)。扩池时更新此钉并补族
        // 标记断言(见 tls_pool doc 注释的取证配方)。
        assert_eq!(tls_pool(UaFamily::Chrome).len(), 2);
        assert_eq!(tls_pool(UaFamily::Firefox).len(), 1);
    }

    #[test]
    fn tls_candidate_always_lands_in_family_pool() {
        for seed in 0..200u64 {
            let fp = tls_candidate(sub(seed, "tls"));
            assert!(
                tls_pool(UaFamily::Chrome).iter().any(|ctor| {
                    let real = ctor();
                    real.ja3_hash == fp.ja3_hash && real.cipher_suites == fp.cipher_suites
                }),
                "seed {seed}: TLS 选型必须落在 Chrome 族池内"
            );
        }
    }

    // ── WebGL vendor↔renderer 配对完整性 ─────────────────────────────────────

    #[test]
    fn webgl_vendor_matches_declared_gpu_vendor() {
        for seed in 1..=100u64 {
            let profile = StealthProfile::from_seed(seed);
            let vendor = profile.webgl.vendor.as_str();
            let renderer = profile.webgl.renderer.as_str();
            let coherent = vendor.contains("Intel") && renderer.contains("Intel")
                || vendor.contains("NVIDIA") && renderer.contains("NVIDIA")
                || vendor.contains("AMD") && renderer.contains("AMD")
                || vendor.contains("Apple") && renderer.contains("Apple");
            assert!(
                coherent,
                "seed {seed}: vendor({vendor}) 与 renderer({renderer}) 必须同厂商"
            );
        }
    }

    // ── 随机身份入口(OS 熵)─────────────────────────────────────────────────

    #[test]
    fn from_random_seed_yields_unrelated_profiles() {
        let a = StealthProfile::from_random_seed();
        let b = StealthProfile::from_random_seed();
        assert_ne!(
            a.canvas.seed(),
            b.canvas.seed(),
            "两次 OS 熵采样必须产出互异身份(碰撞 ~2^-64)"
        );
        assert_ne!(a.clientrects.seed, b.clientrects.seed);
        assert!(a.canvas.seed() > 0);
    }

    #[test]
    fn from_random_seed_matches_coherence_invariants() {
        // 随机入口与固定种子入口走同一派生管线,不变量必须同守。
        for _ in 0..16 {
            let profile = StealthProfile::from_random_seed();
            let ua = profile.navigator.user_agent.clone();
            let platform = profile.navigator.platform.clone();
            assert_eq!(ua.contains("Windows NT"), platform == "Win32");
            assert!(ua.contains("Chrome"));
            assert!(DEVICE_MEMORY_POOL.contains(&profile.navigator.device_memory));
        }
    }

    // ── 进程级默认身份单例(用户裁决 2026-10-21)──────────────────────────────

    #[test]
    fn default_identity_is_process_shared_and_stable() {
        // 同进程重复取用 → 同一套指纹(同启动内指纹稳定:登录态/重载不破;
        // popup/多页共享,禁 per-page 各自随机)。
        let a = StealthProfile::default_identity();
        let b = StealthProfile::default_identity();
        assert!(std::ptr::eq(a, b), "default_identity 必须是同一单例引用");
        assert_eq!(format!("{a:?}"), format!("{b:?}"));
        assert_eq!(a.canvas.seed(), b.canvas.seed());
        assert!(a.canvas.seed() > 0);
    }

    #[test]
    fn default_identity_holds_coherence_invariants() {
        let profile = StealthProfile::default_identity();
        let ua = profile.navigator.user_agent.as_str();
        let platform = profile.navigator.platform.as_str();
        // 默认身份走同一派生管线:档内联动不变量同守(Chrome 形)。
        assert!(ua.contains("Chrome"));
        assert_eq!(ua.contains("Windows NT"), platform == "Win32");
        assert!(DEVICE_MEMORY_POOL.contains(&profile.navigator.device_memory));
        assert!(HARDWARE_CONCURRENCY_POOL.contains(&profile.navigator.hardware_concurrency));
    }

    // ── 子种子原语 ───────────────────────────────────────────────────────────

    #[test]
    fn splitmix64_is_deterministic_and_state_advances() {
        let mut state = 42u64;
        let a = splitmix64(&mut state);
        let b = splitmix64(&mut state);
        assert_ne!(a, b, "同 state 两次推进必须产出不同输出");
        let mut state2 = 42u64;
        assert_eq!(a, splitmix64(&mut state2), "同初值必须同流");
    }
}
