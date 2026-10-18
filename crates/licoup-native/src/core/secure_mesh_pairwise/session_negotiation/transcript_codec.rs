use anyhow::{Context, Result, anyhow, ensure};
use base64::{Engine as _, engine::general_purpose};
use ed25519_dalek::{Signature, Signer, SigningKey};
use hkdf::Hkdf;
use hmac::Mac;
use sha2::{Digest, Sha256};
use zeroize::Zeroizing;

use super::super::support::{
    ACCEPT_SIGNATURE_MAGIC, HANDSHAKE_HASH_LEN, HANDSHAKE_TRANSCRIPT_MAGIC, HmacSha256,
    INITIATOR_FINISHED_MAGIC, INTRO_SIGNATURE_MAGIC, KEY_CONFIRMATION_LEN, KEY_CONFIRMATION_MAGIC,
    ROOT_KEY_LEN, SECURE_MESH_PAIRWISE_CIPHER_SUITE, SIGNATURE_LEN, append_len_prefixed_bytes,
    hash_bytes,
};
use crate::core::secure_mesh::SECURE_MESH_PROTOCOL_VERSION;
use crate::core::secure_mesh_capability_proof::{
    SignedCapabilityProof, encode_signed_capability_proof_json,
};
use crate::core::secure_mesh_session_negotiation::NegotiatedCapabilityBinding;
use crate::core::secure_mesh_trust::DeviceTrustPublicIdentity;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SecureMeshPairwiseSessionIntro {
    pub protocol_version: String,
    pub cipher_suite: String,
    pub session_id: String,
    pub initiator_endpoint_id: String,
    pub responder_endpoint_id: String,
    pub initiator_identity_public_key: Vec<u8>,
    pub initiator_ephemeral_public_key: Vec<u8>,
    pub initiator_initial_ratchet_public_key: Vec<u8>,
    pub responder_signed_prekey_id: String,
    pub responder_one_time_prekey_id: Option<String>,
    pub responder_one_time_mlkem1024_prekey_id: String,
    pub mlkem1024_ciphertext: Vec<u8>,
    pub directory_authorization_digest: String,
    pub initiator_capability_proof: SignedCapabilityProof,
    pub initiator_signature: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SecureMeshPairwiseSessionAccepted {
    pub protocol_version: String,
    pub cipher_suite: String,
    pub session_id: String,
    pub responder_endpoint_id: String,
    pub responder_initial_ratchet_public_key: Vec<u8>,
    pub handshake_transcript_hash: String,
    pub responder_capability_proof: SignedCapabilityProof,
    pub capability_binding: NegotiatedCapabilityBinding,
    pub responder_signature: String,
    pub key_confirmation: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SecureMeshPairwiseSessionFinished {
    pub protocol_version: String,
    pub cipher_suite: String,
    pub session_id: String,
    pub initiator_endpoint_id: String,
    pub responder_endpoint_id: String,
    pub handshake_transcript_hash: String,
    pub capability_transcript_digest: String,
    pub key_confirmation: String,
}

pub(in crate::core::secure_mesh_pairwise) fn intro_signature_payload(
    intro: &SecureMeshPairwiseSessionIntro,
) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    out.extend_from_slice(INTRO_SIGNATURE_MAGIC);
    append_len_prefixed_bytes(&mut out, intro.protocol_version.as_bytes())?;
    append_len_prefixed_bytes(&mut out, intro.cipher_suite.as_bytes())?;
    append_len_prefixed_bytes(&mut out, intro.session_id.as_bytes())?;
    append_len_prefixed_bytes(&mut out, intro.initiator_endpoint_id.as_bytes())?;
    append_len_prefixed_bytes(&mut out, intro.responder_endpoint_id.as_bytes())?;
    append_len_prefixed_bytes(&mut out, &intro.initiator_identity_public_key)?;
    append_len_prefixed_bytes(&mut out, &intro.initiator_ephemeral_public_key)?;
    append_len_prefixed_bytes(&mut out, &intro.initiator_initial_ratchet_public_key)?;
    append_len_prefixed_bytes(&mut out, intro.responder_signed_prekey_id.as_bytes())?;
    match &intro.responder_one_time_prekey_id {
        Some(prekey_id) => {
            out.push(1);
            append_len_prefixed_bytes(&mut out, prekey_id.as_bytes())?;
        }
        None => out.push(0),
    }
    append_len_prefixed_bytes(
        &mut out,
        intro.responder_one_time_mlkem1024_prekey_id.as_bytes(),
    )?;
    append_len_prefixed_bytes(&mut out, &intro.mlkem1024_ciphertext)?;
    append_len_prefixed_bytes(&mut out, intro.directory_authorization_digest.as_bytes())?;
    append_len_prefixed_bytes(
        &mut out,
        &encode_signed_capability_proof_json(&intro.initiator_capability_proof)?,
    )?;
    Ok(out)
}

pub(in crate::core::secure_mesh_pairwise) fn accept_signature_payload(
    accepted: &SecureMeshPairwiseSessionAccepted,
) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    out.extend_from_slice(ACCEPT_SIGNATURE_MAGIC);
    append_len_prefixed_bytes(&mut out, accepted.protocol_version.as_bytes())?;
    append_len_prefixed_bytes(&mut out, accepted.cipher_suite.as_bytes())?;
    append_len_prefixed_bytes(&mut out, accepted.session_id.as_bytes())?;
    append_len_prefixed_bytes(&mut out, accepted.responder_endpoint_id.as_bytes())?;
    append_len_prefixed_bytes(&mut out, &accepted.responder_initial_ratchet_public_key)?;
    append_len_prefixed_bytes(&mut out, accepted.handshake_transcript_hash.as_bytes())?;
    append_len_prefixed_bytes(
        &mut out,
        &encode_signed_capability_proof_json(&accepted.responder_capability_proof)?,
    )?;
    append_len_prefixed_bytes(
        &mut out,
        &serde_json::to_vec(&accepted.capability_binding)
            .context("secure mesh pairwise capability binding serialization failed")?,
    )?;
    Ok(out)
}

pub(in crate::core::secure_mesh_pairwise) fn sign_pairwise_transcript(
    signing_key: &SigningKey,
    payload: &[u8],
) -> String {
    general_purpose::URL_SAFE_NO_PAD.encode(signing_key.sign(payload).to_bytes())
}

pub(in crate::core::secure_mesh_pairwise) fn verify_pairwise_transcript_signature(
    identity: &DeviceTrustPublicIdentity,
    payload: &[u8],
    signature: &str,
    label: &str,
) -> Result<()> {
    let signature_bytes =
        decode_fixed_base64url::<SIGNATURE_LEN>(signature, &format!("{label} signature"))?;
    let signature = Signature::from_bytes(&signature_bytes);
    identity
        .signing_verifying_key()?
        .verify_strict(payload, &signature)
        .map_err(|_| anyhow!("secure mesh pairwise {label} signature verification failed"))
}

pub(in crate::core::secure_mesh_pairwise) fn handshake_transcript_hash(
    intro: &SecureMeshPairwiseSessionIntro,
) -> Result<[u8; HANDSHAKE_HASH_LEN]> {
    let signature =
        decode_fixed_base64url::<SIGNATURE_LEN>(&intro.initiator_signature, "intro signature")?;
    let mut hasher = Sha256::new();
    hasher.update(HANDSHAKE_TRANSCRIPT_MAGIC);
    hasher.update(intro_signature_payload(intro)?);
    hasher.update(signature);
    Ok(hasher.finalize().into())
}

pub(in crate::core::secure_mesh_pairwise) fn key_confirmation_payload(
    accepted: &SecureMeshPairwiseSessionAccepted,
) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    out.extend_from_slice(KEY_CONFIRMATION_MAGIC);
    append_len_prefixed_bytes(&mut out, &accept_signature_payload(accepted)?)?;
    append_len_prefixed_bytes(&mut out, accepted.responder_signature.as_bytes())?;
    Ok(out)
}

pub(in crate::core::secure_mesh_pairwise) fn pairwise_key_confirmation(
    root_key: &[u8; ROOT_KEY_LEN],
    accepted: &SecureMeshPairwiseSessionAccepted,
) -> Result<String> {
    let confirmation_key = derive_key_confirmation_key(root_key, accepted)?;
    let mut mac = <HmacSha256 as Mac>::new_from_slice(confirmation_key.as_ref())
        .map_err(|_| anyhow!("secure mesh pairwise key confirmation initialization failed"))?;
    mac.update(&key_confirmation_payload(accepted)?);
    Ok(general_purpose::URL_SAFE_NO_PAD.encode(mac.finalize().into_bytes()))
}

pub(in crate::core::secure_mesh_pairwise) fn verify_pairwise_key_confirmation(
    root_key: &[u8; ROOT_KEY_LEN],
    accepted: &SecureMeshPairwiseSessionAccepted,
) -> Result<()> {
    let confirmation = decode_fixed_base64url::<KEY_CONFIRMATION_LEN>(
        &accepted.key_confirmation,
        "accept key confirmation",
    )?;
    let confirmation_key = derive_key_confirmation_key(root_key, accepted)?;
    let mut mac = <HmacSha256 as Mac>::new_from_slice(confirmation_key.as_ref())
        .map_err(|_| anyhow!("secure mesh pairwise key confirmation initialization failed"))?;
    mac.update(&key_confirmation_payload(accepted)?);
    mac.verify_slice(&confirmation)
        .map_err(|_| anyhow!("secure mesh pairwise key confirmation failed"))
}

pub(in crate::core::secure_mesh_pairwise) fn initiator_finished_payload(
    finished: &SecureMeshPairwiseSessionFinished,
) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    out.extend_from_slice(INITIATOR_FINISHED_MAGIC);
    append_len_prefixed_bytes(&mut out, finished.protocol_version.as_bytes())?;
    append_len_prefixed_bytes(&mut out, finished.cipher_suite.as_bytes())?;
    append_len_prefixed_bytes(&mut out, finished.session_id.as_bytes())?;
    append_len_prefixed_bytes(&mut out, finished.initiator_endpoint_id.as_bytes())?;
    append_len_prefixed_bytes(&mut out, finished.responder_endpoint_id.as_bytes())?;
    append_len_prefixed_bytes(&mut out, finished.handshake_transcript_hash.as_bytes())?;
    append_len_prefixed_bytes(&mut out, finished.capability_transcript_digest.as_bytes())?;
    Ok(out)
}

pub(in crate::core::secure_mesh_pairwise) fn initiator_finished_key_confirmation(
    root_key: &[u8; ROOT_KEY_LEN],
    finished: &SecureMeshPairwiseSessionFinished,
) -> Result<String> {
    let confirmation_key = derive_initiator_finished_key(root_key, finished)?;
    let mut mac = <HmacSha256 as Mac>::new_from_slice(confirmation_key.as_ref())
        .map_err(|_| anyhow!("secure mesh pairwise finished initialization failed"))?;
    mac.update(&initiator_finished_payload(finished)?);
    Ok(general_purpose::URL_SAFE_NO_PAD.encode(mac.finalize().into_bytes()))
}

pub(in crate::core::secure_mesh_pairwise) fn verify_initiator_finished_key_confirmation(
    root_key: &[u8; ROOT_KEY_LEN],
    finished: &SecureMeshPairwiseSessionFinished,
) -> Result<()> {
    let confirmation = decode_fixed_base64url::<KEY_CONFIRMATION_LEN>(
        &finished.key_confirmation,
        "finished key confirmation",
    )?;
    let confirmation_key = derive_initiator_finished_key(root_key, finished)?;
    let mut mac = <HmacSha256 as Mac>::new_from_slice(confirmation_key.as_ref())
        .map_err(|_| anyhow!("secure mesh pairwise finished initialization failed"))?;
    mac.update(&initiator_finished_payload(finished)?);
    mac.verify_slice(&confirmation)
        .map_err(|_| anyhow!("secure mesh pairwise initiator finished verification failed"))
}

pub(in crate::core::secure_mesh_pairwise) fn derive_initiator_finished_key(
    root_key: &[u8; ROOT_KEY_LEN],
    finished: &SecureMeshPairwiseSessionFinished,
) -> Result<Zeroizing<[u8; KEY_CONFIRMATION_LEN]>> {
    let handshake_hash = decode_fixed_base64url::<HANDSHAKE_HASH_LEN>(
        &finished.handshake_transcript_hash,
        "finished handshake transcript hash",
    )?;
    let capability_digest = crate::core::secure_mesh_capability_proof::decode_sha256_digest(
        &finished.capability_transcript_digest,
        "finished capability transcript digest",
    )?;
    let mut salt = Sha256::new();
    salt.update(INITIATOR_FINISHED_MAGIC);
    salt.update(handshake_hash);
    salt.update(capability_digest);
    let hkdf = Hkdf::<Sha256>::new(Some(&salt.finalize()), root_key);
    let mut info = Vec::new();
    append_len_prefixed_bytes(&mut info, finished.session_id.as_bytes())?;
    append_len_prefixed_bytes(&mut info, finished.initiator_endpoint_id.as_bytes())?;
    append_len_prefixed_bytes(&mut info, finished.responder_endpoint_id.as_bytes())?;
    let mut key = Zeroizing::new([0u8; KEY_CONFIRMATION_LEN]);
    hkdf.expand(&info, key.as_mut())
        .map_err(|_| anyhow!("secure mesh pairwise finished key derivation failed"))?;
    Ok(key)
}

pub(in crate::core::secure_mesh_pairwise) fn derive_key_confirmation_key(
    root_key: &[u8; ROOT_KEY_LEN],
    accepted: &SecureMeshPairwiseSessionAccepted,
) -> Result<Zeroizing<[u8; KEY_CONFIRMATION_LEN]>> {
    let handshake_hash = decode_fixed_base64url::<HANDSHAKE_HASH_LEN>(
        &accepted.handshake_transcript_hash,
        "accept handshake transcript hash",
    )?;
    let hkdf = Hkdf::<Sha256>::new(Some(&handshake_hash), root_key);
    let mut info = Vec::new();
    info.extend_from_slice(KEY_CONFIRMATION_MAGIC);
    append_len_prefixed_bytes(&mut info, accepted.session_id.as_bytes())?;
    append_len_prefixed_bytes(&mut info, accepted.responder_endpoint_id.as_bytes())?;
    let mut key = Zeroizing::new([0u8; KEY_CONFIRMATION_LEN]);
    hkdf.expand(&info, key.as_mut())
        .map_err(|_| anyhow!("secure mesh pairwise key confirmation derivation failed"))?;
    Ok(key)
}

pub(in crate::core::secure_mesh_pairwise) fn decode_fixed_base64url<const N: usize>(
    value: &str,
    label: &str,
) -> Result<[u8; N]> {
    let encoded_length = (N * 8 + 5) / 6;
    ensure!(
        value.len() == encoded_length,
        "secure mesh pairwise {label} length is invalid"
    );
    let mut bytes = [0u8; N];
    let decoded_length = general_purpose::URL_SAFE_NO_PAD
        .decode_slice(value.as_bytes(), &mut bytes)
        .with_context(|| format!("secure mesh pairwise {label} is not base64url"))?;
    ensure!(
        decoded_length == N && general_purpose::URL_SAFE_NO_PAD.encode(bytes) == value,
        "secure mesh pairwise {label} encoding is non-canonical"
    );
    Ok(bytes)
}

pub(in crate::core::secure_mesh_pairwise) fn derive_session_id(
    initiator_identity: &DeviceTrustPublicIdentity,
    responder_identity: &DeviceTrustPublicIdentity,
    initiator_ephemeral_public_key: &[u8],
    responder_signed_prekey_id: &str,
    responder_signed_prekey_public_key: &[u8],
    one_time_prekey_id: Option<&str>,
    one_time_prekey_public_key: Option<&[u8]>,
    one_time_mlkem1024_prekey_id: &str,
    one_time_mlkem1024_prekey_public_key: &[u8],
    mlkem1024_ciphertext: &[u8],
    directory_authorization_digest: &str,
) -> Result<String> {
    ensure!(
        one_time_prekey_id.is_some() == one_time_prekey_public_key.is_some(),
        "secure mesh pairwise one-time prekey transcript is inconsistent"
    );
    let mut out = Vec::new();
    append_len_prefixed_bytes(&mut out, SECURE_MESH_PROTOCOL_VERSION.as_bytes())?;
    append_len_prefixed_bytes(&mut out, SECURE_MESH_PAIRWISE_CIPHER_SUITE.as_bytes())?;
    append_len_prefixed_bytes(&mut out, initiator_identity.endpoint_id.as_bytes())?;
    append_len_prefixed_bytes(&mut out, initiator_identity.fingerprint()?.as_bytes())?;
    append_len_prefixed_bytes(&mut out, responder_identity.endpoint_id.as_bytes())?;
    append_len_prefixed_bytes(&mut out, responder_identity.fingerprint()?.as_bytes())?;
    append_len_prefixed_bytes(&mut out, initiator_ephemeral_public_key)?;
    append_len_prefixed_bytes(&mut out, responder_signed_prekey_id.as_bytes())?;
    append_len_prefixed_bytes(&mut out, responder_signed_prekey_public_key)?;
    append_len_prefixed_bytes(&mut out, one_time_prekey_id.unwrap_or("").as_bytes())?;
    let one_time_prekey_public_key_digest = one_time_prekey_public_key
        .map(|public_key| Sha256::digest(public_key).to_vec())
        .unwrap_or_default();
    append_len_prefixed_bytes(&mut out, &one_time_prekey_public_key_digest)?;
    append_len_prefixed_bytes(&mut out, one_time_mlkem1024_prekey_id.as_bytes())?;
    append_len_prefixed_bytes(
        &mut out,
        &Sha256::digest(one_time_mlkem1024_prekey_public_key),
    )?;
    append_len_prefixed_bytes(&mut out, &Sha256::digest(mlkem1024_ciphertext))?;
    append_len_prefixed_bytes(&mut out, directory_authorization_digest.as_bytes())?;
    Ok(hash_bytes(&out))
}
