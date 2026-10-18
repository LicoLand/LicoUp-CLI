mod support;

use super::test_support::*;
use std::io::Read as _;
use std::time::Duration as StdDuration;
use support::*;

#[test]
#[ignore = "requires an explicitly supplied BadTower candidate and Lico Arc bundle"]
fn two_fresh_licoup_endpoints_round_trip_through_real_badtower() -> Result<()> {
    let kt_authority_scope = format!("badtower-{}", random_base64url(24));
    with_mobile_relay_test_kt_authority_scope(&kt_authority_scope, run_real_badtower_acceptance)
}

fn run_real_badtower_acceptance() -> Result<()> {
    let runtime = AcceptanceRuntime::from_environment()?;
    runtime.record_stage("fresh-endpoints")?;
    let mut pc = FreshEndpoint::create(&runtime.root, "endpoint-pc", &runtime.origin)?;
    let mut mobile = FreshEndpoint::create(&runtime.root, "endpoint-mobile", &runtime.origin)?;
    pc.install_codex_history_fixture(&runtime.private_canary)?;
    runtime.record_stage("pairing")?;
    pair_fresh_endpoints(&mut pc, &mut mobile, &runtime)?;
    let (pc_endpoint_id, pc_fingerprint) = pc.public_identity()?;
    let (mobile_endpoint_id, mobile_fingerprint) = mobile.public_identity()?;

    runtime.record_stage("mailbox-leases")?;
    let lease_params = json!({"leaseSeconds": 300, "allowInteraction": false});
    let pc_lease = pc.invoke(|| pc_check_in(&lease_params))?;
    let mobile_lease = mobile.invoke(|| pc_check_in(&lease_params))?;
    ensure!(
        pc_lease["transportHint"]["stationReportedLeased"] == true
            && mobile_lease["transportHint"]["stationReportedLeased"] == true,
        "BadTower acceptance mailbox lease failed"
    );

    runtime.record_stage("encrypted-send")?;
    let created = mobile.invoke(|| {
        dispatch_mobile_native_action(
            "mobile.relay.commands.createSecure",
            json!({
                "clientIntentId": "badtower_acceptance_sessions_list",
                "commandKind": "agent.sessions.list",
                "targetAgentId": "codex",
                "workspaceId": "default",
                "body": {
                    "limit": 1
                },
                "allowInteraction": false
            }),
        )
    })?;
    let station_reported_accepted = created["transportHint"]["stationReportedAccepted"] == true;
    ensure!(
        station_reported_accepted && created["transportHint"]["stationReportedDuplicate"] == false,
        "BadTower acceptance station did not accept the encrypted command"
    );
    let payload_command_id = created["secureCommandBinding"]["payloadCommandId"]
        .as_str()
        .ok_or_else(|| anyhow!("BadTower acceptance command binding is missing"))?
        .to_string();
    let idempotency_key = created["secureCommandBinding"]["idempotencyKey"]
        .as_str()
        .ok_or_else(|| anyhow!("BadTower acceptance idempotency binding is missing"))?
        .to_string();

    runtime.record_stage("transport-hint-negative")?;
    let early_result = mobile.invoke(|| {
        dispatch_mobile_native_action(
            "mobile.relay.commands.resultSecure",
            json!({
                "leaseSeconds": 300,
                "limit": 1,
                "commandId": payload_command_id,
                "idempotencyKey": idempotency_key
            }),
        )
    })?;
    let early_result_pending = early_result["ok"] == true
        && early_result["pending"] == true
        && early_result["openedResult"].is_null()
        && early_result["bodyRedacted"] == true;
    ensure!(
        early_result_pending,
        "BadTower transport hint was promoted to endpoint result evidence"
    );

    runtime.record_stage("encrypted-poll")?;
    let polled = pc.invoke(|| {
        commands_poll(&json!({
            "leaseSeconds": 300,
            "limit": 1,
            "allowInteraction": false
        }))
    })?;
    let envelopes = polled["envelopes"]
        .as_array()
        .ok_or_else(|| anyhow!("BadTower acceptance poll response is invalid"))?;
    ensure!(
        envelopes.len() == 1,
        "BadTower acceptance encrypted command was not received"
    );
    let observed_envelope = envelopes[0].clone();
    let exact_fields = [
        "ciphertext",
        "contractVersion",
        "envelopeId",
        "expiresAt",
        "mailboxId",
    ];
    let mut observed_fields = observed_envelope
        .as_object()
        .ok_or_else(|| anyhow!("BadTower acceptance envelope is invalid"))?
        .keys()
        .map(String::as_str)
        .collect::<Vec<_>>();
    observed_fields.sort_unstable();
    let mut expected_fields = exact_fields;
    expected_fields.sort_unstable();
    let exact_five_outer_fields = observed_fields == expected_fields;
    ensure!(
        exact_five_outer_fields,
        "BadTower acceptance envelope is not the closed five-field shape"
    );
    let observed_wire = serde_json::to_string(&observed_envelope)?;
    let wire_plaintext_absent = [
        runtime.private_canary.as_str(),
        "agent.sessions.list",
        payload_command_id.as_str(),
        pc_endpoint_id.as_str(),
        pc_fingerprint.as_str(),
        mobile_endpoint_id.as_str(),
        mobile_fingerprint.as_str(),
    ]
    .iter()
    .all(|forbidden| !observed_wire.contains(*forbidden));
    ensure!(
        wire_plaintext_absent,
        "BadTower-visible envelope contains endpoint plaintext"
    );

    runtime.record_stage("nonconformant-negative")?;
    let non_conformant_envelope_rejected =
        reject_non_conformant_envelopes(&runtime.origin, &observed_envelope)?;
    ensure!(
        non_conformant_envelope_rejected,
        "BadTower or LicoUp accepted a non-conformant Lico Arc envelope"
    );

    runtime.record_stage("endpoint-sync")?;
    let synced = pc.invoke(|| {
        commands_sync(&json!({
            "leaseSeconds": 300,
            "limit": 1,
            "targets": [],
            "allowInteraction": false
        }))
    })?;
    let completed = synced["completed"]
        .as_array()
        .ok_or_else(|| anyhow!("BadTower acceptance sync result is invalid"))?;
    ensure!(
        completed.len() == 1
            && completed[0]["ok"] == true
            && completed[0]["completion"]["transportHint"]["result"]["stationReportedAccepted"]
                == true,
        "BadTower acceptance endpoint did not process the encrypted command"
    );

    runtime.record_stage("encrypted-result")?;
    let opened_result = mobile.invoke(|| {
        dispatch_mobile_native_action(
            "mobile.relay.commands.resultSecure",
            json!({
                "leaseSeconds": 300,
                "limit": 1,
                "commandId": payload_command_id,
                "idempotencyKey": idempotency_key,
                "allowInteraction": false
            }),
        )
    })?;
    let result_receipt_id = opened_result["resultReceiptId"]
        .as_str()
        .ok_or_else(|| anyhow!("BadTower acceptance result receipt is missing"))?
        .to_string();
    let round_trip = opened_result["ok"] == true
        && opened_result["bodyRedacted"] == true
        && opened_result["openedResult"]["evaluation"]["commandId"] == payload_command_id
        && opened_result["openedResult"]["evaluation"]["code"] == "execute"
        && opened_result["openedResult"]["execution"]["outcome"] == "result"
        && opened_result["openedResult"]["execution"]["output"]["output"]["mode"]
            == "native-history"
        && opened_result["openedResult"]["execution"]["output"]["output"]["readOnly"] == true
        && opened_result["openedResult"]["execution"]["output"]["output"]["sessions"][0]["agentId"]
            == "codex"
        && opened_result["openedResult"]["execution"]["output"]["output"]["sessions"][0]["messages"]
            [0]["text"]
            == runtime.private_canary
        && opened_result["transportHint"]["delete"]["stationReportedAcknowledged"] == true;
    ensure!(
        round_trip,
        "BadTower acceptance encrypted round trip failed"
    );
    let acknowledged = mobile.invoke(|| {
        dispatch_mobile_native_action(
            "mobile.relay.commands.resultSecure",
            json!({
                "acknowledgeReceiptId": result_receipt_id,
                "allowInteraction": false
            }),
        )
    })?;
    let durable_result_receipt_acknowledged =
        acknowledged["ok"] == true && acknowledged["acknowledged"] == true;
    ensure!(
        durable_result_receipt_acknowledged,
        "BadTower acceptance durable result receipt was not acknowledged"
    );

    runtime.record_stage("post-delete-poll")?;
    let pc_empty = pc.invoke(|| {
        commands_poll(&json!({"leaseSeconds": 300, "limit": 1, "allowInteraction": false}))
    })?;
    let mobile_empty = mobile.invoke(|| {
        commands_poll(&json!({"leaseSeconds": 300, "limit": 1, "allowInteraction": false}))
    })?;
    ensure!(
        pc_empty["envelopes"].as_array().is_some_and(Vec::is_empty)
            && mobile_empty["envelopes"]
                .as_array()
                .is_some_and(Vec::is_empty),
        "BadTower acceptance envelopes were not deleted after endpoint authentication"
    );

    runtime.record_stage("runtime-receipt")?;
    write_runtime_receipt(
        &runtime.receipt_path,
        json!({
            "freshEndpointCount": 2,
            "positiveExchange": station_reported_accepted,
            "roundTrip": round_trip,
            "wirePlaintextAbsent": wire_plaintext_absent,
            "nonConformantEnvelopeRejected": non_conformant_envelope_rejected,
            "transportHintsNonAuthoritative": early_result_pending,
            "exactFiveOuterFields": exact_five_outer_fields,
            "mobileFfiDispatch": true,
            "typedPendingObserved": early_result_pending,
            "durableResultReceiptAcknowledged": durable_result_receipt_acknowledged
        }),
    )
}

fn dispatch_mobile_native_action(action: &str, params: Value) -> Result<Value> {
    crate::ffi::secure_mesh_mobile_ffi::dispatch_json(
        &json!({
            "action": action,
            "params": params
        }),
        "mobile_secure_mesh_native_json_action_unsupported",
    )
}

fn reject_non_conformant_envelopes(origin: &str, observed: &Value) -> Result<bool> {
    let mut unsupported_contract = observed.clone();
    unsupported_contract["contractVersion"] = json!("licoarc.relay.unsupported");
    unsupported_contract["envelopeId"] = json!(random_base64url(24));
    let unsupported_wire = serde_json::to_string(&unsupported_contract)?;
    let client_rejected_unsupported =
        licoup_protocol_bindings::licoarc_relay::LicoArcRelayEnvelope::from_json(&unsupported_wire)
            .is_err();
    let station_rejected_unsupported = station_rejects_json(origin, &unsupported_wire)?;

    let mut extra_field = observed.clone();
    extra_field["envelopeId"] = json!(random_base64url(24));
    extra_field["endpointEvidence"] = json!(true);
    let extra_field_wire = serde_json::to_string(&extra_field)?;
    let client_rejected_extra =
        licoup_protocol_bindings::licoarc_relay::LicoArcRelayEnvelope::from_json(&extra_field_wire)
            .is_err();
    let station_rejected_extra = station_rejects_json(origin, &extra_field_wire)?;

    Ok(client_rejected_unsupported
        && station_rejected_unsupported
        && client_rejected_extra
        && station_rejected_extra)
}

fn station_rejects_json(origin: &str, body: &str) -> Result<bool> {
    let agent = ureq::AgentBuilder::new()
        .timeout(StdDuration::from_secs(5))
        .redirects(0)
        .build();
    match agent
        .post(&format!("{origin}/v1/envelopes"))
        .set("accept", "application/json")
        .set("content-type", "application/json")
        .send_string(body)
    {
        Err(ureq::Error::Status(400, response)) => {
            let mut encoded = Vec::new();
            response
                .into_reader()
                .take(4 * 1024 + 1)
                .read_to_end(&mut encoded)?;
            ensure!(
                encoded.len() <= 4 * 1024,
                "BadTower rejection response is oversized"
            );
            let payload: Value = serde_json::from_slice(&encoded)?;
            Ok(payload["error"]["code"] == "invalid_request")
        }
        Ok(_) | Err(_) => Ok(false),
    }
}
