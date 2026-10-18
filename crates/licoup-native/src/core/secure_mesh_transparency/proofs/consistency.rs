//! RFC 9162 append-only consistency verification and bounded test proof generation.

use anyhow::{Result, anyhow, ensure};

use super::super::constants::{HASH_LEN, KT_JSON_SAFE_INTEGER_MAX, MAX_CONSISTENCY_PROOF_HASHES};
use super::super::json_codec::{parse_hash, parse_hash_path};
use super::super::model::{SecureMeshKtCachedCheckpoint, SecureMeshKtConsistencyProof};
use super::super::signature::{KtFreshnessPolicy, PinnedKtLogKey, SecureMeshSignedTreeHead};
use super::hash::{empty_log_root, log_node_hash};
#[cfg(any(test, feature = "secure-mesh-acceptance-mock-kt"))]
use super::hash::{largest_power_of_two_less_than, merkle_tree_hash};

pub fn verify_kt_consistency(
    proof: &SecureMeshKtConsistencyProof,
    pin: &PinnedKtLogKey,
    freshness_policy: KtFreshnessPolicy,
    now_epoch_seconds: u64,
    cached: &SecureMeshKtCachedCheckpoint,
) -> Result<SecureMeshKtCachedCheckpoint> {
    proof
        .second_signed_tree_head
        .verify(pin, freshness_policy, now_epoch_seconds)?;
    ensure!(
        proof.first_tree_size == cached.tree_size,
        "secure mesh KT consistency cached tree size mismatch"
    );
    ensure!(
        proof.first_root_hash == cached.root_hash,
        "secure mesh KT consistency cached root mismatch"
    );
    ensure!(
        proof.second_tree_size == proof.second_signed_tree_head.tree_size,
        "secure mesh KT consistency second tree size mismatch"
    );
    ensure!(
        proof.first_tree_size <= proof.second_tree_size,
        "secure mesh KT consistency sizes are invalid"
    );
    ensure!(
        proof.first_tree_size <= KT_JSON_SAFE_INTEGER_MAX
            && proof.second_tree_size <= KT_JSON_SAFE_INTEGER_MAX,
        "secure mesh KT consistency size exceeds the cross-language safe range"
    );
    ensure!(
        proof.path.len() <= MAX_CONSISTENCY_PROOF_HASHES,
        "secure mesh KT consistency proof has too many hashes"
    );
    let first_root = parse_hash(&proof.first_root_hash)?;
    let second_root = parse_hash(&proof.second_signed_tree_head.root_hash)?;
    let path = parse_hash_path(&proof.path, MAX_CONSISTENCY_PROOF_HASHES)?;
    verify_rfc9162_consistency(
        proof.first_tree_size,
        proof.second_tree_size,
        first_root,
        second_root,
        &path,
    )?;
    Ok(checkpoint_from_sth(&proof.second_signed_tree_head))
}

pub(crate) fn checkpoint_from_sth(sth: &SecureMeshSignedTreeHead) -> SecureMeshKtCachedCheckpoint {
    SecureMeshKtCachedCheckpoint {
        tree_size: sth.tree_size,
        root_hash: sth.root_hash.clone(),
        map_root_hash: sth.map_root_hash.clone(),
        issued_at_epoch_seconds: sth.issued_at_epoch_seconds,
    }
}

pub(super) fn verify_rfc9162_consistency(
    first_size: u64,
    second_size: u64,
    first_root: [u8; HASH_LEN],
    second_root: [u8; HASH_LEN],
    path: &[[u8; HASH_LEN]],
) -> Result<()> {
    ensure!(
        first_size <= second_size,
        "secure mesh KT consistency sizes are invalid"
    );
    if first_size == 0 {
        ensure!(
            path.is_empty(),
            "secure mesh KT empty-tree consistency proof must be empty"
        );
        ensure!(
            first_root == empty_log_root(),
            "secure mesh KT empty-tree root is invalid"
        );
        return Ok(());
    }
    if first_size == second_size {
        ensure!(
            path.is_empty(),
            "secure mesh KT equal-size consistency proof must be empty"
        );
        ensure!(
            first_root == second_root,
            "secure mesh KT equal-size roots differ"
        );
        return Ok(());
    }

    let mut fn_index = first_size - 1;
    let mut sn = second_size - 1;
    while fn_index & 1 == 1 {
        fn_index >>= 1;
        sn >>= 1;
    }

    let (mut first_hash, mut second_hash, remaining) = if fn_index == 0 {
        (first_root, first_root, path)
    } else {
        let seed = *path
            .first()
            .ok_or_else(|| anyhow!("secure mesh KT consistency proof is truncated"))?;
        (seed, seed, &path[1..])
    };

    for node in remaining {
        ensure!(sn > 0, "secure mesh KT consistency proof has extra hashes");
        if fn_index & 1 == 1 || fn_index == sn {
            first_hash = log_node_hash(node, &first_hash);
            second_hash = log_node_hash(node, &second_hash);
            while fn_index & 1 == 0 && fn_index != 0 {
                fn_index >>= 1;
                sn >>= 1;
            }
        } else {
            second_hash = log_node_hash(&second_hash, node);
        }
        fn_index >>= 1;
        sn >>= 1;
    }
    ensure!(sn == 0, "secure mesh KT consistency proof is truncated");
    ensure!(
        first_hash == first_root,
        "secure mesh KT consistency first root mismatch"
    );
    ensure!(
        second_hash == second_root,
        "secure mesh KT consistency second root mismatch"
    );
    Ok(())
}

#[cfg(any(test, feature = "secure-mesh-acceptance-mock-kt"))]
pub(crate) fn rfc9162_consistency_path(
    leaves: &[[u8; HASH_LEN]],
    first_size: usize,
) -> Result<Vec<[u8; HASH_LEN]>> {
    ensure!(
        first_size <= leaves.len(),
        "secure mesh KT consistency size is invalid"
    );
    if first_size == 0 || first_size == leaves.len() {
        return Ok(Vec::new());
    }
    fn subproof(
        leaves: &[[u8; HASH_LEN]],
        first_size: usize,
        complete_subtree: bool,
        out: &mut Vec<[u8; HASH_LEN]>,
    ) {
        if first_size == leaves.len() {
            if !complete_subtree {
                out.push(merkle_tree_hash(leaves));
            }
            return;
        }
        let split = largest_power_of_two_less_than(leaves.len());
        if first_size <= split {
            subproof(&leaves[..split], first_size, complete_subtree, out);
            out.push(merkle_tree_hash(&leaves[split..]));
        } else {
            subproof(&leaves[split..], first_size - split, false, out);
            out.push(merkle_tree_hash(&leaves[..split]));
        }
    }
    let mut path = Vec::new();
    subproof(leaves, first_size, true, &mut path);
    ensure!(
        path.len() <= MAX_CONSISTENCY_PROOF_HASHES,
        "secure mesh KT consistency path exceeds bound"
    );
    Ok(path)
}
