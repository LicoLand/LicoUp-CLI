use super::constants::*;
use super::manifest_chunk_crypto::*;
use super::transfer::*;
use anyhow::{Result, anyhow};
use serde_json::{Value, json};

pub fn evaluate_file_route_json(params: &Value) -> Result<Value> {
    let manifest_value = params
        .get("manifest")
        .or_else(|| params.get("fileManifest"))
        .ok_or_else(|| anyhow!("secure mesh file manifest is required"))?;
    let manifest = manifest_from_json(manifest_value)?;
    let state = start_file_transfer(&manifest)?;
    let report = file_transfer_resume_report(&state)?;
    Ok(json!({
        "ok": true,
        "fileProtocolVersion": crate::core::secure_mesh::SECURE_MESH_FILE_PROTOCOL_VERSION,
        "fileCryptoStatus": SECURE_MESH_FILE_CRYPTO_STATUS,
        "route": {
            "transport": "SecureEnvelopeDeliveryMailbox",
            "uploadOperation": "secure_mesh.file_chunk.upload",
            "fetchOperation": "secure_mesh.file_chunk.fetch",
            "manifestContentType": SECURE_MESH_FILE_MANIFEST_CONTENT_TYPE,
            "chunkContentType": SECURE_MESH_FILE_CHUNK_CONTENT_TYPE,
            "metadataEncrypted": true
        },
        "transfer": {
            "fileIdHash": state.file_id_hash,
            "totalSize": state.total_size,
            "chunkSize": state.chunk_size,
            "chunkCount": state.chunk_count
        },
        "resume": {
            "receivedChunkCount": report.received_chunk_count,
            "missingChunkIndices": report.missing_chunk_indices,
            "complete": report.complete,
            "ackRequired": report.ack_required,
            "purgeLocalCiphertext": report.purge_local_ciphertext
        }
    }))
}
