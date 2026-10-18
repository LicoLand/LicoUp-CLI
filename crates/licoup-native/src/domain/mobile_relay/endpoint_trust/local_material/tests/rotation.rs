use super::super::{
    material_mutation::ensure_mobile_relay_endpoint_material,
    rotation::rotate_mobile_relay_one_time_prekeys,
};
use crate::domain::mobile_relay::test_runtime_secret_material;
use serde_json::json;

#[test]
fn one_time_rotation_replaces_ephemeral_inventory_once() {
    let mut config = json!({});
    ensure_mobile_relay_endpoint_material(
        &mut config,
        &mut test_runtime_secret_material(stringify!(&mut config)),
        "desktop",
    )
    .unwrap();
    let previous_curve_id = config["mobileRelayE2ee"]["oneTimePrekeyId"]
        .as_str()
        .unwrap()
        .to_string();
    let previous_pq_id = config["mobileRelayE2ee"]["oneTimeMlKem1024PrekeyId"]
        .as_str()
        .unwrap()
        .to_string();
    config["mobileRelayE2ee"]["keyTransparencyResponse"] = json!({"stale": true});

    rotate_mobile_relay_one_time_prekeys(
        &mut config,
        &mut test_runtime_secret_material(stringify!(&mut config)),
    )
    .unwrap();

    let state = &config["mobileRelayE2ee"];
    assert_eq!(state["prekeyPublicationVersion"], 2);
    assert_ne!(state["oneTimePrekeyId"], previous_curve_id);
    assert_ne!(state["oneTimeMlKem1024PrekeyId"], previous_pq_id);
    assert!(state.get("keyTransparencyResponse").is_none());
}
