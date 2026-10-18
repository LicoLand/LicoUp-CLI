use super::constants::*;
use super::json_input::*;
use super::manifest_chunk_crypto::*;
use super::model::*;
use super::primitives::*;
use anyhow::{Context, Result, anyhow, ensure};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};

pub fn evaluate_file_receive_destination_json(params: &Value) -> Result<Value> {
    let manifest_value = params
        .get("manifest")
        .or_else(|| params.get("fileManifest"))
        .ok_or_else(|| anyhow!("secure mesh file manifest is required"))?;
    let manifest = manifest_from_json(manifest_value)?;
    let approved_root = json_optional_text(
        params,
        &[
            "approvedRoot",
            "destinationRoot",
            "receiveRoot",
            "approvedDestinationRoot",
        ],
    )
    .ok_or_else(|| anyhow!("secure mesh file approved destination root is required"))?;
    let conflict_policy = json_optional_text(params, &["conflictPolicy"])
        .unwrap_or_else(|| DEFAULT_FILE_CONFLICT_POLICY.to_string());
    ensure!(
        matches!(
            conflict_policy.as_str(),
            "fail_if_exists" | "rename" | "overwrite_after_confirm"
        ),
        "secure mesh file destination conflict policy is unsupported"
    );
    let approved_root = PathBuf::from(approved_root);
    let relative_destination = receive_relative_path(&manifest)?;
    let destination = validate_receive_destination(&approved_root, &relative_destination)?;
    Ok(json!({
        "ok": true,
        "fileProtocolVersion": crate::core::secure_mesh::SECURE_MESH_FILE_PROTOCOL_VERSION,
        "receivePolicy": {
            "destinationApproved": true,
            "requiresUserApprovedRoot": true,
            "destinationPathRedacted": true,
            "conflictPolicy": conflict_policy,
            "writeOperation": "secure_mesh.file_receive.write"
        },
        "destination": {
            "approvedRootHash": hash_bytes(path_to_bytes(&approved_root).as_slice()),
            "relativePathHash": hash_bytes(path_to_bytes(&relative_destination).as_slice()),
            "fileNameHash": hash_bytes(manifest.file_name.as_bytes()),
            "resolvedPathHash": hash_bytes(path_to_bytes(&destination).as_slice())
        },
        "manifest": {
            "fileIdHash": hash_bytes(manifest.file_id.as_bytes()),
            "totalSize": manifest.total_size,
            "chunkSize": manifest.chunk_size,
            "chunkCount": manifest.chunk_count,
            "metadataEncrypted": true,
            "bodyRedacted": true
        }
    }))
}

pub fn evaluate_file_receive_confirmation_json(params: &Value) -> Result<Value> {
    let manifest_value = params
        .get("manifest")
        .or_else(|| params.get("fileManifest"))
        .ok_or_else(|| anyhow!("secure mesh file manifest is required"))?;
    let manifest = manifest_from_json(manifest_value)?;
    let approved_root = json_optional_text(
        params,
        &[
            "approvedRoot",
            "destinationRoot",
            "receiveRoot",
            "approvedDestinationRoot",
        ],
    )
    .ok_or_else(|| anyhow!("secure mesh file approved destination root is required"))?;
    let conflict_policy = json_optional_text(params, &["conflictPolicy"])
        .unwrap_or_else(|| DEFAULT_FILE_CONFLICT_POLICY.to_string());
    ensure!(
        matches!(
            conflict_policy.as_str(),
            "fail_if_exists" | "rename" | "overwrite_after_confirm"
        ),
        "secure mesh file destination conflict policy is unsupported"
    );
    ensure!(
        !json_bool(params, &["autoPreview", "autoPreviewEnabled"]).unwrap_or(false),
        "secure mesh file auto-preview is disabled before receive confirmation"
    );
    ensure!(
        !json_bool(
            params,
            &[
                "autoIngestion",
                "autoIngestionEnabled",
                "autoImport",
                "autoImportEnabled"
            ],
        )
        .unwrap_or(false),
        "secure mesh file auto-ingestion is disabled before receive confirmation"
    );
    let user_confirmed =
        json_bool(params, &["userConfirmed", "confirmed", "receiveConfirmed"]).unwrap_or(false);
    let approved_root = PathBuf::from(approved_root);
    let relative_destination = receive_relative_path(&manifest)?;
    let destination = validate_receive_destination(&approved_root, &relative_destination)?;
    let manifest_digest = hash_bytes(
        serde_json::to_vec(&manifest_to_json(&manifest))
            .context("secure mesh file manifest digest serialization failed")?
            .as_slice(),
    );
    Ok(json!({
        "ok": true,
        "fileProtocolVersion": crate::core::secure_mesh::SECURE_MESH_FILE_PROTOCOL_VERSION,
        "receiveConfirmation": {
            "required": true,
            "userVisibleConfirmationRequired": true,
            "userConfirmed": user_confirmed,
            "defaultDecision": if user_confirmed { "confirmed" } else { "pending_user_confirmation" },
            "writeAllowed": user_confirmed,
            "localWriteDeferredUntilConfirmed": !user_confirmed,
            "decryptedBytesHiddenUntilConfirmed": !user_confirmed,
            "autoPreviewEnabled": false,
            "autoIngestionEnabled": false,
            "autoPreviewDisabledByDefault": true,
            "autoIngestionDisabledByDefault": true,
            "receiveOperation": "secure_mesh.file_receive.confirm"
        },
        "receivePolicy": {
            "destinationApproved": true,
            "requiresUserApprovedRoot": true,
            "destinationPathRedacted": true,
            "conflictPolicy": conflict_policy,
            "writeOperation": "secure_mesh.file_receive.write"
        },
        "destination": {
            "approvedRootHash": hash_bytes(path_to_bytes(&approved_root).as_slice()),
            "relativePathHash": hash_bytes(path_to_bytes(&relative_destination).as_slice()),
            "fileNameHash": hash_bytes(manifest.file_name.as_bytes()),
            "resolvedPathHash": hash_bytes(path_to_bytes(&destination).as_slice())
        },
        "manifest": {
            "manifestHash": manifest_digest,
            "fileIdHash": hash_bytes(manifest.file_id.as_bytes()),
            "totalSize": manifest.total_size,
            "chunkSize": manifest.chunk_size,
            "chunkCount": manifest.chunk_count,
            "metadataEncrypted": true,
            "bodyRedacted": true
        }
    }))
}

fn receive_relative_path(manifest: &SecureMeshFileManifest) -> Result<PathBuf> {
    validate_manifest(manifest)?;
    let mut path = PathBuf::new();
    if !manifest.relative_path.trim().is_empty() {
        path.push(normalized_relative_path(&manifest.relative_path)?);
    }
    path.push(&manifest.file_name);
    validate_relative_path(&path_to_string(&path)?)?;
    Ok(path)
}

fn validate_receive_destination(root: &Path, relative_path: &Path) -> Result<PathBuf> {
    ensure!(
        root.is_absolute(),
        "secure mesh file approved destination root must be absolute"
    );
    ensure!(
        path_is_clean_relative(relative_path),
        "secure mesh file destination relative path is outside approved root"
    );
    let destination = root.join(relative_path);
    ensure!(
        destination.starts_with(root),
        "secure mesh file destination path is outside approved root"
    );
    Ok(destination)
}
