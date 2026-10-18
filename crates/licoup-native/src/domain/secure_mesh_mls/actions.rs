use std::collections::BTreeMap;

use anyhow::{Result, anyhow};
use serde_json::{Value, json};

use crate::core::secure_mesh_mls::{
    SECURE_MESH_GROUP_MLS_PROTOCOL_VERSION, SECURE_MESH_MLS_CIPHER_SUITE,
};
use crate::core::secure_mesh_mls_product::SECURE_MESH_MLS_PRODUCT_POLICY_STATUS;

use super::commit_process::commit_process;
use super::directory_authorization::require_mls_directory_authority;
use super::group_create::group_create;
use super::group_join::group_join;
use super::member_mutation::{member_add, member_remove};
use super::participant_key_package::{key_package_create, participant_ensure};
use super::payload::{payload_open, payload_seal};

pub const SECURE_MESH_MLS_NATIVE_ACTIONS: &[&str] = &[
    "secure_mesh.mls.status",
    "secure_mesh.mls.participant.ensure",
    "secure_mesh.mls.keyPackage.create",
    "secure_mesh.mls.group.create",
    "secure_mesh.mls.member.add",
    "secure_mesh.mls.member.remove",
    "secure_mesh.mls.group.join",
    "secure_mesh.mls.commit.process",
    "secure_mesh.mls.payload.seal",
    "secure_mesh.mls.payload.open",
];

/// Pure wiring probe for process-startup and mobile FFI health checks.
///
/// Product readiness belongs to [`status`], which intentionally evaluates the
/// persisted relay and transparency state. Runtime loading probes must remain
/// side-effect free so they cannot create client state beside an executable or
/// mutate an installed application bundle.
pub fn runtime_binding_wired() -> bool {
    crate::core::secure_mesh_mls::SECURE_MESH_MLS_STATUS.contains("mlkem1024_epoch_hybrid_payload")
        && crate::core::secure_mesh_mls::runtime_crypto_self_test()
        && SECURE_MESH_MLS_PRODUCT_POLICY_STATUS.contains("cryptographic_native_path_wired")
        && SECURE_MESH_GROUP_MLS_PROTOCOL_VERSION.starts_with("licomesh.secure-mesh.group-mls.")
        && SECURE_MESH_MLS_CIPHER_SUITE.starts_with("MLS_")
        && SECURE_MESH_MLS_NATIVE_ACTIONS.len() >= 10
}

pub fn dispatch(action: &str, params: &Value) -> Result<Value> {
    if action != "secure_mesh.mls.status" {
        crate::domain::mobile_relay::ensure_secure_mesh_protected_operation_allowed()?;
    }
    match action {
        "secure_mesh.mls.status" => status(),
        "secure_mesh.mls.participant.ensure" => participant_ensure(params),
        "secure_mesh.mls.keyPackage.create" => key_package_create(params),
        "secure_mesh.mls.group.create" => group_create(params),
        "secure_mesh.mls.member.add" => member_add(params),
        "secure_mesh.mls.member.remove" => member_remove(params),
        "secure_mesh.mls.group.join" => group_join(params),
        "secure_mesh.mls.commit.process" => commit_process(params),
        "secure_mesh.mls.payload.seal" => payload_seal(params),
        "secure_mesh.mls.payload.open" => payload_open(params),
        _ => Err(anyhow!("secure mesh MLS native action is unsupported")),
    }
}

pub fn status() -> Result<Value> {
    let evaluation = crate::domain::mobile_relay::selected_mobile_relay_capability_evaluation()?;
    let directory_readiness = (|| {
        let (config, identity) =
            crate::domain::mobile_relay::secure_mesh_mls_public_directory_context()?;
        let roster = BTreeMap::from([(identity.endpoint_id.clone(), identity.clone())]);
        require_mls_directory_authority(&config, &identity, &roster)
    })();
    let current_directory_receipts = directory_readiness.is_ok();
    let mut blockers = vec!["physical_multi_client_matrix_pending"];
    if !current_directory_receipts {
        blockers.push("current_key_transparency_receipts_unavailable");
    }
    let directory_status = directory_readiness
        .ok()
        .map(|readiness| {
            json!({
                "current": true,
                "treeSize": readiness.tree_size,
                "receiptCount": readiness.receipt_count,
                "rootCommitted": !readiness.root_hash.is_empty(),
                "mapRootCommitted": !readiness.map_root_hash.is_empty(),
            })
        })
        .unwrap_or_else(|| {
            json!({
                "current": false,
                "treeSize": Value::Null,
                "receiptCount": 0,
                "rootCommitted": false,
                "mapRootCommitted": false,
            })
        });
    Ok(json!({
        "ok": true,
        "protocolVersion": SECURE_MESH_GROUP_MLS_PROTOCOL_VERSION,
        "cipherSuite": SECURE_MESH_MLS_CIPHER_SUITE,
        "openMlsControlPlaneCipherSuite": "MLS_128_DHKEMX25519_CHACHA20POLY1305_SHA256_Ed25519",
        "mlKem1024EpochContributionReady": true,
        "hybridPayloadKeyDerivationReady": true,
        "activeGroupRequiresMlKem1024Epoch": true,
        "productPolicyStatus": SECURE_MESH_MLS_PRODUCT_POLICY_STATUS,
        "cryptographicRuntimeWired": true,
        "nativeActionPathWired": true,
        "localPersistedPairTrustGateWired": true,
        "authorizedDirectoryLeafKtAuthorityWired": true,
        "currentDirectoryReceiptGateWired": true,
        "currentDirectoryReceipts": directory_status,
        "clientProductCallSiteAvailable": false,
        "productionPathAvailable": false,
        "productionReady": false,
        "blockers": blockers,
        "selectedCustody": evaluation.custody(),
        "actions": SECURE_MESH_MLS_NATIVE_ACTIONS,
        "rawProoflessApiExposed": false
    }))
}
