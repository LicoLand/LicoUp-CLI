use serde_json::json;

use super::super::actions::{
    SECURE_MESH_MLS_NATIVE_ACTIONS, dispatch, runtime_binding_wired, status,
};
use super::super::input_codec::reject_caller_asserted_trust;

#[test]
fn mls_action_registry_preserves_the_stable_native_surface() {
    assert_eq!(
        SECURE_MESH_MLS_NATIVE_ACTIONS,
        [
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
        ]
    );
    assert!(runtime_binding_wired());
    assert_eq!(
        dispatch("secure_mesh.mls.status", &json!({})).unwrap()["ok"],
        true
    );
}

#[test]
fn mls_status_keeps_runtime_wiring_distinct_from_production_readiness() {
    let root = std::env::temp_dir().join(format!(
        "lico-mls-status-readiness-{}",
        uuid::Uuid::new_v4()
    ));
    let previous =
        licoup_foundation::platform::paths::set_portable_data_dir_override(Some(root.clone()));
    let status = status().unwrap();
    assert_eq!(status["cryptographicRuntimeWired"], true);
    assert_eq!(status["nativeActionPathWired"], true);
    assert_eq!(status["localPersistedPairTrustGateWired"], true);
    assert_eq!(status["authorizedDirectoryLeafKtAuthorityWired"], true);
    assert_eq!(status["currentDirectoryReceiptGateWired"], true);
    assert_eq!(status["clientProductCallSiteAvailable"], false);
    assert_eq!(status["productionPathAvailable"], false);
    assert_eq!(status["productionReady"], false);
    assert_eq!(
        status["blockers"],
        json!([
            "physical_multi_client_matrix_pending",
            "current_key_transparency_receipts_unavailable"
        ])
    );
    licoup_foundation::platform::paths::set_portable_data_dir_override(previous);
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn mls_product_requests_reject_every_caller_asserted_trust_field() {
    for field in [
        "memberTrustState",
        "removedMemberTrustState",
        "inviterTrustState",
        "committerTrustState",
        "trustedSenderState",
    ] {
        let mut params = json!({});
        params[field] = json!("verified");
        let error = reject_caller_asserted_trust(&params).unwrap_err();
        assert!(error.to_string().contains("caller-asserted trust"));
    }
    let roster_error = reject_caller_asserted_trust(&json!({
        "trustedRoster": [{"identity": {}, "trustState": "verified"}]
    }))
    .unwrap_err();
    assert!(roster_error.to_string().contains("caller-asserted roster"));
}
