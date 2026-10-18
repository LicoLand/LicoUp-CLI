use anyhow::{Context, Result, anyhow, ensure};
use base64::{Engine as _, engine::general_purpose};
use sha2::{Digest, Sha256};

use crate::core::secure_mesh::SECURE_MESH_RESULT_PROTOCOL_VERSION;
use crate::core::secure_mesh_crypto::{
    ContentKey, SealedSecureMeshPayload, SecureMeshContentContext, SecureMeshPayloadKind,
    SecureMeshPlaintext, open_payload, seal_payload,
};

pub const SECURE_MESH_RESULT_CONTENT_TYPE: &str = "application/licomesh.secure-mesh.result.v1";
pub const SECURE_MESH_ERROR_CONTENT_TYPE: &str = "application/licomesh.secure-mesh.error.v1";
pub const SECURE_MESH_RESPONSE_CRYPTO_STATUS: &str = "typed_result_error_aead_available_command_runtime_binding_available_command_gui_binding_available";

const RESULT_MAGIC: &[u8] = b"LCOSM-RES-v1";
const ERROR_MAGIC: &[u8] = b"LCOSM-ERR-v1";
const MAX_ID_BYTES: usize = 255;
const MAX_CONTENT_TYPE_BYTES: usize = 255;
const MAX_ERROR_CODE_BYTES: usize = 255;
const MAX_ERROR_MESSAGE_BYTES: usize = 16 * 1024;
const MAX_RESPONSE_BODY_BYTES: usize = 1024 * 1024;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SecureMeshResultPayload {
    pub command_id: String,
    pub idempotency_key: String,
    pub output_content_type: String,
    pub completed_at: String,
    pub output: Vec<u8>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SecureMeshErrorPayload {
    pub command_id: String,
    pub idempotency_key: String,
    pub error_code: String,
    pub retryable: bool,
    pub occurred_at: String,
    pub error_detail: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EncryptedSecureMeshResultPayload {
    pub sealed: SealedSecureMeshPayload,
    pub ciphertext_hash: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EncryptedSecureMeshErrorPayload {
    pub sealed: SealedSecureMeshPayload,
    pub ciphertext_hash: String,
}

pub fn seal_command_result(
    key: &ContentKey,
    context: &SecureMeshContentContext,
    result: &SecureMeshResultPayload,
) -> Result<EncryptedSecureMeshResultPayload> {
    validate_result(result)?;
    let sealed = seal_payload(
        key,
        context,
        &SecureMeshPlaintext::new(SecureMeshPayloadKind::ResultPayload, encode_result(result)?)
            .with_content_type(SECURE_MESH_RESULT_CONTENT_TYPE),
    )?;
    Ok(EncryptedSecureMeshResultPayload {
        ciphertext_hash: ciphertext_hash(&sealed)?,
        sealed,
    })
}

pub fn open_command_result(
    key: &ContentKey,
    context: &SecureMeshContentContext,
    encrypted: &EncryptedSecureMeshResultPayload,
) -> Result<SecureMeshResultPayload> {
    ensure!(
        ciphertext_hash(&encrypted.sealed)? == encrypted.ciphertext_hash,
        "secure mesh result ciphertext hash mismatch"
    );
    let opened = open_payload(
        key,
        context,
        &encrypted.sealed,
        SecureMeshPayloadKind::ResultPayload,
    )?;
    ensure!(
        opened.content_type.as_deref() == Some(SECURE_MESH_RESULT_CONTENT_TYPE),
        "secure mesh result content type mismatch"
    );
    decode_result(&opened.body)
}

pub fn seal_command_error(
    key: &ContentKey,
    context: &SecureMeshContentContext,
    error: &SecureMeshErrorPayload,
) -> Result<EncryptedSecureMeshErrorPayload> {
    validate_error(error)?;
    let sealed = seal_payload(
        key,
        context,
        &SecureMeshPlaintext::new(SecureMeshPayloadKind::Error, encode_error(error)?)
            .with_content_type(SECURE_MESH_ERROR_CONTENT_TYPE),
    )?;
    Ok(EncryptedSecureMeshErrorPayload {
        ciphertext_hash: ciphertext_hash(&sealed)?,
        sealed,
    })
}

pub fn open_command_error(
    key: &ContentKey,
    context: &SecureMeshContentContext,
    encrypted: &EncryptedSecureMeshErrorPayload,
) -> Result<SecureMeshErrorPayload> {
    ensure!(
        ciphertext_hash(&encrypted.sealed)? == encrypted.ciphertext_hash,
        "secure mesh error ciphertext hash mismatch"
    );
    let opened = open_payload(
        key,
        context,
        &encrypted.sealed,
        SecureMeshPayloadKind::Error,
    )?;
    ensure!(
        opened.content_type.as_deref() == Some(SECURE_MESH_ERROR_CONTENT_TYPE),
        "secure mesh error content type mismatch"
    );
    decode_error(&opened.body)
}

fn validate_result(result: &SecureMeshResultPayload) -> Result<()> {
    validate_text("command_id", &result.command_id, MAX_ID_BYTES)?;
    validate_text("idempotency_key", &result.idempotency_key, MAX_ID_BYTES)?;
    validate_text(
        "output_content_type",
        &result.output_content_type,
        MAX_CONTENT_TYPE_BYTES,
    )?;
    validate_text("completed_at", &result.completed_at, MAX_ID_BYTES)?;
    ensure!(
        result.output.len() <= MAX_RESPONSE_BODY_BYTES,
        "secure mesh result output is too large"
    );
    Ok(())
}

fn validate_error(error: &SecureMeshErrorPayload) -> Result<()> {
    validate_text("command_id", &error.command_id, MAX_ID_BYTES)?;
    validate_text("idempotency_key", &error.idempotency_key, MAX_ID_BYTES)?;
    validate_text("error_code", &error.error_code, MAX_ERROR_CODE_BYTES)?;
    validate_text("occurred_at", &error.occurred_at, MAX_ID_BYTES)?;
    validate_text("error_detail", &error.error_detail, MAX_ERROR_MESSAGE_BYTES)?;
    Ok(())
}

fn validate_text(label: &str, value: &str, max: usize) -> Result<()> {
    ensure!(
        !value.trim().is_empty(),
        "secure mesh response {label} is required"
    );
    ensure!(
        value.len() <= max,
        "secure mesh response {label} is too large"
    );
    Ok(())
}

fn encode_result(result: &SecureMeshResultPayload) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    out.extend_from_slice(RESULT_MAGIC);
    append_len_prefixed_bytes(&mut out, SECURE_MESH_RESULT_PROTOCOL_VERSION.as_bytes())?;
    append_len_prefixed_bytes(&mut out, result.command_id.as_bytes())?;
    append_len_prefixed_bytes(&mut out, result.idempotency_key.as_bytes())?;
    append_len_prefixed_bytes(&mut out, result.output_content_type.as_bytes())?;
    append_len_prefixed_bytes(&mut out, result.completed_at.as_bytes())?;
    append_len_prefixed_bytes(&mut out, &result.output)?;
    Ok(out)
}

fn decode_result(bytes: &[u8]) -> Result<SecureMeshResultPayload> {
    let mut reader = SliceReader::new(bytes);
    reader.expect_bytes(RESULT_MAGIC)?;
    let protocol_version = read_string(&mut reader, "protocol_version")?;
    ensure!(
        protocol_version == SECURE_MESH_RESULT_PROTOCOL_VERSION,
        "secure mesh result protocol version is unsupported"
    );
    let result = SecureMeshResultPayload {
        command_id: read_string(&mut reader, "command_id")?,
        idempotency_key: read_string(&mut reader, "idempotency_key")?,
        output_content_type: read_string(&mut reader, "output_content_type")?,
        completed_at: read_string(&mut reader, "completed_at")?,
        output: reader.read_len_prefixed_bytes()?.to_vec(),
    };
    ensure!(reader.is_empty(), "secure mesh result has trailing bytes");
    validate_result(&result)?;
    Ok(result)
}

fn encode_error(error: &SecureMeshErrorPayload) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    out.extend_from_slice(ERROR_MAGIC);
    append_len_prefixed_bytes(&mut out, SECURE_MESH_RESULT_PROTOCOL_VERSION.as_bytes())?;
    append_len_prefixed_bytes(&mut out, error.command_id.as_bytes())?;
    append_len_prefixed_bytes(&mut out, error.idempotency_key.as_bytes())?;
    append_len_prefixed_bytes(&mut out, error.error_code.as_bytes())?;
    out.push(u8::from(error.retryable));
    append_len_prefixed_bytes(&mut out, error.occurred_at.as_bytes())?;
    append_len_prefixed_bytes(&mut out, error.error_detail.as_bytes())?;
    Ok(out)
}

fn decode_error(bytes: &[u8]) -> Result<SecureMeshErrorPayload> {
    let mut reader = SliceReader::new(bytes);
    reader.expect_bytes(ERROR_MAGIC)?;
    let protocol_version = read_string(&mut reader, "protocol_version")?;
    ensure!(
        protocol_version == SECURE_MESH_RESULT_PROTOCOL_VERSION,
        "secure mesh error protocol version is unsupported"
    );
    let retryable_byte;
    let error = SecureMeshErrorPayload {
        command_id: read_string(&mut reader, "command_id")?,
        idempotency_key: read_string(&mut reader, "idempotency_key")?,
        error_code: read_string(&mut reader, "error_code")?,
        retryable: {
            retryable_byte = reader.read_u8()?;
            match retryable_byte {
                0 => false,
                1 => true,
                _ => return Err(anyhow!("secure mesh error retryable flag is invalid")),
            }
        },
        occurred_at: read_string(&mut reader, "occurred_at")?,
        error_detail: read_string(&mut reader, "error_detail")?,
    };
    ensure!(reader.is_empty(), "secure mesh error has trailing bytes");
    validate_error(&error)?;
    Ok(error)
}

fn ciphertext_hash(sealed: &SealedSecureMeshPayload) -> Result<String> {
    let ciphertext = general_purpose::URL_SAFE_NO_PAD
        .decode(&sealed.ciphertext)
        .context("secure mesh response ciphertext is not base64url")?;
    Ok(hash_bytes(&ciphertext))
}

fn hash_bytes(bytes: &[u8]) -> String {
    general_purpose::URL_SAFE_NO_PAD.encode(Sha256::digest(bytes))
}

fn append_len_prefixed_bytes(out: &mut Vec<u8>, value: &[u8]) -> Result<()> {
    ensure!(
        value.len() <= u32::MAX as usize,
        "secure mesh response field is too large"
    );
    out.extend_from_slice(&(value.len() as u32).to_be_bytes());
    out.extend_from_slice(value);
    Ok(())
}

fn read_string(reader: &mut SliceReader<'_>, label: &str) -> Result<String> {
    let bytes = reader.read_len_prefixed_bytes()?;
    String::from_utf8(bytes.to_vec())
        .map_err(|_| anyhow!("secure mesh response {label} is not utf-8"))
}

struct SliceReader<'a> {
    bytes: &'a [u8],
    offset: usize,
}

impl<'a> SliceReader<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, offset: 0 }
    }

    fn expect_bytes(&mut self, expected: &[u8]) -> Result<()> {
        let actual = self.read_exact(expected.len())?;
        ensure!(
            actual == expected,
            "secure mesh response magic header mismatch"
        );
        Ok(())
    }

    fn read_u8(&mut self) -> Result<u8> {
        let bytes = self.read_exact(1)?;
        Ok(bytes[0])
    }

    fn read_len_prefixed_bytes(&mut self) -> Result<&'a [u8]> {
        let len = u32::from_be_bytes(self.read_exact(4)?.try_into().expect("u32 length")) as usize;
        self.read_exact(len)
    }

    fn read_exact(&mut self, len: usize) -> Result<&'a [u8]> {
        let end = self
            .offset
            .checked_add(len)
            .ok_or_else(|| anyhow!("secure mesh response length overflow"))?;
        ensure!(
            end <= self.bytes.len(),
            "secure mesh response ended unexpectedly"
        );
        let out = &self.bytes[self.offset..end];
        self.offset = end;
        Ok(out)
    }

    fn is_empty(&self) -> bool {
        self.offset == self.bytes.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn secure_mesh_result_round_trips_without_outer_output_leak() {
        let key = ContentKey::from_bytes([7u8; 32]);
        let context = context_fixture("env-result", "msg-result");
        let result = SecureMeshResultPayload {
            command_id: "cmd-a".to_string(),
            idempotency_key: "idem-a".to_string(),
            output_content_type: "application/json".to_string(),
            completed_at: "2026-01-01T00:02:00Z".to_string(),
            output: serde_json::to_vec(&serde_json::json!({
                "ok": true,
                "secret": (["result", "canary"].join("-"))
            }))
            .unwrap(),
        };
        let encrypted = seal_command_result(&key, &context, &result).unwrap();
        assert!(!encrypted.sealed.ciphertext.contains("result-canary"));
        assert!(!encrypted.sealed.encrypted_header.contains("result-canary"));
        assert_eq!(
            open_command_result(&key, &context, &encrypted).unwrap(),
            result
        );
    }

    #[test]
    fn secure_mesh_error_round_trips_without_outer_detail_leak() {
        let key = ContentKey::from_bytes([8u8; 32]);
        let context = context_fixture("env-error", "msg-error");
        let error = SecureMeshErrorPayload {
            command_id: "cmd-a".to_string(),
            idempotency_key: "idem-a".to_string(),
            error_code: "target_agent_failed".to_string(),
            retryable: true,
            occurred_at: "2026-01-01T00:03:00Z".to_string(),
            error_detail: "private failure detail".to_string(),
        };
        let encrypted = seal_command_error(&key, &context, &error).unwrap();
        assert!(!encrypted.sealed.ciphertext.contains("private failure"));
        assert!(
            !encrypted
                .sealed
                .encrypted_header
                .contains("private failure")
        );
        assert_eq!(
            open_command_error(&key, &context, &encrypted).unwrap(),
            error
        );
    }

    #[test]
    fn secure_mesh_result_rejects_wrong_aad_context() {
        let key = ContentKey::from_bytes([9u8; 32]);
        let context = context_fixture("env-result", "msg-result");
        let result = SecureMeshResultPayload {
            command_id: "cmd-a".to_string(),
            idempotency_key: "idem-a".to_string(),
            output_content_type: "text/plain".to_string(),
            completed_at: "2026-01-01T00:02:00Z".to_string(),
            output: b"done".to_vec(),
        };
        let encrypted = seal_command_result(&key, &context, &result).unwrap();
        let wrong_context = context_fixture("env-other", "msg-result");
        let error = open_command_result(&key, &wrong_context, &encrypted).unwrap_err();
        assert!(error.to_string().contains("AAD hash mismatch"));
    }

    #[test]
    fn secure_mesh_error_rejects_corrupted_ciphertext_hash() {
        let key = ContentKey::from_bytes([10u8; 32]);
        let context = context_fixture("env-error", "msg-error");
        let error = SecureMeshErrorPayload {
            command_id: "cmd-a".to_string(),
            idempotency_key: "idem-a".to_string(),
            error_code: "target_agent_failed".to_string(),
            retryable: false,
            occurred_at: "2026-01-01T00:03:00Z".to_string(),
            error_detail: "private failure detail".to_string(),
        };
        let mut encrypted = seal_command_error(&key, &context, &error).unwrap();
        encrypted.ciphertext_hash = hash_bytes(b"wrong");
        let open_error = open_command_error(&key, &context, &encrypted).unwrap_err();
        assert!(open_error.to_string().contains("ciphertext hash mismatch"));
    }

    fn context_fixture(
        envelope_id: impl Into<String>,
        message_id: impl Into<String>,
    ) -> SecureMeshContentContext {
        SecureMeshContentContext::new(
            envelope_id,
            message_id,
            "mailbox-a",
            "pc-b",
            "pc-a",
            "session-response",
            "2026-01-01T00:01:00Z",
            "2026-01-01T00:11:00Z",
        )
    }

    #[test]
    fn secure_mesh_response_protocol_constant_is_bound() {
        assert_eq!(
            crate::core::secure_mesh::SECURE_MESH_PROTOCOL_VERSION,
            "licomesh.secure-mesh.v1"
        );
        assert_eq!(
            SECURE_MESH_RESULT_PROTOCOL_VERSION,
            "licomesh.secure-mesh.result.v1"
        );
    }
}
