//! Authenticated sparse-directory inclusion and non-inclusion proofs.

use anyhow::{Result, anyhow, ensure};
use std::sync::OnceLock;

use super::constants::{
    HASH_LEN, KT_JSON_SAFE_INTEGER_MAX, MAP_EMPTY_DOMAIN, MAP_KEY_DOMAIN, MAP_LEAF_DOMAIN,
    MAP_NODE_DOMAIN, SPARSE_MAP_DEPTH,
};
use super::json_codec::{domain_hash, hex_encode, parse_hash, parse_hash_path, validate_hex_hash};
use super::model::{SecureMeshKtMapEntry, SecureMeshKtMapProof, SecureMeshKtNonInclusionProof};
use super::signature::{KtFreshnessPolicy, PinnedKtLogKey, VerifiedKtFreshness};

pub fn verify_kt_map_inclusion(
    proof: &SecureMeshKtMapProof,
    expected_stable_label: &str,
    expected_leaf_hash: &str,
    expected_version: u64,
    expected_revoked: bool,
    pin: &PinnedKtLogKey,
    freshness_policy: KtFreshnessPolicy,
    now_epoch_seconds: u64,
) -> Result<VerifiedKtFreshness> {
    ensure!(
        proof.stable_label == expected_stable_label,
        "secure mesh KT sparse-map label substitution detected"
    );
    let entry = proof
        .entry
        .as_ref()
        .ok_or_else(|| anyhow!("secure mesh KT sparse-map entry is absent"))?;
    ensure!(
        entry.leaf_hash == expected_leaf_hash,
        "secure mesh KT sparse-map leaf is not latest"
    );
    ensure!(
        entry.version == expected_version,
        "secure mesh KT sparse-map version is not latest"
    );
    ensure!(
        entry.revoked == expected_revoked,
        "secure mesh KT sparse-map revocation state mismatch"
    );
    verify_kt_map_proof(proof, pin, freshness_policy, now_epoch_seconds)
}

pub fn verify_kt_non_inclusion(
    proof: &SecureMeshKtNonInclusionProof,
    expected_stable_label: &str,
    pin: &PinnedKtLogKey,
    freshness_policy: KtFreshnessPolicy,
    now_epoch_seconds: u64,
) -> Result<VerifiedKtFreshness> {
    ensure!(
        proof.stable_label == expected_stable_label,
        "secure mesh KT sparse-map label substitution detected"
    );
    ensure!(
        proof.entry.is_none(),
        "secure mesh KT sparse-map label is present; non-inclusion rejected"
    );
    verify_kt_map_proof(proof, pin, freshness_policy, now_epoch_seconds)
}

fn verify_kt_map_proof(
    proof: &SecureMeshKtMapProof,
    pin: &PinnedKtLogKey,
    freshness_policy: KtFreshnessPolicy,
    now_epoch_seconds: u64,
) -> Result<VerifiedKtFreshness> {
    let freshness = proof
        .signed_tree_head
        .verify(pin, freshness_policy, now_epoch_seconds)?;
    validate_hex_hash("stable_label", &proof.stable_label)?;
    ensure!(
        proof.siblings.len() == SPARSE_MAP_DEPTH,
        "secure mesh KT sparse-map proof depth is invalid"
    );
    let key = sparse_map_key(&proof.stable_label);
    let mut current = match &proof.entry {
        Some(entry) => {
            validate_hex_hash("map leaf_hash", &entry.leaf_hash)?;
            ensure!(
                entry.version <= KT_JSON_SAFE_INTEGER_MAX,
                "secure mesh KT sparse-map version exceeds the cross-language safe range"
            );
            sparse_map_leaf_hash(&key, entry)
        }
        None => sparse_map_default_hashes()[SPARSE_MAP_DEPTH],
    };
    let siblings = parse_hash_path(&proof.siblings, SPARSE_MAP_DEPTH)?;
    for (offset, sibling) in siblings.iter().enumerate() {
        let depth = SPARSE_MAP_DEPTH - 1 - offset;
        current = if bit_at(&key, depth) == 0 {
            sparse_map_node_hash(&current, sibling)
        } else {
            sparse_map_node_hash(sibling, &current)
        };
    }
    ensure!(
        hex_encode(&current) == proof.signed_tree_head.map_root_hash,
        "secure mesh KT sparse-map root mismatch"
    );
    Ok(freshness)
}

pub(super) fn sparse_map_key(stable_label: &str) -> [u8; HASH_LEN] {
    domain_hash(MAP_KEY_DOMAIN, stable_label.as_bytes())
}

pub(super) fn sparse_map_leaf_hash(
    key: &[u8; HASH_LEN],
    entry: &SecureMeshKtMapEntry,
) -> [u8; HASH_LEN] {
    let mut transcript = Vec::with_capacity(HASH_LEN * 2 + 9);
    transcript.extend_from_slice(key);
    // Entry hashes are validated before production verification reaches this helper.
    transcript.extend_from_slice(&parse_hash(&entry.leaf_hash).unwrap_or([0; HASH_LEN]));
    transcript.extend_from_slice(&entry.version.to_be_bytes());
    transcript.push(u8::from(entry.revoked));
    domain_hash(MAP_LEAF_DOMAIN, &transcript)
}

pub(super) fn sparse_map_node_hash(
    left: &[u8; HASH_LEN],
    right: &[u8; HASH_LEN],
) -> [u8; HASH_LEN] {
    let mut transcript = Vec::with_capacity(HASH_LEN * 2);
    transcript.extend_from_slice(left);
    transcript.extend_from_slice(right);
    domain_hash(MAP_NODE_DOMAIN, &transcript)
}

pub(super) fn sparse_map_default_hashes() -> &'static [[u8; HASH_LEN]; SPARSE_MAP_DEPTH + 1] {
    static DEFAULTS: OnceLock<[[u8; HASH_LEN]; SPARSE_MAP_DEPTH + 1]> = OnceLock::new();
    DEFAULTS.get_or_init(|| {
        let mut defaults = [[0u8; HASH_LEN]; SPARSE_MAP_DEPTH + 1];
        defaults[SPARSE_MAP_DEPTH] = domain_hash(MAP_EMPTY_DOMAIN, b"");
        for depth in (0..SPARSE_MAP_DEPTH).rev() {
            defaults[depth] = sparse_map_node_hash(&defaults[depth + 1], &defaults[depth + 1]);
        }
        defaults
    })
}

pub(super) fn bit_at(key: &[u8; HASH_LEN], depth: usize) -> u8 {
    (key[depth / 8] >> (7 - (depth % 8))) & 1
}
