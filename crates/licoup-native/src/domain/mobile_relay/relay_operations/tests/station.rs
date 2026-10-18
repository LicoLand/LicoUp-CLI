use super::super::station::station_context;
use crate::domain::mobile_relay::config::default_config;
use serde_json::json;

#[test]
fn station_context_rejects_disabled_relay_before_transport_construction() {
    let error = station_context(&json!({}), &default_config())
        .err()
        .expect("disabled relay must fail closed");
    assert!(error.to_string().contains("mobile relay is disabled"));
}
