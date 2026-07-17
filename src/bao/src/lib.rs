//! # Bao — unified library
//!
//! Single consumer-facing package. Full stack is **always linked**:
//! SpiderMonkey (`bao_engine`) + servo browser (`bao_browser`) + Node/Bun API
//! (`bun_runtime`) + CDP (`bao_cdp` / `bao_cdp_client`) + Stealth (`bao_stealth`).
//!
//! There are **no** Cargo product features that disable browser, CDP, stealth,
//! or Node API. Runtime knobs (e.g. [`StealthProfile`], permissions) select
//! behaviour without changing the link set.
//!
//! ## Quick start
//!
//! ```no_run
//! use bao::{BaoConfig, BaoRuntime, PageConfig, StealthProfile};
//!
//! fn main() -> Result<(), bao::BrowserError> {
//!     let runtime = BaoRuntime::new(BaoConfig::default())?;
//!     let _pool = runtime.page_pool();
//!     let _ = StealthProfile::firefox_default();
//!     let _ = PageConfig::default();
//!     Ok(())
//! }
//! ```
//!
//! ## CDP (Playwright-style)
//!
//! ```no_run
//! use bao::Browser;
//!
//! fn main() -> Result<(), bao::ConnectError> {
//!     let browser = Browser::connect("memory://bao")?;
//!     let _ = browser;
//!     Ok(())
//! }
//! ```
//!
//! @trace REQ-LIB-001 [level:library]
//! @trace REQ-LIB-003 [level:library]
//! @trace REQ-BRW-003 [level:library]

#![allow(unused_imports)]

// Force-link C stubs so consumer binaries do not GC native symbols.
#[used]
static BAO_NATIVE_STUBS_ANCHOR: unsafe extern "C" fn() = bao_native_stubs::__force_link_entry;

// ── Namespaced full surfaces (always available) ───────────────────────────

/// Browser runtime, PagePool, PageHandle, permissions, screenshots.
pub mod browser {
    pub use bao_browser::*;
}

/// SpiderMonkey engine surface (via `bao_engine` re-exports).
pub mod engine {
    pub use bao_engine::*;
}

/// Node.js / Bun API compatibility runtime (`bun_runtime` crate).
///
/// Note: this module also defines a `BaoRuntime` type that is **not** the same
/// as the top-level [`crate::BaoRuntime`] (browser coordinator). Prefer the
/// top-level name for embedding; use `bao::runtime::` for Node/Bun host setup.
pub mod runtime {
    pub use bun_runtime::*;
}

/// CDP server / router / WS codec surface.
pub mod cdp {
    pub use bao_cdp::*;
}

/// Playwright-style CDP client (`Browser::connect`, Page, …).
pub mod cdp_client {
    pub use bao_cdp_client::*;
}

/// Anti-fingerprint profiles and engine (runtime configuration).
pub mod stealth {
    pub use bao_stealth::*;
}

/// Event loop (epoll tick shared with FilePoll).
pub mod uloop {
    pub use bao_uloop::*;
}

// ── Stable top-level re-exports (consumer happy path) ─────────────────────
// Prefer these over depending on internal crate paths.

// Browser embedding (primary BaoRuntime)
pub use bao_browser::{
    encode_image, run_browser, BaoConfig, BaoRuntime, BrowserConfig, BrowserError, PageConfig,
    PageHandle, PagePool, PageState, Permission, PermissionDenied, PermissionGuard,
    ScreenshotFormat,
};

// Stealth (always linked; enable via profile at runtime)
pub use bao_stealth::{
    BehaviorConfig, BehaviorSimulator, CanvasNoise, FontConfig, Http2Fingerprint, NavigatorProfile,
    ScreenProfile, StealthEngine, StealthHooks, StealthProfile, StealthTlsWireConfig, TlsFingerprint,
    WebGLProfile, AudioProfile,
};

// CDP client entry
pub use bao_cdp_client::{
    Browser, CdpError, ConnectError, Connection, ConnectionConfig, Cookie, DeviceDescriptor,
    Viewport, WaitUntilState,
};

// CDP server types commonly needed alongside the client
pub use bao_cdp::{BackendKind, CdpRouter, CdpServer, CdpSession};

#[cfg(test)]
mod tests {
    use super::*;

    /// Packaging contract: the public crate always declares the full stack.
    /// This drives the shipped `Cargo.toml`, not a hard-coded shadow list alone —
    /// every required dep name must appear in the real manifest text.
    #[test]
    fn cargo_toml_always_depends_on_full_stack() {
        let manifest = include_str!("../Cargo.toml");
        let required = [
            "bao_browser",
            "bao_engine",
            "bun_runtime",
            "bao_cdp",
            "bao_cdp_client",
            "bao_stealth",
            "bao_uloop",
            "bao_native_stubs",
        ];
        for dep in required {
            assert!(
                manifest.contains(dep),
                "public package must always depend on {dep} (unified full stack)"
            );
        }
        // No product feature table that could disable capabilities.
        assert!(
            !manifest.contains("[features]"),
            "public package must not define Cargo [features] for product capability splits"
        );
    }

    /// Real API path: Stealth is linked and constructs a default profile.
    #[test]
    fn stealth_profile_firefox_default_is_available() {
        let profile = StealthProfile::firefox_default();
        let engine = StealthEngine::new(profile);
        // Touch fields that exist on the real shipped type.
        let _tls = engine.tls_config();
        let _nav = engine.navigator();
        assert!(std::mem::size_of_val(engine.profile()) > 0);
    }

    /// Real API path: browser config types are constructible without spinning servo
    /// (full Servo init is env-heavy; config path still exercises shipped constructors).
    #[test]
    fn browser_config_defaults_construct() {
        let cfg = BaoConfig::default();
        let page = PageConfig::default();
        let _ = (cfg, page, ScreenshotFormat::Png);
        // Type identity: top-level BaoRuntime is the browser coordinator.
        let _name = std::any::type_name::<BaoRuntime>();
        assert!(_name.contains("BaoRuntime"));
    }

    /// CDP client Browser type is part of the public surface (connect needs runtime).
    #[test]
    fn cdp_browser_type_is_reexported() {
        let name = std::any::type_name::<Browser>();
        assert!(name.contains("Browser"));
        let _ = std::any::type_name::<ConnectError>();
        let _ = std::any::type_name::<CdpRouter>();
        let _ = std::any::type_name::<CdpServer>();
    }

    /// Namespaced modules expose the same always-on crates.
    #[test]
    fn namespaced_modules_resolve_core_types() {
        let _ = std::any::type_name::<browser::PagePool>();
        let _ = std::any::type_name::<stealth::StealthEngine>();
        let _ = std::any::type_name::<cdp_client::Browser>();
        let _ = std::any::type_name::<cdp::CdpRouter>();
    }
}
