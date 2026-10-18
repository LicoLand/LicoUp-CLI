use anyhow::{Result, anyhow};
use openmls::prelude::{GroupId, MlsGroup};

use super::config::secure_mesh_mls_create_config;
use super::group_model::SecureMeshMlsGroup;
use super::participant::SecureMeshMlsParticipant;

impl SecureMeshMlsGroup {
    pub fn create(owner: &SecureMeshMlsParticipant, group_id: impl AsRef<[u8]>) -> Result<Self> {
        let group = MlsGroup::new_with_group_id(
            &owner.provider,
            &owner.signer,
            &secure_mesh_mls_create_config(),
            GroupId::from_slice(group_id.as_ref()),
            owner.credential_with_key.clone(),
        )
        .map_err(|error| anyhow!("secure mesh MLS group creation failed: {error:?}"))?;
        Self::from_authenticated_group(owner, group)
    }
}
