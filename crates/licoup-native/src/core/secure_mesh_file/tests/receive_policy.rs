use super::support::*;

#[test]
fn secure_mesh_file_receive_destination_redacts_local_paths_and_metadata() {
    let manifest = SecureMeshFileManifest {
        file_id: "file-receive-policy-canary".to_string(),
        file_name: "settlement-private-file-canary.xlsx".to_string(),
        mime_type: "application/x-private-spreadsheet-canary".to_string(),
        relative_path: "approved/subdir/private-relative-canary".to_string(),
        total_size: 16,
        chunk_size: 8,
        chunk_count: 2,
    };
    let approved_root = std::env::temp_dir()
        .join("lico-approved-root-canary")
        .join(uuid::Uuid::new_v4().to_string());
    let decision = evaluate_file_receive_destination_json(&json!({
        "manifest": manifest_json(&manifest),
        "approvedRoot": approved_root.to_string_lossy(),
        "conflictPolicy": "fail_if_exists"
    }))
    .unwrap();

    assert_eq!(decision["receivePolicy"]["destinationApproved"], true);
    assert_eq!(decision["receivePolicy"]["destinationPathRedacted"], true);
    assert_eq!(
        decision["receivePolicy"]["conflictPolicy"],
        "fail_if_exists"
    );
    assert_eq!(decision["manifest"]["metadataEncrypted"], true);
    assert_eq!(decision["manifest"]["bodyRedacted"], true);
    let serialized = serde_json::to_string(&decision).unwrap();
    for forbidden in [
        "file-receive-policy-canary",
        "settlement-private-file-canary.xlsx",
        "application/x-private-spreadsheet-canary",
        "private-relative-canary",
        "lico-approved-root-canary",
        approved_root.to_string_lossy().as_ref(),
    ] {
        assert!(
            !serialized.contains(forbidden),
            "receive destination decision leaked {forbidden}"
        );
    }
    assert!(serialized.contains("approvedRootHash"));
    assert!(serialized.contains("resolvedPathHash"));
}

#[test]
fn secure_mesh_file_receive_destination_rejects_unapproved_paths() {
    let mut manifest = manifest_fixture();
    manifest.file_name = "../evil.txt".to_string();
    let rejected_name = evaluate_file_receive_destination_json(&json!({
        "manifest": manifest_json(&manifest),
        "approvedRoot": std::env::temp_dir().to_string_lossy()
    }))
    .unwrap_err();
    assert!(rejected_name.to_string().contains("path separators"));

    let mut manifest = manifest_fixture();
    manifest.relative_path = "safe/../../escape".to_string();
    let rejected_traversal = evaluate_file_receive_destination_json(&json!({
        "manifest": manifest_json(&manifest),
        "approvedRoot": std::env::temp_dir().to_string_lossy()
    }))
    .unwrap_err();
    assert!(rejected_traversal.to_string().contains("must not traverse"));

    let rejected_root = evaluate_file_receive_destination_json(&json!({
        "manifest": manifest_json(&manifest_fixture()),
        "approvedRoot": "relative-root"
    }))
    .unwrap_err();
    assert!(rejected_root.to_string().contains("must be absolute"));
}

#[test]
fn secure_mesh_file_receive_confirmation_requires_user_action_and_disables_auto_open() {
    let manifest = SecureMeshFileManifest {
        file_id: "file-confirmation-policy-canary".to_string(),
        file_name: "private-confirmation-file-canary.pdf".to_string(),
        mime_type: "application/x-confirmation-canary".to_string(),
        relative_path: "phone/private-confirmation-relative-canary".to_string(),
        total_size: 16,
        chunk_size: 8,
        chunk_count: 2,
    };
    let approved_root = std::env::temp_dir()
        .join("lico-confirmation-approved-root-canary")
        .join(uuid::Uuid::new_v4().to_string());
    let pending = evaluate_file_receive_confirmation_json(&json!({
        "manifest": manifest_json(&manifest),
        "approvedRoot": approved_root.to_string_lossy()
    }))
    .unwrap();

    assert_eq!(pending["receiveConfirmation"]["required"], true);
    assert_eq!(
        pending["receiveConfirmation"]["userVisibleConfirmationRequired"],
        true
    );
    assert_eq!(pending["receiveConfirmation"]["userConfirmed"], false);
    assert_eq!(pending["receiveConfirmation"]["writeAllowed"], false);
    assert_eq!(
        pending["receiveConfirmation"]["localWriteDeferredUntilConfirmed"],
        true
    );
    assert_eq!(
        pending["receiveConfirmation"]["decryptedBytesHiddenUntilConfirmed"],
        true
    );
    assert_eq!(pending["receiveConfirmation"]["autoPreviewEnabled"], false);
    assert_eq!(
        pending["receiveConfirmation"]["autoIngestionEnabled"],
        false
    );

    let confirmed = evaluate_file_receive_confirmation_json(&json!({
        "manifest": manifest_json(&manifest),
        "approvedRoot": approved_root.to_string_lossy(),
        "userConfirmed": true
    }))
    .unwrap();
    assert_eq!(confirmed["receiveConfirmation"]["userConfirmed"], true);
    assert_eq!(confirmed["receiveConfirmation"]["writeAllowed"], true);
    assert_eq!(
        confirmed["receiveConfirmation"]["autoPreviewEnabled"],
        false
    );
    assert_eq!(
        confirmed["receiveConfirmation"]["autoIngestionEnabled"],
        false
    );

    let rejected_preview = evaluate_file_receive_confirmation_json(&json!({
        "manifest": manifest_json(&manifest),
        "approvedRoot": approved_root.to_string_lossy(),
        "autoPreview": true
    }))
    .unwrap_err();
    assert!(rejected_preview.to_string().contains("auto-preview"));

    let rejected_ingestion = evaluate_file_receive_confirmation_json(&json!({
        "manifest": manifest_json(&manifest),
        "approvedRoot": approved_root.to_string_lossy(),
        "autoIngestion": true
    }))
    .unwrap_err();
    assert!(rejected_ingestion.to_string().contains("auto-ingestion"));

    let serialized = serde_json::to_string(&json!({
        "pending": pending,
        "confirmed": confirmed
    }))
    .unwrap();
    for forbidden in [
        "file-confirmation-policy-canary",
        "private-confirmation-file-canary.pdf",
        "application/x-confirmation-canary",
        "private-confirmation-relative-canary",
        "lico-confirmation-approved-root-canary",
        approved_root.to_string_lossy().as_ref(),
    ] {
        assert!(
            !serialized.contains(forbidden),
            "receive confirmation leaked {forbidden}"
        );
    }
}
