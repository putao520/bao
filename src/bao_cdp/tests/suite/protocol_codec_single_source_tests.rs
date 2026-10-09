// @trace TEST-CDP-016-CODEC-SS [req:REQ-CDP-001] [level:unit]
// M4 single-source lock (e152 audit DUP-CODEC-WRAPPERS): the JSON-RPC 2.0
// codec reachable through `bao_cdp` must be the *same* function items as
// `cdp_server`'s — re-exported, never re-implemented. Reintroducing a local
// copy in `bao_cdp::protocol` compiles but changes the item, failing the
// pointer-identity assertions below.

use bao_cdp::{parse_message, serialize_event, serialize_response};
use cdp_server::{CdpEvent, CdpMessage, CdpResponse};

#[test]
fn parse_message_is_cdp_server_item() {
    let via_bao_cdp: fn(&str) -> Option<CdpMessage> = parse_message;
    let direct: fn(&str) -> Option<CdpMessage> = cdp_server::parse_message;
    assert_eq!(via_bao_cdp, direct);
}

#[test]
fn serialize_response_is_cdp_server_item() {
    let via_bao_cdp: fn(&CdpResponse) -> String = serialize_response;
    let direct: fn(&CdpResponse) -> String = cdp_server::serialize_response;
    assert_eq!(via_bao_cdp, direct);
}

#[test]
fn serialize_event_is_cdp_server_item() {
    let via_bao_cdp: fn(&CdpEvent) -> String = serialize_event;
    let direct: fn(&CdpEvent) -> String = cdp_server::serialize_event;
    assert_eq!(via_bao_cdp, direct);
}

// Wire types themselves are the same items (re-export, not re-definition):
// a locally re-defined type would carry the `bao_cdp::` crate path here.
#[test]
fn wire_types_are_cdp_server_types() {
    assert_eq!(
        std::any::type_name::<bao_cdp::CdpMessage>(),
        std::any::type_name::<cdp_server::CdpMessage>()
    );
    assert_eq!(
        std::any::type_name::<bao_cdp::CdpResponse>(),
        std::any::type_name::<CdpResponse>()
    );
    assert_eq!(
        std::any::type_name::<bao_cdp::CdpError>(),
        std::any::type_name::<cdp_server::CdpError>()
    );
    assert_eq!(
        std::any::type_name::<bao_cdp::CdpEvent>(),
        std::any::type_name::<cdp_server::CdpEvent>()
    );
}
