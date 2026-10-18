use anyhow::{Context, Result, anyhow, ensure};
use openmls::prelude::{
    GroupId, MlsGroup, MlsMessageBodyIn, MlsMessageIn, StagedWelcome,
    tls_codec::Deserialize as TlsDeserialize,
};
use openmls_traits::OpenMlsProvider;

use super::capability_extension::{
    SecureMeshMlsCapabilityExtension, decode_secure_mesh_mls_capability_extension,
};
use super::config::secure_mesh_mls_join_config;
use super::group_model::SecureMeshMlsGroup;
use super::participant::SecureMeshMlsParticipant;

impl SecureMeshMlsGroup {
    #[cfg(test)]
    pub(crate) fn join_from_welcome(
        participant: &SecureMeshMlsParticipant,
        welcome_message: &[u8],
    ) -> Result<Self> {
        Self::join_from_welcome_for_runtime_crypto_self_test(participant, welcome_message)
    }

    pub(super) fn join_from_welcome_for_runtime_crypto_self_test(
        participant: &SecureMeshMlsParticipant,
        welcome_message: &[u8],
    ) -> Result<Self> {
        let welcome = match MlsMessageIn::tls_deserialize_exact(welcome_message)
            .context("secure mesh MLS welcome deserialization failed")?
            .extract()
        {
            MlsMessageBodyIn::Welcome(welcome) => welcome,
            _ => return Err(anyhow!("secure mesh MLS message is not a welcome")),
        };
        let staged_join = StagedWelcome::new_from_welcome(
            &participant.provider,
            &secure_mesh_mls_join_config(),
            welcome,
            None,
        )
        .map_err(|error| anyhow!("secure mesh MLS staged welcome failed: {error:?}"))?;
        ensure!(
            matches!(
                decode_secure_mesh_mls_capability_extension(
                    staged_join.group_context().extensions()
                )?,
                SecureMeshMlsCapabilityExtension::AwaitingMemberNegotiation { .. }
            ),
            "secure mesh MLS active capability extension requires product verification"
        );
        let group = staged_join
            .into_group(&participant.provider)
            .map_err(|error| anyhow!("secure mesh MLS welcome join failed: {error:?}"))?;
        Self::from_authenticated_group(participant, group)
    }

    pub(crate) fn join_from_welcome_with_capability_verifier(
        participant: &SecureMeshMlsParticipant,
        welcome_message: &[u8],
        verifier: impl FnOnce(&SecureMeshMlsCapabilityExtension) -> Result<()>,
    ) -> Result<Self> {
        let welcome = match MlsMessageIn::tls_deserialize_exact(welcome_message)
            .context("secure mesh MLS welcome deserialization failed")?
            .extract()
        {
            MlsMessageBodyIn::Welcome(welcome) => welcome,
            _ => return Err(anyhow!("secure mesh MLS message is not a welcome")),
        };
        let staged_join = StagedWelcome::new_from_welcome(
            &participant.provider,
            &secure_mesh_mls_join_config(),
            welcome,
            None,
        )
        .map_err(|error| anyhow!("secure mesh MLS staged welcome failed: {error:?}"))?;
        let extension =
            decode_secure_mesh_mls_capability_extension(staged_join.group_context().extensions())?;
        extension.require_active()?;
        verifier(&extension)?;
        let group = staged_join
            .into_group(&participant.provider)
            .map_err(|error| anyhow!("secure mesh MLS welcome join failed: {error:?}"))?;
        let joined = Self::from_authenticated_group(participant, group)?;
        ensure!(
            joined.capability_extension()? == extension,
            "secure mesh MLS joined capability extension mismatch"
        );
        Ok(joined)
    }

    pub fn load_from_provider(
        participant: &SecureMeshMlsParticipant,
        group_id: impl AsRef<[u8]>,
    ) -> Result<Self> {
        let group_id = GroupId::from_slice(group_id.as_ref());
        let group = MlsGroup::load(participant.provider.storage(), &group_id)
            .map_err(|error| anyhow!("secure mesh MLS group storage load failed: {error:?}"))?
            .ok_or_else(|| anyhow!("secure mesh MLS group is missing from provider storage"))?;
        Self::from_authenticated_group(participant, group)
    }
}
