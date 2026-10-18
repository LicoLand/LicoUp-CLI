//! RFC 9162 domain-separated log hashing shared by proof generation and verification.

use anyhow::Result;
use sha2::{Digest, Sha256};

use super::super::constants::{COMBINED_MAP_ROOT_LOG_ENTRY_DOMAIN, HASH_LEN};
use super::super::json_codec::parse_hash;

pub(crate) fn kt_log_leaf_hash(bytes: &[u8]) -> [u8; HASH_LEN] {
    let mut hasher = Sha256::new();
    hasher.update([0x00]);
    hasher.update(bytes);
    hasher.finalize().into()
}

pub(crate) fn map_root_log_leaf_hash(map_root_hash: &str) -> Result<[u8; HASH_LEN]> {
    let mut entry = Vec::with_capacity(COMBINED_MAP_ROOT_LOG_ENTRY_DOMAIN.len() + HASH_LEN);
    entry.extend_from_slice(COMBINED_MAP_ROOT_LOG_ENTRY_DOMAIN);
    entry.extend_from_slice(&parse_hash(map_root_hash)?);
    Ok(kt_log_leaf_hash(&entry))
}

pub(super) fn log_node_hash(left: &[u8; HASH_LEN], right: &[u8; HASH_LEN]) -> [u8; HASH_LEN] {
    let mut hasher = Sha256::new();
    hasher.update([0x01]);
    hasher.update(left);
    hasher.update(right);
    hasher.finalize().into()
}

pub(super) fn empty_log_root() -> [u8; HASH_LEN] {
    Sha256::digest([]).into()
}

#[cfg(any(test, feature = "secure-mesh-acceptance-mock-kt"))]
pub(crate) fn merkle_tree_hash(leaves: &[[u8; HASH_LEN]]) -> [u8; HASH_LEN] {
    match leaves.len() {
        0 => empty_log_root(),
        1 => leaves[0],
        count => {
            let split = largest_power_of_two_less_than(count);
            log_node_hash(
                &merkle_tree_hash(&leaves[..split]),
                &merkle_tree_hash(&leaves[split..]),
            )
        }
    }
}

#[cfg(any(test, feature = "secure-mesh-acceptance-mock-kt"))]
pub(super) fn largest_power_of_two_less_than(value: usize) -> usize {
    debug_assert!(value > 1);
    1usize << (usize::BITS - 1 - (value - 1).leading_zeros())
}
