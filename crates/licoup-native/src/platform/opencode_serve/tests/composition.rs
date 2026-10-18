use anyhow::Result;
use serde_json::Value;

use super::super::*;

#[test]
fn facade_preserves_the_public_lifecycle_and_driver_transport_contract() {
    let _: fn(&Value) -> Result<Value> = ensure;
    let _: fn(&Value) -> Result<Value> = start;
    let _: fn(&Value) -> Result<Value> = restart;
    let _: fn(&Value) -> Result<Value> = stop;
    let _: fn(&Value) -> Result<Value> = status;
    let _: fn(&str) -> Result<ServeEndpoint> = ensure_attach_endpoint;
    let _: fn(&str) -> std::result::Result<Value, local_service::http::HttpFailure> = get_json;
    let endpoint = ServeEndpoint::new("127.0.0.1", DEFAULT_PORT);
    assert_eq!(endpoint.port, 24173);
}
