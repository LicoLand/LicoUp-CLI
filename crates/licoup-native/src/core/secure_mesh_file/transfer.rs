use super::constants::*;
use super::manifest_chunk_crypto::*;
use super::model::*;
use super::primitives::*;
use anyhow::{Result, anyhow, ensure};
use serde_json::{Value, json};
use std::collections::{HashMap, VecDeque};

#[derive(Clone, Debug, Eq, PartialEq)]
struct SecureMeshQueuedFileTransfer {
    recipient_endpoint_hash: String,
    state: SecureMeshFileTransferState,
    queued_ciphertext_bytes: usize,
    receive_confirmed: bool,
}

#[derive(Debug)]
pub struct SecureMeshFileTransferQueue {
    max_active_transfers: usize,
    max_ciphertext_bytes: usize,
    queued_ciphertext_bytes: usize,
    order: VecDeque<String>,
    transfers: HashMap<String, SecureMeshQueuedFileTransfer>,
}

impl Default for SecureMeshFileTransferQueue {
    fn default() -> Self {
        Self::new(
            DEFAULT_MAX_QUEUED_FILE_TRANSFERS,
            DEFAULT_MAX_QUEUED_FILE_CIPHERTEXT_BYTES,
        )
        .expect("default secure mesh file transfer queue bounds are valid")
    }
}

impl SecureMeshFileTransferQueue {
    pub fn new(max_active_transfers: usize, max_ciphertext_bytes: usize) -> Result<Self> {
        ensure!(
            max_active_transfers > 0 && max_active_transfers <= DEFAULT_MAX_QUEUED_FILE_TRANSFERS,
            "secure mesh file transfer queue active-transfer bound is invalid"
        );
        ensure!(
            max_ciphertext_bytes > 0
                && max_ciphertext_bytes <= DEFAULT_MAX_QUEUED_FILE_CIPHERTEXT_BYTES,
            "secure mesh file transfer queue ciphertext-byte bound is invalid"
        );
        Ok(Self {
            max_active_transfers,
            max_ciphertext_bytes,
            queued_ciphertext_bytes: 0,
            order: VecDeque::with_capacity(max_active_transfers),
            transfers: HashMap::with_capacity(max_active_transfers),
        })
    }

    pub fn enqueue(
        &mut self,
        manifest: &SecureMeshFileManifest,
        recipient_endpoint_id: &str,
    ) -> Result<String> {
        validate_crypto_context_text(
            "file transfer recipient endpoint",
            recipient_endpoint_id,
            MAX_FILE_CRYPTO_CONTEXT_BYTES,
        )?;
        ensure!(
            self.transfers.len() < self.max_active_transfers,
            "secure mesh file transfer queue is full"
        );
        let transfer_id = file_transfer_queue_id(manifest, recipient_endpoint_id);
        ensure!(
            !self.transfers.contains_key(&transfer_id),
            "secure mesh file transfer is already queued"
        );
        self.transfers.insert(
            transfer_id.clone(),
            SecureMeshQueuedFileTransfer {
                recipient_endpoint_hash: hash_bytes(recipient_endpoint_id.as_bytes()),
                state: start_file_transfer(manifest)?,
                queued_ciphertext_bytes: 0,
                receive_confirmed: false,
            },
        );
        self.order.push_back(transfer_id.clone());
        Ok(transfer_id)
    }

    pub fn record_chunk(
        &mut self,
        transfer_id: &str,
        encrypted: &EncryptedSecureMeshFileChunk,
    ) -> Result<SecureMeshFileResumeReport> {
        let transfer = self
            .transfers
            .get_mut(transfer_id)
            .ok_or_else(|| anyhow!("secure mesh file transfer is not queued"))?;
        let already_received = transfer
            .state
            .received_chunks
            .get(encrypted.chunk_index as usize)
            .and_then(Option::as_ref)
            .is_some();
        let ciphertext_bytes = encrypted.sealed.ciphertext_size;
        if !already_received {
            let next_total = self
                .queued_ciphertext_bytes
                .checked_add(ciphertext_bytes)
                .ok_or_else(|| anyhow!("secure mesh file transfer queue byte count overflow"))?;
            ensure!(
                next_total <= self.max_ciphertext_bytes,
                "secure mesh file transfer queue ciphertext-byte bound exceeded"
            );
        }
        let report = record_file_chunk_receipt(&mut transfer.state, encrypted)?;
        if !already_received {
            transfer.queued_ciphertext_bytes += ciphertext_bytes;
            self.queued_ciphertext_bytes += ciphertext_bytes;
        }
        Ok(report)
    }

    pub fn confirm_receive(&mut self, transfer_id: &str) -> Result<()> {
        let transfer = self
            .transfers
            .get_mut(transfer_id)
            .ok_or_else(|| anyhow!("secure mesh file transfer is not queued"))?;
        ensure!(
            file_transfer_resume_report(&transfer.state)?.complete,
            "secure mesh file transfer cannot be confirmed before complete"
        );
        transfer.receive_confirmed = true;
        Ok(())
    }

    pub fn acknowledge(
        &mut self,
        transfer_id: &str,
        acknowledged_at: impl Into<String>,
    ) -> Result<SecureMeshFileResumeReport> {
        let transfer = self
            .transfers
            .get_mut(transfer_id)
            .ok_or_else(|| anyhow!("secure mesh file transfer is not queued"))?;
        ensure!(
            transfer.receive_confirmed,
            "secure mesh file transfer requires receive confirmation before ACK"
        );
        acknowledge_file_transfer(&mut transfer.state, acknowledged_at)
    }

    pub fn purge_acknowledged(&mut self, transfer_id: &str) -> Result<usize> {
        let transfer = self
            .transfers
            .get(transfer_id)
            .ok_or_else(|| anyhow!("secure mesh file transfer is not queued"))?;
        ensure!(
            file_transfer_resume_report(&transfer.state)?.purge_local_ciphertext,
            "secure mesh file transfer cannot be purged before ACK"
        );
        let purged_bytes = transfer.queued_ciphertext_bytes;
        self.transfers.remove(transfer_id);
        self.order.retain(|queued| queued != transfer_id);
        self.queued_ciphertext_bytes = self
            .queued_ciphertext_bytes
            .checked_sub(purged_bytes)
            .ok_or_else(|| anyhow!("secure mesh file transfer queue byte count underflow"))?;
        Ok(purged_bytes)
    }

    pub fn redacted_status(&self) -> Value {
        json!({
            "activeTransferCount": self.transfers.len(),
            "queuedCiphertextBytes": self.queued_ciphertext_bytes,
            "maxActiveTransfers": self.max_active_transfers,
            "maxCiphertextBytes": self.max_ciphertext_bytes,
            "orderedTransferIds": self.order,
            "recipientEndpointHashes": self.order.iter().filter_map(|id| {
                self.transfers.get(id).map(|transfer| transfer.recipient_endpoint_hash.clone())
            }).collect::<Vec<_>>(),
            "bodyRedacted": true
        })
    }
}

pub fn start_file_transfer(
    manifest: &SecureMeshFileManifest,
) -> Result<SecureMeshFileTransferState> {
    validate_manifest_for_transfer(manifest)?;
    let chunk_count = usize::try_from(manifest.chunk_count)
        .map_err(|_| anyhow!("secure mesh file chunk count is too large"))?;
    Ok(SecureMeshFileTransferState {
        file_id: manifest.file_id.clone(),
        file_id_hash: hash_bytes(manifest.file_id.as_bytes()),
        total_size: manifest.total_size,
        chunk_size: manifest.chunk_size,
        chunk_count: manifest.chunk_count,
        received_chunks: vec![None; chunk_count],
        acknowledged_at: None,
    })
}

pub(super) fn file_transfer_queue_id(
    manifest: &SecureMeshFileManifest,
    recipient_endpoint_id: &str,
) -> String {
    hash_bytes(
        format!(
            "licomesh.secure-mesh.file-transfer-queue.v1\0{}\0{}",
            hash_bytes(manifest.file_id.as_bytes()),
            hash_bytes(recipient_endpoint_id.as_bytes())
        )
        .as_bytes(),
    )
}

pub fn record_file_chunk_receipt(
    state: &mut SecureMeshFileTransferState,
    encrypted: &EncryptedSecureMeshFileChunk,
) -> Result<SecureMeshFileResumeReport> {
    ensure!(
        state.acknowledged_at.is_none(),
        "secure mesh file transfer is already acknowledged"
    );
    ensure!(
        encrypted.file_id_hash == state.file_id_hash,
        "secure mesh file transfer chunk file id mismatch"
    );
    ensure!(
        encrypted.chunk_index < state.chunk_count,
        "secure mesh file transfer chunk index is outside manifest bounds"
    );
    validate_chunk_plaintext_size(state, encrypted.chunk_index, encrypted.plaintext_size)?;
    let receipt = SecureMeshFileChunkReceipt {
        chunk_index: encrypted.chunk_index,
        ciphertext_hash: encrypted.ciphertext_hash.clone(),
        plaintext_size: encrypted.plaintext_size,
    };
    let slot = state
        .received_chunks
        .get_mut(encrypted.chunk_index as usize)
        .ok_or_else(|| anyhow!("secure mesh file transfer chunk index is outside state bounds"))?;
    match slot {
        Some(existing) if existing == &receipt => return file_transfer_resume_report(state),
        Some(_) => {
            return Err(anyhow!(
                "secure mesh file transfer duplicate chunk has conflicting hash"
            ));
        }
        None => *slot = Some(receipt),
    }
    file_transfer_resume_report(state)
}

pub fn acknowledge_file_transfer(
    state: &mut SecureMeshFileTransferState,
    acknowledged_at: impl Into<String>,
) -> Result<SecureMeshFileResumeReport> {
    let acknowledged_at = acknowledged_at.into();
    ensure!(
        !acknowledged_at.trim().is_empty(),
        "secure mesh file transfer ack timestamp is required"
    );
    let report = file_transfer_resume_report(state)?;
    ensure!(
        report.complete,
        "secure mesh file transfer cannot be acknowledged before complete"
    );
    state.acknowledged_at = Some(acknowledged_at);
    file_transfer_resume_report(state)
}

pub fn file_transfer_resume_report(
    state: &SecureMeshFileTransferState,
) -> Result<SecureMeshFileResumeReport> {
    ensure_transfer_total_matches_receipts(state)?;
    let missing_chunk_indices = state
        .received_chunks
        .iter()
        .enumerate()
        .filter_map(|(index, receipt)| {
            if receipt.is_none() {
                Some(index as u32)
            } else {
                None
            }
        })
        .collect::<Vec<_>>();
    let received_chunk_count = state.received_chunks.len() - missing_chunk_indices.len();
    let complete = missing_chunk_indices.is_empty();
    Ok(SecureMeshFileResumeReport {
        received_chunk_count,
        missing_chunk_indices,
        complete,
        ack_required: complete && state.acknowledged_at.is_none(),
        purge_local_ciphertext: complete && state.acknowledged_at.is_some(),
    })
}

pub(super) fn validate_chunk_plaintext_size(
    state: &SecureMeshFileTransferState,
    chunk_index: u32,
    plaintext_size: usize,
) -> Result<()> {
    ensure!(
        plaintext_size > 0,
        "secure mesh file transfer chunk is empty"
    );
    let expected_size = expected_chunk_size(state, chunk_index)?;
    ensure!(
        plaintext_size == expected_size,
        "secure mesh file transfer chunk size does not match manifest"
    );
    Ok(())
}

pub(super) fn expected_chunk_size(
    state: &SecureMeshFileTransferState,
    chunk_index: u32,
) -> Result<usize> {
    ensure!(
        chunk_index < state.chunk_count,
        "secure mesh file transfer chunk index is outside manifest bounds"
    );
    if chunk_index + 1 < state.chunk_count {
        return usize::try_from(state.chunk_size)
            .map_err(|_| anyhow!("secure mesh file chunk size is too large"));
    }
    let consumed_before_last = u64::from(state.chunk_size) * u64::from(state.chunk_count - 1);
    usize::try_from(state.total_size - consumed_before_last)
        .map_err(|_| anyhow!("secure mesh file final chunk size is too large"))
}

pub(super) fn ensure_transfer_total_matches_receipts(
    state: &SecureMeshFileTransferState,
) -> Result<()> {
    if state.received_chunks.iter().any(Option::is_none) {
        return Ok(());
    }
    let total = state
        .received_chunks
        .iter()
        .filter_map(|receipt| receipt.as_ref())
        .map(|receipt| receipt.plaintext_size as u64)
        .sum::<u64>();
    ensure!(
        total == state.total_size,
        "secure mesh file transfer received size does not match manifest"
    );
    Ok(())
}
