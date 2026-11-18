use anyhow::Result;
use serde_json::Value;

use super::super::*;

#[test]
fn facade_preserves_gateway_lifecycle_and_attach_contract() {
    let _: fn(&Value) -> Result<Value> = ensure;
    let _: fn(&Value) -> Result<Value> = start;
    let _: fn(&Value) -> Result<Value> = restart;
    let _: fn(&Value) -> Result<Value> = stop;
    let _: fn(&Value) -> Result<Value> = status;
    let _: fn(&str) -> Result<GatewayEndpoint> = ensure_attach_endpoint;
    assert_eq!(DEFAULT_PORT, 24189);
    assert_eq!(VENDOR_DEFAULT_PORT, 18789);
}
