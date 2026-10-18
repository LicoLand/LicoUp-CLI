use super::constants::*;
use super::model::*;
use super::primitives::*;
use anyhow::{Result, anyhow, ensure};
use base64::{Engine as _, engine::general_purpose};
use chacha20poly1305::{
    Key, KeyInit, XChaCha20Poly1305, XNonce,
    aead::{Aead, Payload as AeadPayload},
};
use hkdf::Hkdf;
use hmac::{Hmac, Mac};
use rand_core::{OsRng, RngCore};
use sha2::{Digest, Sha256};
use std::collections::HashSet;
use zeroize::Zeroizing;

use crate::core::secure_mesh_crypto::SecureMeshContentContext;

pub fn seal_file_root_key_for_pairwise_device(
    root_key: &FileRootKey,
    wrap_secret: &FileKeyWrapSecret,
    context: &SecureMeshFileProtectionContext,
) -> Result<FileKeyEnvelope> {
    ensure!(
        context.envelope_mode() == FileKeyEnvelopeMode::PairwiseDevice,
        "secure mesh pairwise file key context has the wrong channel mode"
    );
    seal_file_root_key(root_key, wrap_secret, context)
}

pub fn seal_file_root_key_for_pairwise_devices<'a>(
    root_key: &FileRootKey,
    targets: impl IntoIterator<Item = (&'a FileKeyWrapSecret, &'a SecureMeshFileProtectionContext)>,
) -> Result<Vec<FileKeyEnvelope>> {
    let mut envelopes = Vec::new();
    let mut recipients = HashSet::new();
    let mut first_context: Option<&SecureMeshFileProtectionContext> = None;
    for (wrap_secret, context) in targets {
        ensure!(
            context.envelope_mode() == FileKeyEnvelopeMode::PairwiseDevice,
            "secure mesh pairwise file key target has the wrong channel mode"
        );
        if let Some(first) = first_context {
            ensure!(
                first.same_transfer_as(context),
                "secure mesh pairwise file key targets do not describe one transfer"
            );
        } else {
            first_context = Some(context);
        }
        ensure!(
            recipients.insert(context.recipient_endpoint_id().to_string()),
            "secure mesh pairwise file key recipient is duplicated"
        );
        envelopes.push(seal_file_root_key_for_pairwise_device(
            root_key,
            wrap_secret,
            context,
        )?);
    }
    ensure!(
        !envelopes.is_empty(),
        "secure mesh pairwise file key target list is empty"
    );
    Ok(envelopes)
}

pub fn open_file_root_key_for_pairwise_device(
    envelope: &FileKeyEnvelope,
    wrap_secret: &FileKeyWrapSecret,
    context: &SecureMeshFileProtectionContext,
    now_unix_seconds: u64,
) -> Result<FileRootKey> {
    ensure!(
        context.envelope_mode() == FileKeyEnvelopeMode::PairwiseDevice,
        "secure mesh pairwise file key context has the wrong channel mode"
    );
    ensure!(
        envelope.mode == FileKeyEnvelopeMode::PairwiseDevice,
        "secure mesh pairwise file key envelope has the wrong channel mode"
    );
    open_file_root_key(envelope, wrap_secret, context, now_unix_seconds)
}

pub fn seal_file_root_key_for_mls_epoch(
    root_key: &FileRootKey,
    exporter_secret: &FileKeyWrapSecret,
    context: &SecureMeshFileProtectionContext,
) -> Result<FileKeyEnvelope> {
    ensure!(
        context.envelope_mode() == FileKeyEnvelopeMode::MlsEpoch,
        "secure mesh MLS file key context has the wrong channel mode"
    );
    seal_file_root_key(root_key, exporter_secret, context)
}

pub fn open_file_root_key_for_mls_epoch(
    envelope: &FileKeyEnvelope,
    exporter_secret: &FileKeyWrapSecret,
    context: &SecureMeshFileProtectionContext,
    current_epoch: u64,
    now_unix_seconds: u64,
) -> Result<FileRootKey> {
    ensure!(
        context.envelope_mode() == FileKeyEnvelopeMode::MlsEpoch,
        "secure mesh MLS file key context has the wrong channel mode"
    );
    ensure!(
        context.mls_epoch() == Some(current_epoch),
        "secure mesh MLS file key context is not for the current epoch"
    );
    ensure!(
        envelope.mode == FileKeyEnvelopeMode::MlsEpoch && envelope.epoch == Some(current_epoch),
        "secure mesh MLS file key envelope is not for the current epoch"
    );
    open_file_root_key(envelope, exporter_secret, context, now_unix_seconds)
}

pub fn authenticate_file_chunk_receipt(
    root_key: &FileRootKey,
    context: &SecureMeshFileProtectionContext,
    encrypted: &EncryptedSecureMeshFileChunk,
    now_unix_seconds: u64,
) -> Result<AuthenticatedSecureMeshFileReceipt> {
    context.validate()?;
    context.ensure_not_expired(now_unix_seconds)?;
    ensure_file_chunk_context(context, encrypted.chunk_index)?;
    validate_file_hash("ciphertext hash", &encrypted.ciphertext_hash)?;
    let aad = file_authenticated_data(
        context,
        FILE_AAD_RECEIPT_PURPOSE,
        Some(encrypted.chunk_index),
        &encrypted.ciphertext_hash,
    )?;
    let key = derive_file_key(
        root_key.as_bytes(),
        FILE_HKDF_RECEIPT_DOMAIN,
        aad.as_slice(),
    )?;
    let mut mac = <Hmac<Sha256> as Mac>::new_from_slice(key.as_ref())
        .map_err(|_| anyhow!("secure mesh file receipt MAC key is invalid"))?;
    mac.update(&aad);
    Ok(AuthenticatedSecureMeshFileReceipt {
        chunk_index: encrypted.chunk_index,
        ciphertext_hash: encrypted.ciphertext_hash.clone(),
        authentication_tag: general_purpose::URL_SAFE_NO_PAD.encode(mac.finalize().into_bytes()),
    })
}

pub fn verify_file_chunk_receipt(
    root_key: &FileRootKey,
    context: &SecureMeshFileProtectionContext,
    receipt: &AuthenticatedSecureMeshFileReceipt,
    now_unix_seconds: u64,
) -> Result<()> {
    context.validate()?;
    context.ensure_not_expired(now_unix_seconds)?;
    ensure_file_chunk_context(context, receipt.chunk_index)?;
    validate_file_hash("ciphertext hash", &receipt.ciphertext_hash)?;
    let tag = decode_exact_base64url(
        "file receipt authentication tag",
        &receipt.authentication_tag,
        32,
    )?;
    let aad = file_authenticated_data(
        context,
        FILE_AAD_RECEIPT_PURPOSE,
        Some(receipt.chunk_index),
        &receipt.ciphertext_hash,
    )?;
    let key = derive_file_key(
        root_key.as_bytes(),
        FILE_HKDF_RECEIPT_DOMAIN,
        aad.as_slice(),
    )?;
    let mut mac = <Hmac<Sha256> as Mac>::new_from_slice(key.as_ref())
        .map_err(|_| anyhow!("secure mesh file receipt MAC key is invalid"))?;
    mac.update(&aad);
    mac.verify_slice(&tag)
        .map_err(|_| anyhow!("secure mesh file receipt authentication failed"))
}

pub(super) fn seal_file_root_key(
    root_key: &FileRootKey,
    wrap_secret: &FileKeyWrapSecret,
    context: &SecureMeshFileProtectionContext,
) -> Result<FileKeyEnvelope> {
    context.validate()?;
    let aad = file_authenticated_data(
        context,
        FILE_AAD_KEY_WRAP_PURPOSE,
        None,
        context.file_hash(),
    )?;
    let context_digest = file_aad_digest(&aad);
    let key = derive_file_key(
        wrap_secret.as_bytes(),
        FILE_HKDF_KEY_WRAP_DOMAIN,
        aad.as_slice(),
    )?;
    let mut frame = Zeroizing::new(Vec::with_capacity(file_key_envelope_frame_bytes()));
    frame.extend_from_slice(FILE_KEY_ENVELOPE_FRAME_MAGIC);
    frame.extend_from_slice(root_key.as_bytes());
    frame.extend_from_slice(&context_digest);
    ensure!(
        frame.len() == file_key_envelope_frame_bytes(),
        "secure mesh file key envelope frame length is invalid"
    );
    let mut nonce = [0u8; FILE_KEY_ENVELOPE_NONCE_BYTES];
    OsRng.fill_bytes(&mut nonce);
    let cipher = XChaCha20Poly1305::new(Key::from_slice(key.as_ref()));
    let ciphertext = cipher
        .encrypt(
            XNonce::from_slice(&nonce),
            AeadPayload {
                msg: frame.as_slice(),
                aad: &aad,
            },
        )
        .map_err(|_| anyhow!("secure mesh file key envelope encryption failed"))?;
    let envelope = FileKeyEnvelope {
        schema: FILE_KEY_ENVELOPE_SCHEMA.to_string(),
        suite: SECURE_MESH_FILE_KEY_SUITE.to_string(),
        mode: context.envelope_mode(),
        context_digest: general_purpose::URL_SAFE_NO_PAD.encode(context_digest),
        epoch: context.mls_epoch(),
        expires_at_unix_seconds: context.expires_at_unix_seconds,
        nonce: general_purpose::URL_SAFE_NO_PAD.encode(nonce),
        ciphertext: general_purpose::URL_SAFE_NO_PAD.encode(ciphertext),
    };
    envelope.validate()?;
    Ok(envelope)
}

pub(super) fn open_file_root_key(
    envelope: &FileKeyEnvelope,
    wrap_secret: &FileKeyWrapSecret,
    context: &SecureMeshFileProtectionContext,
    now_unix_seconds: u64,
) -> Result<FileRootKey> {
    envelope.validate()?;
    context.validate()?;
    context.ensure_not_expired(now_unix_seconds)?;
    ensure!(
        envelope.mode == context.envelope_mode(),
        "secure mesh file key envelope channel context mismatch"
    );
    ensure!(
        envelope.epoch == context.mls_epoch(),
        "secure mesh file key envelope epoch context mismatch"
    );
    ensure!(
        envelope.expires_at_unix_seconds == context.expires_at_unix_seconds,
        "secure mesh file key envelope expiry context mismatch"
    );
    let aad = file_authenticated_data(
        context,
        FILE_AAD_KEY_WRAP_PURPOSE,
        None,
        context.file_hash(),
    )?;
    let context_digest = file_aad_digest(&aad);
    ensure!(
        decode_exact_base64url("file key context digest", &envelope.context_digest, 32)?
            == context_digest,
        "secure mesh file key envelope context mismatch"
    );
    let key = derive_file_key(
        wrap_secret.as_bytes(),
        FILE_HKDF_KEY_WRAP_DOMAIN,
        aad.as_slice(),
    )?;
    let nonce = decode_exact_base64url(
        "file key nonce",
        &envelope.nonce,
        FILE_KEY_ENVELOPE_NONCE_BYTES,
    )?;
    let ciphertext = decode_exact_base64url(
        "file key ciphertext",
        &envelope.ciphertext,
        file_key_envelope_frame_bytes() + FILE_KEY_ENVELOPE_TAG_BYTES,
    )?;
    let cipher = XChaCha20Poly1305::new(Key::from_slice(key.as_ref()));
    let plaintext = cipher
        .decrypt(
            XNonce::from_slice(&nonce),
            AeadPayload {
                msg: &ciphertext,
                aad: &aad,
            },
        )
        .map_err(|_| anyhow!("secure mesh file key envelope authentication failed"))?;
    let plaintext = Zeroizing::new(plaintext);
    ensure!(
        plaintext.len() == file_key_envelope_frame_bytes()
            && plaintext.starts_with(FILE_KEY_ENVELOPE_FRAME_MAGIC),
        "secure mesh file key envelope frame is invalid"
    );
    let key_start = FILE_KEY_ENVELOPE_FRAME_MAGIC.len();
    let digest_start = key_start + FILE_ROOT_KEY_BYTES;
    ensure!(
        plaintext[digest_start..] == context_digest,
        "secure mesh file key envelope frame context mismatch"
    );
    let mut root_key = [0u8; FILE_ROOT_KEY_BYTES];
    root_key.copy_from_slice(&plaintext[key_start..digest_start]);
    Ok(FileRootKey::from_bytes(root_key))
}

pub(super) fn file_key_envelope_frame_bytes() -> usize {
    FILE_KEY_ENVELOPE_FRAME_MAGIC.len() + FILE_ROOT_KEY_BYTES + 32
}

pub(super) fn derive_file_key(
    input_key_material: &[u8],
    domain: &[u8],
    aad: &[u8],
) -> Result<Zeroizing<[u8; 32]>> {
    let mut info = Vec::with_capacity(domain.len() + aad.len() + 8);
    append_len_prefixed_bytes(&mut info, domain)?;
    append_len_prefixed_bytes(&mut info, aad)?;
    let hkdf = Hkdf::<Sha256>::new(Some(FILE_HKDF_SALT), input_key_material);
    let mut key = Zeroizing::new([0u8; 32]);
    hkdf.expand(&info, key.as_mut())
        .map_err(|_| anyhow!("secure mesh file HKDF expansion failed"))?;
    Ok(key)
}

pub(super) fn file_authenticated_data(
    context: &SecureMeshFileProtectionContext,
    purpose: &[u8],
    chunk_index: Option<u32>,
    object_hash: &str,
) -> Result<Vec<u8>> {
    context.validate()?;
    validate_authenticated_digest("file AAD object hash", object_hash)?;
    if let Some(index) = chunk_index {
        ensure_file_chunk_context(context, index)?;
    }
    let mut aad = Vec::with_capacity(1024);
    append_len_prefixed_bytes(&mut aad, FILE_AAD_MAGIC)?;
    append_len_prefixed_bytes(&mut aad, SECURE_MESH_FILE_KEY_SUITE.as_bytes())?;
    append_len_prefixed_bytes(&mut aad, purpose)?;
    append_len_prefixed_bytes(&mut aad, context.file_id.as_bytes())?;
    match chunk_index {
        Some(index) => {
            aad.push(1);
            aad.extend_from_slice(&index.to_be_bytes());
        }
        None => aad.push(0),
    }
    aad.extend_from_slice(&context.chunk_count.to_be_bytes());
    append_len_prefixed_bytes(&mut aad, context.file_hash.as_bytes())?;
    append_len_prefixed_bytes(&mut aad, object_hash.as_bytes())?;
    append_len_prefixed_bytes(
        &mut aad,
        context.content_context.sender_endpoint_id.as_bytes(),
    )?;
    append_len_prefixed_bytes(
        &mut aad,
        context.content_context.recipient_endpoint_id.as_bytes(),
    )?;
    append_len_prefixed_bytes(&mut aad, context.content_context.session_id.as_bytes())?;
    match &context.channel {
        SecureMeshFileChannelBinding::PairwiseDevice => {
            append_len_prefixed_bytes(&mut aad, b"pairwise-device")?;
        }
        SecureMeshFileChannelBinding::MlsEpoch { group_id, epoch } => {
            append_len_prefixed_bytes(&mut aad, b"mls-epoch")?;
            append_len_prefixed_bytes(&mut aad, group_id.as_bytes())?;
            aad.extend_from_slice(&epoch.to_be_bytes());
        }
    }
    aad.extend_from_slice(&context.expires_at_unix_seconds.to_be_bytes());
    append_len_prefixed_bytes(&mut aad, context.content_context.envelope_id.as_bytes())?;
    append_len_prefixed_bytes(&mut aad, context.content_context.message_id.as_bytes())?;
    append_len_prefixed_bytes(
        &mut aad,
        context.content_context.opaque_mailbox_id.as_bytes(),
    )?;
    append_len_prefixed_bytes(&mut aad, context.content_context.created_at.as_bytes())?;
    append_len_prefixed_bytes(&mut aad, context.content_context.expires_at.as_bytes())?;
    Ok(aad)
}

pub(super) fn file_aad_digest(aad: &[u8]) -> [u8; 32] {
    Sha256::digest(aad).into()
}

pub(super) fn scoped_file_content_context(
    context: &SecureMeshFileProtectionContext,
    aad: &[u8],
) -> SecureMeshContentContext {
    let mut scoped = context.content_context.clone();
    scoped.message_id = format!(
        "file-aad-v2:{}",
        general_purpose::URL_SAFE_NO_PAD.encode(file_aad_digest(aad))
    );
    scoped
}

pub(super) fn ensure_file_chunk_context(
    context: &SecureMeshFileProtectionContext,
    chunk_index: u32,
) -> Result<()> {
    ensure!(
        chunk_index < context.chunk_count,
        "secure mesh file chunk index is outside the protected manifest"
    );
    Ok(())
}

pub(super) fn authenticated_file_chunk_hash(
    root_key: &FileRootKey,
    context: &SecureMeshFileProtectionContext,
    chunk_index: u32,
    chunk_bytes: &[u8],
) -> Result<String> {
    let aad = file_authenticated_data(
        context,
        FILE_AAD_CHUNK_HASH_PURPOSE,
        Some(chunk_index),
        context.file_hash(),
    )?;
    let key = derive_file_key(root_key.as_bytes(), FILE_HKDF_CHUNK_HASH_DOMAIN, &aad)?;
    let mut mac = <Hmac<Sha256> as Mac>::new_from_slice(key.as_ref())
        .map_err(|_| anyhow!("secure mesh file chunk hash key is invalid"))?;
    mac.update(&aad);
    mac.update(chunk_bytes);
    Ok(format!(
        "hmac-sha256:{}",
        general_purpose::URL_SAFE_NO_PAD.encode(mac.finalize().into_bytes())
    ))
}
