use std::collections::{BTreeMap, BTreeSet};

use anyhow::{Context, Result, anyhow, ensure};
use openmls::prelude::Extensions;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::core::secure_mesh_capability::SecurityCapability;
use crate::core::secure_mesh_capability_proof::SignedCapabilityProof;
use crate::core::secure_mesh_mls_pq_epoch::{
    MLS_ML_KEM_1024_EPOCH_EXTENSION_TYPE_ID, SecureMeshMlsMlKem1024EpochExtension,
};
use crate::core::secure_mesh_session_negotiation::NegotiatedCapabilityBinding;

use super::constants::{MLS_CAPABILITY_EXTENSION_SCHEMA_VERSION, MLS_CAPABILITY_EXTENSION_TYPE_ID};

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub(crate) struct SecureMeshMlsMemberCapabilityProof {
    pub endpoint_id: String,
    pub accepted_at_unix_seconds: i64,
    pub proof: SignedCapabilityProof,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(
    deny_unknown_fields,
    tag = "kind",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
pub(crate) enum SecureMeshMlsRosterTransition {
    MemberAdded {
        member_endpoint_id: String,
        pair_binding: NegotiatedCapabilityBinding,
    },
    MemberRemoved {
        member_endpoint_id: String,
    },
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(
    deny_unknown_fields,
    tag = "state",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
pub(crate) enum SecureMeshMlsCapabilityExtension {
    AwaitingMemberNegotiation {
        schema_version: u32,
    },
    Active {
        schema_version: u32,
        activated_at_epoch: u64,
        previous_extension_digest: Option<String>,
        committer_endpoint_id: String,
        roster_transition: Box<SecureMeshMlsRosterTransition>,
        member_capability_proofs: BTreeMap<String, SecureMeshMlsMemberCapabilityProof>,
        group_negotiated_protocol_capabilities: BTreeSet<SecurityCapability>,
    },
}

impl SecureMeshMlsCapabilityExtension {
    pub(super) fn awaiting_member_negotiation() -> Self {
        Self::AwaitingMemberNegotiation {
            schema_version: MLS_CAPABILITY_EXTENSION_SCHEMA_VERSION,
        }
    }

    pub(crate) fn require_active(&self) -> Result<()> {
        match self {
            Self::Active { schema_version, .. }
                if *schema_version == MLS_CAPABILITY_EXTENSION_SCHEMA_VERSION =>
            {
                Ok(())
            }
            Self::Active { .. } => Err(anyhow!(
                "secure mesh MLS capability extension schema is unsupported"
            )),
            Self::AwaitingMemberNegotiation { .. } => Err(anyhow!(
                "secure mesh MLS member capability negotiation is incomplete"
            )),
        }
    }

    pub(crate) fn group_negotiated_protocol_capabilities(
        &self,
    ) -> Option<&BTreeSet<SecurityCapability>> {
        match self {
            Self::Active {
                group_negotiated_protocol_capabilities,
                ..
            } => Some(group_negotiated_protocol_capabilities),
            Self::AwaitingMemberNegotiation { .. } => None,
        }
    }
}

pub(super) fn secure_mesh_mls_leaf_capabilities() -> openmls::prelude::Capabilities {
    openmls::prelude::Capabilities::new(
        None,
        // Explicit capabilities are validated under the strict Reject policy,
        // so the leaf must list the ciphersuite it actually uses.
        Some(&[super::config::secure_mesh_mls_ciphersuite()]),
        Some(&[
            openmls::prelude::ExtensionType::Unknown(MLS_CAPABILITY_EXTENSION_TYPE_ID),
            openmls::prelude::ExtensionType::Unknown(MLS_ML_KEM_1024_EPOCH_EXTENSION_TYPE_ID),
        ]),
        None,
        Some(&[openmls::prelude::CredentialType::Basic]),
    )
}

pub(super) fn secure_mesh_mls_group_context_extensions(
    capability_extension: &SecureMeshMlsCapabilityExtension,
) -> Result<Extensions<openmls::prelude::GroupContext>> {
    secure_mesh_mls_group_context_extensions_with_pq(capability_extension, None)
}

pub(super) fn secure_mesh_mls_group_context_extensions_with_pq(
    capability_extension: &SecureMeshMlsCapabilityExtension,
    pq_epoch_extension: Option<&SecureMeshMlsMlKem1024EpochExtension>,
) -> Result<Extensions<openmls::prelude::GroupContext>> {
    let encoded = serde_json::to_vec(capability_extension)
        .context("secure mesh MLS capability extension serialization failed")?;
    let mut extensions = vec![
        openmls::prelude::Extension::RequiredCapabilities(
            openmls::prelude::RequiredCapabilitiesExtension::new(
                &[
                    openmls::prelude::ExtensionType::Unknown(MLS_CAPABILITY_EXTENSION_TYPE_ID),
                    openmls::prelude::ExtensionType::Unknown(
                        MLS_ML_KEM_1024_EPOCH_EXTENSION_TYPE_ID,
                    ),
                ],
                &[],
                &[openmls::prelude::CredentialType::Basic],
            ),
        ),
        openmls::prelude::Extension::Unknown(
            MLS_CAPABILITY_EXTENSION_TYPE_ID,
            openmls::prelude::UnknownExtension(encoded),
        ),
    ];
    if let Some(pq_epoch_extension) = pq_epoch_extension {
        extensions.push(openmls::prelude::Extension::Unknown(
            MLS_ML_KEM_1024_EPOCH_EXTENSION_TYPE_ID,
            openmls::prelude::UnknownExtension(
                serde_json::to_vec(pq_epoch_extension)
                    .context("secure mesh MLS ML-KEM-1024 epoch extension serialization failed")?,
            ),
        ));
    }
    Extensions::try_from(extensions)
        .map_err(|error| anyhow!("secure mesh MLS capability extensions are invalid: {error:?}"))
}

pub(super) fn decode_secure_mesh_mls_pq_epoch_extension(
    extensions: &Extensions<openmls::prelude::GroupContext>,
) -> Result<SecureMeshMlsMlKem1024EpochExtension> {
    let encoded = extensions
        .unknown(MLS_ML_KEM_1024_EPOCH_EXTENSION_TYPE_ID)
        .ok_or_else(|| anyhow!("secure mesh MLS ML-KEM-1024 epoch extension is missing"))?;
    serde_json::from_slice(&encoded.0)
        .map_err(|_| anyhow!("secure mesh MLS ML-KEM-1024 epoch extension is invalid"))
}

pub(super) fn decode_secure_mesh_mls_capability_extension(
    extensions: &Extensions<openmls::prelude::GroupContext>,
) -> Result<SecureMeshMlsCapabilityExtension> {
    let encoded = extensions
        .unknown(MLS_CAPABILITY_EXTENSION_TYPE_ID)
        .ok_or_else(|| anyhow!("secure mesh MLS capability extension is missing"))?;
    let extension: SecureMeshMlsCapabilityExtension = serde_json::from_slice(&encoded.0)
        .map_err(|_| anyhow!("secure mesh MLS capability extension is invalid"))?;
    match &extension {
        SecureMeshMlsCapabilityExtension::AwaitingMemberNegotiation { schema_version }
        | SecureMeshMlsCapabilityExtension::Active { schema_version, .. } => ensure!(
            *schema_version == MLS_CAPABILITY_EXTENSION_SCHEMA_VERSION,
            "secure mesh MLS capability extension schema is unsupported"
        ),
    }
    Ok(extension)
}

pub(crate) fn secure_mesh_mls_capability_extension_digest(
    extension: &SecureMeshMlsCapabilityExtension,
) -> Result<String> {
    let encoded = serde_json::to_vec(extension)
        .context("secure mesh MLS capability extension digest encoding failed")?;
    let digest: [u8; 32] = Sha256::digest(encoded).into();
    Ok(crate::core::secure_mesh_capability_proof::encode_sha256_digest(&digest))
}
