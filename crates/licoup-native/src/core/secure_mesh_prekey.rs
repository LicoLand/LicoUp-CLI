mod inventory;
mod key_package;
mod pairwise;
mod validation;

pub use inventory::{
    SECURE_MESH_PREKEY_STATUS, SecureMeshInventoryStatus, evaluate_prekey_inventory,
};
pub use key_package::{
    SECURE_MESH_KEYPACKAGE_PROTOCOL_VERSION, SECURE_MESH_KEYPACKAGE_WIRE_CIPHER_SUITE,
    SecureMeshKeyPackageRecord, sign_key_package_record, verify_key_package_record,
};
pub use pairwise::{
    SECURE_MESH_PREKEY_PROTOCOL_VERSION, SecureMeshPairwisePreKeyBundle,
    SecureMeshPreKeyBundleValidation, SecureMeshPreKeyKind, SecureMeshPreKeyRecord,
    SecureMeshPreKeyValidationPolicy, one_time_prekey_batch_digest,
    prekey_public_key_from_base64url, sign_prekey_record, signed_prekey_bundle_digest,
    validate_pairwise_prekey_bundle, verify_prekey_record,
};

#[cfg(test)]
mod tests;
#[cfg(test)]
pub(crate) use tests::support::authorize_test_pairwise_prekey_bundle;
