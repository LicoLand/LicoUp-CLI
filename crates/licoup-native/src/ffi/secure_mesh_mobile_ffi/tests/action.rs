use super::test_support::*;

#[test]
fn mobile_ffi_native_action_contract_is_shared_by_platform_bridges() {
    assert_eq!(
        MOBILE_RELAY_NATIVE_ACTIONS,
        &[
            "mobile.relay.config.get",
            "mobile.relay.config.set",
            "mobile.relay.pairing.claim",
            "mobile.relay.pairing.status",
            "mobile.relay.commands.createSecure",
            "mobile.relay.commands.resultSecure",
            "mobile.relay.commands.resultReplayProof",
            "mobile.relay.e2ee.status",
            "secure_mesh.status",
            "secure_mesh.kt.configureAuthority",
            "secure_mesh.kt.publicationRequest",
            "secure_mesh.kt.revocationRequest",
            "secure_mesh.kt.provision",
            "secure_mesh.kt.gossip",
            "secure_mesh.kt.selfMonitor",
            "secure_mesh.kt.status",
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
            "secure_mesh.command.execute",
            "secure_mesh.deviceTrust.evaluate",
            "secure_mesh.deviceTrust.verifyQr",
            "secure_mesh.deviceTrust.verifySas",
            "secure_mesh.deviceTrust.rotate",
            "secure_mesh.deviceTrust.revoke",
            "secure_mesh.deviceTrust.recover",
            "secure_mesh.lifecycle.serviceAction",
            "secure_mesh.file.route",
            "secure_mesh.file.receiveDestination",
            "secure_mesh.file.receiveConfirmation",
            "secure_mesh.file.handoffProof",
            "secure_mesh.approval.request",
            "secure_mesh.approval.fanout",
            "secure_mesh.approval.respond",
            "secure_mesh.approval.inbox",
            "secure_mesh.approval.adapterCapability",
        ]
    );
    let mut sorted = MOBILE_RELAY_NATIVE_ACTIONS.to_vec();
    sorted.sort_unstable();
    sorted.dedup();
    assert_eq!(sorted.len(), MOBILE_RELAY_NATIVE_ACTIONS.len());
    assert!(
        crate::domain::mobile_relay::SECURE_MESH_KT_NATIVE_ACTIONS
            .iter()
            .all(|action| MOBILE_RELAY_NATIVE_ACTIONS.contains(action))
    );
    assert!(
        crate::domain::secure_mesh_mls::SECURE_MESH_MLS_NATIVE_ACTIONS
            .iter()
            .all(|action| MOBILE_RELAY_NATIVE_ACTIONS.contains(action))
    );
    assert!(secure_mesh_action_requires_protected_operation_gate(
        "secure_mesh.mls.member.remove"
    ));
    assert!(secure_mesh_action_requires_protected_operation_gate(
        "secure_mesh.command.execute"
    ));
}

#[test]
fn mobile_ffi_unsupported_action_uses_calling_platform_error_code() {
    let response = dispatch_json(
        &json!({
            "action": "mobile.relay.unknown",
            "params": {}
        }),
        "ios_secure_mesh_native_json_action_unsupported",
    )
    .unwrap();
    assert_eq!(response.get("ok").and_then(Value::as_bool), Some(false));
    assert_eq!(
        response.get("code").and_then(Value::as_str),
        Some("ios_secure_mesh_native_json_action_unsupported")
    );
    assert_eq!(
        response.get("action").and_then(Value::as_str),
        Some("mobile.relay.unknown")
    );
}

#[test]
fn mobile_ffi_kt_status_is_routed_and_rejects_unknown_fields() {
    let root = std::env::temp_dir().join(format!(
        "lico-mobile-ffi-kt-status-{}",
        uuid::Uuid::new_v4()
    ));
    let previous =
        licoup_foundation::platform::paths::set_portable_data_dir_override(Some(root.clone()));
    let status = dispatch_json(
        &json!({
            "action": "secure_mesh.kt.status",
            "params": {}
        }),
        "mobile_secure_mesh_native_json_action_unsupported",
    )
    .unwrap();
    assert_eq!(status["ok"], true);
    assert_eq!(status["configured"], false);
    let failure = dispatch_json(
        &json!({
            "action": "secure_mesh.kt.status",
            "params": {"callerAssertedTrust": "verified"}
        }),
        "mobile_secure_mesh_native_json_action_unsupported",
    )
    .unwrap_err()
    .to_string();
    assert_eq!(failure, "native_operation_failed");
    assert!(!failure.contains("callerAssertedTrust"));
    licoup_foundation::platform::paths::set_portable_data_dir_override(previous);
    let _ = std::fs::remove_dir_all(root);
}
