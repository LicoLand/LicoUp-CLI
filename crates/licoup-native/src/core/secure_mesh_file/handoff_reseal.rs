use super::delivery_projection::*;
use super::json_input::*;
use super::manifest_chunk_crypto::*;
use super::model::*;
use super::primitives::*;
use super::receive_policy::{
    evaluate_file_receive_confirmation_json, evaluate_file_receive_destination_json,
};
use super::route_policy::evaluate_file_route_json;
use super::transfer::SecureMeshFileTransferQueue;
use anyhow::{Context, Result, anyhow, ensure};
use base64::{Engine as _, engine::general_purpose};
use serde_json::{Value, json};
use std::collections::HashSet;

use crate::core::secure_mesh_crypto::SecureMeshContentContext;

pub fn evaluate_file_handoff_proof_json(params: &Value) -> Result<Value> {
    let manifest = if let Some(value) = params
        .get("manifest")
        .or_else(|| params.get("fileManifest"))
    {
        manifest_from_json(value)?
    } else {
        default_handoff_proof_manifest()
    };
    ensure!(
        manifest.chunk_count == 1,
        "secure mesh file handoff proof currently requires one chunk"
    );
    let chunk_bytes = handoff_proof_chunk_bytes(params, &manifest)?;
    let chunk = SecureMeshFileChunk {
        file_id: manifest.file_id.clone(),
        chunk_index: 0,
        bytes: chunk_bytes,
    };
    validate_chunk_plaintext_matches_manifest(&manifest, &chunk)?;

    let source_key = FileRootKey::generate();
    let file_hash = hash_bytes(&chunk.bytes);
    let sender_endpoint = json_optional_text(params, &["senderEndpoint"])
        .unwrap_or_else(|| "android-physical-endpoint".to_string());
    let desktop_endpoint = json_optional_text(params, &["desktopEndpoint"])
        .unwrap_or_else(|| "desktop-reseal-endpoint".to_string());
    let recipient_endpoints = handoff_recipient_endpoints(params)?;

    let source_manifest_context = SecureMeshFileProtectionContext::for_pairwise_device(
        handoff_context(
            "source_manifest",
            "msg_source_manifest",
            &sender_endpoint,
            &desktop_endpoint,
            "session_source_to_desktop",
        ),
        manifest.file_id.clone(),
        manifest.chunk_count,
        file_hash.clone(),
        1_800_000_000,
    )?;
    let source_chunk_context = SecureMeshFileProtectionContext::for_pairwise_device(
        handoff_context(
            "source_chunk_0",
            "msg_source_chunk_0",
            &sender_endpoint,
            &desktop_endpoint,
            "session_source_to_desktop",
        ),
        manifest.file_id.clone(),
        manifest.chunk_count,
        file_hash.clone(),
        1_800_000_000,
    )?;
    let encrypted_source_manifest =
        seal_file_manifest(&source_key, &source_manifest_context, &manifest)?;
    let encrypted_source_chunk = seal_file_chunk(&source_key, &source_chunk_context, &chunk)?;
    let source_manifest_delivery = file_manifest_delivery_json(&encrypted_source_manifest);
    let source_chunk_delivery = file_chunk_delivery_json(&encrypted_source_chunk);

    let opened_manifest = open_file_manifest(
        &source_key,
        &source_manifest_context,
        &encrypted_source_manifest,
    )?;
    let opened_chunk =
        open_file_chunk(&source_key, &source_chunk_context, &encrypted_source_chunk)?;
    ensure!(
        opened_manifest == manifest && opened_chunk == chunk,
        "secure mesh file handoff source open mismatch"
    );

    let mut recipient_deliveries = Vec::new();
    let mut recipient_server_visible = Vec::new();
    let mut resealed_manifest_hashes = HashSet::new();
    let mut resealed_chunk_hashes = HashSet::new();
    let mut first_resealed_manifest_hash = String::new();
    let mut first_resealed_chunk_hash = String::new();
    let mut first_resealed_manifest_size = 0usize;
    let mut first_resealed_chunk_size = 0usize;
    let mut all_recipients_opened_resealed = true;
    let mut all_wrong_recipients_rejected = true;
    let mut all_recipients_endpoint_specific_reseal_ready = true;
    let mut all_transfers_ack_purged = true;
    let mut transfer_queue = SecureMeshFileTransferQueue::default();

    for (recipient_index, recipient_endpoint) in recipient_endpoints.iter().enumerate() {
        let recipient_key = FileRootKey::generate();
        let recipient_index_label = recipient_index + 1;
        let recipient_session = format!("session_desktop_to_recipient_{recipient_index_label}");
        let recipient_manifest_envelope = format!("resealed_manifest_{recipient_index_label}");
        let recipient_manifest_message = format!("msg_resealed_manifest_{recipient_index_label}");
        let recipient_chunk_envelope = format!("resealed_chunk_0_{recipient_index_label}");
        let recipient_chunk_message = format!("msg_resealed_chunk_0_{recipient_index_label}");
        let recipient_manifest_context = SecureMeshFileProtectionContext::for_pairwise_device(
            handoff_context(
                &recipient_manifest_envelope,
                &recipient_manifest_message,
                &desktop_endpoint,
                recipient_endpoint,
                &recipient_session,
            ),
            manifest.file_id.clone(),
            manifest.chunk_count,
            file_hash.clone(),
            1_800_000_000,
        )?;
        let recipient_chunk_context = SecureMeshFileProtectionContext::for_pairwise_device(
            handoff_context(
                &recipient_chunk_envelope,
                &recipient_chunk_message,
                &desktop_endpoint,
                recipient_endpoint,
                &recipient_session,
            ),
            manifest.file_id.clone(),
            manifest.chunk_count,
            file_hash.clone(),
            1_800_000_000,
        )?;
        let resealed_manifest = seal_file_manifest(
            &recipient_key,
            &recipient_manifest_context,
            &opened_manifest,
        )?;
        let resealed_chunk =
            seal_file_chunk(&recipient_key, &recipient_chunk_context, &opened_chunk)?;
        let resealed_manifest_delivery = file_manifest_delivery_json(&resealed_manifest);
        let resealed_chunk_delivery = file_chunk_delivery_json(&resealed_chunk);

        let recipient_opened_manifest = open_file_manifest(
            &recipient_key,
            &recipient_manifest_context,
            &resealed_manifest,
        )?;
        let recipient_opened_chunk =
            open_file_chunk(&recipient_key, &recipient_chunk_context, &resealed_chunk)?;
        let recipient_opened_resealed =
            recipient_opened_manifest == manifest && recipient_opened_chunk == chunk;
        all_recipients_opened_resealed &= recipient_opened_resealed;

        let wrong_recipient_context = SecureMeshFileProtectionContext::for_pairwise_device(
            handoff_context(
                &recipient_manifest_envelope,
                &recipient_manifest_message,
                &desktop_endpoint,
                "wrong-recipient-endpoint",
                &recipient_session,
            ),
            manifest.file_id.clone(),
            manifest.chunk_count,
            file_hash.clone(),
            1_800_000_000,
        )?;
        let wrong_recipient_rejected =
            open_file_manifest(&recipient_key, &wrong_recipient_context, &resealed_manifest)
                .is_err();
        all_wrong_recipients_rejected &= wrong_recipient_rejected;

        let endpoint_specific_reseal_ready = encrypted_source_manifest.ciphertext_hash
            != resealed_manifest.ciphertext_hash
            && encrypted_source_chunk.ciphertext_hash != resealed_chunk.ciphertext_hash;
        all_recipients_endpoint_specific_reseal_ready &= endpoint_specific_reseal_ready;
        resealed_manifest_hashes.insert(resealed_manifest.ciphertext_hash.clone());
        resealed_chunk_hashes.insert(resealed_chunk.ciphertext_hash.clone());

        let transfer_id = transfer_queue.enqueue(&recipient_opened_manifest, recipient_endpoint)?;
        let receipt = transfer_queue.record_chunk(&transfer_id, &resealed_chunk)?;
        let ack_before_confirmation_rejected = transfer_queue
            .acknowledge(&transfer_id, "2026-01-01T00:00:00.000Z")
            .is_err();
        transfer_queue.confirm_receive(&transfer_id)?;
        let acknowledged = transfer_queue.acknowledge(&transfer_id, "2026-01-01T00:00:01.000Z")?;
        let purged_ciphertext_bytes = transfer_queue.purge_acknowledged(&transfer_id)?;
        let transfer_ack_purged = receipt.complete
            && receipt.ack_required
            && ack_before_confirmation_rejected
            && !acknowledged.ack_required
            && acknowledged.purge_local_ciphertext
            && purged_ciphertext_bytes == resealed_chunk.sealed.ciphertext_size;
        all_transfers_ack_purged &= transfer_ack_purged;

        if recipient_index == 0 {
            first_resealed_manifest_hash = resealed_manifest.ciphertext_hash.clone();
            first_resealed_chunk_hash = resealed_chunk.ciphertext_hash.clone();
            first_resealed_manifest_size = resealed_manifest.sealed.ciphertext_size;
            first_resealed_chunk_size = resealed_chunk.sealed.ciphertext_size;
        }

        recipient_server_visible.push(json!({
            "manifest": resealed_manifest_delivery,
            "chunk": resealed_chunk_delivery
        }));
        recipient_deliveries.push(json!({
            "recipientIndex": recipient_index_label,
            "recipientEndpointHash": hash_bytes(recipient_endpoint.as_bytes()),
            "recipientOpenedResealed": recipient_opened_resealed,
            "wrongRecipientRejected": wrong_recipient_rejected,
            "endpointSpecificResealReady": endpoint_specific_reseal_ready,
            "transferAckPurged": transfer_ack_purged,
            "resealedManifestCiphertextHash": resealed_manifest.ciphertext_hash,
            "resealedChunkCiphertextHash": resealed_chunk.ciphertext_hash,
            "resealedManifestCiphertextSize": resealed_manifest.sealed.ciphertext_size,
            "resealedChunkCiphertextSize": resealed_chunk.sealed.ciphertext_size,
            "receivedChunkCount": receipt.received_chunk_count,
            "ackRequiredBeforeAck": receipt.ack_required,
            "completeBeforeAck": receipt.complete,
            "ackBeforeReceiveConfirmationRejected": ack_before_confirmation_rejected,
            "ackRequiredAfterAck": acknowledged.ack_required,
            "purgeLocalCiphertext": acknowledged.purge_local_ciphertext,
            "purgedCiphertextBytes": purged_ciphertext_bytes
        }));
    }

    let recipient_count = recipient_endpoints.len();
    let multi_recipient_independent_reseal_ready = recipient_count > 1
        && resealed_manifest_hashes.len() == recipient_count
        && resealed_chunk_hashes.len() == recipient_count;
    let route = evaluate_file_route_json(&json!({ "manifest": manifest_to_json(&manifest) }))?;
    let approved_root = json_optional_text(params, &["approvedRoot", "receiveRoot"])
        .unwrap_or_else(|| std::env::temp_dir().to_string_lossy().into_owned());
    let receive_destination = evaluate_file_receive_destination_json(&json!({
        "manifest": manifest_to_json(&manifest),
        "approvedRoot": approved_root
    }))?;
    let receive_confirmation = evaluate_file_receive_confirmation_json(&json!({
        "manifest": manifest_to_json(&manifest),
        "approvedRoot": approved_root
    }))?;

    let server_visible = json!({
        "sourceManifest": source_manifest_delivery,
        "sourceChunk": source_chunk_delivery,
        "recipientDeliveries": recipient_server_visible,
        "route": route,
        "receiveDestination": receive_destination,
        "receiveConfirmation": receive_confirmation
    });
    let forbidden_canaries_absent = handoff_forbidden_canaries_absent(&server_visible);
    let endpoint_specific_reseal_ready =
        all_recipients_endpoint_specific_reseal_ready && multi_recipient_independent_reseal_ready;
    let transfer_queue_status = transfer_queue.redacted_status();
    let transfer_queue_drained = transfer_queue_status["activeTransferCount"].as_u64() == Some(0)
        && transfer_queue_status["queuedCiphertextBytes"].as_u64() == Some(0);

    Ok(json!({
        "ok": true,
        "fileProtocolVersion": crate::core::secure_mesh::SECURE_MESH_FILE_PROTOCOL_VERSION,
        "proofKind": "endpoint_specific_file_handoff_reseal",
        "metadataEncrypted": true,
        "bodyRedacted": true,
        "sourceOpenedByDesktop": true,
        "recipientOpenedResealed": all_recipients_opened_resealed,
        "wrongRecipientRejected": all_wrong_recipients_rejected,
        "endpointSpecificResealReady": endpoint_specific_reseal_ready,
        "recipientCount": recipient_count,
        "allRecipientsOpenedResealed": all_recipients_opened_resealed,
        "allRecipientsWrongRecipientRejected": all_wrong_recipients_rejected,
        "allRecipientsEndpointSpecificResealReady": all_recipients_endpoint_specific_reseal_ready,
        "multiRecipientIndependentResealReady": multi_recipient_independent_reseal_ready,
        "allRecipientTransfersAckPurged": all_transfers_ack_purged,
        "boundedTransferQueueReady": transfer_queue_drained,
        "deliveryJsonRedacted": forbidden_canaries_absent,
        "serverVisibleNoPlaintext": forbidden_canaries_absent,
        "routePolicyReady": route["route"]["metadataEncrypted"].as_bool() == Some(true),
        "receiveDestinationPolicyReady": receive_destination["receivePolicy"]["destinationApproved"].as_bool() == Some(true) &&
            receive_destination["receivePolicy"]["destinationPathRedacted"].as_bool() == Some(true),
        "receiveConfirmationPolicyReady": receive_confirmation["receiveConfirmation"]["required"].as_bool() == Some(true) &&
            receive_confirmation["receiveConfirmation"]["userVisibleConfirmationRequired"].as_bool() == Some(true) &&
            receive_confirmation["receiveConfirmation"]["writeAllowed"].as_bool() == Some(false) &&
            receive_confirmation["receiveConfirmation"]["autoPreviewEnabled"].as_bool() == Some(false) &&
            receive_confirmation["receiveConfirmation"]["autoIngestionEnabled"].as_bool() == Some(false),
        "transfer": {
            "chunkCount": manifest.chunk_count,
            "recipientCount": recipient_count,
            "allRecipientTransfersAckPurged": all_transfers_ack_purged,
            "boundedTransferQueueReady": transfer_queue_drained,
            "queue": transfer_queue_status
        },
        "recipientDeliveries": recipient_deliveries,
        "delivery": {
            "sourceManifestCiphertextHash": encrypted_source_manifest.ciphertext_hash,
            "sourceChunkCiphertextHash": encrypted_source_chunk.ciphertext_hash,
            "resealedManifestCiphertextHash": first_resealed_manifest_hash,
            "resealedChunkCiphertextHash": first_resealed_chunk_hash,
            "sourceManifestCiphertextSize": encrypted_source_manifest.sealed.ciphertext_size,
            "sourceChunkCiphertextSize": encrypted_source_chunk.sealed.ciphertext_size,
            "resealedManifestCiphertextSize": first_resealed_manifest_size,
            "resealedChunkCiphertextSize": first_resealed_chunk_size
        }
    }))
}

pub(super) fn handoff_recipient_endpoints(params: &Value) -> Result<Vec<String>> {
    let mut endpoints = if let Some(values) = params.get("recipientEndpoints") {
        let array = values.as_array().ok_or_else(|| {
            anyhow!("secure mesh file handoff recipientEndpoints must be an array")
        })?;
        array
            .iter()
            .filter_map(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToString::to_string)
            .collect::<Vec<_>>()
    } else if let Some(endpoint) = json_optional_text(params, &["recipientEndpoint"]) {
        vec![endpoint, "secondary-phone-recipient-endpoint".to_string()]
    } else {
        vec![
            "iphone-recipient-endpoint".to_string(),
            "android-recipient-endpoint".to_string(),
        ]
    };
    endpoints.sort();
    endpoints.dedup();
    ensure!(
        endpoints.len() >= 2,
        "secure mesh file handoff requires at least two recipient endpoints"
    );
    Ok(endpoints)
}

pub(super) fn default_handoff_proof_manifest() -> SecureMeshFileManifest {
    let chunk = default_handoff_proof_chunk_bytes();
    SecureMeshFileManifest {
        file_id: "handoff-proof-file-id-private-file-canary".to_string(),
        file_name: "handoff-proof-private-file-canary.pdf".to_string(),
        mime_type: "application/x-handoff-private-file-canary".to_string(),
        relative_path: "phone/handoff/private-relative-canary".to_string(),
        total_size: chunk.len() as u64,
        chunk_size: chunk.len() as u32,
        chunk_count: 1,
    }
}

pub(super) fn default_handoff_proof_chunk_bytes() -> Vec<u8> {
    b"file-body-plaintext-secret-canary-content".to_vec()
}

pub(super) fn handoff_proof_chunk_bytes(
    params: &Value,
    manifest: &SecureMeshFileManifest,
) -> Result<Vec<u8>> {
    if let Some(encoded) = json_optional_text(params, &["chunkBytesBase64url", "chunkBase64url"]) {
        let bytes = general_purpose::URL_SAFE_NO_PAD
            .decode(encoded)
            .context("secure mesh file handoff chunk is not base64url")?;
        ensure!(!bytes.is_empty(), "secure mesh file handoff chunk is empty");
        return Ok(bytes);
    }
    if manifest.file_id == "handoff-proof-file-id-private-file-canary" {
        return Ok(default_handoff_proof_chunk_bytes());
    }
    let size = usize::try_from(manifest.total_size)
        .map_err(|_| anyhow!("secure mesh file handoff chunk size is too large"))?;
    ensure!(size > 0, "secure mesh file handoff chunk is empty");
    Ok(vec![0xA5; size])
}

pub(super) fn handoff_context(
    envelope: &str,
    message: &str,
    sender: &str,
    recipient: &str,
    session: &str,
) -> SecureMeshContentContext {
    SecureMeshContentContext::new(
        format!("env_{envelope}"),
        message,
        "mailbox_file_handoff",
        sender,
        recipient,
        session,
        "2026-01-01T00:00:00.000Z",
        "2026-01-01T00:10:00.000Z",
    )
}

pub(super) fn handoff_forbidden_canaries_absent(value: &Value) -> bool {
    let serialized = value.to_string();
    [
        "handoff-proof-file-id-private-file-canary",
        "handoff-proof-private-file-canary.pdf",
        "application/x-handoff-private-file-canary",
        "private-relative-canary",
        "file-body-plaintext-secret-canary-content",
    ]
    .iter()
    .all(|forbidden| !serialized.contains(forbidden))
}
