use std::collections::{BTreeMap, BTreeSet};

use anyhow::{Context, Result, anyhow, ensure};
use base64::{Engine as _, engine::general_purpose};
use openmls::prelude::{
    BasicCredential, Credential, CredentialType, GroupId, LeafNodeIndex, MlsGroup,
    MlsMessageBodyOut, ProcessedMessage, Sender, tls_codec::Serialize as TlsSerialize,
};
use openmls_traits::OpenMlsProvider;
use sha2::{Digest, Sha256};

use crate::core::secure_mesh_mls_pq_epoch::{
    SecureMeshMlsMlKem1024EpochExtension, mlkem1024_epoch_extension_digest, mlkem1024_member_id,
    open_mlkem1024_epoch_extension,
};
use crate::core::secure_mesh_pqxdh::validate_ml_kem_1024_public_key;

use super::capability_extension::{
    SecureMeshMlsCapabilityExtension, decode_secure_mesh_mls_capability_extension,
    decode_secure_mesh_mls_pq_epoch_extension, secure_mesh_mls_capability_extension_digest,
};
use super::codec::{append_mls_len_prefixed_bytes, hash_bytes};
use super::constants::{SECURE_MESH_GROUP_MLS_PROTOCOL_VERSION, SECURE_MESH_MLS_CIPHER_SUITE};
use super::durable_store::SecureMeshMlsGroupMetadata;
use super::group_model::SecureMeshMlsGroup;
use super::key_package::SecureMeshMlsKeyPackage;
use super::participant::SecureMeshMlsParticipant;

impl SecureMeshMlsGroup {
    pub(crate) fn load(
        participant: &SecureMeshMlsParticipant,
        group_id: impl AsRef<[u8]>,
    ) -> Result<Self> {
        Self::load_optional(participant, group_id)?
            .ok_or_else(|| anyhow!("secure mesh MLS group is not available in selected custody"))
    }

    pub(crate) fn load_optional(
        participant: &SecureMeshMlsParticipant,
        group_id: impl AsRef<[u8]>,
    ) -> Result<Option<Self>> {
        let group_id = GroupId::from_slice(group_id.as_ref());
        let group = MlsGroup::load(participant.provider.storage(), &group_id)
            .map_err(|error| anyhow!("secure mesh MLS group load failed: {error:?}"))?;
        match group {
            Some(group) => Ok(Some(Self::from_authenticated_group(participant, group)?)),
            None => Ok(None),
        }
    }

    pub(crate) fn capability_extension(&self) -> Result<SecureMeshMlsCapabilityExtension> {
        decode_secure_mesh_mls_capability_extension(self.group.extensions())
    }

    pub(crate) fn mlkem1024_epoch_extension(&self) -> Result<SecureMeshMlsMlKem1024EpochExtension> {
        decode_secure_mesh_mls_pq_epoch_extension(self.group.extensions())
    }

    pub(super) fn current_mlkem1024_recipient_public_keys(
        &self,
        participant: &SecureMeshMlsParticipant,
    ) -> Result<BTreeMap<String, Vec<u8>>> {
        if let Some(extension) = &self.mlkem1024_epoch_extension {
            return extension
                .recipients
                .iter()
                .map(|(member_id, wrap)| {
                    let public_key = general_purpose::URL_SAFE_NO_PAD
                        .decode(&wrap.public_key_base64url)
                        .context("secure mesh MLS ML-KEM-1024 roster key is not base64url")?;
                    validate_ml_kem_1024_public_key(&public_key)?;
                    Ok((member_id.clone(), public_key))
                })
                .collect();
        }
        ensure!(
            matches!(
                self.capability_extension()?,
                SecureMeshMlsCapabilityExtension::AwaitingMemberNegotiation { .. }
            ) && self.member_count() == 1,
            "secure mesh MLS active group ML-KEM-1024 epoch state is missing"
        );
        Ok(BTreeMap::from([(
            mlkem1024_member_id(&participant.credential_identity_bytes()?)?,
            participant.provider.mlkem1024_seed.public_key(),
        )]))
    }

    pub fn require_active_capability_negotiation(&self) -> Result<()> {
        self.capability_extension()?.require_active()
    }

    pub fn own_leaf_index(&self) -> LeafNodeIndex {
        self.group.own_leaf_index()
    }

    pub fn member_count(&self) -> usize {
        self.group.members().count()
    }

    pub fn is_active(&self) -> bool {
        self.group.is_active()
    }

    pub fn epoch(&self) -> u64 {
        self.group.epoch().as_u64()
    }

    pub fn group_id_bytes(&self) -> Result<Vec<u8>> {
        Ok(self.group.group_id().as_slice().to_vec())
    }

    pub(crate) fn capability_add_base_transcript_digest(
        &self,
        key_package: &SecureMeshMlsKeyPackage,
    ) -> Result<String> {
        let mut transcript = Vec::with_capacity(96);
        transcript.extend_from_slice(b"LICO-SM-MLS-CAPABILITY-ADD-BASE-v1");
        append_mls_len_prefixed_bytes(
            &mut transcript,
            SECURE_MESH_GROUP_MLS_PROTOCOL_VERSION.as_bytes(),
        )?;
        append_mls_len_prefixed_bytes(&mut transcript, SECURE_MESH_MLS_CIPHER_SUITE.as_bytes())?;
        append_mls_len_prefixed_bytes(&mut transcript, self.group.group_id().as_slice())?;
        transcript.extend_from_slice(&self.epoch().to_be_bytes());
        let mut member_credentials = self.member_credential_identities()?;
        member_credentials.sort_unstable();
        transcript.extend_from_slice(&(member_credentials.len() as u32).to_be_bytes());
        for credential in member_credentials {
            append_mls_len_prefixed_bytes(&mut transcript, &credential)?;
        }
        append_mls_len_prefixed_bytes(
            &mut transcript,
            secure_mesh_mls_capability_extension_digest(&self.capability_extension()?)?.as_bytes(),
        )?;
        append_mls_len_prefixed_bytes(&mut transcript, key_package.as_public_bytes())?;
        transcript.extend_from_slice(&self.epoch().saturating_add(1).to_be_bytes());
        let digest: [u8; 32] = Sha256::digest(transcript).into();
        Ok(crate::core::secure_mesh_capability_proof::encode_sha256_digest(&digest))
    }

    pub fn member_credential_identities(&self) -> Result<Vec<Vec<u8>>> {
        Ok(self
            .member_credential_signing_pairs()?
            .into_iter()
            .map(|(credential, _)| credential)
            .collect())
    }

    pub fn member_credential_signing_pairs(&self) -> Result<Vec<(Vec<u8>, Vec<u8>)>> {
        let mut pairs = Vec::new();
        for member in self.group.members() {
            pairs.push((
                basic_credential_identity(&member.credential)?,
                member.signature_key.as_slice().to_vec(),
            ));
        }
        Ok(pairs)
    }

    pub(crate) fn member_leaf_index_for_identity(
        &self,
        credential_identity: &[u8],
        signing_public_key: &[u8],
    ) -> Result<LeafNodeIndex> {
        let mut matched = None;
        for member in self.group.members() {
            if basic_credential_identity(&member.credential)? == credential_identity
                && member.signature_key.as_slice() == signing_public_key
            {
                ensure!(
                    matched.replace(member.index).is_none(),
                    "secure mesh MLS identity resolves to multiple member leaves"
                );
            }
        }
        matched.ok_or_else(|| {
            anyhow!("secure mesh MLS identity does not resolve to an exact member leaf")
        })
    }

    pub fn public_metadata(
        &self,
        participant_endpoint_id: impl Into<String>,
    ) -> Result<SecureMeshMlsGroupMetadata> {
        let group_id = self.group.group_id().as_slice();
        let mut public_state = Vec::new();
        public_state.extend_from_slice(b"LICO-SM-MLS-PUBLIC-STATE-v1");
        append_mls_len_prefixed_bytes(&mut public_state, group_id)?;
        append_mls_len_prefixed_bytes(&mut public_state, &self.authenticated_group_context)?;
        public_state.extend_from_slice(&self.epoch().to_be_bytes());
        public_state.extend_from_slice(&self.own_leaf_index().u32().to_be_bytes());
        public_state.push(u8::from(self.is_active()));
        append_mls_len_prefixed_bytes(
            &mut public_state,
            secure_mesh_mls_capability_extension_digest(&self.capability_extension()?)?.as_bytes(),
        )?;
        if let Some(pq_epoch_extension) = &self.mlkem1024_epoch_extension {
            append_mls_len_prefixed_bytes(
                &mut public_state,
                mlkem1024_epoch_extension_digest(pq_epoch_extension)?.as_bytes(),
            )?;
        }
        let mut roster = self
            .group
            .members()
            .map(|member| {
                Ok((
                    basic_credential_identity(&member.credential)?,
                    member.signature_key.as_slice().to_vec(),
                ))
            })
            .collect::<Result<Vec<_>>>()?;
        roster.sort_unstable();
        public_state.extend_from_slice(&(roster.len() as u32).to_be_bytes());
        for (credential, signing_public_key) in roster {
            append_mls_len_prefixed_bytes(&mut public_state, &credential)?;
            append_mls_len_prefixed_bytes(&mut public_state, &signing_public_key)?;
        }
        Ok(SecureMeshMlsGroupMetadata {
            group_id_hash: hash_bytes(group_id),
            public_state_digest: hash_bytes(&public_state),
            epoch: self.epoch(),
            member_count: self.member_count(),
            own_leaf_index: self.own_leaf_index().u32(),
            active: self.is_active(),
            participant_endpoint_id: participant_endpoint_id.into(),
        })
    }

    pub(super) fn from_authenticated_group(
        participant: &SecureMeshMlsParticipant,
        group: MlsGroup,
    ) -> Result<Self> {
        let authenticated_group_context = authenticated_group_context_bytes(&group, participant)?;
        let mut result = Self {
            group,
            authenticated_group_context,
            mlkem1024_epoch_extension: None,
            mlkem1024_epoch_secret: None,
        };
        result.refresh_mlkem1024_epoch_state(participant)?;
        Ok(result)
    }

    pub(super) fn refresh_authenticated_group_context(
        &mut self,
        participant: &SecureMeshMlsParticipant,
    ) -> Result<()> {
        self.authenticated_group_context =
            authenticated_group_context_bytes(&self.group, participant)?;
        self.refresh_mlkem1024_epoch_state(participant)?;
        Ok(())
    }

    pub(super) fn refresh_mlkem1024_epoch_state(
        &mut self,
        participant: &SecureMeshMlsParticipant,
    ) -> Result<()> {
        let capability_extension = self.capability_extension()?;
        if matches!(
            capability_extension,
            SecureMeshMlsCapabilityExtension::AwaitingMemberNegotiation { .. }
        ) {
            self.mlkem1024_epoch_extension = None;
            self.mlkem1024_epoch_secret = None;
            return Ok(());
        }
        capability_extension.require_active()?;
        let extension = self.mlkem1024_epoch_extension()?;
        if !self.is_active() {
            self.mlkem1024_epoch_extension = Some(extension);
            self.mlkem1024_epoch_secret = None;
            return Ok(());
        }
        let expected_member_ids = self
            .member_credential_identities()?
            .into_iter()
            .map(|identity| mlkem1024_member_id(&identity))
            .collect::<Result<BTreeSet<_>>>()?;
        let secret = open_mlkem1024_epoch_extension(
            self.group.group_id().as_slice(),
            self.epoch(),
            &expected_member_ids,
            &participant.credential_identity_bytes()?,
            &participant.provider.mlkem1024_seed,
            &extension,
        )?;
        self.mlkem1024_epoch_extension = Some(extension);
        self.mlkem1024_epoch_secret = Some(secret);
        Ok(())
    }

    pub(super) fn authenticated_member_sender(
        &self,
        processed: &ProcessedMessage,
    ) -> Result<(Vec<u8>, Vec<u8>, LeafNodeIndex)> {
        let leaf_index = match processed.sender() {
            Sender::Member(leaf_index) => *leaf_index,
            _ => {
                return Err(anyhow!(
                    "secure mesh MLS product message sender is not a group member"
                ));
            }
        };
        let member = self
            .group
            .members()
            .find(|member| member.index == leaf_index)
            .ok_or_else(|| anyhow!("secure mesh MLS authenticated sender leaf is missing"))?;
        ensure!(
            member.credential == *processed.credential(),
            "secure mesh MLS authenticated sender credential does not match leaf"
        );
        let credential_identity = basic_credential_identity(processed.credential())?;
        Ok((credential_identity, member.signature_key, leaf_index))
    }
}

fn authenticated_group_context_bytes(
    group: &MlsGroup,
    participant: &SecureMeshMlsParticipant,
) -> Result<Vec<u8>> {
    let group_info = group
        .export_group_info(participant.provider.crypto(), &participant.signer, false)
        .map_err(|error| {
            anyhow!("secure mesh MLS authenticated group context export failed: {error:?}")
        })?;
    let MlsMessageBodyOut::GroupInfo(group_info) = group_info.body() else {
        return Err(anyhow!(
            "secure mesh MLS authenticated group context export returned an invalid body"
        ));
    };
    group_info
        .group_context()
        .tls_serialize_detached()
        .context("secure mesh MLS authenticated group context serialization failed")
}

pub(super) fn basic_credential_identity(credential: &Credential) -> Result<Vec<u8>> {
    if matches!(credential.credential_type(), CredentialType::Basic) {
        return Ok(credential.serialized_content().to_vec());
    }
    let basic = BasicCredential::try_from(credential.clone())
        .map_err(|error| anyhow!("secure mesh MLS sender credential is not basic: {error:?}"))?;
    Ok(basic.identity().to_vec())
}
