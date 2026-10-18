//! Existing XChaCha20-Poly1305 private-header protection bound to Lico Arc AAD.

use anyhow::{Result, anyhow, ensure};
use chacha20poly1305::{
    Key, KeyInit, XChaCha20Poly1305, XNonce,
    aead::{Aead, Payload as AeadPayload},
};
use rand_core::{OsRng, RngCore};
use zeroize::Zeroizing;

use super::constants::{
    LICOARC_ENCRYPTED_HEADER_BYTES, RELAY_HEADER_FRAME_BYTES, RELAY_HEADER_KEY_BYTES,
    RELAY_HEADER_NONCE_BYTES, RELAY_HEADER_TAG_BYTES,
};
use super::draft::LicoArcRelayEnvelopeDraft;
use super::envelope::LicoArcRelayEnvelope;
use super::private_header_frame::{
    decode_private_relay_header_frame, encode_private_relay_header_frame,
};

pub fn seal_private_relay_header(
    draft: &LicoArcRelayEnvelopeDraft,
    header_key: &[u8],
    private_header: &[u8],
) -> Result<[u8; LICOARC_ENCRYPTED_HEADER_BYTES]> {
    let mut nonce = [0u8; RELAY_HEADER_NONCE_BYTES];
    OsRng.fill_bytes(&mut nonce);
    seal_private_relay_header_with_nonce(draft, header_key, private_header, nonce)
}

pub fn open_private_relay_header<'a>(
    envelope: &LicoArcRelayEnvelope,
    candidate_header_keys: impl IntoIterator<Item = &'a [u8]>,
) -> Result<Zeroizing<Vec<u8>>> {
    let wire = envelope.decoded_encrypted_header()?;
    ensure!(
        wire.len() == LICOARC_ENCRYPTED_HEADER_BYTES,
        "secure mesh private relay header wire length is invalid"
    );
    let (nonce, encrypted) = wire.split_at(RELAY_HEADER_NONCE_BYTES);
    let aad = envelope.authenticated_outer_data()?;
    let mut attempted = 0usize;
    for header_key in candidate_header_keys {
        attempted = attempted
            .checked_add(1)
            .ok_or_else(|| anyhow!("secure mesh private relay header key count overflow"))?;
        ensure!(
            attempted <= 1_024,
            "secure mesh private relay header candidate-key limit exceeded"
        );
        if header_key.len() != RELAY_HEADER_KEY_BYTES {
            continue;
        }
        let cipher = XChaCha20Poly1305::new(Key::from_slice(header_key));
        let Ok(plaintext) = cipher.decrypt(
            XNonce::from_slice(nonce),
            AeadPayload {
                msg: encrypted,
                aad: &aad,
            },
        ) else {
            continue;
        };
        return decode_private_relay_header_frame(Zeroizing::new(plaintext));
    }
    Err(anyhow!(
        "secure mesh private relay header authentication failed"
    ))
}

pub(in crate::licoarc_relay) fn seal_private_relay_header_with_nonce(
    draft: &LicoArcRelayEnvelopeDraft,
    header_key: &[u8],
    private_header: &[u8],
    nonce: [u8; RELAY_HEADER_NONCE_BYTES],
) -> Result<[u8; LICOARC_ENCRYPTED_HEADER_BYTES]> {
    ensure!(
        header_key.len() == RELAY_HEADER_KEY_BYTES,
        "secure mesh private relay header key length is invalid"
    );
    let plaintext = encode_private_relay_header_frame(private_header)?;
    let aad = draft.authenticated_outer_data()?;
    let cipher = XChaCha20Poly1305::new(Key::from_slice(header_key));
    let encrypted = cipher
        .encrypt(
            XNonce::from_slice(&nonce),
            AeadPayload {
                msg: plaintext.as_slice(),
                aad: &aad,
            },
        )
        .map_err(|_| anyhow!("secure mesh private relay header encryption failed"))?;
    ensure!(
        encrypted.len() == RELAY_HEADER_FRAME_BYTES + RELAY_HEADER_TAG_BYTES,
        "secure mesh private relay header ciphertext length is invalid"
    );
    let mut wire = [0u8; LICOARC_ENCRYPTED_HEADER_BYTES];
    wire[..RELAY_HEADER_NONCE_BYTES].copy_from_slice(&nonce);
    wire[RELAY_HEADER_NONCE_BYTES..].copy_from_slice(&encrypted);
    Ok(wire)
}
