use ed25519_dalek::SigningKey;
use rand_core::OsRng;

use crate::core::secure_mesh_mls::SecureMeshMlsKeyPackage;
use crate::core::secure_mesh_mls_product::participant_from_device_identity;
use crate::core::secure_mesh_trust::DeviceTrustPublicIdentity;

use super::super::group_state::public_local_participant;
use super::super::input_codec::{MAX_KEY_PACKAGE_BYTES, hex_sha256};

#[test]
fn participant_projection_and_key_package_remain_identity_bound() {
    let identity_key = SigningKey::generate(&mut OsRng);
    let signing_key = SigningKey::generate(&mut OsRng);
    let identity = DeviceTrustPublicIdentity::new(
        "desktop_gui:participant-key-package",
        identity_key.verifying_key().to_bytes(),
        signing_key.verifying_key().to_bytes(),
        1,
    )
    .unwrap();
    let participant = participant_from_device_identity(&identity, &signing_key).unwrap();

    let projection = public_local_participant(&identity, &participant).unwrap();
    assert_eq!(projection["identity"]["endpointId"], identity.endpoint_id);
    assert_eq!(projection["credentialBound"], true);
    assert_eq!(projection.get("privateKeyMaterial"), None);

    let key_package = participant.generate_key_package().unwrap();
    assert!(!key_package.as_public_bytes().is_empty());
    assert!(key_package.as_public_bytes().len() <= MAX_KEY_PACKAGE_BYTES);
    assert_eq!(hex_sha256(key_package.as_public_bytes()).len(), 64);
    let decoded =
        SecureMeshMlsKeyPackage::from_public_bytes(key_package.as_public_bytes()).unwrap();
    assert_eq!(decoded.as_public_bytes(), key_package.as_public_bytes());
}
