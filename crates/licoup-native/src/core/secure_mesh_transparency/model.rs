//! Wire-safe proof models and privacy-preserving directory labels.

use anyhow::{Result, anyhow, ensure};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::constants::{
    DIRECTORY_LABEL_DOMAIN, DIRECTORY_SCOPE_COMMITMENT_DOMAIN, HASH_LEN,
    MAX_TRANSPARENCY_FIELD_BYTES, SECURE_MESH_KT_GOSSIP_CONTENT_TYPE,
};
use super::json_codec::{
    append_len_prefixed, canonical_json, consistency_to_json, domain_hash, hex_encode,
    parse_consistency_json, parse_sth_json, required_json_text, sth_to_json, validate_leaf_body,
};
use super::proofs::kt_log_leaf_hash;
use super::signature::SecureMeshSignedTreeHead;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct SecureMeshTransparencyLeafBody {
    /// Opaque commitment computed locally from the directory tenancy scope. Raw tenant,
    /// account, and workspace identifiers never enter a directory proof or peer descriptor.
    pub directory_scope_commitment: String,
    pub endpoint_id: String,
    pub endpoint_kind: String,
    pub identity_public_key: String,
    pub signing_public_key: String,
    pub fingerprint: String,
    pub rotation_epoch: u64,
    pub directory_state: String,
    pub updated_at: String,
}

impl SecureMeshTransparencyLeafBody {
    /// RFC 9162-style leaf hash: `SHA-256(0x00 || canonical_leaf_bytes)`.
    pub fn leaf_hash(&self) -> Result<[u8; HASH_LEN]> {
        validate_leaf_body(self)?;
        let value = serde_json::json!({
            "directoryScopeCommitment": self.directory_scope_commitment,
            "endpointId": self.endpoint_id,
            "endpointKind": self.endpoint_kind,
            "identityPublicKey": self.identity_public_key,
            "signingPublicKey": self.signing_public_key,
            "fingerprint": self.fingerprint,
            "rotationEpoch": self.rotation_epoch,
            "directoryState": self.directory_state,
            "updatedAt": self.updated_at,
        });
        Ok(kt_log_leaf_hash(canonical_json(&value)?.as_bytes()))
    }

    pub fn leaf_hash_hex(&self) -> Result<String> {
        Ok(hex_encode(&self.leaf_hash()?))
    }

    /// Stable, unambiguous logical directory label. Version and mutable keys are excluded.
    pub fn directory_key(&self) -> String {
        stable_directory_label(&self.directory_scope_commitment, &self.endpoint_id)
    }

    pub fn is_revoked(&self) -> bool {
        self.directory_state.eq_ignore_ascii_case("revoked")
    }
}

pub fn directory_scope_commitment(tenant_id: &str, account_id: &str, workspace_id: &str) -> String {
    let mut transcript = Vec::new();
    append_len_prefixed(&mut transcript, tenant_id.as_bytes());
    append_len_prefixed(&mut transcript, account_id.as_bytes());
    append_len_prefixed(&mut transcript, workspace_id.as_bytes());
    hex_encode(&domain_hash(DIRECTORY_SCOPE_COMMITMENT_DOMAIN, &transcript))
}

pub fn stable_directory_label(directory_scope_commitment: &str, endpoint_id: &str) -> String {
    let mut transcript = Vec::new();
    append_len_prefixed(&mut transcript, directory_scope_commitment.as_bytes());
    append_len_prefixed(&mut transcript, endpoint_id.as_bytes());
    hex_encode(&domain_hash(DIRECTORY_LABEL_DOMAIN, &transcript))
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct SecureMeshKtInclusionProof {
    pub leaf_index: u64,
    pub tree_size: u64,
    pub leaf_hash: String,
    /// RFC 9162 audit path, ordered from the leaf level towards the root.
    pub siblings: Vec<String>,
    pub signed_tree_head: SecureMeshSignedTreeHead,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct SecureMeshKtConsistencyProof {
    pub first_tree_size: u64,
    pub second_tree_size: u64,
    pub first_root_hash: String,
    /// RFC 9162 consistency path. Never contains the full leaf set.
    pub path: Vec<String>,
    pub second_signed_tree_head: SecureMeshSignedTreeHead,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct SecureMeshKtMapEntry {
    pub leaf_hash: String,
    pub version: u64,
    pub revoked: bool,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct SecureMeshKtMapProof {
    pub stable_label: String,
    /// `None` proves non-inclusion through the sparse-map empty leaf.
    pub entry: Option<SecureMeshKtMapEntry>,
    /// Exactly 256 hashes, ordered from leaf level to root.
    pub siblings: Vec<String>,
    pub signed_tree_head: SecureMeshSignedTreeHead,
}

pub type SecureMeshKtNonInclusionProof = SecureMeshKtMapProof;

pub(crate) struct DirectoryComponentCommitments<'a> {
    pub identity_fingerprint: &'a str,
    pub identity_rotation_epoch: u64,
    pub identity_key_digest: &'a str,
    pub pairwise_prekey_version: u64,
    pub signed_prekey_digest: &'a str,
    pub one_time_prekey_digest: &'a str,
    pub mls_key_package_version: u64,
    pub mls_key_package_digest: &'a str,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SecureMeshKtCachedCheckpoint {
    pub tree_size: u64,
    pub root_hash: String,
    pub map_root_hash: String,
    pub issued_at_epoch_seconds: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SecureMeshKtAuthorizationReceipt {
    pub stable_label: String,
    pub purpose: String,
    pub directory_version: u64,
    pub leaf_hash: String,
    pub revoked: bool,
    pub tree_size: u64,
    pub root_hash: String,
    pub map_root_hash: String,
    pub issued_at_epoch_seconds: u64,
    pub observed_at_epoch_seconds: u64,
    pub validated_at_epoch_seconds: u64,
    pub expires_at_epoch_seconds: u64,
    pub identity_fingerprint: String,
    pub identity_rotation_epoch: u64,
    pub identity_key_digest: String,
    pub pairwise_prekey_version: u64,
    pub signed_prekey_digest: String,
    pub one_time_prekey_digest: String,
    pub mls_key_package_version: u64,
    pub mls_key_package_digest: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct SecureMeshKtGossipPayload {
    pub content_type: String,
    pub signed_tree_head: SecureMeshSignedTreeHead,
    pub consistency_proof: Option<SecureMeshKtConsistencyProof>,
}

impl SecureMeshKtGossipPayload {
    pub fn from_sth(
        signed_tree_head: SecureMeshSignedTreeHead,
        consistency_proof: Option<SecureMeshKtConsistencyProof>,
    ) -> Self {
        Self {
            content_type: SECURE_MESH_KT_GOSSIP_CONTENT_TYPE.to_string(),
            signed_tree_head,
            consistency_proof,
        }
    }

    pub fn to_json_bytes(&self) -> Result<Vec<u8>> {
        let consistency = self.consistency_proof.as_ref().map(consistency_to_json);
        Ok(serde_json::to_vec(&serde_json::json!({
            "contentType": self.content_type,
            "signedTreeHead": sth_to_json(&self.signed_tree_head),
            "consistencyProof": consistency,
        }))?)
    }

    pub fn from_json_bytes(bytes: &[u8]) -> Result<Self> {
        ensure!(
            bytes.len() <= MAX_TRANSPARENCY_FIELD_BYTES * 4,
            "secure mesh KT gossip payload is too large"
        );
        let value: Value = serde_json::from_slice(bytes)
            .map_err(|_| anyhow!("secure mesh KT gossip payload is invalid"))?;
        let sth = parse_sth_json(
            value
                .get("signedTreeHead")
                .ok_or_else(|| anyhow!("secure mesh KT gossip signedTreeHead is required"))?,
        )?;
        let consistency_proof = match value.get("consistencyProof") {
            None | Some(Value::Null) => None,
            Some(proof) => Some(parse_consistency_json(proof)?),
        };
        Ok(Self {
            content_type: required_json_text(&value, "contentType")?,
            signed_tree_head: sth,
            consistency_proof,
        })
    }
}
