use serde_json::json;

use super::super::model::{GatewayEndpoint, endpoint_from_state};

#[test]
fn endpoint_projection_keeps_http_and_websocket_identity_together() {
    let endpoint = endpoint_from_state(&json!({
        "host": "127.0.0.1",
        "port": 24190,
        "attachUrl": "http://127.0.0.1:24190",
        "wsUrl": "ws://127.0.0.1:24190/custom"
    }));
    assert_eq!(endpoint.port, 24190);
    assert_eq!(endpoint.ws_url, "ws://127.0.0.1:24190/custom");
    assert_eq!(
        GatewayEndpoint::new("127.0.0.1", 24189).attach_url,
        "http://127.0.0.1:24189"
    );
}
