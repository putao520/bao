// REQ-CDP-003: CDP module public API — server entry + WS / JSON-RPC codec
// @trace REQ-CDP-001 [entity:CdpRouter] [entity:CdpServer]
// @trace REQ-PURE-009 [level:library] [entity:HttpServer,HttpServerConfig]
// @trace REQ-IMPL-03 [level:library] (CDP server = Phase 3)
//
// TASK-6 (DEC-CDP-001): evaluate_js 注入式 domain handler 已删除,
// CDP 命令分发由 bao_cdp_client::CDPRdpBridge 接管。本 crate 退化为
// 对外 CDP server 入口(Playwright 兼容)+ 基础设施(RFC 6455 codec、
// JSON-RPC 编解码、Target 路由),被 bao_cdp_client 复用。
//
// TASK-18 (REQ-CDP-UWS-001): RFC 6455 codec / handshake / masking 已迁移
// 至 `bun_uws`(ws_codec / ws_handshake / ws_client / ws_server)。本 crate
// 通过 `pub use bun_uws::*` 重导出,删除自写 ws_codec/ws_handshake/ws。
// 所有 WebSocket 表面入口统一在 `bun_uws`(bao_cdp / bao_cdp_client 仅依赖
// bun_uws,无 tungstenite)。
//
// TASK-4-CDP: Removed the dead synchronous `CDPServer`/`CDPSession`/
// `CDPCommand`/`CDPServerError`/`WebSocketConnection` server entry + the
// `respond_json`/`respond_raw`/`rand_id` helpers. Production now uses
// `cdp_server::CdpServer` (re-exported below) for the Playwright-compatible
// async server; tests that need a CDP server use `cdp_server::CdpServer`
// directly. The `CdpRouter` (internal/external session routing) and the BAO
// domain dispatch (`protocol::handle_command`) remain here.

// ---------------------------------------------------------------------------
// §1 CdpServer + wire types — re-exports from the `cdp-server` crate
// ---------------------------------------------------------------------------

// The async Playwright-compatible CDP server lives in `cdp-server`.
// Re-exported so callers can use `bao_cdp::CdpServer`.
pub use cdp_server::{BaoEvent, CdpServer, ConsoleMessage};

// JSON-RPC 2.0 wire types are owned by `cdp-server` and re-exported here.
// TASK-4-CDP removed the byte-for-byte duplicate definitions that used to
// live in `bao_cdp::protocol`. The codec helpers (parse_message/
// serialize_response/serialize_event) stay in `bao_cdp::protocol` as thin
// wrappers over these types (the cdp-server `protocol` module is private,
// so its functions cannot be re-exported directly).
pub use cdp_server::{CdpError, CdpEvent, CdpMessage, CdpResponse};

// WebSocket surface — re-exported from `bun_uws` (REQ-CDP-UWS-001).
// Removed: bao_cdp::{ws, ws_codec, ws_handshake} self-written modules.
pub use bun_uws::ws_codec::{self, FrameDecoder, FrameEncoder, FrameHeader, Message, Opcode};
pub use bun_uws::ws_handshake::{
    self, client_handshake, compute_accept, generate_sec_websocket_key, server_handshake,
    HandshakeError,
};
pub use bun_uws::ws_server::{self, ReplayStream, WsServerConnection};

mod backend;
pub mod domains;
mod protocol;
mod router;
pub mod servo_bridge;

// BAO-specific 11-domain CDP command dispatch + JSON-RPC 2.0 codec helpers.
// Wire types come from cdp_server (re-exported above); the codec helpers are
// thin serde wrappers in `bao_cdp::protocol`.
pub use protocol::{handle_command, parse_message, serialize_event, serialize_response};
pub use router::{BackendKind, CdpRouter, CdpSession, ExternalBrowser};
pub use servo_bridge::{
    bridge_channel, BridgeCommand, BridgeReceiver, BridgeResponse, BridgeSender,
};
