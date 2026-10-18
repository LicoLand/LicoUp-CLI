//! Non-authorizing diagnostic hash-chain helper.

use sha2::{Digest, Sha256};

use super::json_codec::hex_encode;

/// Diagnostic-only unsigned hash-chain helper. It cannot construct an authorized leaf.
pub fn diagnostic_hash_chain_tree_head(previous_tree_head: &str, leaf_hash: &str) -> String {
    let previous = if previous_tree_head.is_empty() {
        "GENESIS"
    } else {
        previous_tree_head
    };
    hex_encode(&Sha256::digest(
        format!("{previous}:{leaf_hash}").as_bytes(),
    ))
}
