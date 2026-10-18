use super::test_support::*;

#[test]
fn secure_mesh_pairwise_runtime_self_test_covers_pqxdh_and_triple_ratchet() {
    assert!(runtime_crypto_self_test());
}
