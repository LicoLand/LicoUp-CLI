use super::test_support::*;

#[test]
fn mobile_ffi_mls_product_path_exchanges_an_authenticated_payload_between_clients() {
    let root = std::env::temp_dir().join(format!(
        "lico-mls-ffi-product-path-{}",
        uuid::Uuid::new_v4()
    ));
    let alice_dir = root.join("alice");
    let bob_dir = root.join("bob");
    let alice_store = Arc::new(EphemeralSecretStore::new());
    let bob_store = Arc::new(EphemeralSecretStore::new());

    initialize_mls_ffi_client(&alice_dir, alice_store.clone(), "desktop_gui");
    initialize_mls_ffi_client(&bob_dir, bob_store.clone(), "mobile");

    let alice_key_package = call_mls_ffi(
        &alice_dir,
        alice_store.clone(),
        "secure_mesh.mls.keyPackage.create",
        json!({"allowInteraction": true}),
    );
    let alice_identity = alice_key_package["identity"].clone();
    let bob_key_package = call_mls_ffi(
        &bob_dir,
        bob_store.clone(),
        "secure_mesh.mls.keyPackage.create",
        json!({"allowInteraction": true}),
    );
    let bob_identity = bob_key_package["identity"].clone();
    let bob_identity_typed = mls_ffi_identity(&bob_identity);
    let bob_key_package_bytes = general_purpose::URL_SAFE_NO_PAD
        .decode(bob_key_package["keyPackageBase64url"].as_str().unwrap())
        .unwrap();
    let previous = licoup_foundation::platform::paths::set_portable_data_dir_override(Some(
        alice_dir.join("portable-data"),
    ));
    let selected: Arc<dyn SecureMeshSecretStore> = alice_store.clone();
    let bob_directory_response =
        crate::domain::mobile_relay::with_mobile_relay_secret_store_override(selected, || {
            crate::domain::mobile_relay::secure_mesh_mls_test_directory_response(
                &bob_identity_typed,
                &bob_key_package_bytes,
                2,
                1,
            )
        })
        .unwrap();
    licoup_foundation::platform::paths::set_portable_data_dir_override(previous);
    let alice_identity_typed = mls_ffi_identity(&alice_identity);
    let alice_key_package_bytes = general_purpose::URL_SAFE_NO_PAD
        .decode(alice_key_package["keyPackageBase64url"].as_str().unwrap())
        .unwrap();
    let previous = licoup_foundation::platform::paths::set_portable_data_dir_override(Some(
        bob_dir.join("portable-data"),
    ));
    let selected: Arc<dyn SecureMeshSecretStore> = bob_store.clone();
    let alice_directory_response =
        crate::domain::mobile_relay::with_mobile_relay_secret_store_override(selected, || {
            crate::domain::mobile_relay::secure_mesh_mls_test_directory_response(
                &alice_identity_typed,
                &alice_key_package_bytes,
                2,
                1,
            )
        })
        .unwrap();
    licoup_foundation::platform::paths::set_portable_data_dir_override(previous);
    initialize_mls_ffi_peer(&alice_dir, alice_store.clone(), &bob_identity);
    initialize_mls_ffi_peer(&bob_dir, bob_store.clone(), &alice_identity);
    let group_id = general_purpose::URL_SAFE_NO_PAD.encode(b"ffi-product-group");

    let created = call_mls_ffi(
        &alice_dir,
        alice_store.clone(),
        "secure_mesh.mls.group.create",
        json!({
            "groupIdBase64url": group_id,
            "allowInteraction": true
        }),
    );
    assert_eq!(created["memberCount"], 1);
    assert_eq!(created["capabilityNegotiated"], false);

    let added = call_mls_ffi(
        &alice_dir,
        alice_store.clone(),
        "secure_mesh.mls.member.add",
        json!({
            "groupIdBase64url": group_id,
            "memberKeyPackageId": bob_key_package["keyPackageId"],
            "memberKeyPackageBase64url": bob_key_package["keyPackageBase64url"],
            "memberIdentity": bob_identity.clone(),
            "memberCapabilityProof": bob_key_package["capabilityProof"].clone(),
            "memberDirectoryVersion": 2,
            "memberKeyPackageVersion": 1,
            "untrustedDirectoryResponse": bob_directory_response.clone(),
            "allowInteraction": true
        }),
    );
    assert_eq!(added["group"]["memberCount"], 2);
    assert_eq!(added["group"]["capabilityNegotiated"], true);
    let remove_epoch = added["group"]["epoch"].as_u64().unwrap();

    let alice_roster = json!([
        {"identity": alice_identity.clone()},
        {
            "identity": bob_identity.clone(),
            "directoryVersion": 2,
            "keyPackageVersion": 1,
            "keyPackageDigest": bob_key_package["keyPackageId"].clone(),
            "untrustedDirectoryResponse": bob_directory_response,
        }
    ]);
    let bob_roster = json!([
        {
            "identity": alice_identity.clone(),
            "directoryVersion": 2,
            "keyPackageVersion": 1,
            "keyPackageDigest": alice_key_package["keyPackageId"].clone(),
            "untrustedDirectoryResponse": alice_directory_response,
        },
        {"identity": bob_identity.clone()}
    ]);
    let joined = call_mls_ffi(
        &bob_dir,
        bob_store.clone(),
        "secure_mesh.mls.group.join",
        json!({
            "groupIdBase64url": group_id,
            "inviterIdentity": alice_identity.clone(),
            "expectedRosterEndpointIds": [
                alice_identity["endpointId"],
                bob_identity["endpointId"]
            ],
            "trustedRoster": bob_roster.clone(),
            "welcomeMessageBase64url": added["welcomeMessageBase64url"],
            "allowInteraction": true
        }),
    );
    assert_eq!(joined["memberCount"], 2);
    assert_eq!(joined["capabilityNegotiated"], true);

    let context = json!({
        "envelopeId": "ffi-env-1",
        "messageId": "ffi-msg-1",
        "opaqueMailboxId": "ffi-mailbox-1",
        "senderEndpointId": alice_identity["endpointId"],
        "recipientEndpointId": bob_identity["endpointId"],
        "sessionId": "ffi-mls-session-1",
        "createdAt": "2026-07-12T00:00:00Z",
        "expiresAt": "2026-07-12T00:10:00Z"
    });
    let sealed = call_mls_ffi(
        &alice_dir,
        alice_store.clone(),
        "secure_mesh.mls.payload.seal",
        json!({
            "groupIdBase64url": group_id,
            "trustedRoster": alice_roster,
            "context": context,
            "payloadKind": "command",
            "bodyBase64url": general_purpose::URL_SAFE_NO_PAD.encode(b"authenticated-ping"),
            "contentType": "application/octet-stream",
            "allowInteraction": true
        }),
    );
    assert_eq!(sealed["bodyRedacted"], true);
    let opened = call_mls_ffi(
        &bob_dir,
        bob_store,
        "secure_mesh.mls.payload.open",
        json!({
            "groupIdBase64url": group_id,
            "trustedSenderIdentity": alice_identity,
            "trustedRoster": bob_roster,
            "context": context,
            "expectedPayloadKind": "command",
            "messageBase64url": sealed["messageBase64url"],
            "allowInteraction": true
        }),
    );
    assert_eq!(
        general_purpose::URL_SAFE_NO_PAD
            .decode(opened["bodyBase64url"].as_str().unwrap())
            .unwrap(),
        b"authenticated-ping"
    );
    assert!(
        !serde_json::to_string(&opened)
            .unwrap()
            .contains("privateKeyBase64url")
    );
    let bob_endpoint_id = bob_identity["endpointId"].clone();
    let removed = call_mls_ffi(
        &alice_dir,
        alice_store,
        "secure_mesh.mls.member.remove",
        json!({
            "groupIdBase64url": group_id,
            "expectedEpoch": remove_epoch,
            "memberIdentity": bob_identity,
            "allowInteraction": true
        }),
    );
    assert_eq!(removed["group"]["memberCount"], 1);
    assert_eq!(removed["memberEndpointId"], bob_endpoint_id);
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn mobile_ffi_kt_product_chain_fails_closed_without_external_gossip_authority() {
    let files_dir = std::env::temp_dir().join(format!(
        "lico-mobile-ffi-kt-product-chain-{}",
        uuid::Uuid::new_v4()
    ));
    let store = Arc::new(EphemeralSecretStore::new());
    let mut log = SecureMeshKtLog::with_identity(
        SigningKey::generate(&mut OsRng),
        "user-configured-test-log",
        "user-configured-test-key",
    );
    let pin = log.pin();
    let scope = directory_scope_commitment("test-tenant", "test-account", "test-workspace");
    let call = |action: &str, params: Value| -> anyhow::Result<Value> {
        let selected: Arc<dyn SecureMeshSecretStore> = store.clone();
        dispatch_json_with_files_dir_and_pairwise_secret_store(
            &json!({"action": action, "params": params}).to_string(),
            files_dir.to_string_lossy().as_ref(),
            "test_secure_mesh_action_unsupported",
            selected,
        )
    };

    let prepare_params = json!({
        "operation": "prepare",
        "directoryScopeCommitment": scope,
        "pin": {
            "logId": pin.log_id(),
            "keyId": pin.key_id(),
            "publicKeyHex": pin.public_key_hex(),
            "provenance": "user-configured-external"
        },
        "maxSthAgeSeconds": 3600,
        "maxFutureSkewSeconds": 300
    });
    let prepared = call("secure_mesh.kt.configureAuthority", prepare_params.clone()).unwrap();
    assert_eq!(prepared["status"], "confirmation_required");
    let mut confirm_params = prepare_params;
    confirm_params["operation"] = json!("confirm");
    confirm_params["authorityChallengeId"] = prepared["authorityChallengeId"].clone();
    confirm_params["confirmAuthorityConfiguration"] = json!(true);
    confirm_params["allowInteraction"] = json!(true);
    let configured = call("secure_mesh.kt.configureAuthority", confirm_params).unwrap();
    assert_eq!(configured["directoryResponseAccepted"], false);
    assert_eq!(configured["productionAuthority"], false);
    let missing_key_package = call(
        "secure_mesh.kt.publicationRequest",
        json!({"endpointKind": "mobile", "allowInteraction": true}),
    )
    .unwrap_err()
    .to_string();
    assert_eq!(missing_key_package, "native_operation_failed");
    assert!(!missing_key_package.contains("KeyPackage"));

    let key_package = call(
        "secure_mesh.mls.keyPackage.create",
        json!({"endpointKind": "mobile", "allowInteraction": true}),
    )
    .unwrap();
    assert!(key_package["keyPackageVersion"].as_u64().unwrap() > 0);
    assert_eq!(key_package["directoryPublicationRequired"], true);
    let publication = call(
        "secure_mesh.kt.publicationRequest",
        json!({"endpointKind": "mobile", "allowInteraction": true}),
    )
    .unwrap();
    let claim: SecureMeshDirectoryLeafClaim =
        serde_json::from_value(publication["claim"].clone()).unwrap();
    assert!(claim.key_material.mls_key_package_version > 0);
    assert_ne!(
        claim.key_material.mls_key_package_digest,
        "0000000000000000000000000000000000000000000000000000000000000000"
    );
    let now = u64::try_from(time::OffsetDateTime::now_utc().unix_timestamp()).unwrap();
    let index = log
        .append_hashed_directory_leaf(
            &claim.stable_label(),
            claim.version(),
            claim.revoked(),
            claim.leaf_hash().unwrap(),
        )
        .unwrap();
    let response = UntrustedDirectoryResponse {
        claim: claim.clone(),
        inclusion: log.inclusion_proof_at(index, now).unwrap(),
        latest_map: log.map_proof_at(&claim.stable_label(), now).unwrap(),
        consistency: None,
    };
    let mut mutated = response.clone();
    mutated.claim.key_material.mls_key_package_digest =
        "ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff".to_string();
    let mismatched_claim = call(
        "secure_mesh.kt.provision",
        json!({"response": mutated, "allowInteraction": true}),
    )
    .unwrap_err()
    .to_string();
    assert_eq!(mismatched_claim, "native_operation_failed");
    assert!(!mismatched_claim.contains("pending local claim"));
    let caller_pin = call(
        "secure_mesh.kt.provision",
        json!({
            "response": response,
            "pin": {"caller": "forbidden"},
            "allowInteraction": true
        }),
    )
    .unwrap_err()
    .to_string();
    assert_eq!(caller_pin, "native_operation_failed");
    assert!(!caller_pin.contains("unsupported field"));
    let blocked = call(
        "secure_mesh.kt.provision",
        json!({"response": response, "allowInteraction": true}),
    )
    .unwrap_err()
    .to_string();
    assert_eq!(blocked, "native_operation_failed");
    assert!(!blocked.contains("peer-gossip"));

    let _ = std::fs::remove_dir_all(files_dir);
}
