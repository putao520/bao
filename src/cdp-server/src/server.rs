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

use crate::bao_event::ConsoleMessage;
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
    /// Receiver for typed console messages forwarded from servo delegates.
    /// Each message is either a structured CDP event (ConsoleMessage::Event)
    /// or a plain log (ConsoleMessage::Log).
    console_rx: Option<std::sync::mpsc::Receiver<ConsoleMessage>>,
}

impl CdpServer {
    /// Create CdpServer with an empty domain registry.
    /// For production use, prefer `with_registry()` with a pre-built registry
    /// (e.g. `DomainRegistry<DomainDispatch>` for enum dispatch).
    pub fn new(config: ServerConfig) -> Self {
        let registry: Arc<crate::DomainRegistry<crate::EmptyHandler>> =
            Arc::new(crate::DomainRegistry::new());
        Self::with_registry(config, registry)
    }

    /// Create CdpServer with a pre-built registry (e.g. DomainRegistry<DomainDispatch>
    /// for enum dispatch). Any `Arc<R>` where `R: RegistryDispatch` is accepted.
    pub fn with_registry<R: crate::RegistryDispatch + 'static>(config: ServerConfig, registry: Arc<R>) -> Self {
        let sessions = Arc::new(Mutex::new(HashMap::new()));
        let broadcaster = Arc::new(EventBroadcaster::new(Arc::clone(&sessions)));
        CdpServer {
            config,
            registry,
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

    /// Set the typed console message receiver. Messages are ConsoleMessage
    /// variants forwarded from servo's show_console_message callbacks.
    pub fn set_console_receiver(&mut self, rx: std::sync::mpsc::Receiver<ConsoleMessage>) {
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

        log::info!("CDP listening on ws://{}:{}", self.config.host, self.config.port);

        loop {
            // Drain session events (not used currently, but placeholder for future command channel).
            self.check_session_timeouts();

            // Accept new connections.
            match listener.accept() {
                Ok((stream, _addr)) => {
                    self.handle_connection(stream);
                }
                Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {}
                Err(e) => log::warn!("CDP accept error: {}", e),
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

            // Drain typed console messages from servo delegates and broadcast as CDP events.
            // ConsoleMessage::Event variants are routed to domain-specific events via BaoEvent::broadcast().
            // ConsoleMessage::Log variants are forwarded as Runtime.consoleAPICalled + Log.entryAdded.
            if let Some(ref rx) = self.console_rx {
                while let Ok(msg) = rx.try_recv() {
                    match msg {
                        ConsoleMessage::Event(event) => {
                            event.broadcast(&*self.broadcaster);
                        }
                        ConsoleMessage::Log { level, text } => {
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
                    log::warn!("CDP WebSocket accept error: {}", e);
                    return;
                }
            };

            let session_id = generate_session_id();
            let session = CdpSession::new(session_id.clone(), target_id, ws, is_browser);
            let session_count = self.sessions.lock().map(|m| m.len()).unwrap_or(0);
            if session_count >= self.config.max_sessions {
                log::warn!("CDP max sessions reached, rejecting");
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
    use crate::bao_event::BaoEvent;

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
        let (tx, rx) = std::sync::mpsc::channel::<ConsoleMessage>();
        server.set_console_receiver(rx);
        assert!(server.console_rx.is_some());
        // Send a Log message through the channel
        tx.send(ConsoleMessage::Log { level: "info".into(), text: "hello".into() }).unwrap();
        let msg = server.console_rx.as_ref().unwrap().try_recv().unwrap();
        match msg {
            ConsoleMessage::Log { level, text } => {
                assert_eq!(level, "info");
                assert_eq!(text, "hello");
            }
            ConsoleMessage::Event(_) => panic!("expected Log, got Event"),
        }
    }

    #[test]
    fn cdp_server_console_rx_drain_multiple_messages() {
        let mut server = CdpServer::new(ServerConfig::default());
        let (tx, rx) = std::sync::mpsc::channel::<ConsoleMessage>();
        server.set_console_receiver(rx);
        tx.send(ConsoleMessage::Log { level: "info".into(), text: "msg1".into() }).unwrap();
        tx.send(ConsoleMessage::Log { level: "error".into(), text: "msg2".into() }).unwrap();
        tx.send(ConsoleMessage::Log { level: "warning".into(), text: "msg3".into() }).unwrap();
        let rx_ref = server.console_rx.as_ref().unwrap();
        let mut messages = Vec::new();
        while let Ok(msg) = rx_ref.try_recv() {
            messages.push(msg);
        }
        assert_eq!(messages.len(), 3);
    }

    #[test]
    fn cdp_server_console_rx_event_variant() {
        let mut server = CdpServer::new(ServerConfig::default());
        let (tx, rx) = std::sync::mpsc::channel::<ConsoleMessage>();
        server.set_console_receiver(rx);
        tx.send(ConsoleMessage::Event(BaoEvent::PageLoadEventFired { timestamp: 12345.0 })).unwrap();
        let msg = server.console_rx.as_ref().unwrap().try_recv().unwrap();
        match msg {
            ConsoleMessage::Event(BaoEvent::PageLoadEventFired { timestamp }) => {
                assert_eq!(timestamp, 12345.0);
            }
            other => panic!("expected Event(PageLoadEventFired), got {:?}", other),
        }
    }

    #[test]
    fn cdp_server_console_rx_debugger_script_parsed_event() {
        let mut server = CdpServer::new(ServerConfig::default());
        let (tx, rx) = std::sync::mpsc::channel::<ConsoleMessage>();
        server.set_console_receiver(rx);
        tx.send(ConsoleMessage::Event(BaoEvent::DebuggerScriptParsed {
            script_id: "1".into(),
            url: "test.js".into(),
            start_line: 0,
            end_line: 10,
        })).unwrap();
        let msg = server.console_rx.as_ref().unwrap().try_recv().unwrap();
        match msg {
            ConsoleMessage::Event(BaoEvent::DebuggerScriptParsed { script_id, url, .. }) => {
                assert_eq!(script_id, "1");
                assert_eq!(url, "test.js");
            }
            other => panic!("expected Event(DebuggerScriptParsed), got {:?}", other),
        }
    }

    #[test]
    fn cdp_server_console_rx_debugger_paused_event() {
        let mut server = CdpServer::new(ServerConfig::default());
        let (tx, rx) = std::sync::mpsc::channel::<ConsoleMessage>();
        server.set_console_receiver(rx);
        tx.send(ConsoleMessage::Event(BaoEvent::DebuggerPaused {
            call_frames: serde_json::json!([]),
            reason: "breakpoint".into(),
            hit_breakpoints: serde_json::json!([]),
        })).unwrap();
        let msg = server.console_rx.as_ref().unwrap().try_recv().unwrap();
        match msg {
            ConsoleMessage::Event(BaoEvent::DebuggerPaused { reason, .. }) => {
                assert_eq!(reason, "breakpoint");
            }
            other => panic!("expected Event(DebuggerPaused), got {:?}", other),
        }
    }

    #[test]
    fn cdp_server_console_rx_runtime_exception_event() {
        let mut server = CdpServer::new(ServerConfig::default());
        let (tx, rx) = std::sync::mpsc::channel::<ConsoleMessage>();
        server.set_console_receiver(rx);
        tx.send(ConsoleMessage::Event(BaoEvent::RuntimeExceptionThrown {
            timestamp: 100.0,
            text: "TypeError: x is not a function".into(),
            url: "test.js".into(),
            line: 10,
            column: 5,
            stack_trace: serde_json::Value::Null,
        })).unwrap();
        let msg = server.console_rx.as_ref().unwrap().try_recv().unwrap();
        match msg {
            ConsoleMessage::Event(BaoEvent::RuntimeExceptionThrown { text, .. }) => {
                assert_eq!(text, "TypeError: x is not a function");
            }
            other => panic!("expected Event(RuntimeExceptionThrown), got {:?}", other),
        }
    }

    #[test]
    fn cdp_server_console_rx_all_event_variants() {
        let mut server = CdpServer::new(ServerConfig::default());
        let (tx, rx) = std::sync::mpsc::channel::<ConsoleMessage>();
        server.set_console_receiver(rx);
        let events = vec![
            ConsoleMessage::Event(BaoEvent::FetchRequestPaused {
                request_id: "r1".into(), url: "http://test.com".into(),
                method: "GET".into(), headers: serde_json::json!({}),
                post_data: None, resource_type: "Document".into(),
            }),
            ConsoleMessage::Event(BaoEvent::NetworkRequestWillBeSent {
                request_id: "req1".into(), url: "http://test.com".into(),
                method: "GET".into(), headers: serde_json::json!({}),
                request: serde_json::json!({}), timestamp: 0.0, resource_type: "Document".into(),
            }),
            ConsoleMessage::Event(BaoEvent::NetworkResponseReceived {
                request_id: "req2".into(), url: "http://test.com".into(),
                status: 200, status_text: "OK".into(), headers: serde_json::json!({}),
                timestamp: 0.0, resource_type: "Document".into(),
            }),
            ConsoleMessage::Event(BaoEvent::NetworkLoadingFailed {
                request_id: "req3".into(), resource_type: "XHR".into(),
                error_text: "Network error".into(), timestamp: 0.0,
            }),
            ConsoleMessage::Event(BaoEvent::DebuggerScriptParsed {
                script_id: "1".into(), url: "test.js".into(), start_line: 0, end_line: 10,
            }),
            ConsoleMessage::Event(BaoEvent::DebuggerPaused {
                call_frames: serde_json::json!([]), reason: "other".into(),
                hit_breakpoints: serde_json::json!([]),
            }),
            ConsoleMessage::Event(BaoEvent::RuntimeExceptionThrown {
                timestamp: 0.0, text: String::new(), url: String::new(),
                line: 0, column: 0, stack_trace: serde_json::Value::Null,
            }),
            ConsoleMessage::Event(BaoEvent::PageLoadEventFired { timestamp: 0.0 }),
        ];
        for evt in &events {
            tx.send(evt.clone()).unwrap();
        }
        let rx_ref = server.console_rx.as_ref().unwrap();
        let mut count = 0;
        while let Ok(msg) = rx_ref.try_recv() {
            assert!(matches!(msg, ConsoleMessage::Event(_)));
            count += 1;
        }
        assert_eq!(count, 8);
    }
}
