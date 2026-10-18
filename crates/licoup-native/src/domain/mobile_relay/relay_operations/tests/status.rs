use super::super::status::e2ee_status;
use anyhow::Result;
use serde_json::Value;

#[test]
fn e2ee_status_has_one_authorization_aware_projection_entrypoint() {
    let entrypoint: fn(&Value) -> Result<Value> = e2ee_status;
    let _ = entrypoint;
}
