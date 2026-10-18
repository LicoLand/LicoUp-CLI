use anyhow::{Context, Result, anyhow, ensure};
use base64::{Engine as _, engine::general_purpose};
use hmac::Hmac;
use sha2::{Digest, Sha256};
use zeroize::Zeroizing;

pub const SECURE_MESH_PAIRWISE_CIPHER_SUITE: &str =
    "licomesh.pqxdh-triple-ratchet.v1.x25519-ed25519-mlkem1024-hkdfsha256-chacha20poly1305";
pub const SECURE_MESH_PAIRWISE_STATUS: &str = "authenticated_transcript_pqxdh_mlkem1024_triple_ratchet_encrypted_headers_bounded_skipped_header_keys_capability_bound_explicit_finished_bilateral_key_confirmation_unique_bound_snapshots_sesame_session_manager_multi_device_fanout_payload_codec_cross_endpoint_command_result_relay_available_independent_review_pending";

pub(super) const ROOT_KEY_LEN: usize = 32;
pub(super) const CHAIN_KEY_LEN: usize = 32;
pub(super) const MESSAGE_KEY_LEN: usize = 32;
pub(super) const HEADER_KEY_LEN: usize = 32;
pub(super) const NONCE_LEN: usize = 12;
pub(super) const PUBLIC_KEY_LEN: usize = 32;
pub(super) const MAX_SKIPPED_KEYS: usize = 32;
pub(super) const MAX_REPLAY_IDS: usize = 256;
pub(super) const MAX_CIPHERTEXT_BYTES: usize = 16 * 1024 * 1024;
pub(super) const MAX_CONTENT_ENCRYPTED_HEADER_BYTES: usize = 1024;
pub(super) const MAX_SPARSE_PQ_HEADER_BYTES: usize = 512;
pub(super) const MAX_PERSISTED_SECRET_SNAPSHOT_BYTES: usize = 2 * 1024 * 1024;
pub(super) const MAX_ENCODED_SPARSE_PQ_RATCHET_BYTES: usize = (1024 * 1024 * 8 + 5) / 6;
pub(super) const MAX_ENDPOINT_ID_LEN: usize = 255;
pub(super) const MAX_MESSAGE_ID_LEN: usize = 255;
pub(super) const MAX_PERSISTED_CAPABILITY_PROOF_USES: usize = 4096;

pub(super) const PQXDH_CLASSICAL_SALT_DOMAIN: &[u8] =
    b"licomesh.secure-mesh.pqxdh-classical.salt.v1";
pub(super) const PQXDH_CLASSICAL_INFO_DOMAIN: &[u8] =
    b"licomesh.secure-mesh.pqxdh-classical.info.v1";
pub(super) const CHAIN_INFO_DOMAIN: &[u8] = b"licomesh.secure-mesh.double-ratchet.chain.v1";
pub(super) const ROOT_INFO_DOMAIN: &[u8] = b"licomesh.secure-mesh.double-ratchet.root.v1";
pub(super) const MESSAGE_AAD_MAGIC: &[u8] = b"LCOSM-PAIRWISE-AAD-v1";
pub(super) const PAYLOAD_AAD_BINDING_MAGIC: &[u8] = b"LCOSM-PAIRWISE-PAYLOAD-AAD-v1";
pub(super) const SECRET_DOMAIN: &[u8] = b"LCOSM-PAIRWISE-SECRET-v1";
pub(super) const INTRO_SIGNATURE_MAGIC: &[u8] = b"LCOSM-PAIRWISE-INTRO-SIGNATURE-v1";
pub(super) const ACCEPT_SIGNATURE_MAGIC: &[u8] = b"LCOSM-PAIRWISE-ACCEPT-SIGNATURE-v1";
pub(super) const HANDSHAKE_TRANSCRIPT_MAGIC: &[u8] = b"LCOSM-PAIRWISE-HANDSHAKE-v1";
pub(super) const KEY_CONFIRMATION_MAGIC: &[u8] = b"LCOSM-PAIRWISE-KEY-CONFIRMATION-v1";
pub(super) const CAPABILITY_BOUND_KEY_SCHEDULE_MAGIC: &[u8] =
    b"LCOSM-PAIRWISE-CAPABILITY-BOUND-KEY-SCHEDULE-v1";
pub(super) const INITIATOR_FINISHED_MAGIC: &[u8] = b"LCOSM-PAIRWISE-INITIATOR-FINISHED-v1";
pub(super) const HANDSHAKE_HASH_LEN: usize = 32;
pub(super) const SIGNATURE_LEN: usize = 64;
pub(super) const KEY_CONFIRMATION_LEN: usize = 32;
pub(super) const PAIRWISE_SNAPSHOT_SCHEMA_VERSION: u32 = 11;
pub(super) const PAIRWISE_SECRET_STORE_CLASS: &str = "pairwiseSessionSnapshot";

pub(super) type HmacSha256 = Hmac<Sha256>;

pub const SECURE_MESH_PAIRWISE_CAPABILITY_POLICY_REVISION: u64 = 1;

pub(super) fn parse_key_bytes(bytes: &[u8], label: &str) -> Result<[u8; PUBLIC_KEY_LEN]> {
    ensure!(
        bytes.len() == PUBLIC_KEY_LEN,
        "secure mesh pairwise {label} length is invalid"
    );
    let mut out = [0u8; PUBLIC_KEY_LEN];
    out.copy_from_slice(bytes);
    Ok(out)
}

pub(super) fn decode_secret_32(value: &str) -> Result<[u8; PUBLIC_KEY_LEN]> {
    let expected_encoded_length = (PUBLIC_KEY_LEN * 8 + 5) / 6;
    ensure!(
        value.len() == expected_encoded_length,
        "secure mesh pairwise persisted secret length is invalid"
    );
    let mut bytes = [0u8; PUBLIC_KEY_LEN];
    let decoded_length = general_purpose::URL_SAFE_NO_PAD
        .decode_slice(value.as_bytes(), &mut bytes)
        .context("secure mesh pairwise persisted secret is not base64url")?;
    ensure!(
        decoded_length == PUBLIC_KEY_LEN,
        "secure mesh pairwise persisted secret length is invalid"
    );
    let canonical = Zeroizing::new(general_purpose::URL_SAFE_NO_PAD.encode(bytes));
    ensure!(
        canonical.as_str() == value,
        "secure mesh pairwise persisted secret encoding is non-canonical"
    );
    Ok(bytes)
}

pub(super) fn encode_secret(bytes: &[u8; PUBLIC_KEY_LEN]) -> String {
    general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

pub(super) fn sha256_hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

pub(super) fn append_len_prefixed_bytes(out: &mut Vec<u8>, value: &[u8]) -> Result<()> {
    let len = u32::try_from(value.len())
        .map_err(|_| anyhow!("secure mesh pairwise field is too large"))?;
    out.extend_from_slice(&len.to_be_bytes());
    out.extend_from_slice(value);
    Ok(())
}

pub(super) fn encoded_len_limit(decoded_bytes: usize) -> usize {
    decoded_bytes.div_ceil(3).saturating_mul(4)
}

pub(super) fn validate_endpoint_id(value: &str) -> Result<()> {
    let value = require_text(value.to_string(), "endpoint id")?;
    ensure!(
        value.len() <= MAX_ENDPOINT_ID_LEN,
        "secure mesh pairwise endpoint id is too large"
    );
    Ok(())
}

pub(super) fn validate_message_id(value: &str) -> Result<()> {
    let value = require_text(value.to_string(), "message id")?;
    ensure!(
        value.len() <= MAX_MESSAGE_ID_LEN,
        "secure mesh pairwise message id is too large"
    );
    Ok(())
}

pub(super) fn require_text(value: String, label: &str) -> Result<String> {
    ensure!(
        !value.trim().is_empty(),
        "secure mesh pairwise {label} is required"
    );
    Ok(value)
}

pub(super) fn require_sha256_hex(value: String, label: &str) -> Result<String> {
    ensure!(
        value.len() == 64
            && value
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)),
        "secure mesh pairwise {label} must be canonical lowercase SHA-256 hex"
    );
    Ok(value)
}

pub(super) fn hash_bytes(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    format!(
        "sha256:{}",
        digest
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>()
    )
}
