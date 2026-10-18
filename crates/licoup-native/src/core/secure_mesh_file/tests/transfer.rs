use super::support::*;

#[test]
fn secure_mesh_file_transfer_tracks_resume_ack_and_purge_state() {
    let key = key_fixture();
    let manifest = manifest_fixture();
    let mut state = start_file_transfer(&manifest).unwrap();
    let chunks = encrypted_chunks_fixture(&key, &manifest);

    let report = record_file_chunk_receipt(&mut state, &chunks[0]).unwrap();
    assert_eq!(report.received_chunk_count, 1);
    assert_eq!(report.missing_chunk_indices, vec![1, 2]);
    assert!(!report.complete);

    let report = record_file_chunk_receipt(&mut state, &chunks[2]).unwrap();
    assert_eq!(report.missing_chunk_indices, vec![1]);
    assert!(!report.ack_required);

    let duplicate = record_file_chunk_receipt(&mut state, &chunks[2]).unwrap();
    assert_eq!(duplicate.missing_chunk_indices, vec![1]);

    let complete = record_file_chunk_receipt(&mut state, &chunks[1]).unwrap();
    assert!(complete.complete);
    assert!(complete.ack_required);
    assert!(!complete.purge_local_ciphertext);

    let acknowledged = acknowledge_file_transfer(&mut state, "2026-01-01T00:01:00.000Z").unwrap();
    assert!(acknowledged.complete);
    assert!(!acknowledged.ack_required);
    assert!(acknowledged.purge_local_ciphertext);
}

#[test]
fn secure_mesh_file_transfer_queue_is_bounded_confirmed_and_purged() {
    let key = key_fixture();
    let manifest = manifest_fixture();
    let chunks = encrypted_chunks_fixture(&key, &manifest);
    let total_ciphertext_bytes = chunks
        .iter()
        .map(|chunk| chunk.sealed.ciphertext_size)
        .sum::<usize>();
    let mut queue = SecureMeshFileTransferQueue::new(1, total_ciphertext_bytes).unwrap();
    let transfer_id = queue
        .enqueue(&manifest, "recipient:endpoint:queue")
        .unwrap();
    assert!(
        queue
            .enqueue(&manifest, "recipient:endpoint:second")
            .unwrap_err()
            .to_string()
            .contains("queue is full")
    );
    for chunk in &chunks {
        queue.record_chunk(&transfer_id, chunk).unwrap();
    }
    let duplicate = queue.record_chunk(&transfer_id, &chunks[0]).unwrap();
    assert!(duplicate.complete);
    assert!(
        queue
            .acknowledge(&transfer_id, "2026-01-01T00:01:00.000Z")
            .unwrap_err()
            .to_string()
            .contains("receive confirmation")
    );
    queue.confirm_receive(&transfer_id).unwrap();
    let acknowledged = queue
        .acknowledge(&transfer_id, "2026-01-01T00:01:01.000Z")
        .unwrap();
    assert!(acknowledged.purge_local_ciphertext);
    assert_eq!(
        queue.purge_acknowledged(&transfer_id).unwrap(),
        total_ciphertext_bytes
    );
    let status = queue.redacted_status();
    assert_eq!(status["activeTransferCount"], 0);
    assert_eq!(status["queuedCiphertextBytes"], 0);
    let serialized = serde_json::to_string(&status).unwrap();
    assert!(!serialized.contains(&manifest.file_id));
    assert!(!serialized.contains("recipient:endpoint:queue"));
}

#[test]
fn secure_mesh_file_transfer_queue_rejects_ciphertext_byte_overflow_without_mutation() {
    let key = key_fixture();
    let manifest = manifest_fixture();
    let chunks = encrypted_chunks_fixture(&key, &manifest);
    let mut queue =
        SecureMeshFileTransferQueue::new(1, chunks[0].sealed.ciphertext_size.saturating_sub(1))
            .unwrap();
    let transfer_id = queue
        .enqueue(&manifest, "recipient:endpoint:bounded")
        .unwrap();
    assert!(
        queue
            .record_chunk(&transfer_id, &chunks[0])
            .unwrap_err()
            .to_string()
            .contains("ciphertext-byte bound exceeded")
    );
    let status = queue.redacted_status();
    assert_eq!(status["queuedCiphertextBytes"], 0);
    assert_eq!(status["activeTransferCount"], 1);
}

#[test]
fn secure_mesh_file_transfer_rejects_conflicting_duplicate_chunk() {
    let key = key_fixture();
    let manifest = manifest_fixture();
    let mut state = start_file_transfer(&manifest).unwrap();
    let chunks = encrypted_chunks_fixture(&key, &manifest);
    record_file_chunk_receipt(&mut state, &chunks[0]).unwrap();

    let mut conflicting = chunks[0].clone();
    conflicting.ciphertext_hash = "sha256:conflicting".to_string();
    let error = record_file_chunk_receipt(&mut state, &conflicting).unwrap_err();
    assert!(error.to_string().contains("conflicting hash"));
}

#[test]
fn secure_mesh_file_transfer_rejects_manifest_chunk_count_mismatch() {
    let mut manifest = manifest_fixture();
    manifest.total_size = 25;
    let error = start_file_transfer(&manifest).unwrap_err();
    assert!(error.to_string().contains("chunk count does not match"));
}
