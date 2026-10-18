use super::super::crypto_operation::PairwiseDirectoryGate;

#[test]
fn ciphertext_operations_distinguish_data_and_gossip_directory_gates() {
    assert_ne!(
        PairwiseDirectoryGate::Required,
        PairwiseDirectoryGate::KtGossipControl
    );
}
