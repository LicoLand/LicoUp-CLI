use super::support::*;
#[test]
fn secure_mesh_openmls_runtime_crypto_self_test_passes() {
    assert!(runtime_crypto_self_test());
}
