use ed25519_dalek::SigningKey;
use rand_core::OsRng;
use serde_json::json;
use time::OffsetDateTime;

use crate::core::secure_mesh_directory::DirectoryAuthorizationPurpose;
use crate::core::secure_mesh_transparency::SecureMeshKtLog;

use super::super::directory_authorization::authorize_member_add_directory_response;
use super::super::input_codec::{MemberAddRequest, hex_sha256};
use super::support::{
    append_test_directory_response, test_directory_claim, test_identity, test_kt_config,
};

#[test]
fn mls_member_add_uses_explicit_local_pin_and_persisted_endpoint_checkpoint() {
    let root = std::env::temp_dir().join(format!("lico-mls-kt-authority-{}", uuid::Uuid::new_v4()));
    let previous =
        licoup_foundation::platform::paths::set_portable_data_dir_override(Some(root.clone()));
    let local = test_identity("desktop_gui:mls-kt-local");
    let member = test_identity("mobile:mls-kt-member");
    let mut log = SecureMeshKtLog::new(SigningKey::generate(&mut OsRng));
    let config = test_kt_config(&log);
    let first_claim = test_directory_claim(&member, 7, 11, &hex_sha256(b"key-package-v1"));
    let first_response = append_test_directory_response(&mut log, &first_claim, 100, None);
    let first = authorize_member_add_directory_response(
        &config,
        &local,
        first_response.clone(),
        OffsetDateTime::from_unix_timestamp(100).unwrap(),
    )
    .unwrap();
    assert_eq!(first.purpose(), DirectoryAuthorizationPurpose::MlsMemberAdd);
    assert_eq!(first.claim().version(), 7);

    let second_claim = test_directory_claim(&member, 8, 12, &hex_sha256(b"key-package-v2"));
    let second_response = append_test_directory_response(&mut log, &second_claim, 101, Some(1));
    let second = authorize_member_add_directory_response(
        &config,
        &local,
        second_response,
        OffsetDateTime::from_unix_timestamp(101).unwrap(),
    )
    .unwrap();
    assert_eq!(second.claim().version(), 8);

    let rollback = authorize_member_add_directory_response(
        &config,
        &local,
        first_response,
        OffsetDateTime::from_unix_timestamp(102).unwrap(),
    )
    .unwrap_err();
    assert!(rollback.to_string().contains("rollback"));

    licoup_foundation::platform::paths::set_portable_data_dir_override(previous);
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn mls_member_add_has_no_default_kt_pin() {
    let local = test_identity("desktop_gui:mls-kt-no-default");
    let member = test_identity("mobile:mls-kt-no-default-member");
    let mut log = SecureMeshKtLog::new(SigningKey::generate(&mut OsRng));
    let claim = test_directory_claim(&member, 1, 1, &hex_sha256(b"key-package"));
    let response = append_test_directory_response(&mut log, &claim, 100, None);
    let error = authorize_member_add_directory_response(
        &json!({}),
        &local,
        response,
        OffsetDateTime::from_unix_timestamp(100).unwrap(),
    )
    .unwrap_err();
    assert!(
        error
            .to_string()
            .contains("local KT pin configuration is required")
    );
    let caller_pin = serde_json::from_value::<MemberAddRequest>(json!({
        "ktLogPin": {"publicKeyHex": "caller-controlled"}
    }))
    .err()
    .expect("caller-provided KT pin must be rejected");
    assert!(caller_pin.to_string().contains("unknown field `ktLogPin`"));
}

#[test]
fn mls_member_add_rejects_response_signed_by_a_non_pinned_log() {
    let root = std::env::temp_dir().join(format!("lico-mls-kt-wrong-pin-{}", uuid::Uuid::new_v4()));
    let previous =
        licoup_foundation::platform::paths::set_portable_data_dir_override(Some(root.clone()));
    let local = test_identity("desktop_gui:mls-kt-wrong-pin");
    let member = test_identity("mobile:mls-kt-wrong-pin-member");
    let mut response_log = SecureMeshKtLog::new(SigningKey::generate(&mut OsRng));
    let pinned_log = SecureMeshKtLog::new(SigningKey::generate(&mut OsRng));
    let claim = test_directory_claim(&member, 1, 1, &hex_sha256(b"key-package"));
    let response = append_test_directory_response(&mut response_log, &claim, 100, None);
    let error = authorize_member_add_directory_response(
        &test_kt_config(&pinned_log),
        &local,
        response,
        OffsetDateTime::from_unix_timestamp(100).unwrap(),
    )
    .unwrap_err();
    assert!(error.to_string().contains("signature is invalid"));

    licoup_foundation::platform::paths::set_portable_data_dir_override(previous);
    let _ = std::fs::remove_dir_all(root);
}
