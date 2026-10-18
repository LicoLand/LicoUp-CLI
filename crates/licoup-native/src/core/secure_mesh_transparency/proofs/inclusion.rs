//! RFC 9162 inclusion proof verification and bounded acceptance-test path generation.

use anyhow::{Result, ensure};

use super::super::constants::{HASH_LEN, MAX_INCLUSION_PROOF_HASHES};
use super::super::json_codec::{hex_encode, parse_hash, parse_hash_path};
use super::super::model::SecureMeshKtInclusionProof;
use super::super::signature::{KtFreshnessPolicy, PinnedKtLogKey, VerifiedKtFreshness};
use super::hash::log_node_hash;
#[cfg(any(test, feature = "secure-mesh-acceptance-mock-kt"))]
use super::hash::{largest_power_of_two_less_than, merkle_tree_hash};

pub fn verify_kt_inclusion(
    proof: &SecureMeshKtInclusionProof,
    pin: &PinnedKtLogKey,
    freshness_policy: KtFreshnessPolicy,
    now_epoch_seconds: u64,
) -> Result<VerifiedKtFreshness> {
    let freshness = proof
        .signed_tree_head
        .verify(pin, freshness_policy, now_epoch_seconds)?;
    ensure!(
        proof.tree_size == proof.signed_tree_head.tree_size,
        "secure mesh KT inclusion tree size mismatch"
    );
    ensure!(
        proof.tree_size > 0,
        "secure mesh KT inclusion tree is empty"
    );
    ensure!(
        proof.leaf_index < proof.tree_size,
        "secure mesh KT inclusion leaf index is out of range"
    );
    ensure!(
        proof.siblings.len() <= MAX_INCLUSION_PROOF_HASHES,
        "secure mesh KT inclusion proof has too many hashes"
    );
    let leaf = parse_hash(&proof.leaf_hash)?;
    let path = parse_hash_path(&proof.siblings, MAX_INCLUSION_PROOF_HASHES)?;
    let root = fold_rfc9162_inclusion(leaf, proof.leaf_index, proof.tree_size, &path)?;
    ensure!(
        hex_encode(&root) == proof.signed_tree_head.root_hash,
        "secure mesh KT inclusion root mismatch"
    );
    Ok(freshness)
}

pub(super) fn fold_rfc9162_inclusion(
    mut hash: [u8; HASH_LEN],
    leaf_index: u64,
    tree_size: u64,
    path: &[[u8; HASH_LEN]],
) -> Result<[u8; HASH_LEN]> {
    ensure!(tree_size > 0, "secure mesh KT inclusion tree is empty");
    ensure!(
        leaf_index < tree_size,
        "secure mesh KT inclusion index is invalid"
    );
    let mut fn_index = leaf_index;
    let mut sn = tree_size - 1;
    for sibling in path {
        ensure!(sn > 0, "secure mesh KT inclusion proof has extra hashes");
        if fn_index & 1 == 1 || fn_index == sn {
            hash = log_node_hash(sibling, &hash);
            while fn_index & 1 == 0 && fn_index != 0 {
                fn_index >>= 1;
                sn >>= 1;
            }
        } else {
            hash = log_node_hash(&hash, sibling);
        }
        fn_index >>= 1;
        sn >>= 1;
    }
    ensure!(sn == 0, "secure mesh KT inclusion proof is truncated");
    Ok(hash)
}

#[cfg(any(test, feature = "secure-mesh-acceptance-mock-kt"))]
pub(crate) fn rfc9162_inclusion_path(
    leaves: &[[u8; HASH_LEN]],
    leaf_index: usize,
) -> Result<Vec<[u8; HASH_LEN]>> {
    ensure!(
        leaf_index < leaves.len(),
        "secure mesh KT inclusion index is invalid"
    );
    fn recurse(leaves: &[[u8; HASH_LEN]], index: usize, out: &mut Vec<[u8; HASH_LEN]>) {
        if leaves.len() <= 1 {
            return;
        }
        let split = largest_power_of_two_less_than(leaves.len());
        if index < split {
            recurse(&leaves[..split], index, out);
            out.push(merkle_tree_hash(&leaves[split..]));
        } else {
            recurse(&leaves[split..], index - split, out);
            out.push(merkle_tree_hash(&leaves[..split]));
        }
    }
    let mut path = Vec::new();
    recurse(leaves, leaf_index, &mut path);
    ensure!(
        path.len() <= MAX_INCLUSION_PROOF_HASHES,
        "secure mesh KT inclusion path exceeds bound"
    );
    Ok(path)
}
