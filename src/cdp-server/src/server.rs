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
    /// Receiver for console messages forwarded from servo delegates.
    /// Each message is (level, text) — e.g. ("info", "hello world").
    console_rx: Option<std::sync::mpsc::Receiver<(String, String)>>,
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
            console_rx: None,
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

    /// Set the console message receiver. Messages are (level, text) tuples
    /// forwarded from servo's show_console_message callbacks.
    pub fn set_console_receiver(&mut self, rx: std::sync::mpsc::Receiver<(String, String)>) {
        self.console_rx = Some(rx);
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

            // Drain console messages from servo delegates and broadcast as CDP events.
            // Special prefixes are routed to domain-specific events:
            //   __BAO_FETCH_INTERCEPT__ → Fetch.requestPaused
            //   __BAO_NETWORK_REQUEST__ → Network.requestWillBeSent
            //   __BAO_NETWORK_RESPONSE__ → Network.responseReceived
            //   __BAO_NETWORK_LOADING_FAILED__ → Network.loadingFailed
            //   __BAO_DEBUGGER_SCRIPT__ → Debugger.scriptParsed
            // All others → Log.entryAdded
            if let Some(ref rx) = self.console_rx {
                while let Ok((level, text)) = rx.try_recv() {
                    if let Some(payload) = text.strip_prefix("__BAO_FETCH_INTERCEPT__") {
                        if let Ok(info) = serde_json::from_str::<serde_json::Value>(payload) {
                            self.broadcaster.send_event(
                                "Fetch.requestPaused",
                                serde_json::json!({
                                    "requestId": info["id"],
                                    "request": {
                                        "url": info["url"],
                                        "method": info["method"],
                                        "headers": info.get("headers").unwrap_or(&serde_json::json!({})),
                                    },
                                    "resourceType": info.get("resourceType").unwrap_or(&serde_json::json!("Other")),
                                    "networkStage": "Request",
                                }),
                            );
                        }
                    } else if let Some(payload) = text.strip_prefix("__BAO_NETWORK_REQUEST__") {
                        if let Ok(info) = serde_json::from_str::<serde_json::Value>(payload) {
                            self.broadcaster.send_event(
                                "Network.requestWillBeSent",
                                serde_json::json!({
                                    "requestId": info["id"],
                                    "request": info.get("request").cloned().unwrap_or(serde_json::json!({
                                        "url": info["url"],
                                        "method": info["method"],
                                    })),
                                    "timestamp": info.get("timestamp").cloned().unwrap_or(serde_json::json!(0.0)),
                                    "type": info.get("type").cloned().unwrap_or(serde_json::json!("Other")),
                                }),
                            );
                        }
                    } else if let Some(payload) = text.strip_prefix("__BAO_NETWORK_RESPONSE__") {
                        if let Ok(info) = serde_json::from_str::<serde_json::Value>(payload) {
                            self.broadcaster.send_event(
                                "Network.responseReceived",
                                serde_json::json!({
                                    "requestId": info["id"],
                                    "response": {
                                        "url": info["url"],
                                        "status": info["status"],
                                        "statusText": info["statusText"],
                                        "headers": info.get("headers").unwrap_or(&serde_json::json!({})),
                                    },
                                    "timestamp": info.get("timestamp").cloned().unwrap_or(serde_json::json!(0.0)),
                                    "type": info.get("type").cloned().unwrap_or(serde_json::json!("Other")),
                                }),
                            );
                            self.broadcaster.send_event(
                                "Network.loadingFinished",
                                serde_json::json!({
                                    "requestId": info["id"],
                                    "timestamp": info.get("timestamp").cloned().unwrap_or(serde_json::json!(0.0)),
                                }),
                            );
                        }
                    } else if let Some(payload) = text.strip_prefix("__BAO_NETWORK_LOADING_FAILED__") {
                        if let Ok(info) = serde_json::from_str::<serde_json::Value>(payload) {
                            self.broadcaster.send_event(
                                "Network.loadingFailed",
                                serde_json::json!({
                                    "requestId": info["id"],
                                    "type": info.get("type").cloned().unwrap_or(serde_json::json!("Other")),
                                    "errorText": "Network error",
                                    "timestamp": info.get("timestamp").cloned().unwrap_or(serde_json::json!(0.0)),
                                }),
                            );
                        }
                    } else if let Some(payload) = text.strip_prefix("__BAO_DEBUGGER_SCRIPT__") {
                        if let Ok(info) = serde_json::from_str::<serde_json::Value>(payload) {
                            self.broadcaster.send_event(
                                "Debugger.scriptParsed",
                                serde_json::json!({
                                    "scriptId": info["id"],
                                    "url": info["url"],
                                    "startLine": info.get("startLine").unwrap_or(&serde_json::json!(0)),
                                    "endLine": info.get("endLine").unwrap_or(&serde_json::json!(0)),
                                }),
                            );
                        }
                    } else if let Some(payload) = text.strip_prefix("__BAO_RUNTIME_EXCEPTION__") {
                        if let Ok(info) = serde_json::from_str::<serde_json::Value>(payload) {
                            self.broadcaster.send_event(
                                "Runtime.exceptionThrown",
                                serde_json::json!({
                                    "timestamp": info.get("timestamp").unwrap_or(&serde_json::json!(0.0)),
                                    "exceptionDetails": {
                                        "text": info.get("text").unwrap_or(&serde_json::json!("")),
                                        "url": info.get("url").unwrap_or(&serde_json::json!("")),
                                        "lineNumber": info.get("line").unwrap_or(&serde_json::json!(0)),
                                        "columnNumber": info.get("column").unwrap_or(&serde_json::json!(0)),
                                        "stackTrace": info.get("stackTrace").unwrap_or(&serde_json::json!(null)),
                                    },
                                }),
                            );
                        }
                    } else if let Some(payload) = text.strip_prefix("__BAO_PAGE_LOAD__") {
                        if let Ok(info) = serde_json::from_str::<serde_json::Value>(payload) {
                            self.broadcaster.send_event(
                                "Page.loadEventFired",
                                serde_json::json!({
                                    "timestamp": info.get("timestamp").unwrap_or(&serde_json::json!(0.0)),
                                }),
                            );
                        }
                    } else if let Some(payload) = text.strip_prefix("__BAO_DEBUGGER_PAUSE__") {
                        if let Ok(info) = serde_json::from_str::<serde_json::Value>(payload) {
                            self.broadcaster.send_event(
                                "Debugger.paused",
                                serde_json::json!({
                                    "callFrames": info.get("callFrames").unwrap_or(&serde_json::json!([])),
                                    "reason": info.get("reason").unwrap_or(&serde_json::json!("other")),
                                    "hitBreakpoints": info.get("hitBreakpoints").unwrap_or(&serde_json::json!([])),
                                }),
                            );
                        }
                    } else if !text.starts_with("__BAO_") {
                        // Non-prefixed console messages → Runtime.consoleAPICalled
                        self.broadcaster.send_event(
                            "Runtime.consoleAPICalled",
                            serde_json::json!({
                                "type": match level.as_str() {
                                    "debug" => "debug",
                                    "info" => "info",
                                    "warning" => "warning",
                                    "error" => "error",
                                    "verbose" => "verbose",
                                    _ => "log",
                                },
                                "args": [serde_json::json!(text)],
                                "timestamp": std::time::SystemTime::now()
                                    .duration_since(std::time::UNIX_EPOCH)
                                    .unwrap_or_default()
                                    .as_millis() as f64,
                            }),
                        );
                        self.broadcaster.send_event(
                            "Log.entryAdded",
                            serde_json::json!({
                                "entry": {
                                    "source": "javascript",
                                    "level": level,
                                    "text": text,
                                    "timestamp": std::time::SystemTime::now()
                                        .duration_since(std::time::UNIX_EPOCH)
                                        .unwrap_or_default()
                                        .as_millis() as f64,
                                }
                            }),
                        );
                    } else {
                        self.broadcaster.send_event(
                            "Log.entryAdded",
                            serde_json::json!({
                                "entry": {
                                    "source": "javascript",
                                    "level": level,
                                    "text": text,
                                    "timestamp": std::time::SystemTime::now()
                                        .duration_since(std::time::UNIX_EPOCH)
                                        .unwrap_or_default()
                                        .as_millis() as f64,
                                }
                            }),
                        );
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

    // --- Console receiver tests (REQ-CDP-007) ---

    #[test]
    fn cdp_server_default_has_no_console_receiver() {
        let server = CdpServer::new(ServerConfig::default());
        assert!(server.console_rx.is_none());
    }

    #[test]
    fn cdp_server_set_console_receiver_stores_receiver() {
        let mut server = CdpServer::new(ServerConfig::default());
        let (tx, rx) = std::sync::mpsc::channel::<(String, String)>();
        server.set_console_receiver(rx);
        assert!(server.console_rx.is_some());
        // Send a message through the channel
        tx.send(("info".into(), "hello".into())).unwrap();
        let (level, text) = server.console_rx.as_ref().unwrap().try_recv().unwrap();
        assert_eq!(level, "info");
        assert_eq!(text, "hello");
    }

    #[test]
    fn cdp_server_console_rx_drain_multiple_messages() {
        let mut server = CdpServer::new(ServerConfig::default());
        let (tx, rx) = std::sync::mpsc::channel::<(String, String)>();
        server.set_console_receiver(rx);
        tx.send(("info".into(), "msg1".into())).unwrap();
        tx.send(("error".into(), "msg2".into())).unwrap();
        tx.send(("warning".into(), "msg3".into())).unwrap();
        let rx_ref = server.console_rx.as_ref().unwrap();
        let mut messages = Vec::new();
        while let Ok((level, text)) = rx_ref.try_recv() {
            messages.push((level, text));
        }
        assert_eq!(messages.len(), 3);
        assert_eq!(messages[0], ("info".into(), "msg1".into()));
        assert_eq!(messages[1], ("error".into(), "msg2".into()));
        assert_eq!(messages[2], ("warning".into(), "msg3".into()));
    }

    #[test]
    fn cdp_server_console_rx_runtime_exception_prefix() {
        let mut server = CdpServer::new(ServerConfig::default());
        let (tx, rx) = std::sync::mpsc::channel::<(String, String)>();
        server.set_console_receiver(rx);
        tx.send(("error".into(), "__BAO_RUNTIME_EXCEPTION__{\"text\":\"TypeError: x is not a function\",\"url\":\"test.js\",\"line\":10,\"column\":5}".into())).unwrap();
        let rx_ref = server.console_rx.as_ref().unwrap();
        let (level, text) = rx_ref.try_recv().unwrap();
        assert!(text.starts_with("__BAO_RUNTIME_EXCEPTION__"));
    }

    #[test]
    fn cdp_server_console_rx_page_load_prefix() {
        let mut server = CdpServer::new(ServerConfig::default());
        let (tx, rx) = std::sync::mpsc::channel::<(String, String)>();
        server.set_console_receiver(rx);
        tx.send(("info".into(), "__BAO_PAGE_LOAD__{\"timestamp\":12345.0}".into())).unwrap();
        let rx_ref = server.console_rx.as_ref().unwrap();
        let (level, text) = rx_ref.try_recv().unwrap();
        assert!(text.starts_with("__BAO_PAGE_LOAD__"));
    }

    #[test]
    fn cdp_server_console_rx_debugger_pause_prefix() {
        let mut server = CdpServer::new(ServerConfig::default());
        let (tx, rx) = std::sync::mpsc::channel::<(String, String)>();
        server.set_console_receiver(rx);
        tx.send(("info".into(), "__BAO_DEBUGGER_PAUSE__{\"reason\":\"breakpoint\",\"callFrames\":[]}".into())).unwrap();
        let rx_ref = server.console_rx.as_ref().unwrap();
        let (level, text) = rx_ref.try_recv().unwrap();
        assert!(text.starts_with("__BAO_DEBUGGER_PAUSE__"));
    }

    #[test]
    fn cdp_server_console_rx_known_bao_prefixes_are_recognized() {
        let mut server = CdpServer::new(ServerConfig::default());
        let (tx, rx) = std::sync::mpsc::channel::<(String, String)>();
        server.set_console_receiver(rx);
        let prefixes = vec![
            "__BAO_NETWORK_REQUEST__{}",
            "__BAO_NETWORK_RESPONSE__{}",
            "__BAO_NETWORK_LOADING_FAILED__{}",
            "__BAO_FETCH_INTERCEPT__{}",
            "__BAO_DEBUGGER_SCRIPT__{}",
            "__BAO_RUNTIME_EXCEPTION__{}",
            "__BAO_PAGE_LOAD__{}",
            "__BAO_DEBUGGER_PAUSE__{}",
        ];
        for msg in &prefixes {
            tx.send(("info".into(), msg.to_string())).unwrap();
        }
        let rx_ref = server.console_rx.as_ref().unwrap();
        let mut count = 0;
        while let Ok((_, text)) = rx_ref.try_recv() {
            assert!(text.starts_with("__BAO_"));
            count += 1;
        }
        assert_eq!(count, 8);
    }
}
