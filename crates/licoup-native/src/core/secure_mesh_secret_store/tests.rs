use super::*;
use anyhow::Result;

mod secret_material;

struct FixtureStore;

impl SecureMeshSecretStore for FixtureStore {
    fn backend(&self) -> &'static str {
        "fixture"
    }

    fn supported(&self) -> bool {
        true
    }

    fn set_secret(&self, _handle: &SecretStoreHandle, _secret: SecretBytes) -> Result<()> {
        Ok(())
    }

    fn get_secret(&self, _handle: &SecretStoreHandle) -> Result<Option<SecretBytes>> {
        Ok(None)
    }

    fn delete_secret(&self, _handle: &SecretStoreHandle) -> Result<()> {
        Ok(())
    }
}

#[test]
fn authorization_session_enforces_exact_operation_budget() {
    let request = SecretStoreAuthorizationRequest::noninteractive("fixture", 2);
    let session = FixtureStore.begin_authorized_session(&request).unwrap();
    let handle = SecretStoreHandle::new("scope", "key").unwrap();
    FixtureStore
        .set_secret_with_session(
            &session,
            &handle,
            SecretBytes::try_from_bytes(b"synthetic-secret".to_vec()).unwrap(),
        )
        .unwrap();
    FixtureStore
        .get_secret_with_session(&session, &handle)
        .unwrap();
    assert_eq!(session.remaining_operation_count(), 0);
    assert!(
        FixtureStore
            .delete_secret_with_session(&session, &handle)
            .is_err()
    );
}

#[test]
fn secret_handle_rejects_ambiguous_or_empty_subjects() {
    assert!(SecretStoreHandle::new("", "key").is_err());
    assert!(SecretStoreHandle::new("scope", "").is_err());
    assert!(SecretStoreHandle::new("scope", "nested:key").is_err());
}
