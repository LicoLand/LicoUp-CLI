use crate::core::secure_mesh_capability::{CustodyRestartSemantics, SecretCustodyStrategy};

use super::super::ephemeral::EphemeralSecretStore;
use crate::core::secure_mesh_secret_store::{
    SecretBytes, SecretStoreAuthorizationRequest, SecretStoreHandle, SecureMeshSecretStore,
};

fn secret(value: &str) -> SecretBytes {
    SecretBytes::try_from_string(value.to_owned()).unwrap()
}

#[test]
fn ephemeral_strategy_zeroizing_store_has_explicit_restart_repair_semantics() {
    let store = EphemeralSecretStore::new();
    let handle = SecretStoreHandle::new("ephemeral", "identity-key").unwrap();
    let request = SecretStoreAuthorizationRequest::noninteractive("ephemeral operation", 3);
    let session = store.begin_authorized_session(&request).unwrap();
    store
        .set_secret_with_session(&session, &handle, secret("secret-value"))
        .unwrap();
    assert_eq!(
        store
            .get_secret_with_session(&session, &handle)
            .unwrap()
            .as_ref()
            .map(SecretBytes::expose_bytes),
        Some(b"secret-value".as_slice())
    );
    store.delete_secret_with_session(&session, &handle).unwrap();
    assert_eq!(session.remaining_operation_count(), 0);
    assert_eq!(
        session
            .capability_report()
            .and_then(|report| report.custody.as_ref())
            .map(|selection| selection.strategy),
        Some(SecretCustodyStrategy::MemoryOnlyEphemeral)
    );
    assert_eq!(
        session
            .capability_report()
            .and_then(|report| report.custody.as_ref())
            .map(|selection| selection.restart_semantics),
        Some(CustodyRestartSemantics::RePairRekeyAfterRestart)
    );

    let restarted_store = EphemeralSecretStore::new();
    assert!(restarted_store.get_secret(&handle).unwrap().is_none());
}
