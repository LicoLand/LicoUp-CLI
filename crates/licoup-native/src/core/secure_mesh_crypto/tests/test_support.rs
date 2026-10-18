pub(super) use super::super::{
    constants::{
        CONTENT_KEY_LEN, CONTENT_NONCE_LEN, MAX_CONTEXT_FIELD_BYTES, PRIVATE_CONTEXT_AEAD_AAD,
    },
    content_key::ContentKey,
    frame_codec::{encode_plaintext, encode_private_context_frame},
    header_codec::encode_private_context_header,
    key_derivation::derive_private_context_aead_key,
    model::{
        SealedSecureMeshPrivateContextPayload, SecureMeshContentContext, SecureMeshPayloadKind,
        SecureMeshPlaintext,
    },
    padding::{
        add_bucket_padding, padding_bucket_for_ciphertext_size, remove_authenticated_padding,
    },
    private_context::{open_private_context_payload, seal_private_context_payload_with_nonce},
    public_payload::{open_payload, seal_payload_with_nonce},
};
pub(super) use base64::{Engine as _, engine::general_purpose};
pub(super) use chacha20poly1305::{
    ChaCha20Poly1305, Key, Nonce,
    aead::{Aead, KeyInit, Payload as AeadPayload},
};
pub(super) use licoup_protocol_bindings::{
    LARGE_PADDING_BUCKET_STEP_BYTES, MAX_PADDING_BUCKET_BYTES, MIN_PADDING_BUCKET_BYTES,
    POWER_OF_TWO_PADDING_LIMIT_BYTES, validate_authenticated_padding_bucket,
};
pub(super) use sha2::{Digest, Sha256};

pub(super) fn context_fixture() -> SecureMeshContentContext {
    SecureMeshContentContext::new(
        "env_test",
        "msg_test",
        "mailbox_test",
        "desktop_gui:alpha",
        "mobile:beta",
        "pairwise_session_test",
        "2026-01-01T00:00:00.000Z",
        "2026-01-01T00:10:00.000Z",
    )
}

pub(super) fn key_fixture(byte: u8) -> ContentKey {
    ContentKey::from_bytes([byte; CONTENT_KEY_LEN])
}
