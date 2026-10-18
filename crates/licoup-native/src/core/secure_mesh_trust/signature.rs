use super::codec::append_len_prefixed_bytes;
use super::decision::trusted_for_sensitive_use;
use super::identity::DeviceTrustPublicIdentity;
use super::input::trust_state_label;
use super::model::{DeviceCrossSignature, DeviceTrustRecord, DeviceTrustState};
use super::{CROSS_SIGNATURE_MAGIC, SECURE_MESH_DEVICE_TRUST_PROTOCOL_VERSION, TRUST_RECORD_MAGIC};
use anyhow::{Context, Result, anyhow, ensure};
use base64::{Engine as _, engine::general_purpose};
use ed25519_dalek::{Signature, Signer, SigningKey};

pub fn sign_device_cross_signature(
    signer_key: &SigningKey,
    signer_identity: &DeviceTrustPublicIdentity,
    subject: &DeviceTrustPublicIdentity,
    roster_epoch: u64,
) -> Result<DeviceCrossSignature> {
    signer_identity.validate()?;
    subject.validate()?;
    let payload = cross_signature_payload(
        &signer_identity.endpoint_id,
        &subject.endpoint_id,
        &subject.fingerprint()?,
        roster_epoch,
    )?;
    let signature = signer_key.sign(&payload);
    Ok(DeviceCrossSignature {
        protocol_version: SECURE_MESH_DEVICE_TRUST_PROTOCOL_VERSION.to_string(),
        signer_endpoint_id: signer_identity.endpoint_id.clone(),
        subject_endpoint_id: subject.endpoint_id.clone(),
        subject_fingerprint: subject.fingerprint()?,
        roster_epoch,
        signature: general_purpose::URL_SAFE_NO_PAD.encode(signature.to_bytes()),
    })
}

pub fn verify_device_cross_signature(
    signer: &DeviceTrustPublicIdentity,
    subject: &DeviceTrustPublicIdentity,
    cross_signature: &DeviceCrossSignature,
) -> Result<DeviceTrustState> {
    ensure!(
        cross_signature.protocol_version == SECURE_MESH_DEVICE_TRUST_PROTOCOL_VERSION,
        "secure mesh device cross-signature protocol is unsupported"
    );
    ensure!(
        cross_signature.signer_endpoint_id == signer.endpoint_id,
        "secure mesh device cross-signature signer mismatch"
    );
    ensure!(
        cross_signature.subject_endpoint_id == subject.endpoint_id,
        "secure mesh device cross-signature subject mismatch"
    );
    ensure!(
        cross_signature.subject_fingerprint == subject.fingerprint()?,
        "secure mesh device cross-signature fingerprint mismatch"
    );
    let payload = cross_signature_payload(
        &cross_signature.signer_endpoint_id,
        &cross_signature.subject_endpoint_id,
        &cross_signature.subject_fingerprint,
        cross_signature.roster_epoch,
    )?;
    let signature_bytes = general_purpose::URL_SAFE_NO_PAD
        .decode(&cross_signature.signature)
        .context("secure mesh device cross-signature is not base64url")?;
    let signature = Signature::from_slice(&signature_bytes)
        .map_err(|error| anyhow!("secure mesh device cross-signature is invalid: {error:?}"))?;
    signer
        .signing_verifying_key()?
        .verify_strict(&payload, &signature)
        .map_err(|_| anyhow!("secure mesh device cross-signature verification failed"))?;
    Ok(DeviceTrustState::CrossSigned)
}

pub fn sign_device_trust_record(
    signer_key: &SigningKey,
    signer_identity: &DeviceTrustPublicIdentity,
    peer_identity: &DeviceTrustPublicIdentity,
    trust_state: DeviceTrustState,
    roster_epoch: u64,
    verification_method: impl Into<String>,
    issued_at_epoch_seconds: u64,
    expires_at_epoch_seconds: u64,
) -> Result<DeviceTrustRecord> {
    signer_identity.validate()?;
    peer_identity.validate()?;
    ensure!(
        trusted_for_sensitive_use(&trust_state),
        "secure mesh device trust record must be verified or cross-signed"
    );
    ensure!(
        expires_at_epoch_seconds > issued_at_epoch_seconds,
        "secure mesh device trust record expiry must be after issue time"
    );
    let verification_method = verification_method.into();
    ensure!(
        !verification_method.trim().is_empty() && verification_method.len() <= 80,
        "secure mesh device trust record verification method is invalid"
    );
    let peer_fingerprint = peer_identity.fingerprint()?;
    let payload = trust_record_payload(
        &signer_identity.endpoint_id,
        &peer_identity.endpoint_id,
        &peer_fingerprint,
        trust_state_label(&trust_state),
        roster_epoch,
        verification_method.trim(),
        issued_at_epoch_seconds,
        expires_at_epoch_seconds,
    )?;
    let signature = signer_key.sign(&payload);
    Ok(DeviceTrustRecord {
        protocol_version: SECURE_MESH_DEVICE_TRUST_PROTOCOL_VERSION.to_string(),
        signer_endpoint_id: signer_identity.endpoint_id.clone(),
        peer_endpoint_id: peer_identity.endpoint_id.clone(),
        peer_fingerprint,
        trust_state,
        roster_epoch,
        verification_method: verification_method.trim().to_string(),
        issued_at_epoch_seconds,
        expires_at_epoch_seconds,
        signature: general_purpose::URL_SAFE_NO_PAD.encode(signature.to_bytes()),
    })
}

pub fn verify_device_trust_record(
    signer_identity: &DeviceTrustPublicIdentity,
    peer_identity: &DeviceTrustPublicIdentity,
    record: &DeviceTrustRecord,
    now_epoch_seconds: u64,
) -> Result<DeviceTrustState> {
    ensure!(
        record.protocol_version == SECURE_MESH_DEVICE_TRUST_PROTOCOL_VERSION,
        "secure mesh device trust record protocol is unsupported"
    );
    ensure!(
        record.signer_endpoint_id == signer_identity.endpoint_id,
        "secure mesh device trust record signer mismatch"
    );
    ensure!(
        record.peer_endpoint_id == peer_identity.endpoint_id,
        "secure mesh device trust record peer mismatch"
    );
    ensure!(
        record.peer_fingerprint == peer_identity.fingerprint()?,
        "secure mesh device trust record fingerprint mismatch"
    );
    ensure!(
        trusted_for_sensitive_use(&record.trust_state),
        "secure mesh device trust record is not trusted for sensitive use"
    );
    ensure!(
        record.issued_at_epoch_seconds <= now_epoch_seconds,
        "secure mesh device trust record is not valid yet"
    );
    ensure!(
        now_epoch_seconds < record.expires_at_epoch_seconds,
        "secure mesh device trust record has expired"
    );
    let payload = trust_record_payload(
        &record.signer_endpoint_id,
        &record.peer_endpoint_id,
        &record.peer_fingerprint,
        trust_state_label(&record.trust_state),
        record.roster_epoch,
        &record.verification_method,
        record.issued_at_epoch_seconds,
        record.expires_at_epoch_seconds,
    )?;
    let signature_bytes = general_purpose::URL_SAFE_NO_PAD
        .decode(&record.signature)
        .context("secure mesh device trust record signature is not base64url")?;
    let signature = Signature::from_slice(&signature_bytes).map_err(|error| {
        anyhow!("secure mesh device trust record signature is invalid: {error:?}")
    })?;
    signer_identity
        .signing_verifying_key()?
        .verify_strict(&payload, &signature)
        .map_err(|_| anyhow!("secure mesh device trust record verification failed"))?;
    Ok(record.trust_state.clone())
}

fn cross_signature_payload(
    signer_endpoint_id: &str,
    subject_endpoint_id: &str,
    subject_fingerprint: &str,
    roster_epoch: u64,
) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    out.extend_from_slice(CROSS_SIGNATURE_MAGIC);
    append_len_prefixed_bytes(
        &mut out,
        SECURE_MESH_DEVICE_TRUST_PROTOCOL_VERSION.as_bytes(),
    )?;
    append_len_prefixed_bytes(&mut out, signer_endpoint_id.as_bytes())?;
    append_len_prefixed_bytes(&mut out, subject_endpoint_id.as_bytes())?;
    append_len_prefixed_bytes(&mut out, subject_fingerprint.as_bytes())?;
    out.extend_from_slice(&roster_epoch.to_be_bytes());
    Ok(out)
}

fn trust_record_payload(
    signer_endpoint_id: &str,
    peer_endpoint_id: &str,
    peer_fingerprint: &str,
    trust_state: &str,
    roster_epoch: u64,
    verification_method: &str,
    issued_at_epoch_seconds: u64,
    expires_at_epoch_seconds: u64,
) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    out.extend_from_slice(TRUST_RECORD_MAGIC);
    append_len_prefixed_bytes(
        &mut out,
        SECURE_MESH_DEVICE_TRUST_PROTOCOL_VERSION.as_bytes(),
    )?;
    append_len_prefixed_bytes(&mut out, signer_endpoint_id.as_bytes())?;
    append_len_prefixed_bytes(&mut out, peer_endpoint_id.as_bytes())?;
    append_len_prefixed_bytes(&mut out, peer_fingerprint.as_bytes())?;
    append_len_prefixed_bytes(&mut out, trust_state.as_bytes())?;
    out.extend_from_slice(&roster_epoch.to_be_bytes());
    append_len_prefixed_bytes(&mut out, verification_method.as_bytes())?;
    out.extend_from_slice(&issued_at_epoch_seconds.to_be_bytes());
    out.extend_from_slice(&expires_at_epoch_seconds.to_be_bytes());
    Ok(out)
}
