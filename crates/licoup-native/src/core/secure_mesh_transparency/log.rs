//! Test and explicit acceptance-only transparency authority and proof builder.

use anyhow::{Result, anyhow, ensure};
use ed25519_dalek::VerifyingKey;

use super::constants::{
    HASH_LEN, MAX_TRANSPARENCY_LEAVES, SECURE_MESH_KT_PROTOCOL_VERSION, SPARSE_MAP_DEPTH,
};
use super::json_codec::{hex_encode, sth_sign_payload, validate_hex_hash};
use super::model::{
    SecureMeshKtConsistencyProof, SecureMeshKtInclusionProof, SecureMeshKtMapEntry,
    SecureMeshKtMapProof, SecureMeshKtNonInclusionProof, SecureMeshTransparencyLeafBody,
};
use super::proofs::{
    map_root_log_leaf_hash, merkle_tree_hash, rfc9162_consistency_path, rfc9162_inclusion_path,
};
use super::signature::{PinnedKtLogKey, SecureMeshSignedTreeHead};
use super::sparse_map::{
    bit_at, sparse_map_default_hashes, sparse_map_key, sparse_map_leaf_hash, sparse_map_node_hash,
};

/// Separately operated test/acceptance authority. This type and its private key are absent from
/// ordinary builds and rejected in release profiles, while production code receives only
/// `PinnedKtLogKey` plus untrusted proof objects.
#[cfg(any(test, feature = "secure-mesh-acceptance-mock-kt"))]
pub struct SecureMeshKtLog {
    log_signing_key: ed25519_dalek::SigningKey,
    log_id: String,
    key_id: String,
    leaves: Vec<[u8; HASH_LEN]>,
    latest: std::collections::BTreeMap<String, SecureMeshKtMapEntry>,
}

#[cfg(any(test, feature = "secure-mesh-acceptance-mock-kt"))]
impl SecureMeshKtLog {
    pub fn new(log_signing_key: ed25519_dalek::SigningKey) -> Self {
        Self::with_identity(log_signing_key, "test-log", "test-key")
    }

    pub fn with_identity(
        log_signing_key: ed25519_dalek::SigningKey,
        log_id: impl Into<String>,
        key_id: impl Into<String>,
    ) -> Self {
        Self {
            log_signing_key,
            log_id: log_id.into(),
            key_id: key_id.into(),
            leaves: Vec::new(),
            latest: std::collections::BTreeMap::new(),
        }
    }

    pub fn pin(&self) -> PinnedKtLogKey {
        PinnedKtLogKey::from_acceptance_mock_ed25519_bytes(
            self.log_id.clone(),
            self.key_id.clone(),
            *self.log_signing_key.verifying_key().as_bytes(),
        )
        .expect("test KT pin")
    }

    pub fn log_verifying_key(&self) -> VerifyingKey {
        self.log_signing_key.verifying_key()
    }

    pub fn tree_size(&self) -> u64 {
        self.leaves.len() as u64
    }

    pub fn append_leaf(&mut self, body: &SecureMeshTransparencyLeafBody) -> Result<u64> {
        self.append_hashed_directory_leaf(
            &body.directory_key(),
            body.rotation_epoch,
            body.is_revoked(),
            body.leaf_hash()?,
        )
    }

    pub fn append_hashed_directory_leaf(
        &mut self,
        stable_label: &str,
        version: u64,
        revoked: bool,
        leaf_hash: [u8; HASH_LEN],
    ) -> Result<u64> {
        ensure!(
            self.leaves.len() < MAX_TRANSPARENCY_LEAVES,
            "secure mesh KT test log has too many leaves"
        );
        validate_hex_hash("stable_label", stable_label)?;
        if let Some(previous) = self.latest.get(stable_label) {
            ensure!(
                version > previous.version,
                "secure mesh KT directory version must increase"
            );
            ensure!(
                !(previous.revoked && !revoked),
                "secure mesh KT test authority refuses revoked resurrection"
            );
        }
        self.latest.insert(
            stable_label.to_string(),
            SecureMeshKtMapEntry {
                leaf_hash: hex_encode(&leaf_hash),
                version,
                revoked,
            },
        );
        let leaf_index = self.leaves.len() as u64;
        self.leaves
            .push(map_root_log_leaf_hash(&hex_encode(&self.map_root_hash()))?);
        Ok(leaf_index)
    }

    pub fn force_append_hashed_directory_leaf_for_adversarial_test(
        &mut self,
        stable_label: &str,
        version: u64,
        revoked: bool,
        leaf_hash: [u8; HASH_LEN],
    ) -> Result<u64> {
        ensure!(
            self.leaves.len() < MAX_TRANSPARENCY_LEAVES,
            "secure mesh KT test log has too many leaves"
        );
        validate_hex_hash("stable_label", stable_label)?;
        self.latest.insert(
            stable_label.to_string(),
            SecureMeshKtMapEntry {
                leaf_hash: hex_encode(&leaf_hash),
                version,
                revoked,
            },
        );
        let leaf_index = self.leaves.len() as u64;
        self.leaves
            .push(map_root_log_leaf_hash(&hex_encode(&self.map_root_hash()))?);
        Ok(leaf_index)
    }

    pub fn force_remove_map_label_for_adversarial_test(
        &mut self,
        stable_label: &str,
    ) -> Result<u64> {
        self.latest.remove(stable_label);
        let leaf_index = self.leaves.len() as u64;
        self.leaves
            .push(map_root_log_leaf_hash(&hex_encode(&self.map_root_hash()))?);
        Ok(leaf_index)
    }

    pub fn append_empty_map_checkpoint_for_test(&mut self) -> Result<u64> {
        ensure!(
            self.latest.is_empty(),
            "secure mesh KT test map is not empty"
        );
        let leaf_index = self.leaves.len() as u64;
        self.leaves
            .push(map_root_log_leaf_hash(&hex_encode(&self.map_root_hash()))?);
        Ok(leaf_index)
    }

    pub fn append_current_map_checkpoint_for_test(&mut self) -> Result<u64> {
        ensure!(
            self.leaves.len() < MAX_TRANSPARENCY_LEAVES,
            "secure mesh KT test log has too many leaves"
        );
        let leaf_index = self.leaves.len() as u64;
        self.leaves
            .push(map_root_log_leaf_hash(&hex_encode(&self.map_root_hash()))?);
        Ok(leaf_index)
    }

    pub fn mark_revoked_placeholder(
        &mut self,
        body: &SecureMeshTransparencyLeafBody,
    ) -> Result<(u64, SecureMeshTransparencyLeafBody)> {
        let mut revoked = body.clone();
        revoked.rotation_epoch = body.rotation_epoch.saturating_add(1);
        revoked.directory_state = "revoked".to_string();
        let index = self.append_leaf(&revoked)?;
        Ok((index, revoked))
    }

    pub fn root_hash(&self) -> [u8; HASH_LEN] {
        merkle_tree_hash(&self.leaves)
    }

    pub fn map_root_hash(&self) -> [u8; HASH_LEN] {
        self.sparse_levels()[0]
            .get(&[0u8; HASH_LEN])
            .copied()
            .unwrap_or_else(|| sparse_map_default_hashes()[0])
    }

    pub fn sign_tree_head(&self, issued_at_epoch_seconds: u64) -> Result<SecureMeshSignedTreeHead> {
        use ed25519_dalek::Signer;

        let mut sth = SecureMeshSignedTreeHead {
            protocol_version: SECURE_MESH_KT_PROTOCOL_VERSION.to_string(),
            log_id: self.log_id.clone(),
            key_id: self.key_id.clone(),
            tree_size: self.tree_size(),
            root_hash: hex_encode(&self.root_hash()),
            map_root_hash: hex_encode(&self.map_root_hash()),
            issued_at_epoch_seconds,
            signature: String::new(),
        };
        sth.signature = hex_encode(
            &self
                .log_signing_key
                .sign(&sth_sign_payload(&sth)?)
                .to_bytes(),
        );
        Ok(sth)
    }

    pub fn inclusion_proof(&self, leaf_index: u64) -> Result<SecureMeshKtInclusionProof> {
        self.inclusion_proof_at(leaf_index, 0)
    }

    pub fn inclusion_proof_at(
        &self,
        leaf_index: u64,
        issued_at_epoch_seconds: u64,
    ) -> Result<SecureMeshKtInclusionProof> {
        let index = usize::try_from(leaf_index)
            .map_err(|_| anyhow!("secure mesh KT test leaf index is invalid"))?;
        ensure!(
            index < self.leaves.len(),
            "secure mesh KT leaf index is out of range"
        );
        Ok(SecureMeshKtInclusionProof {
            leaf_index,
            tree_size: self.tree_size(),
            leaf_hash: hex_encode(&self.leaves[index]),
            siblings: rfc9162_inclusion_path(&self.leaves, index)?
                .iter()
                .map(|hash| hex_encode(hash))
                .collect(),
            signed_tree_head: self.sign_tree_head(issued_at_epoch_seconds)?,
        })
    }

    pub fn consistency_proof(&self, first_tree_size: u64) -> Result<SecureMeshKtConsistencyProof> {
        self.consistency_proof_at(first_tree_size, 0)
    }

    pub fn consistency_proof_at(
        &self,
        first_tree_size: u64,
        issued_at_epoch_seconds: u64,
    ) -> Result<SecureMeshKtConsistencyProof> {
        let first = usize::try_from(first_tree_size)
            .map_err(|_| anyhow!("secure mesh KT consistency size is invalid"))?;
        ensure!(
            first <= self.leaves.len(),
            "secure mesh KT consistency size is invalid"
        );
        Ok(SecureMeshKtConsistencyProof {
            first_tree_size,
            second_tree_size: self.tree_size(),
            first_root_hash: hex_encode(&merkle_tree_hash(&self.leaves[..first])),
            path: rfc9162_consistency_path(&self.leaves, first)?
                .iter()
                .map(|hash| hex_encode(hash))
                .collect(),
            second_signed_tree_head: self.sign_tree_head(issued_at_epoch_seconds)?,
        })
    }

    pub fn map_proof_at(
        &self,
        stable_label: &str,
        issued_at_epoch_seconds: u64,
    ) -> Result<SecureMeshKtMapProof> {
        validate_hex_hash("stable_label", stable_label)?;
        let key = sparse_map_key(stable_label);
        let levels = self.sparse_levels();
        let defaults = sparse_map_default_hashes();
        let mut siblings = Vec::with_capacity(SPARSE_MAP_DEPTH);
        for depth in (0..SPARSE_MAP_DEPTH).rev() {
            let mut sibling_prefix = normalized_prefix(&key, depth + 1);
            flip_bit(&mut sibling_prefix, depth);
            siblings.push(hex_encode(
                levels[depth + 1]
                    .get(&sibling_prefix)
                    .unwrap_or(&defaults[depth + 1]),
            ));
        }
        Ok(SecureMeshKtMapProof {
            stable_label: stable_label.to_string(),
            entry: self.latest.get(stable_label).cloned(),
            siblings,
            signed_tree_head: self.sign_tree_head(issued_at_epoch_seconds)?,
        })
    }

    pub fn non_inclusion_proof(&self, stable_label: &str) -> Result<SecureMeshKtNonInclusionProof> {
        let proof = self.map_proof_at(stable_label, 0)?;
        ensure!(
            proof.entry.is_none(),
            "secure mesh KT directory label is present"
        );
        Ok(proof)
    }

    fn sparse_levels(&self) -> Vec<std::collections::BTreeMap<[u8; HASH_LEN], [u8; HASH_LEN]>> {
        let defaults = sparse_map_default_hashes();
        let mut levels = (0..=SPARSE_MAP_DEPTH)
            .map(|_| std::collections::BTreeMap::new())
            .collect::<Vec<_>>();
        for (stable_label, entry) in &self.latest {
            let key = sparse_map_key(stable_label);
            levels[SPARSE_MAP_DEPTH].insert(key, sparse_map_leaf_hash(&key, entry));
        }
        for depth in (0..SPARSE_MAP_DEPTH).rev() {
            let mut parents = std::collections::BTreeMap::<
                [u8; HASH_LEN],
                (Option<[u8; HASH_LEN]>, Option<[u8; HASH_LEN]>),
            >::new();
            for (child_prefix, child_hash) in &levels[depth + 1] {
                let parent_prefix = normalized_prefix(child_prefix, depth);
                let children = parents.entry(parent_prefix).or_default();
                if bit_at(child_prefix, depth) == 0 {
                    children.0 = Some(*child_hash);
                } else {
                    children.1 = Some(*child_hash);
                }
            }
            for (parent_prefix, (left, right)) in parents {
                let hash = sparse_map_node_hash(
                    &left.unwrap_or(defaults[depth + 1]),
                    &right.unwrap_or(defaults[depth + 1]),
                );
                if hash != defaults[depth] {
                    levels[depth].insert(parent_prefix, hash);
                }
            }
        }
        levels
    }
}

#[cfg(any(test, feature = "secure-mesh-acceptance-mock-kt"))]
fn normalized_prefix(key: &[u8; HASH_LEN], depth: usize) -> [u8; HASH_LEN] {
    let mut out = *key;
    if depth >= SPARSE_MAP_DEPTH {
        return out;
    }
    let full_bytes = depth / 8;
    let remaining_bits = depth % 8;
    if remaining_bits == 0 {
        out[full_bytes..].fill(0);
    } else {
        out[full_bytes] &= 0xff << (8 - remaining_bits);
        out[full_bytes + 1..].fill(0);
    }
    out
}

#[cfg(any(test, feature = "secure-mesh-acceptance-mock-kt"))]
fn flip_bit(key: &mut [u8; HASH_LEN], depth: usize) {
    key[depth / 8] ^= 1 << (7 - (depth % 8));
}
