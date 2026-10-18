use ed25519_dalek::SigningKey;
use rand_core::OsRng;

use crate::core::secure_mesh_mls_product::{
    create_product_group, participant_from_device_identity,
};
use crate::core::secure_mesh_trust::{DeviceTrustPublicIdentity, DeviceTrustState};

use super::super::group_state::{group_status_json, reconcile_group_metadata};

#[test]
fn group_create_reconciles_one_authoritative_local_projection() {
    let root = std::env::temp_dir().join(format!(
        "lico-mls-group-create-projection-{}",
        uuid::Uuid::new_v4()
    ));
    let previous =
        licoup_foundation::platform::paths::set_portable_data_dir_override(Some(root.clone()));
    let identity_key = SigningKey::generate(&mut OsRng);
    let signing_key = SigningKey::generate(&mut OsRng);
    let identity = DeviceTrustPublicIdentity::new(
        "desktop_gui:group-create-projection",
        identity_key.verifying_key().to_bytes(),
        signing_key.verifying_key().to_bytes(),
        1,
    )
    .unwrap();
    let participant = participant_from_device_identity(&identity, &signing_key).unwrap();
    let group = create_product_group(
        &participant,
        &identity,
        &DeviceTrustState::Verified,
        b"group-create-projection",
    )
    .unwrap();

    let mut group_store = crate::platform::secure_mesh_mls_store::open(
        crate::domain::mobile_relay::secure_mesh_mls_state_dir()
            .unwrap()
            .join("group-state.sqlite3"),
    )
    .unwrap();
    let record = reconcile_group_metadata(&mut group_store, &group, &identity).unwrap();
    let status = group_status_json(&group, &record);
    assert_eq!(status["memberCount"], 1);
    assert_eq!(status["active"], true);
    assert_eq!(status["participantScopeRedacted"], true);
    assert_eq!(status.get("participantEndpointId"), None);

    licoup_foundation::platform::paths::set_portable_data_dir_override(previous);
    let _ = std::fs::remove_dir_all(root);
}
