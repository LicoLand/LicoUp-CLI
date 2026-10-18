use std::collections::BTreeMap;

use anyhow::Result;
use serde_json::{Value, json};

use crate::core::secure_mesh_mls::SecureMeshMlsGroup;
use crate::core::secure_mesh_mls_product::create_product_group;
use crate::core::secure_mesh_trust::DeviceTrustState;

use super::directory_authorization::require_mls_directory_authority;
use super::group_state::{group_status_json, reconcile_group_metadata};
use super::input_codec::{GroupCreateRequest, MAX_GROUP_ID_BYTES, decode_base64url, parse_params};
use super::participant_runtime::{
    ParticipantRequirement, group_state_store, with_local_participant,
};

pub(super) fn group_create(params: &Value) -> Result<Value> {
    let request: GroupCreateRequest = parse_params(params)?;
    let group_id = decode_base64url(
        &request.group_id_base64url,
        "MLS group id",
        MAX_GROUP_ID_BYTES,
    )?;
    with_local_participant(params, ParticipantRequirement::CreateIfMissing, |runtime| {
        let local_roster = BTreeMap::from([(
            runtime.identity.endpoint_id.clone(),
            runtime.identity.clone(),
        )]);
        let directory_readiness =
            require_mls_directory_authority(runtime.config, runtime.identity, &local_roster)?;
        let group = match SecureMeshMlsGroup::load_optional(runtime.participant, &group_id)? {
            Some(group) => group,
            None => create_product_group(
                runtime.participant,
                runtime.identity,
                &DeviceTrustState::Verified,
                &group_id,
            )?,
        };
        runtime.persist_participant()?;
        let metadata = reconcile_group_metadata(
            group_state_store(&mut *runtime.group_store)?,
            &group,
            runtime.identity,
        )?;
        let mut response = group_status_json(&group, &metadata);
        response["directoryAuthority"] = json!({
            "current": true,
            "treeSize": directory_readiness.tree_size,
            "receiptCount": directory_readiness.receipt_count,
            "rootCommitted": !directory_readiness.root_hash.is_empty(),
            "mapRootCommitted": !directory_readiness.map_root_hash.is_empty(),
        });
        Ok((response, false))
    })
}
