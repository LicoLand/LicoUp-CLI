use anyhow::Result;
use time::OffsetDateTime;

use super::super::{
    SecureMeshKeyPackageRecord, SecureMeshPairwisePreKeyBundle, SecureMeshPreKeyBundleValidation,
    SecureMeshPreKeyKind, SecureMeshPreKeyRecord, SecureMeshPreKeyValidationPolicy,
    evaluate_prekey_inventory, validate_pairwise_prekey_bundle, verify_key_package_record,
    verify_prekey_record,
};
use crate::core::secure_mesh_directory::AuthorizedDirectoryLeaf;
use crate::core::secure_mesh_trust::{DeviceTrustPublicIdentity, DeviceTrustState};

#[test]
fn facade_preserves_pairwise_keypackage_inventory_and_shared_validation_entrypoints() {
    let _: fn(
        &DeviceTrustPublicIdentity,
        SecureMeshPreKeyKind,
        &SecureMeshPreKeyRecord,
        OffsetDateTime,
    ) -> Result<()> = verify_prekey_record;
    let _: fn(
        &SecureMeshPairwisePreKeyBundle,
        &AuthorizedDirectoryLeaf,
        &SecureMeshPreKeyValidationPolicy,
        OffsetDateTime,
    ) -> Result<SecureMeshPreKeyBundleValidation> = validate_pairwise_prekey_bundle;
    let _: fn(
        &DeviceTrustPublicIdentity,
        DeviceTrustState,
        &SecureMeshKeyPackageRecord,
        bool,
        OffsetDateTime,
    ) -> Result<()> = verify_key_package_record;
    let _ = evaluate_prekey_inventory(true, 1, 1, 1, 1);
}
