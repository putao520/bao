// @trace REQ-CDS-006 [entity:DomainRegistry] [entity:ServerConfig]
// cdp-server — Generic CDP (Chrome DevTools Protocol) server framework.
// Transport layer (HTTP discovery + WebSocket) + session management +
// message routing + event broadcast + domain handler registry.
// Zero knowledge of any browser engine.

use serde_json::Value;

mod protocol;
mod registry;
mod event;
mod session;
mod transport;
mod server;

pub use protocol::{CdpMessage, CdpResponse, CdpError, CdpEvent, SessionError};
pub use registry::DomainRegistry;
pub use event::EventBroadcaster;
pub use session::{CdpSession, SessionState};
pub use server::CdpServer;
pub use transport::{
    TargetInfo, parse_close_request, parse_activate_request, parse_new_request,
    is_websocket_upgrade,
};

// ---------------------------------------------------------------------------
// §2.1 DomainHandler Trait
// ---------------------------------------------------------------------------

/// CDP Domain handler trait. Each implementation handles one CDP domain
/// (e.g. Page, Runtime, DOM). All browser-specific logic lives here.
///
/// Constraints: `Send + Sync` (cross-thread safe).
pub trait DomainHandler: Send + Sync {
    /// Returns the CDP domain name (e.g. "Page", "Runtime").
    fn domain_name(&self) -> &'static str;

    /// Handle a CDP command. `command` is the full method string
    /// (e.g. "Page.navigate"). `params` is the JSON params object.
    fn handle_command(
        &self,
        command: &str,
        params: Value,
        event_sender: &dyn EventSender,
    ) -> Result<Value, CdpError>;

    /// Called when a session enables this domain for the first time.
    fn on_session_created(&self, _session_id: &str) {}

    /// Called when a session is destroyed (while this domain was enabled).
    fn on_session_destroyed(&self, _session_id: &str) {}
}

// ---------------------------------------------------------------------------
// §2.2 EventSender Trait
// ---------------------------------------------------------------------------

/// Event sender trait. Implemented internally by cdp-server, injected into
/// DomainHandlers so they can broadcast CDP events.
///
/// Constraints: `Send + Sync + Clone`.
pub trait EventSender: Send + Sync {
    /// Broadcast an event to all sessions that have enabled the domain
    /// extracted from `method` (format: "Domain.eventName").
    fn send_event(&self, method: &str, params: Value);
}

// ---------------------------------------------------------------------------
// §2.3 TargetProvider Trait
// ---------------------------------------------------------------------------

/// Browser target manager trait. Implemented by the backend (e.g. bao_cdp)
/// to provide target discovery/creation/closure.
///
/// Constraints: `Send + Sync`.
pub trait TargetProvider: Send + Sync {
    /// List all available browser targets.
    fn list_targets(&self) -> Vec<TargetInfo>;

    /// Create a new target (open a new page).
    fn create_target(&self, url: &str) -> Result<TargetInfo, String>;

    /// Close the specified target.
    fn close_target(&self, target_id: &str) -> Result<(), String>;

    /// Activate (bring to front) the specified target.
    fn activate_target(&self, target_id: &str) -> Result<(), String>;
}

// ---------------------------------------------------------------------------
// §8 ServerConfig Entity
// ---------------------------------------------------------------------------

/// CDP server configuration. Controls bind address, timeouts, concurrency
/// limits and version strings.
pub struct ServerConfig {
    pub host: String,
    pub port: u16,
    pub http_timeout_seconds: u64,
    pub max_sessions: usize,
    pub browser_name: String,
    pub protocol_version: String,
    pub user_agent: Option<String>,
    pub v8_version: Option<String>,
    pub webkit_version: Option<String>,
}

impl Default for ServerConfig {
    fn default() -> Self {
        ServerConfig {
            host: "127.0.0.1".into(),
            port: 9222,
            http_timeout_seconds: 30,
            max_sessions: 100,
            browser_name: "Bao/0.1.0".into(),
            protocol_version: "1.3".into(),
            user_agent: None,
            v8_version: None,
            webkit_version: None,
        }
    }
}

impl ServerConfig {
    pub fn builder() -> ServerConfigBuilder {
        ServerConfigBuilder::default()
    }
}

#[derive(Default)]
pub struct ServerConfigBuilder {
    inner: ServerConfig,
}


impl ServerConfigBuilder {
    pub fn host(mut self, host: impl Into<String>) -> Self {
        self.inner.host = host.into();
        self
    }

    pub fn port(mut self, port: u16) -> Self {
        self.inner.port = port;
        self
    }

    pub fn http_timeout_seconds(mut self, seconds: u64) -> Self {
        self.inner.http_timeout_seconds = seconds;
        self
    }

    pub fn max_sessions(mut self, max: usize) -> Self {
        self.inner.max_sessions = max;
        self
    }

    pub fn browser_name(mut self, name: impl Into<String>) -> Self {
        self.inner.browser_name = name.into();
        self
    }

    pub fn user_agent(mut self, ua: impl Into<String>) -> Self {
        self.inner.user_agent = Some(ua.into());
        self
    }

    pub fn v8_version(mut self, ver: impl Into<String>) -> Self {
        self.inner.v8_version = Some(ver.into());
        self
    }

    pub fn webkit_version(mut self, ver: impl Into<String>) -> Self {
        self.inner.webkit_version = Some(ver.into());
        self
    }

    pub fn build(self) -> ServerConfig {
        self.inner
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // -- ServerConfig defaults --

    #[test]
    fn server_config_default_host_is_127_0_0_1() {
        assert_eq!(ServerConfig::default().host, "127.0.0.1");
    }

    #[test]
    fn server_config_default_port_is_9222() {
        assert_eq!(ServerConfig::default().port, 9222);
    }

    #[test]
    fn server_config_default_timeout_is_30() {
        assert_eq!(ServerConfig::default().http_timeout_seconds, 30);
    }

    #[test]
    fn server_config_default_max_sessions_is_100() {
        assert_eq!(ServerConfig::default().max_sessions, 100);
    }

    #[test]
    fn server_config_default_browser_name_is_Bao() {
        assert_eq!(ServerConfig::default().browser_name, "Bao/0.1.0");
    }

    #[test]
    fn server_config_default_protocol_version_is_1_3() {
        assert_eq!(ServerConfig::default().protocol_version, "1.3");
    }

    #[test]
    fn server_config_default_user_agent_is_none() {
        assert!(ServerConfig::default().user_agent.is_none());
    }

    #[test]
    fn server_config_default_v8_version_is_none() {
        assert!(ServerConfig::default().v8_version.is_none());
    }

    #[test]
    fn server_config_default_webkit_version_is_none() {
        assert!(ServerConfig::default().webkit_version.is_none());
    }

    // -- ServerConfigBuilder setters --

    #[test]
    fn builder_sets_host() {
        assert_eq!(ServerConfig::builder().host("0.0.0.0").build().host, "0.0.0.0");
    }

    #[test]
    fn builder_sets_port() {
        assert_eq!(ServerConfig::builder().port(8080).build().port, 8080);
    }

    #[test]
    fn builder_sets_timeout() {
        assert_eq!(ServerConfig::builder().http_timeout_seconds(60).build().http_timeout_seconds, 60);
    }

    #[test]
    fn builder_sets_max_sessions() {
        assert_eq!(ServerConfig::builder().max_sessions(50).build().max_sessions, 50);
    }

    #[test]
    fn builder_sets_browser_name() {
        assert_eq!(ServerConfig::builder().browser_name("Chrome/120").build().browser_name, "Chrome/120");
    }

    #[test]
    fn builder_sets_user_agent() {
        let ua = ServerConfig::builder().user_agent("Mozilla/5.0").build().user_agent;
        assert_eq!(ua.as_deref(), Some("Mozilla/5.0"));
    }

    #[test]
    fn builder_sets_v8_version() {
        let ver = ServerConfig::builder().v8_version("12.0").build().v8_version;
        assert_eq!(ver.as_deref(), Some("12.0"));
    }

    #[test]
    fn builder_sets_webkit_version() {
        let ver = ServerConfig::builder().webkit_version("537.36").build().webkit_version;
        assert_eq!(ver.as_deref(), Some("537.36"));
    }

    #[test]
    fn builder_chaining_all_fields() {
        let cfg = ServerConfig::builder()
            .host("0.0.0.0")
            .port(9223)
            .http_timeout_seconds(120)
            .max_sessions(200)
            .browser_name("TestBrowser")
            .user_agent("TestAgent")
            .v8_version("13.0")
            .webkit_version("600.0")
            .build();
        assert_eq!(cfg.host, "0.0.0.0");
        assert_eq!(cfg.port, 9223);
        assert_eq!(cfg.http_timeout_seconds, 120);
        assert_eq!(cfg.max_sessions, 200);
        assert_eq!(cfg.browser_name, "TestBrowser");
        assert_eq!(cfg.user_agent.as_deref(), Some("TestAgent"));
        assert_eq!(cfg.v8_version.as_deref(), Some("13.0"));
        assert_eq!(cfg.webkit_version.as_deref(), Some("600.0"));
    }

    #[test]
    fn builder_default_then_build_equals_default_config() {
        let built = ServerConfig::builder().build();
        let default = ServerConfig::default();
        assert_eq!(built.host, default.host);
        assert_eq!(built.port, default.port);
        assert_eq!(built.http_timeout_seconds, default.http_timeout_seconds);
        assert_eq!(built.max_sessions, default.max_sessions);
        assert_eq!(built.browser_name, default.browser_name);
        assert_eq!(built.protocol_version, default.protocol_version);
        assert_eq!(built.user_agent, default.user_agent);
        assert_eq!(built.v8_version, default.v8_version);
        assert_eq!(built.webkit_version, default.webkit_version);
    }
}
