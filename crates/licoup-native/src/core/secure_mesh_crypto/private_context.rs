use anyhow::{Result, anyhow};
use base64::{Engine as _, engine::general_purpose};
use chacha20poly1305::{
    ChaCha20Poly1305, Key, Nonce,
    aead::{Aead, KeyInit, Payload as AeadPayload},
};
use rand_core::{OsRng, RngCore};
use zeroize::Zeroizing;

use super::{
    constants::{CONTENT_NONCE_LEN, PRIVATE_CONTEXT_AEAD_AAD},
    content_key::ContentKey,
    frame_codec::{decode_private_context_frame, encode_private_context_frame},
    header_codec::{
        decode_canonical_base64url, decode_private_context_header, encode_private_context_header,
    },
    key_derivation::derive_private_context_aead_key,
    model::{
        OpenedSecureMeshPrivateContextPayload, SealedSecureMeshPrivateContextPayload,
        SecureMeshContentContext, SecureMeshPlaintext,
    },
    padding::{add_bucket_padding, remove_authenticated_padding},
    validation::validate_plaintext,
};
use licoup_protocol_bindings::validate_authenticated_padding_bucket;

impl SealedSecureMeshPrivateContextPayload {
    pub(crate) fn from_encoded_parts(
        encrypted_header: String,
        ciphertext: String,
        ciphertext_size: usize,
    ) -> Result<Self> {
        let sealed = Self {
            encrypted_header,
            ciphertext,
            ciphertext_size,
        };
        sealed.validate()?;
        Ok(sealed)
    }

    fn validate(&self) -> Result<()> {
        decode_private_context_header(&self.encrypted_header)?;
        validate_authenticated_padding_bucket(self.ciphertext_size)?;
        decode_canonical_base64url(
            "private-context ciphertext",
            &self.ciphertext,
            self.ciphertext_size,
            self.ciphertext_size,
        )?;
        Ok(())
    }
}

pub(crate) fn seal_private_context_payload(
    key: &ContentKey,
    context: &SecureMeshContentContext,
    plaintext: &SecureMeshPlaintext,
) -> Result<SealedSecureMeshPrivateContextPayload> {
    let mut nonce = [0u8; CONTENT_NONCE_LEN];
    OsRng.fill_bytes(&mut nonce);
    seal_private_context_payload_with_nonce(key, context, plaintext, nonce)
}

pub(crate) fn open_private_context_payload(
    key: &ContentKey,
    sealed: &SealedSecureMeshPrivateContextPayload,
) -> Result<OpenedSecureMeshPrivateContextPayload> {
    sealed.validate()?;
    let nonce = decode_private_context_header(&sealed.encrypted_header)?;
    let ciphertext = decode_canonical_base64url(
        "private-context ciphertext",
        &sealed.ciphertext,
        sealed.ciphertext_size,
        sealed.ciphertext_size,
    )?;
    let derived_key = derive_private_context_aead_key(key)?;
    let cipher = ChaCha20Poly1305::new(Key::from_slice(derived_key.as_slice()));
    let plaintext = Zeroizing::new(
        cipher
            .decrypt(
                Nonce::from_slice(&nonce),
                AeadPayload {
                    msg: &ciphertext,
                    aad: PRIVATE_CONTEXT_AEAD_AAD,
                },
            )
            .map_err(|_| anyhow!("secure mesh private-context payload authentication failed"))?,
    );
    let unpadded = remove_authenticated_padding(&plaintext)?;
    decode_private_context_frame(unpadded)
}

pub(in crate::core::secure_mesh_crypto) fn seal_private_context_payload_with_nonce(
    key: &ContentKey,
    context: &SecureMeshContentContext,
    plaintext: &SecureMeshPlaintext,
    nonce: [u8; CONTENT_NONCE_LEN],
) -> Result<SealedSecureMeshPrivateContextPayload> {
    context.validate()?;
    validate_plaintext(plaintext)?;
    let encoded_plaintext = encode_private_context_frame(context, plaintext)?;
    let padded_plaintext = add_bucket_padding(&encoded_plaintext)?;
    let derived_key = derive_private_context_aead_key(key)?;
    let cipher = ChaCha20Poly1305::new(Key::from_slice(derived_key.as_slice()));
    let ciphertext = cipher
        .encrypt(
            Nonce::from_slice(&nonce),
            AeadPayload {
                msg: padded_plaintext.as_slice(),
                aad: PRIVATE_CONTEXT_AEAD_AAD,
            },
        )
        .map_err(|_| anyhow!("secure mesh private-context payload encryption failed"))?;
    SealedSecureMeshPrivateContextPayload::from_encoded_parts(
        encode_private_context_header(&nonce),
        general_purpose::URL_SAFE_NO_PAD.encode(&ciphertext),
        ciphertext.len(),
    )
}
