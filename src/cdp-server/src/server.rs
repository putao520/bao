// @trace REQ-CDS-001 [entity:CdpServer]
// @trace REQ-CDS-002 [entity:CdpTarget]
// @trace REQ-CDS-003 [entity:CdpSessionGeneric]
// @trace REQ-CDS-007 [entity:CdpServer]
// CdpServer main event loop: TCP accept, HTTP discovery, WS upgrade,
// command routing, target management.

use std::collections::HashMap;
use std::io::Read;
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tungstenite::accept;

use crate::event::EventBroadcaster;
use crate::registry::SharedRegistry;
use crate::session::{CdpSession, ReplayStream};
use crate::transport::{self, TargetInfo};
use crate::{EventSender, ServerConfig, TargetProvider};

pub struct CdpServer {
    config: ServerConfig,
    registry: SharedRegistry,
    target_provider: Option<Arc<dyn TargetProvider>>,
    broadcaster: Arc<EventBroadcaster>,
    sessions: Arc<Mutex<HashMap<String, Arc<Mutex<CdpSession>>>>>,
}

impl CdpServer {
    pub fn new(config: ServerConfig) -> Self {
        let sessions = Arc::new(Mutex::new(HashMap::new()));
        let broadcaster = Arc::new(EventBroadcaster::new(Arc::clone(&sessions)));
        CdpServer {
            config,
            registry: Arc::new(crate::registry::DomainRegistry::new()),
            target_provider: None,
            broadcaster,
            sessions,
        }
    }

    pub fn registry(&self) -> &SharedRegistry {
        &self.registry
    }

    pub fn broadcaster(&self) -> Arc<EventBroadcaster> {
        Arc::clone(&self.broadcaster)
    }

    pub fn set_target_provider(&mut self, provider: Arc<dyn TargetProvider>) {
        self.target_provider = Some(provider);
    }

    pub fn port(&self) -> u16 {
        self.config.port
    }

    pub fn ws_url_for_target(&self, target_id: &str) -> String {
        format!(
            "ws://{}:{}/devtools/page/{}",
            self.config.host, self.config.port, target_id
        )
    }

    /// Main event loop. Blocks until shutdown.
    pub fn run(&mut self) -> Result<(), String> {
        let addr = format!("{}:{}", self.config.host, self.config.port);
        let listener = TcpListener::bind(&addr).map_err(|e| format!("bind: {}", e))?;
        listener
            .set_nonblocking(true)
            .map_err(|e| format!("nonblocking: {}", e))?;

        eprintln!("CDP listening on ws://{}:{}", self.config.host, self.config.port);

        loop {
            // Drain session events (not used currently, but placeholder for future command channel).
            self.check_session_timeouts();

            // Accept new connections.
            match listener.accept() {
                Ok((stream, _addr)) => {
                    self.handle_connection(stream);
                }
                Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {}
                Err(e) => eprintln!("CDP accept error: {}", e),
            }

            // Process existing sessions.
            let mut to_remove = Vec::new();
            {
                let sessions = self.sessions.lock().map_err(|e| format!("lock: {}", e))?;
                for (id, session) in sessions.iter() {
                    let mut session = match session.lock() {
                        Ok(s) => s,
                        Err(_) => continue,
                    };
                    let event_sender: Box<dyn EventSender> = self.broadcaster.sender();
                    if session.process(&self.registry, event_sender.as_ref()).is_err() {
                        to_remove.push(id.clone());
                        let domains = session.enabled_domains();
                        let sid = session.session_id().to_string();
                        session.begin_close();
                        drop(session);
                        self.registry.notify_session_destroyed(&domains, &sid);
                    }
                }
            }

            for id in to_remove {
                if let Ok(mut sessions) = self.sessions.lock() {
                    if let Some(session) = sessions.remove(&id) {
                        if let Ok(mut s) = session.lock() {
                            s.finalize();
                        }
                    }
                }
            }

            std::thread::sleep(Duration::from_millis(10));
        }
    }

    fn handle_connection(&self, mut stream: TcpStream) {
        let mut buf = [0u8; 8192];
        stream.set_nonblocking(false).ok();
        let n = match stream.read(&mut buf) {
            Ok(n) if n > 0 => n,
            _ => return,
        };
        let request = match std::str::from_utf8(&buf[..n]) {
            Ok(s) => s,
            Err(_) => return,
        };

        // Check for close/activate/new before general handling.
        if let Some(target_id) = transport::parse_close_request(request) {
            if let Some(ref provider) = self.target_provider {
                match provider.close_target(&target_id) {
                    Ok(()) => {
                        transport::respond_json(
                            &mut stream,
                            &serde_json::json!({"success": true, "targetId": target_id}),
                        );
                        // Broadcast Target.targetDestroyed event.
                        self.broadcaster.send_event(
                            "Target.targetDestroyed",
                            serde_json::json!({"targetId": target_id}),
                        );
                    }
                    Err(e) => {
                        transport::respond_raw(&mut stream, &format!("500 {}", e));
                    }
                }
            } else {
                transport::respond_raw(&mut stream, "500 No target provider");
            }
            return;
        }

        if let Some(target_id) = transport::parse_activate_request(request) {
            if let Some(ref provider) = self.target_provider {
                match provider.activate_target(&target_id) {
                    Ok(()) => transport::respond_raw(&mut stream, "Target activated"),
                    Err(e) => transport::respond_raw(&mut stream, &format!("500 {}", e)),
                }
            }
            return;
        }

        if let Some(url) = transport::parse_new_request(request) {
            if let Some(ref provider) = self.target_provider {
                match provider.create_target(&url) {
                    Ok(info) => {
                        let json = serde_json::to_value(&info).unwrap_or_default();
                        transport::respond_json(&mut stream, &json);
                    }
                    Err(e) => {
                        transport::respond_raw(&mut stream, &format!("500 {}", e));
                    }
                }
            }
            return;
        }

        // GET /json/version and /json/list
        if request.starts_with("GET /json/version") || (request.starts_with("GET /json") && !request.starts_with("GET /json/")) {
            let targets = self.get_target_list();
            transport::handle_http_request(&mut stream, request, &self.config, &targets);
            return;
        }

        // WebSocket upgrade.
        if request.contains("Upgrade: websocket") || request.contains("upgrade: websocket") {
            let (target_id, is_browser) = if let Some(rest) = request.strip_prefix("GET /devtools/page/") {
                (rest.split(' ').next().unwrap_or("").to_string(), false)
            } else if request.starts_with("GET /devtools/browser") {
                ("__browser__".to_string(), true)
            } else {
                return;
            };

            let replay = ReplayStream::new(stream, buf[..n].to_vec());
            let ws = match accept(replay) {
                Ok(ws) => ws,
                Err(e) => {
                    eprintln!("CDP WebSocket accept error: {}", e);
                    return;
                }
            };

            let session_id = generate_session_id();
            let session = CdpSession::new(session_id.clone(), target_id, ws, is_browser);
            let session_count = self.sessions.lock().map(|m| m.len()).unwrap_or(0);
            if session_count >= self.config.max_sessions {
                eprintln!("CDP max sessions reached, rejecting");
                return;
            }
            if let Ok(mut sessions) = self.sessions.lock() {
                sessions.insert(session_id, Arc::new(Mutex::new(session)));
            }
        } else {
            transport::respond_raw(&mut stream, "HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\n\r\n");
        }
    }

    fn get_target_list(&self) -> Vec<TargetInfo> {
        if let Some(ref provider) = self.target_provider {
            provider.list_targets()
        } else {
            Vec::new()
        }
    }

    fn check_session_timeouts(&self) {
        // Placeholder for future session timeout management.
    }
}

fn generate_session_id() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let d = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default();
    let ns = d.as_nanos() as u64;
    format!("{:016x}", ns ^ (ns >> 17) ^ (ns >> 35))
}

// ---------------------------------------------------------------------------
// § Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cdp_server_config_stores_host_port_browser_name() {
        let config = ServerConfig {
            host: "127.0.0.1".into(),
            port: 9222,
            browser_name: "Bao/0.1.0".into(),
            ..Default::default()
        };
        let server = CdpServer::new(config);
        assert_eq!(server.port(), 9222);
    }

    #[test]
    fn server_config_default_values() {
        let config = ServerConfig::default();
        assert_eq!(config.host, "127.0.0.1");
        assert_eq!(config.port, 9222);
        assert_eq!(config.http_timeout_seconds, 30);
        assert_eq!(config.max_sessions, 100);
        assert_eq!(config.browser_name, "Bao/0.1.0");
        assert_eq!(config.protocol_version, "1.3");
        assert!(config.user_agent.is_none());
        assert!(config.v8_version.is_none());
        assert!(config.webkit_version.is_none());
    }

    #[test]
    fn server_config_builder_pattern() {
        let config = ServerConfig::builder()
            .host("0.0.0.0")
            .port(9333)
            .http_timeout_seconds(60)
            .max_sessions(200)
            .browser_name("TestBrowser/1.0")
            .user_agent("TestAgent")
            .v8_version("12.0")
            .webkit_version("602.1")
            .build();
        assert_eq!(config.host, "0.0.0.0");
        assert_eq!(config.port, 9333);
        assert_eq!(config.http_timeout_seconds, 60);
        assert_eq!(config.max_sessions, 200);
        assert_eq!(config.browser_name, "TestBrowser/1.0");
        assert_eq!(config.user_agent, Some("TestAgent".into()));
        assert_eq!(config.v8_version, Some("12.0".into()));
        assert_eq!(config.webkit_version, Some("602.1".into()));
    }

    #[test]
    fn ws_url_format_contains_host_port() {
        let config = ServerConfig {
            host: "127.0.0.1".into(),
            port: 9222,
            ..Default::default()
        };
        let server = CdpServer::new(config);
        let ws_url = server.ws_url_for_target("abc123");
        assert!(ws_url.starts_with("ws://127.0.0.1:9222/devtools/page/"));
        assert!(ws_url.ends_with("abc123"));
    }

    #[test]
    fn generate_session_id_format() {
        let id = generate_session_id();
        assert_eq!(id.len(), 16);
        assert!(id.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn cdp_server_has_registry_and_broadcaster() {
        let server = CdpServer::new(ServerConfig::default());
        let _registry = server.registry();
        let _broadcaster = server.broadcaster();
    }
}
