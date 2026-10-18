use super::super::test_support::*;

fn secure_command_create_test_params(client_intent_id: &str, text: &str) -> Value {
    with_station_params(json!({
        "clientIntentId": client_intent_id,
        "commandKind": "agent.message.send",
        "targetAgentId": "codex",
        "workspaceId": "default",
        "body": {
            "text": text
        },
        "secretOverrideTransport": RUNTIME_SECRET_OVERRIDE_TRANSPORT,
        "secretOverrides": {
            "mobileRelayE2eeSecretStore": {
                "contract": "rust_secure_mesh_secret_store_handle_v1",
                "namespace": MOBILE_RELAY_PLATFORM_SECRET_STORE_NAMESPACE,
                "rawJsonSecretOverridesUsed": false
            }
        }
    }))
}

#[test]
fn mobile_relay_encrypted_local_effect_command_requires_local_confirmation() {
    let dir = temp_dir("mobile-relay-local-effect-confirmation");
    let previous = set_portable_data_dir_override(Some(dir.to_path_buf()));
    let mut pc_config = default_config();
    let mut mobile_config = default_config();
    pair_mobile_relay_configs(&mut pc_config, &mut mobile_config);
    let mobile_material = test_runtime_secret_material(stringify!(&mobile_config));
    let pc_material = test_runtime_secret_material(stringify!(&pc_config));
    let mobile_endpoint = local_endpoint_state(&mobile_config, &mobile_material).unwrap();
    let pc_endpoint = local_endpoint_state(&pc_config, &pc_material).unwrap();
    let command_payload = json!({
        "schema": crate::core::secure_mesh::SECURE_MESH_COMMAND_PROTOCOL_VERSION,
        "commandId": "cmd_mobile_relay_local_effect",
        "commandKind": "secure_mesh.device.verify",
        "senderIdentity": {
            "endpointId": mobile_endpoint.endpoint_id,
            "identityFingerprint": mobile_endpoint.fingerprint,
            "trustState": "verified",
            "endpointKind": mobile_endpoint.endpoint_kind
        },
        "targetBinding": {
            "targetEndpointId": pc_endpoint.endpoint_id,
            "targetAgentId": null,
            "workspaceId": "default"
        },
        "riskClass": "local_effect",
        "requiresUserConfirmation": false,
        "idempotencyKey": "idem_mobile_relay_local_effect",
        "createdAt": now_iso(),
        "expiresAt": timestamp_after_seconds(MOBILE_RELAY_COMMAND_TTL_SECONDS).unwrap(),
        "body": {
            "privateCanary": "local-effect-body-canary"
        }
    });
    drop(mobile_material);
    drop(pc_material);
    persist_test_runtime_secret_material(stringify!(&pc_config)).unwrap();
    save_config(&mut pc_config).unwrap();
    let envelope = seal_mobile_relay_payload(
        &mobile_config,
        &mut test_runtime_secret_material(stringify!(&mobile_config)),
        crate::core::secure_mesh_crypto::SecureMeshPayloadKind::Command,
        &command_payload,
    )
    .unwrap();

    let result_envelope = execute_secure_envelope_command(
        &json!({
            "type": SECURE_MESH_ENVELOPE_COMMAND,
            "envelope": envelope
        }),
        &json!({}),
    )
    .unwrap();
    let result = opened_result_payload(&mobile_config, &result_envelope);
    assert_eq!(result["evaluation"]["accepted"], true);
    assert_eq!(result["evaluation"]["shouldExecute"], false);
    assert_eq!(result["evaluation"]["code"], "user_confirmation_required");
    assert_eq!(result["execution"]["outcome"], "error");
    assert_eq!(result["bodyRedacted"], true);
    let serialized = serde_json::to_string(&result).unwrap();
    assert!(!serialized.contains("local-effect-body-canary"));

    set_portable_data_dir_override(previous);
}

#[test]
fn command_result_secure_consumes_canonical_sync_and_acks_after_open() {
    let dir = temp_dir("mobile-relay-secure-result-canonical-sync");
    let previous = set_portable_data_dir_override(Some(dir.to_path_buf()));
    let (pc_config, mut mobile_config, _envelope) = paired_command_envelope_fixture();
    let result_envelope = seal_mobile_relay_payload(
        &pc_config,
        &mut test_runtime_secret_material(stringify!(&pc_config)),
        crate::core::secure_mesh_crypto::SecureMeshPayloadKind::ResultPayload,
        &json!({
            "ok": true,
            "result": "encrypted-result-canary",
            "evaluation": {
                "commandId": "cmd-canonical-result"
            },
            "execution": {
                "commandId": "cmd-canonical-result",
                "idempotencyKey": "idem-canonical-result"
            },
            "bodyRedacted": true
        }),
    )
    .unwrap();
    let expected_envelope_id = result_envelope["envelopeId"].as_str().unwrap().to_string();
    let station = CanonicalStation::start(5, vec![result_envelope]);
    mobile_config["pairingId"] = json!("pair_canonical_result_sync");
    mobile_config["mobileToken"] = json!("mobile-token-canonical-result-sync");
    mobile_config["relayEnabled"] = json!(true);
    mobile_config["stationBaseUrl"] = json!(station.url());
    persist_test_runtime_secret_material(stringify!(&mobile_config)).unwrap();
    save_config(&mut mobile_config).unwrap();

    let result_params = with_station_params(json!({
        "commandId": "cmd-canonical-result",
        "idempotencyKey": "idem-canonical-result"
    }));
    let output = command_result_secure(&result_params).unwrap();

    assert_eq!(output["ok"], true);
    assert_eq!(output["bodyRedacted"], true);
    assert_eq!(output["openedResult"]["result"], "encrypted-result-canary");
    assert_eq!(
        output["transportHint"]["lease"]["stationReportedLeased"],
        true
    );
    assert_eq!(
        output["transportHint"]["delete"]["stationReportedAcknowledged"],
        true
    );
    let receipt_id = output["resultReceiptId"].as_str().unwrap().to_string();
    let recovered_after_response_loss = command_result_secure(&result_params).unwrap();
    assert_eq!(
        recovered_after_response_loss["openedResult"],
        output["openedResult"]
    );
    assert_eq!(recovered_after_response_loss["resultReceiptId"], receipt_id);
    assert!(recovered_after_response_loss.get("transportHint").is_none());
    for _ in 0..2 {
        let acknowledged = command_result_secure(&with_station_params(json!({
            "acknowledgeReceiptId": receipt_id
        })))
        .unwrap();
        assert_eq!(acknowledged["ok"], true);
        assert_eq!(acknowledged["acknowledged"], true);
    }
    let serialized = serde_json::to_string(&output).unwrap();
    assert!(!serialized.contains("mobile-token-canonical-result-sync"));
    assert!(station.request_path(4).ends_with(&expected_envelope_id));
    station.assert_operations(&[
        BadTowerStationOperation::LeaseMailbox,
        BadTowerStationOperation::ReceiveEnvelopes,
        BadTowerStationOperation::LeaseMailbox,
        BadTowerStationOperation::ReceiveEnvelopes,
        BadTowerStationOperation::DeleteEnvelope,
    ]);

    station.join();
    set_portable_data_dir_override(previous);
}

#[test]
fn command_result_secure_reuses_single_operation_auth_batch_for_fetch_and_result_open() {
    let dir = temp_dir("mobile-relay-secure-result-single-operation-auth-batch");
    let previous = set_portable_data_dir_override(Some(dir.to_path_buf()));
    let secret_store = Arc::new(EphemeralSecretStore::new());
    let mobile_store_override: Arc<dyn SecureMeshSecretStore> = secret_store.clone();
    let pairwise_store_override: Arc<dyn SecureMeshSecretStore> = secret_store.clone();

    with_mobile_relay_secret_store_override(mobile_store_override, || {
        with_pairwise_secret_store_override(pairwise_store_override, || {
            let (pc_config, mut mobile_config, _envelope) = paired_command_envelope_fixture();
            let result_envelope = seal_mobile_relay_payload(
                &pc_config,
                &mut test_runtime_secret_material(stringify!(&pc_config)),
                crate::core::secure_mesh_crypto::SecureMeshPayloadKind::ResultPayload,
                &json!({
                    "ok": true,
                    "result": "single-auth-result-canary",
                    "evaluation": {
                        "commandId": "cmd-single-auth-result"
                    },
                    "execution": {
                        "commandId": "cmd-single-auth-result",
                        "idempotencyKey": "idem-single-auth-result"
                    },
                    "bodyRedacted": true
                }),
            )?;
            let station = CanonicalStation::start(5, vec![result_envelope]);
            mobile_config["pairingId"] = json!("pair_secure_result_single_auth_batch");
            mobile_config["mobileToken"] = json!("mobile-token-secure-result-single-auth");
            mobile_config["relayEnabled"] = json!(true);
            mobile_config["stationBaseUrl"] = json!(station.url());
            persist_test_runtime_secret_material(stringify!(&mobile_config))?;
            persist_config_secret_material_to_secret_store(
                &mut mobile_config,
                secret_store.as_ref(),
                MOBILE_RELAY_PLATFORM_SECRET_STORE_NAMESPACE,
            )?;
            save_config(&mut mobile_config)?;
            let baseline_session_count = secret_store.authorization_session_count();

            let output = command_result_secure(&with_station_params(json!({
                "commandId": "cmd-single-auth-result",
                "idempotencyKey": "idem-single-auth-result"
            })))?;

            assert_eq!(output["ok"], true);
            assert_eq!(
                output["openedResult"]["result"],
                "single-auth-result-canary"
            );
            assert_eq!(
                secret_store.authorization_session_count(),
                baseline_session_count + 1
            );
            assert_eq!(
                secret_store.authorization_session_reasons()[baseline_session_count],
                "Mobile Relay secure result operation authorization batch"
            );
            station.assert_operations(&[
                BadTowerStationOperation::LeaseMailbox,
                BadTowerStationOperation::ReceiveEnvelopes,
                BadTowerStationOperation::LeaseMailbox,
                BadTowerStationOperation::ReceiveEnvelopes,
                BadTowerStationOperation::DeleteEnvelope,
            ]);
            station.join();
            Ok(())
        })
    })
    .unwrap();

    set_portable_data_dir_override(previous);
}

#[test]
fn command_result_secure_returns_typed_pending_when_station_has_no_result() {
    let dir = temp_dir("mobile-relay-secure-result-pending");
    let previous = set_portable_data_dir_override(Some(dir.to_path_buf()));
    let (_pc_config, mut mobile_config, _envelope) = paired_command_envelope_fixture();
    let station = CanonicalStation::start(4, Vec::new());
    mobile_config["pairingId"] = json!("pair_secure_result_pending");
    mobile_config["relayEnabled"] = json!(true);
    mobile_config["stationBaseUrl"] = json!(station.url());
    persist_test_runtime_secret_material(stringify!(&mobile_config)).unwrap();
    save_config(&mut mobile_config).unwrap();

    let output = command_result_secure(&with_station_params(json!({
        "commandId": "cmd-pending-result",
        "idempotencyKey": "idem-pending-result"
    })))
    .unwrap();

    assert_eq!(output["ok"], true);
    assert_eq!(output["pending"], true);
    assert_eq!(output["openedResult"], Value::Null);
    assert_eq!(output["bodyRedacted"], true);
    station.assert_operations(&[
        BadTowerStationOperation::LeaseMailbox,
        BadTowerStationOperation::ReceiveEnvelopes,
        BadTowerStationOperation::LeaseMailbox,
        BadTowerStationOperation::ReceiveEnvelopes,
    ]);
    station.join();
    set_portable_data_dir_override(previous);
}

#[test]
fn command_result_secure_caches_unmatched_results_without_head_of_line_blocking() {
    let dir = temp_dir("mobile-relay-secure-result-out-of-order-inbox");
    let previous = set_portable_data_dir_override(Some(dir.to_path_buf()));
    let (pc_config, mut mobile_config, _envelope) = paired_command_envelope_fixture();
    let mut results = Vec::new();
    for index in 0..12 {
        results.push(
            seal_mobile_relay_payload(
                &pc_config,
                &mut test_runtime_secret_material(stringify!(&pc_config)),
                crate::core::secure_mesh_crypto::SecureMeshPayloadKind::ResultPayload,
                &json!({
                    "ok": true,
                    "result": format!("out-of-order-result-{index}"),
                    "evaluation": {
                        "commandId": format!("cmd-out-of-order-{index}")
                    },
                    "execution": {
                        "commandId": format!("cmd-out-of-order-{index}"),
                        "idempotencyKey": format!("idem-out-of-order-{index}")
                    },
                    "bodyRedacted": true
                }),
            )
            .unwrap(),
        );
    }
    let station = CanonicalStation::start(16, results);
    mobile_config["pairingId"] = json!("pair_secure_result_out_of_order");
    mobile_config["relayEnabled"] = json!(true);
    mobile_config["stationBaseUrl"] = json!(station.url());
    persist_test_runtime_secret_material(stringify!(&mobile_config)).unwrap();
    save_config(&mut mobile_config).unwrap();

    let target = command_result_secure(&with_station_params(json!({
        "commandId": "cmd-out-of-order-11",
        "idempotencyKey": "idem-out-of-order-11"
    })))
    .unwrap();
    assert_eq!(target["pending"], false);
    assert_eq!(target["openedResult"]["result"], "out-of-order-result-11");

    let first_cached = command_result_secure(&with_station_params(json!({
        "commandId": "cmd-out-of-order-0",
        "idempotencyKey": "idem-out-of-order-0"
    })))
    .unwrap();
    assert_eq!(first_cached["pending"], false);
    assert_eq!(
        first_cached["openedResult"]["result"],
        "out-of-order-result-0"
    );
    assert!(first_cached.get("transportHint").is_none());
    assert_eq!(
        station
            .operations()
            .iter()
            .filter(|operation| **operation == BadTowerStationOperation::DeleteEnvelope)
            .count(),
        12
    );
    station.join();
    set_portable_data_dir_override(previous);
}

#[test]
fn command_create_secure_reuses_single_operation_auth_batch_for_hydrate_and_seal() {
    let dir = temp_dir("mobile-relay-secure-command-create-single-auth-batch");
    let previous = set_portable_data_dir_override(Some(dir.to_path_buf()));
    let secret_store = Arc::new(EphemeralSecretStore::new());
    let mobile_store_override: Arc<dyn SecureMeshSecretStore> = secret_store.clone();
    let pairwise_store_override: Arc<dyn SecureMeshSecretStore> = secret_store.clone();

    with_mobile_relay_secret_store_override(mobile_store_override, || {
        with_pairwise_secret_store_override(pairwise_store_override, || {
            let mut pc_config = default_config();
            let mut mobile_config = default_config();
            pair_mobile_relay_configs(&mut pc_config, &mut mobile_config);
            mobile_config["pairingId"] = json!("pair_secure_command_create_single_auth_batch");
            mobile_config["mobileToken"] = json!("mobile-token-single-auth-create");
            mobile_config["relayEnabled"] = json!(true);
            persist_test_runtime_secret_material(stringify!(&mobile_config))?;
            persist_config_secret_material_to_secret_store(
                &mut mobile_config,
                secret_store.as_ref(),
                MOBILE_RELAY_PLATFORM_SECRET_STORE_NAMESPACE,
            )?;
            let station = CanonicalStation::start(1, Vec::new());
            mobile_config["stationBaseUrl"] = json!(station.url());
            save_config(&mut mobile_config)?;
            let baseline_session_count = secret_store.authorization_session_count();

            let output = command_create_secure(&with_station_params(json!({
                "commandKind": "agent.message.send",
                "targetAgentId": "codex",
                "workspaceId": "default",
                "body": {
                    "text": "single-auth-create-canary"
                },
                "secretOverrideTransport": RUNTIME_SECRET_OVERRIDE_TRANSPORT,
                "secretOverrides": {
                    "mobileRelayE2eeSecretStore": {
                        "contract": "rust_secure_mesh_secret_store_handle_v1",
                        "namespace": MOBILE_RELAY_PLATFORM_SECRET_STORE_NAMESPACE,
                        "rawJsonSecretOverridesUsed": false
                    }
                }
            })))?;

            assert_eq!(output["ok"], true);
            assert_eq!(
                output["secureCommandBinding"]["commandKind"],
                "agent.message.send"
            );
            assert!(
                output["secureCommandBinding"]["payloadCommandId"]
                    .as_str()
                    .is_some_and(|value| !value.is_empty())
            );
            assert!(
                output["secureCommandBinding"]["idempotencyKey"]
                    .as_str()
                    .is_some_and(|value| !value.is_empty())
            );
            assert_eq!(
                secret_store.authorization_session_count(),
                baseline_session_count + 1
            );
            assert_eq!(
                secret_store.authorization_session_reasons()[baseline_session_count],
                "Mobile Relay secure command create authorization batch"
            );
            assert_eq!(
                secret_store.authorization_session_operation_counts()[baseline_session_count],
                mobile_relay_e2ee_secret_store_authorization_batch_operation_count()
                    .saturating_add(3)
            );
            let request = station.request_body(0);
            let request_body = serde_json::from_str::<Value>(&request).unwrap();
            assert_eq!(
                request_body
                    .as_object()
                    .unwrap()
                    .keys()
                    .map(String::as_str)
                    .collect::<std::collections::BTreeSet<_>>(),
                [
                    "ciphertext",
                    "contractVersion",
                    "envelopeId",
                    "expiresAt",
                    "mailboxId",
                ]
                .into_iter()
                .collect()
            );
            assert!(!request.contains(SECURE_MESH_ENVELOPE_COMMAND));
            assert!(!request.contains("single-auth-create-canary"));
            assert!(!request.contains("mobile-token-single-auth-create"));
            assert!(!request.contains("commandId"));
            station.assert_operations(&[BadTowerStationOperation::SendEnvelope]);
            station.join();
            Ok(())
        })
    })
    .unwrap();

    set_portable_data_dir_override(previous);
}

#[test]
fn command_create_secure_retries_the_exact_pending_envelope_after_lost_response() {
    let dir = temp_dir("mobile-relay-secure-command-exact-pending-retry");
    let previous = set_portable_data_dir_override(Some(dir.to_path_buf()));
    let secret_store = Arc::new(EphemeralSecretStore::new());
    let mobile_store_override: Arc<dyn SecureMeshSecretStore> = secret_store.clone();
    let pairwise_store_override: Arc<dyn SecureMeshSecretStore> = secret_store.clone();

    with_mobile_relay_secret_store_override(mobile_store_override, || {
        with_pairwise_secret_store_override(pairwise_store_override, || {
            let mut pc_config = default_config();
            let mut mobile_config = default_config();
            pair_mobile_relay_configs(&mut pc_config, &mut mobile_config);
            mobile_config["pairingId"] = json!("pair_secure_command_exact_pending_retry");
            mobile_config["relayEnabled"] = json!(true);
            persist_test_runtime_secret_material(stringify!(&mobile_config))?;
            persist_config_secret_material_to_secret_store(
                &mut mobile_config,
                secret_store.as_ref(),
                MOBILE_RELAY_PLATFORM_SECRET_STORE_NAMESPACE,
            )?;
            let station = CanonicalStation::start_with_first_send_response_dropped(2);
            mobile_config["stationBaseUrl"] = json!(station.url());
            save_config(&mut mobile_config)?;
            let params = secure_command_create_test_params(
                "intent_exact_pending_retry",
                "exact-pending-retry-canary",
            );

            let first_error = command_create_secure(&params).unwrap_err().to_string();
            assert!(first_error.contains("transport_outcome_unknown"));
            let recovered = command_create_secure(&params)?;

            assert_eq!(recovered["ok"], true);
            assert_eq!(
                recovered["secureCommandBinding"]["recoveredPendingDelivery"],
                true
            );
            assert_eq!(recovered["transportHint"]["stationReportedDuplicate"], true);
            assert_eq!(station.request_body(0), station.request_body(1));
            station.assert_operations(&[
                BadTowerStationOperation::SendEnvelope,
                BadTowerStationOperation::SendEnvelope,
            ]);
            station.join();
            Ok(())
        })
    })
    .unwrap();
    set_portable_data_dir_override(previous);
}

#[test]
fn command_create_secure_never_substitutes_a_different_pending_intent() {
    let dir = temp_dir("mobile-relay-secure-command-distinct-pending-intent");
    let previous = set_portable_data_dir_override(Some(dir.to_path_buf()));
    let secret_store = Arc::new(EphemeralSecretStore::new());
    let mobile_store_override: Arc<dyn SecureMeshSecretStore> = secret_store.clone();
    let pairwise_store_override: Arc<dyn SecureMeshSecretStore> = secret_store.clone();

    with_mobile_relay_secret_store_override(mobile_store_override, || {
        with_pairwise_secret_store_override(pairwise_store_override, || {
            let mut pc_config = default_config();
            let mut mobile_config = default_config();
            pair_mobile_relay_configs(&mut pc_config, &mut mobile_config);
            mobile_config["pairingId"] = json!("pair_secure_command_distinct_pending_intent");
            mobile_config["relayEnabled"] = json!(true);
            persist_test_runtime_secret_material(stringify!(&mobile_config))?;
            persist_config_secret_material_to_secret_store(
                &mut mobile_config,
                secret_store.as_ref(),
                MOBILE_RELAY_PLATFORM_SECRET_STORE_NAMESPACE,
            )?;
            let station = CanonicalStation::start_with_first_send_response_dropped(1);
            mobile_config["stationBaseUrl"] = json!(station.url());
            save_config(&mut mobile_config)?;

            let first_error = command_create_secure(&secure_command_create_test_params(
                "intent_pending_a",
                "pending-intent-a",
            ))
            .unwrap_err()
            .to_string();
            assert!(first_error.contains("transport_outcome_unknown"));
            let second_error = command_create_secure(&secure_command_create_test_params(
                "intent_pending_b",
                "pending-intent-b",
            ))
            .unwrap_err()
            .to_string();

            assert!(second_error.contains("a different secure command delivery is pending"));
            assert_eq!(station.operations().len(), 1);
            station.join();
            Ok(())
        })
    })
    .unwrap();
    set_portable_data_dir_override(previous);
}

#[test]
fn command_result_replay_proof_reuses_single_operation_auth_batch_for_fetch_and_replay_check() {
    let dir = temp_dir("mobile-relay-result-replay-proof-single-operation-auth-batch");
    let previous = set_portable_data_dir_override(Some(dir.to_path_buf()));
    let secret_store = Arc::new(EphemeralSecretStore::new());
    let mobile_store_override: Arc<dyn SecureMeshSecretStore> = secret_store.clone();
    let pairwise_store_override: Arc<dyn SecureMeshSecretStore> = secret_store.clone();

    with_mobile_relay_secret_store_override(mobile_store_override, || {
        with_pairwise_secret_store_override(pairwise_store_override, || {
            let (pc_config, mut mobile_config, _envelope) = paired_command_envelope_fixture();
            let result_envelope = seal_mobile_relay_payload(
                &pc_config,
                &mut test_runtime_secret_material(stringify!(&pc_config)),
                crate::core::secure_mesh_crypto::SecureMeshPayloadKind::ResultPayload,
                &json!({
                    "ok": true,
                    "evaluation": {
                        "code": "execute"
                    },
                    "execution": {
                        "outcome": "result"
                    },
                    "bodyRedacted": true
                }),
            )?;
            let station = CanonicalStation::start(5, vec![result_envelope]);
            mobile_config["pairingId"] = json!("pair_result_replay_single_auth_batch");
            mobile_config["mobileToken"] = json!("mobile-token-result-replay-single-auth");
            mobile_config["relayEnabled"] = json!(true);
            mobile_config["stationBaseUrl"] = json!(station.url());
            persist_test_runtime_secret_material(stringify!(&mobile_config))?;
            persist_config_secret_material_to_secret_store(
                &mut mobile_config,
                secret_store.as_ref(),
                MOBILE_RELAY_PLATFORM_SECRET_STORE_NAMESPACE,
            )?;
            save_config(&mut mobile_config)?;
            let baseline_session_count = secret_store.authorization_session_count();

            let proof = command_result_replay_proof(&with_station_params(json!({})))?;

            assert_eq!(proof["ok"], true);
            assert_eq!(proof["replayRejected"], true);
            assert_eq!(
                proof["transportHint"]["delete"]["stationReportedAcknowledged"],
                true
            );
            assert_eq!(
                secret_store.authorization_session_count(),
                baseline_session_count + 1
            );
            assert_eq!(
                secret_store.authorization_session_reasons()[baseline_session_count],
                "Mobile Relay secure result replay proof authorization batch"
            );
            station.assert_operations(&[
                BadTowerStationOperation::LeaseMailbox,
                BadTowerStationOperation::ReceiveEnvelopes,
                BadTowerStationOperation::LeaseMailbox,
                BadTowerStationOperation::ReceiveEnvelopes,
                BadTowerStationOperation::DeleteEnvelope,
            ]);
            station.join();
            Ok(())
        })
    })
    .unwrap();

    set_portable_data_dir_override(previous);
}

#[test]
fn mobile_relay_commands_sync_reuses_single_operation_auth_batch_for_secure_commands() {
    let dir = temp_dir("mobile-relay-commands-sync-single-operation-auth-batch");
    let previous = set_portable_data_dir_override(Some(dir.clone()));
    let secret_store = Arc::new(EphemeralSecretStore::new());
    let mobile_store_override: Arc<dyn SecureMeshSecretStore> = secret_store.clone();
    let pairwise_store_override: Arc<dyn SecureMeshSecretStore> = secret_store.clone();

    with_mobile_relay_secret_store_override(mobile_store_override, || {
        with_pairwise_secret_store_override(pairwise_store_override, || {
            let mut pc_config = default_config();
            let mut mobile_config = default_config();
            pair_mobile_relay_configs(&mut pc_config, &mut mobile_config);
            let mut envelopes = Vec::new();
            for index in 0..2 {
                let payload = secure_command_payload(
                    &mobile_config,
                    &mut test_runtime_secret_material(stringify!(&mobile_config)),
                    "agent.sessions.list",
                    Some("codex"),
                    "default",
                    json!({
                        "limit": index + 1
                    }),
                )?;
                envelopes.push(seal_mobile_relay_payload(
                    &mobile_config,
                    &mut test_runtime_secret_material(stringify!(&mobile_config)),
                    crate::core::secure_mesh_crypto::SecureMeshPayloadKind::Command,
                    &payload,
                )?);
            }
            let station = CanonicalStation::start(10, envelopes);
            pc_config["pairingId"] = json!("pair_commands_sync_single_auth_batch");
            pc_config["pcToken"] = json!("pc-token-commands-sync-single-auth");
            pc_config["relayEnabled"] = json!(true);
            pc_config["stationBaseUrl"] = json!(station.url());
            persist_test_runtime_secret_material(stringify!(&pc_config))?;
            persist_config_secret_material_to_secret_store(
                &mut pc_config,
                secret_store.as_ref(),
                MOBILE_RELAY_PLATFORM_SECRET_STORE_NAMESPACE,
            )?;
            save_config(&mut pc_config)?;
            let baseline_session_count = secret_store.authorization_session_count();

            let output =
                crate::domain::secure_mesh_command_runtime::with_secure_command_test_history_home(
                    &dir,
                    || {
                        commands_sync(&with_station_params(json!({
                            "targets": [],
                            "limit": 2,
                            "allowInteraction": false
                        })))
                    },
                )?;

            assert_eq!(output["ok"], true);
            let completed = output["completed"].as_array().unwrap();
            assert_eq!(completed.len(), 2);
            assert!(completed.iter().all(|command| command["ok"] == true));
            assert_eq!(
                secret_store.authorization_session_count(),
                baseline_session_count + 1
            );
            assert_eq!(
                secret_store.authorization_session_reasons()[baseline_session_count],
                "Mobile Relay commands sync operation authorization batch"
            );
            assert!(
                !secret_store.authorization_session_allow_interactions()[baseline_session_count]
            );
            station.assert_operations(&[
                BadTowerStationOperation::LeaseMailbox,
                BadTowerStationOperation::LeaseMailbox,
                BadTowerStationOperation::LeaseMailbox,
                BadTowerStationOperation::ReceiveEnvelopes,
                BadTowerStationOperation::LeaseMailbox,
                BadTowerStationOperation::ReceiveEnvelopes,
                BadTowerStationOperation::SendEnvelope,
                BadTowerStationOperation::DeleteEnvelope,
                BadTowerStationOperation::SendEnvelope,
                BadTowerStationOperation::DeleteEnvelope,
            ]);
            station.join();
            Ok(())
        })
    })
    .unwrap();

    set_portable_data_dir_override(previous);
}

#[test]
fn commands_sync_recovers_the_exact_result_outbox_after_lost_station_response() {
    let dir = temp_dir("mobile-relay-commands-sync-result-outbox-recovery");
    let previous = set_portable_data_dir_override(Some(dir.clone()));
    let secret_store = Arc::new(EphemeralSecretStore::new());
    let mobile_store_override: Arc<dyn SecureMeshSecretStore> = secret_store.clone();
    let pairwise_store_override: Arc<dyn SecureMeshSecretStore> = secret_store.clone();

    with_mobile_relay_secret_store_override(mobile_store_override, || {
        with_pairwise_secret_store_override(pairwise_store_override, || {
            let mut pc_config = default_config();
            let mut mobile_config = default_config();
            pair_mobile_relay_configs(&mut pc_config, &mut mobile_config);
            let payload = secure_command_payload(
                &mobile_config,
                &mut test_runtime_secret_material(stringify!(&mobile_config)),
                "agent.sessions.list",
                Some("codex"),
                "default",
                json!({"limit": 1}),
            )?;
            let command_envelope = seal_mobile_relay_payload(
                &mobile_config,
                &mut test_runtime_secret_material(stringify!(&mobile_config)),
                crate::core::secure_mesh_crypto::SecureMeshPayloadKind::Command,
                &payload,
            )?;
            let station =
                CanonicalStation::start_with_envelopes_and_first_send_response_dropped(
                    16,
                    vec![command_envelope],
                );
            pc_config["pairingId"] = json!("pair_commands_sync_result_outbox_recovery");
            pc_config["relayEnabled"] = json!(true);
            pc_config["stationBaseUrl"] = json!(station.url());
            persist_test_runtime_secret_material(stringify!(&pc_config))?;
            persist_config_secret_material_to_secret_store(
                &mut pc_config,
                secret_store.as_ref(),
                MOBILE_RELAY_PLATFORM_SECRET_STORE_NAMESPACE,
            )?;
            save_config(&mut pc_config)?;
            let sync_params = with_station_params(json!({
                "targets": [],
                "limit": 1,
                "allowInteraction": false
            }));

            let first =
                crate::domain::secure_mesh_command_runtime::with_secure_command_test_history_home(
                    &dir,
                    || commands_sync(&sync_params),
                )?;
            assert_eq!(
                first["completed"][0]["completion"]["code"],
                "mobile_relay_result_delivery_pending"
            );
            let recovered =
                crate::domain::secure_mesh_command_runtime::with_secure_command_test_history_home(
                    &dir,
                    || commands_sync(&sync_params),
                )?;

            assert_eq!(recovered["completed"].as_array().unwrap().len(), 1);
            assert_eq!(recovered["completed"][0]["ok"], true);
            assert_eq!(
                recovered["completed"][0]["completion"]["code"],
                "mobile_relay_pending_result_recovered"
            );
            assert_eq!(
                recovered["completed"][0]["completion"]["transportHint"]["result"]
                    ["stationReportedDuplicate"],
                true
            );
            assert_eq!(station.request_body(6), station.request_body(9));
            station.join();
            Ok(())
        })
    })
    .unwrap();

    set_portable_data_dir_override(previous);
}

/// Age the durable pending result record past the window its envelope was
/// sealed in.
///
/// The record is what recovery reads, so moving it into the past reproduces
/// exactly the state a real outbox reaches once the wall clock passes the
/// committed result's `expiresAt`: the ratchet has already advanced, the peer
/// has never received the result, and only the elapsed time has changed.
fn expire_pending_result_delivery() -> Result<()> {
    let path = crate::domain::mobile_relay::pairwise_session::mobile_relay_pairwise_store_path()?;
    let past = (time::OffsetDateTime::now_utc() - time::Duration::minutes(1))
        .format(&time::format_description::well_known::Rfc3339)?;
    let connection = rusqlite::Connection::open(&path)?;
    let envelope_json: String = connection.query_row(
        "SELECT envelope_json FROM secure_mesh_pairwise_pending_deliveries
         WHERE delivery_kind='result'",
        [],
        |row| row.get(0),
    )?;
    let mut envelope: Value = serde_json::from_str(&envelope_json)?;
    envelope["expiresAt"] = json!(past);
    let changed = connection.execute(
        "UPDATE secure_mesh_pairwise_pending_deliveries
         SET expires_at=?1, envelope_json=?2 WHERE delivery_kind='result'",
        rusqlite::params![past, serde_json::to_string(&envelope)?],
    )?;
    anyhow::ensure!(changed == 1, "the committed result outbox record is gone");
    Ok(())
}

#[test]
fn commands_sync_never_expires_an_admitted_result_by_elapsed_time() {
    let dir = temp_dir("mobile-relay-commands-sync-result-outbox-expiry");
    let previous = set_portable_data_dir_override(Some(dir.clone()));
    let secret_store = Arc::new(EphemeralSecretStore::new());
    let mobile_store_override: Arc<dyn SecureMeshSecretStore> = secret_store.clone();
    let pairwise_store_override: Arc<dyn SecureMeshSecretStore> = secret_store.clone();

    with_mobile_relay_secret_store_override(mobile_store_override, || {
        with_pairwise_secret_store_override(pairwise_store_override, || {
            let mut pc_config = default_config();
            let mut mobile_config = default_config();
            pair_mobile_relay_configs(&mut pc_config, &mut mobile_config);
            let payload = secure_command_payload(
                &mobile_config,
                &mut test_runtime_secret_material(stringify!(&mobile_config)),
                "agent.sessions.list",
                Some("codex"),
                "default",
                json!({"limit": 1}),
            )?;
            let command_envelope = seal_mobile_relay_payload(
                &mobile_config,
                &mut test_runtime_secret_material(stringify!(&mobile_config)),
                crate::core::secure_mesh_crypto::SecureMeshPayloadKind::Command,
                &payload,
            )?;
            let station = CanonicalStation::start_with_envelopes_and_first_send_response_dropped(
                16,
                vec![command_envelope],
            );
            pc_config["pairingId"] = json!("pair_commands_sync_result_outbox_expiry");
            pc_config["relayEnabled"] = json!(true);
            pc_config["stationBaseUrl"] = json!(station.url());
            persist_test_runtime_secret_material(stringify!(&pc_config))?;
            persist_config_secret_material_to_secret_store(
                &mut pc_config,
                secret_store.as_ref(),
                MOBILE_RELAY_PLATFORM_SECRET_STORE_NAMESPACE,
            )?;
            save_config(&mut pc_config)?;
            let sync_params = with_station_params(json!({
                "targets": [],
                "limit": 1,
                "allowInteraction": false
            }));

            // The command is executed and its result is committed with the
            // pending delivery, but the station's first response is lost, so the
            // result stays in the durable outbox.
            let first =
                crate::domain::secure_mesh_command_runtime::with_secure_command_test_history_home(
                    &dir,
                    || commands_sync(&sync_params),
                )?;
            assert_eq!(
                first["completed"][0]["completion"]["code"],
                "mobile_relay_result_delivery_pending"
            );

            expire_pending_result_delivery()?;

            // Recovery must deliver the result it already admitted. A committed
            // ratchet step is not undone by elapsed time, so the outbox never
            // reports that re-pairing is required.
            let recovered =
                crate::domain::secure_mesh_command_runtime::with_secure_command_test_history_home(
                    &dir,
                    || commands_sync(&sync_params),
                )?;
            assert_eq!(recovered["completed"].as_array().unwrap().len(), 1);
            assert_eq!(recovered["completed"][0]["ok"], true);
            assert_eq!(
                recovered["completed"][0]["completion"]["code"],
                "mobile_relay_pending_result_recovered"
            );
            station.join();
            Ok(())
        })
    })
    .unwrap();

    set_portable_data_dir_override(previous);
}

#[test]
fn mobile_relay_secure_command_execute_reuses_single_operation_auth_batch_for_open_and_result_seal()
{
    let dir = temp_dir("mobile-relay-secure-command-single-operation-auth-batch");
    let previous = set_portable_data_dir_override(Some(dir.to_path_buf()));
    let secret_store = Arc::new(EphemeralSecretStore::new());
    let store_override: Arc<dyn SecureMeshSecretStore> = secret_store.clone();

    with_pairwise_secret_store_override(store_override, || {
        let (mut pc_config, mobile_config, envelope) = paired_command_envelope_fixture();
        persist_test_runtime_secret_material(stringify!(&pc_config))?;
        save_config(&mut pc_config).unwrap();
        let baseline_session_count = secret_store.authorization_session_count();

        let result_envelope = execute_secure_envelope_command(
            &json!({
                "type": SECURE_MESH_ENVELOPE_COMMAND,
                "envelope": envelope
            }),
            &json!({}),
        )?;

        assert_eq!(
            secret_store.authorization_session_count(),
            baseline_session_count + 1
        );
        assert_eq!(
            secret_store.authorization_session_reasons()[baseline_session_count],
            "Mobile Relay secure command operation authorization batch"
        );
        assert_eq!(
            secret_store.authorization_session_operation_counts()[baseline_session_count],
            5
        );
        let result = opened_result_payload(&mobile_config, &result_envelope);
        assert_eq!(result["evaluation"]["code"], "execute");
        assert_eq!(result["execution"]["outcome"], "result");
        Ok(())
    })
    .unwrap();

    set_portable_data_dir_override(previous);
}
