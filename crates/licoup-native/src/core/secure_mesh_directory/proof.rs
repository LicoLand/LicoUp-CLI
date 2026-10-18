use super::*;

pub(super) fn authorized_leaf_digest(
    leaf_hash: &str,
    purpose: DirectoryAuthorizationPurpose,
    inclusion: &SecureMeshKtInclusionProof,
    latest_map: &SecureMeshKtMapProof,
    consistency: Option<&SecureMeshKtConsistencyProof>,
    freshness: &VerifiedKtFreshness,
    pin: &PinnedKtLogKey,
) -> String {
    let mut transcript = Vec::new();
    transcript.extend_from_slice(AUTHORIZED_LEAF_DOMAIN);
    append_len_prefixed(&mut transcript, leaf_hash.as_bytes());
    append_len_prefixed(&mut transcript, purpose.stable_code().as_bytes());
    append_len_prefixed(&mut transcript, pin.log_id().as_bytes());
    append_len_prefixed(&mut transcript, pin.key_id().as_bytes());
    append_len_prefixed(&mut transcript, pin.provenance().stable_code().as_bytes());
    append_proof_evidence(
        &mut transcript,
        inclusion,
        latest_map,
        consistency,
        freshness,
    );
    hex_encode(&Sha256::digest(transcript))
}

pub(super) fn authorized_leaf_transcript_binding_digest(
    leaf_hash: &str,
    purpose: DirectoryAuthorizationPurpose,
    pin: &PinnedKtLogKey,
) -> String {
    let mut transcript = Vec::new();
    transcript.extend_from_slice(AUTHORIZED_LEAF_TRANSCRIPT_BINDING_DOMAIN);
    append_len_prefixed(&mut transcript, leaf_hash.as_bytes());
    append_len_prefixed(&mut transcript, purpose.stable_code().as_bytes());
    append_len_prefixed(&mut transcript, pin.log_id().as_bytes());
    append_len_prefixed(&mut transcript, pin.key_id().as_bytes());
    append_len_prefixed(&mut transcript, pin.provenance().stable_code().as_bytes());
    hex_encode(&Sha256::digest(transcript))
}

pub(super) fn authorized_absence_digest(
    stable_label: &str,
    absence_map: &SecureMeshKtMapProof,
    consistency: Option<&SecureMeshKtConsistencyProof>,
    freshness: &VerifiedKtFreshness,
    pin: &PinnedKtLogKey,
) -> String {
    let mut transcript = Vec::new();
    transcript.extend_from_slice(AUTHORIZED_ABSENCE_DOMAIN);
    append_len_prefixed(&mut transcript, stable_label.as_bytes());
    append_len_prefixed(&mut transcript, pin.log_id().as_bytes());
    append_len_prefixed(&mut transcript, pin.key_id().as_bytes());
    append_len_prefixed(&mut transcript, pin.provenance().stable_code().as_bytes());
    append_len_prefixed(
        &mut transcript,
        absence_map.signed_tree_head.root_hash.as_bytes(),
    );
    append_len_prefixed(
        &mut transcript,
        absence_map.signed_tree_head.map_root_hash.as_bytes(),
    );
    transcript.extend_from_slice(&absence_map.signed_tree_head.tree_size.to_be_bytes());
    transcript.extend_from_slice(&freshness.observed_at_epoch_seconds.to_be_bytes());
    if let Some(proof) = consistency {
        transcript.extend_from_slice(&proof.first_tree_size.to_be_bytes());
        transcript.extend_from_slice(&proof.second_tree_size.to_be_bytes());
    }
    hex_encode(&Sha256::digest(transcript))
}

pub(super) fn append_proof_evidence(
    transcript: &mut Vec<u8>,
    inclusion: &SecureMeshKtInclusionProof,
    latest_map: &SecureMeshKtMapProof,
    consistency: Option<&SecureMeshKtConsistencyProof>,
    freshness: &VerifiedKtFreshness,
) {
    append_len_prefixed(transcript, inclusion.signed_tree_head.root_hash.as_bytes());
    append_len_prefixed(
        transcript,
        inclusion.signed_tree_head.map_root_hash.as_bytes(),
    );
    transcript.extend_from_slice(&inclusion.signed_tree_head.tree_size.to_be_bytes());
    transcript.extend_from_slice(&inclusion.leaf_index.to_be_bytes());
    transcript.extend_from_slice(&(inclusion.siblings.len() as u64).to_be_bytes());
    transcript.extend_from_slice(&(latest_map.siblings.len() as u64).to_be_bytes());
    transcript.extend_from_slice(&freshness.issued_at_epoch_seconds.to_be_bytes());
    if let Some(proof) = consistency {
        transcript.push(1);
        transcript.extend_from_slice(&proof.first_tree_size.to_be_bytes());
        transcript.extend_from_slice(&proof.second_tree_size.to_be_bytes());
        transcript.extend_from_slice(&(proof.path.len() as u64).to_be_bytes());
    } else {
        transcript.push(0);
    }
}

pub(super) fn validate_digest(label: &str, value: &str) -> Result<()> {
    ensure!(
        value.len() == HASH_HEX_LEN && value.bytes().all(|byte| byte.is_ascii_hexdigit()),
        "secure mesh directory {label} must be a SHA-256 hex digest"
    );
    Ok(())
}

pub(super) fn parse_digest(value: &str) -> Result<[u8; 32]> {
    validate_digest("digest", value)?;
    let mut bytes = [0u8; 32];
    for (index, chunk) in value.as_bytes().chunks(2).enumerate() {
        let text = std::str::from_utf8(chunk)
            .map_err(|_| anyhow!("secure mesh directory digest is invalid"))?;
        bytes[index] = u8::from_str_radix(text, 16)
            .map_err(|_| anyhow!("secure mesh directory digest is invalid"))?;
    }
    Ok(bytes)
}

pub(super) fn append_len_prefixed(out: &mut Vec<u8>, value: &[u8]) {
    out.extend_from_slice(&(value.len() as u64).to_be_bytes());
    out.extend_from_slice(value);
}

pub(super) fn hex_encode(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

const _: () = assert!(MAX_DIRECTORY_PROOF_HASHES == 256);
