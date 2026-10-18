use std::collections::BTreeMap;

use anyhow::{Result, anyhow, ensure};
use base64::{Engine as _, engine::general_purpose};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use time::OffsetDateTime;

use crate::core::secure_mesh_capability_proof::SignedCapabilityProof;
use crate::core::secure_mesh_crypto::{SecureMeshContentContext, SecureMeshPayloadKind};
use crate::core::secure_mesh_directory::{
    DirectoryAuthorizationPurpose, UntrustedDirectoryResponse,
};
use crate::core::secure_mesh_trust::{DeviceTrustPublicIdentity, DeviceTrustState};

use super::directory_authorization::{
    authorize_member_directory_response, require_mls_directory_authority_with_local_policy,
};

pub(super) const MAX_GROUP_ID_BYTES: usize = 255;
pub(super) const MAX_KEY_PACKAGE_BYTES: usize = 256 * 1024;
pub(super) const MAX_MLS_MESSAGE_BYTES: usize = 20 * 1024 * 1024;
pub(super) const MAX_PAYLOAD_BYTES: usize = 16 * 1024 * 1024;

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct GroupCreateRequest {
    pub(super) group_id_base64url: String,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub(super) struct MemberAddRequest {
    pub(super) group_id_base64url: String,
    pub(super) member_key_package_id: String,
    pub(super) member_key_package_base64url: String,
    pub(super) member_directory_version: u64,
    pub(super) member_key_package_version: u64,
    pub(super) member_identity: PublicIdentityInput,
    pub(super) member_capability_proof: SignedCapabilityProof,
    pub(super) untrusted_directory_response: UntrustedDirectoryResponse,
    #[serde(default)]
    pub(super) allow_interaction: Option<bool>,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub(super) struct MemberRemoveRequest {
    pub(super) group_id_base64url: String,
    pub(super) expected_epoch: u64,
    pub(super) member_identity: PublicIdentityInput,
    #[serde(default)]
    pub(super) allow_interaction: Option<bool>,
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct GroupJoinRequest {
    pub(super) group_id_base64url: String,
    pub(super) inviter_identity: PublicIdentityInput,
    pub(super) expected_roster_endpoint_ids: Vec<String>,
    pub(super) trusted_roster: Vec<TrustedIdentityInput>,
    pub(super) welcome_message_base64url: String,
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct CommitProcessRequest {
    pub(super) group_id_base64url: String,
    pub(super) committer_identity: PublicIdentityInput,
    pub(super) added_member_identity: Option<PublicIdentityInput>,
    pub(super) removed_member_identity: Option<PublicIdentityInput>,
    pub(super) trusted_roster: Vec<TrustedIdentityInput>,
    pub(super) commit_message_base64url: String,
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct PayloadSealRequest {
    pub(super) group_id_base64url: String,
    pub(super) trusted_roster: Vec<TrustedIdentityInput>,
    pub(super) context: ContentContextInput,
    pub(super) payload_kind: String,
    pub(super) body_base64url: String,
    pub(super) content_type: Option<String>,
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct PayloadOpenRequest {
    pub(super) group_id_base64url: String,
    pub(super) trusted_sender_identity: PublicIdentityInput,
    pub(super) trusted_roster: Vec<TrustedIdentityInput>,
    pub(super) context: ContentContextInput,
    pub(super) expected_payload_kind: String,
    pub(super) message_base64url: String,
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct PublicIdentityInput {
    pub(super) endpoint_id: String,
    pub(super) identity_public_key_base64url: String,
    pub(super) signing_public_key_base64url: String,
    pub(super) rotation_epoch: u64,
}

impl PublicIdentityInput {
    pub(super) fn to_identity(&self) -> Result<DeviceTrustPublicIdentity> {
        DeviceTrustPublicIdentity::new(
            self.endpoint_id.clone(),
            decode_key_32(&self.identity_public_key_base64url, "identity public key")?,
            decode_key_32(&self.signing_public_key_base64url, "signing public key")?,
            self.rotation_epoch,
        )
    }
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct TrustedIdentityInput {
    pub(super) identity: PublicIdentityInput,
    #[serde(default)]
    pub(super) directory_version: Option<u64>,
    #[serde(default)]
    pub(super) key_package_version: Option<u64>,
    #[serde(default)]
    pub(super) key_package_digest: Option<String>,
    #[serde(default)]
    pub(super) untrusted_directory_response: Option<UntrustedDirectoryResponse>,
}

pub(super) struct TrustedRoster {
    pub(super) identities: BTreeMap<String, DeviceTrustPublicIdentity>,
    pub(super) trust_states: BTreeMap<String, DeviceTrustState>,
}

impl TrustedRoster {
    pub(super) fn state_for(
        &self,
        identity: &DeviceTrustPublicIdentity,
    ) -> Result<&DeviceTrustState> {
        let trusted = self.identities.get(&identity.endpoint_id).ok_or_else(|| {
            anyhow!("secure mesh MLS local identity is missing from trusted roster")
        })?;
        ensure!(
            trusted == identity,
            "secure mesh MLS trusted roster local identity binding differs"
        );
        self.trust_states
            .get(&identity.endpoint_id)
            .ok_or_else(|| anyhow!("secure mesh MLS trusted roster state is missing"))
    }
}

pub(super) fn trusted_roster(
    inputs: &[TrustedIdentityInput],
    config: &Value,
    local_identity: &DeviceTrustPublicIdentity,
) -> Result<TrustedRoster> {
    trusted_roster_with_local_policy(inputs, config, local_identity, true)
}

pub(super) fn trusted_roster_with_local_policy(
    inputs: &[TrustedIdentityInput],
    config: &Value,
    local_identity: &DeviceTrustPublicIdentity,
    require_local_member: bool,
) -> Result<TrustedRoster> {
    ensure!(
        !inputs.is_empty() && inputs.len() <= 256,
        "secure mesh MLS trusted roster size is invalid"
    );
    let mut identities = BTreeMap::new();
    let mut trust_states = BTreeMap::new();
    for input in inputs {
        let identity = input.identity.to_identity()?;
        if let Some(response) = input.untrusted_directory_response.clone() {
            ensure!(
                identity != *local_identity,
                "secure mesh MLS local directory refresh must use the self-monitor route"
            );
            let directory_version = input.directory_version.ok_or_else(|| {
                anyhow!("secure mesh MLS roster directory version is required with KT evidence")
            })?;
            let key_package_version = input.key_package_version.ok_or_else(|| {
                anyhow!("secure mesh MLS roster KeyPackage version is required with KT evidence")
            })?;
            let key_package_digest = input.key_package_digest.as_deref().ok_or_else(|| {
                anyhow!("secure mesh MLS roster KeyPackage digest is required with KT evidence")
            })?;
            let now = OffsetDateTime::now_utc();
            authorize_member_directory_response(
                config,
                local_identity,
                response.clone(),
                now,
                DirectoryAuthorizationPurpose::MlsKeyPackage,
                &identity,
                directory_version,
                key_package_digest,
                key_package_version,
            )?;
            authorize_member_directory_response(
                config,
                local_identity,
                response,
                now,
                DirectoryAuthorizationPurpose::MlsMemberAdd,
                &identity,
                directory_version,
                key_package_digest,
                key_package_version,
            )?;
        } else {
            ensure!(
                input.directory_version.is_none()
                    && input.key_package_version.is_none()
                    && input.key_package_digest.is_none(),
                "secure mesh MLS roster KT commitment fields require directory evidence"
            );
        }
        let state = if identity == *local_identity {
            DeviceTrustState::Verified
        } else {
            crate::domain::mobile_relay::persisted_mobile_relay_peer_trust_state(
                config,
                local_identity,
                &identity,
            )?
        };
        ensure!(
            identities
                .insert(identity.endpoint_id.clone(), identity.clone())
                .is_none(),
            "secure mesh MLS trusted roster contains a duplicate endpoint"
        );
        trust_states.insert(identity.endpoint_id.clone(), state);
    }
    require_mls_directory_authority_with_local_policy(
        config,
        local_identity,
        &identities,
        require_local_member,
    )?;
    Ok(TrustedRoster {
        identities,
        trust_states,
    })
}

pub(super) fn reject_caller_asserted_trust(params: &Value) -> Result<()> {
    for field in [
        "memberTrustState",
        "removedMemberTrustState",
        "inviterTrustState",
        "committerTrustState",
        "trustedSenderState",
    ] {
        ensure!(
            params.get(field).is_none(),
            "secure mesh MLS caller-asserted trust state is forbidden"
        );
    }
    if let Some(roster) = params.get("trustedRoster").and_then(Value::as_array) {
        ensure!(
            roster.iter().all(|entry| {
                entry
                    .as_object()
                    .is_some_and(|object| !object.contains_key("trustState"))
            }),
            "secure mesh MLS caller-asserted roster trust state is forbidden"
        );
    }
    Ok(())
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct ContentContextInput {
    pub(super) envelope_id: String,
    pub(super) message_id: String,
    pub(super) opaque_mailbox_id: String,
    pub(super) sender_endpoint_id: String,
    pub(super) recipient_endpoint_id: String,
    pub(super) session_id: String,
    pub(super) created_at: String,
    pub(super) expires_at: String,
}

impl ContentContextInput {
    pub(super) fn to_context(&self) -> SecureMeshContentContext {
        SecureMeshContentContext::new(
            &self.envelope_id,
            &self.message_id,
            &self.opaque_mailbox_id,
            &self.sender_endpoint_id,
            &self.recipient_endpoint_id,
            &self.session_id,
            &self.created_at,
            &self.expires_at,
        )
    }
}

pub(super) fn parse_params<T: for<'de> Deserialize<'de>>(params: &Value) -> Result<T> {
    ensure!(
        params.is_object(),
        "secure mesh MLS action params must be an object"
    );
    serde_json::from_value(params.clone())
        .map_err(|_| anyhow!("secure mesh MLS action params are invalid"))
}

pub(super) fn parse_payload_kind(value: &str) -> Result<SecureMeshPayloadKind> {
    match value {
        "command" => Ok(SecureMeshPayloadKind::Command),
        "result" => Ok(SecureMeshPayloadKind::ResultPayload),
        "error" => Ok(SecureMeshPayloadKind::Error),
        "file_chunk" => Ok(SecureMeshPayloadKind::FileChunk),
        "file_manifest" => Ok(SecureMeshPayloadKind::FileManifest),
        "service_action" => Ok(SecureMeshPayloadKind::ServiceAction),
        "typing_indicator" => Ok(SecureMeshPayloadKind::TypingIndicator),
        "read_receipt" => Ok(SecureMeshPayloadKind::ReadReceipt),
        _ => Err(anyhow!("secure mesh MLS payload kind is invalid")),
    }
}

pub(super) fn identity_to_json(identity: &DeviceTrustPublicIdentity) -> Value {
    json!({
        "endpointId": identity.endpoint_id,
        "identityPublicKeyBase64url": encode_base64url(&identity.identity_public_key),
        "signingPublicKeyBase64url": encode_base64url(&identity.signing_public_key),
        "rotationEpoch": identity.rotation_epoch
    })
}

fn decode_key_32(value: &str, label: &str) -> Result<[u8; 32]> {
    let decoded = decode_base64url(value, label, 32)?;
    ensure!(
        decoded.len() == 32,
        "secure mesh MLS {label} length is invalid"
    );
    decoded
        .try_into()
        .map_err(|_| anyhow!("secure mesh MLS {label} length is invalid"))
}

pub(super) fn decode_base64url(value: &str, label: &str, max_len: usize) -> Result<Vec<u8>> {
    ensure!(
        !value.is_empty() && !value.contains('='),
        "secure mesh {label} encoding is invalid"
    );
    let decoded = general_purpose::URL_SAFE_NO_PAD
        .decode(value)
        .map_err(|_| anyhow!("secure mesh {label} encoding is invalid"))?;
    ensure!(
        !decoded.is_empty() && decoded.len() <= max_len,
        "secure mesh {label} size is invalid"
    );
    ensure!(
        encode_base64url(&decoded) == value,
        "secure mesh {label} encoding is noncanonical"
    );
    Ok(decoded)
}

pub(super) fn encode_base64url(value: &[u8]) -> String {
    general_purpose::URL_SAFE_NO_PAD.encode(value)
}

pub(super) fn hex_sha256(value: &[u8]) -> String {
    Sha256::digest(value)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}
