use super::super::{SECURE_MESH_DIAGNOSTIC_HASH_CHAIN_STATUS, diagnostic_hash_chain_tree_head};

#[test]
fn diagnostic_hash_chain_is_deterministic_and_explicitly_non_authorizing() {
    let leaf_hash = "11".repeat(32);
    let genesis = diagnostic_hash_chain_tree_head("", &leaf_hash);
    let repeated = diagnostic_hash_chain_tree_head("", &leaf_hash);
    let advanced = diagnostic_hash_chain_tree_head(&genesis, &leaf_hash);

    assert_eq!(genesis, repeated);
    assert_ne!(genesis, advanced);
    assert_eq!(genesis.len(), 64);
    assert!(SECURE_MESH_DIAGNOSTIC_HASH_CHAIN_STATUS.contains("non_authorizing"));
}
