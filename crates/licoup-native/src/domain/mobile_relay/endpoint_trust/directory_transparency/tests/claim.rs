use super::super::claim::{build_local_directory_claim, local_pairwise_prekey_bundle_from_config};
use super::support::endpoint_config;
use serde_json::json;

#[test]
fn local_claim_requires_complete_material_and_canonical_digests() {
    assert!(local_pairwise_prekey_bundle_from_config(&json!({})).is_err());
    let config = endpoint_config();
    assert!(
        build_local_directory_claim(&config, &"A".repeat(64), 1, "active", &"b".repeat(64), 1,)
            .is_err()
    );
    let claim =
        build_local_directory_claim(&config, &"a".repeat(64), 1, "active", &"b".repeat(64), 1)
            .unwrap();
    assert_eq!(claim.endpoint.directory_state, "active");
    assert_eq!(claim.key_material.pairwise_prekey_version, 1);
}
