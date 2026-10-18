use super::super::claim::build_local_directory_claim;
use crate::core::secure_mesh_directory::SecureMeshDirectoryLeafClaim;
use crate::domain::mobile_relay::endpoint_trust::ensure_mobile_relay_endpoint_material;
use crate::domain::mobile_relay::test_runtime_secret_material;
use serde_json::{Value, json};

pub(super) fn endpoint_config() -> Value {
    let mut config = json!({});
    ensure_mobile_relay_endpoint_material(
        &mut config,
        &mut test_runtime_secret_material(stringify!(&mut config)),
        "desktop",
    )
    .unwrap();
    config
}

pub(super) fn local_claim(config: &Value) -> SecureMeshDirectoryLeafClaim {
    build_local_directory_claim(config, &"a".repeat(64), 1, "active", &"b".repeat(64), 1).unwrap()
}
