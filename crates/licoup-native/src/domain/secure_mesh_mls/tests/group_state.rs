use ed25519_dalek::SigningKey;
use rand_core::OsRng;

use crate::core::secure_mesh_mls_product::{
    create_product_group, participant_from_device_identity,
};
use crate::core::secure_mesh_trust::{DeviceTrustPublicIdentity, DeviceTrustState};

use super::super::group_state::{reconcile_group_metadata, require_group_base_current};
use super::super::journal_recovery::current_group_metadata;

#[test]
fn mls_join_missing_snapshot_rejects_existing_durable_authority_before_crypto() {
    let root = std::env::temp_dir().join(format!(
        "lico-mls-join-durable-authority-{}",
        uuid::Uuid::new_v4()
    ));
    let previous =
        licoup_foundation::platform::paths::set_portable_data_dir_override(Some(root.clone()));
    let identity_key = SigningKey::generate(&mut OsRng);
    let signing_key = SigningKey::generate(&mut OsRng);
    let identity = DeviceTrustPublicIdentity::new(
        "mobile:join-durable-authority",
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
        b"join-durable-authority-group",
    )
    .unwrap();
    let metadata = current_group_metadata(&group, &identity).unwrap();
    let mut group_store = crate::platform::secure_mesh_mls_store::open(
        crate::domain::mobile_relay::secure_mesh_mls_state_dir()
            .unwrap()
            .join("group-state.sqlite3"),
    )
    .unwrap();
    reconcile_group_metadata(&mut group_store, &group, &identity).unwrap();

    let error = require_group_base_current(
        &group_store,
        None,
        &metadata.group_id_hash,
        &metadata.participant_endpoint_id,
    )
    .unwrap_err();
    assert!(error.to_string().contains("diverges from durable metadata"));

    licoup_foundation::platform::paths::set_portable_data_dir_override(previous);
    let _ = std::fs::remove_dir_all(root);
}
