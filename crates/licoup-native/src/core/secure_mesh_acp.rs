use anyhow::{Result, anyhow, bail, ensure};
use sha2::{Digest, Sha256};

use crate::core::secure_mesh::SECURE_MESH_PROTOCOL_VERSION;
use crate::core::secure_mesh_crypto::{
    OpenedSecureMeshPayload, SecureMeshContentContext, SecureMeshPayloadKind, SecureMeshPlaintext,
};
use crate::core::secure_mesh_pairwise::SecureMeshPairwiseSession;
use licoup_protocol_bindings::licoarc_relay::LicoArcRelayEnvelope;

pub const SECURE_MESH_ACP_ENVELOPE_PROTOCOL: &str = "licomesh.secure-mesh.acp-envelope.v1";
pub const SECURE_MESH_ACP_STATUS: &str = "acp_protected_envelope_aad_available_plaintext_protected_payload_relay_blocked_independent_review_pending_pqxdh_mlkem1024_triple_ratchet";

const ACP_AAD_MAGIC: &[u8] = b"LCOSM-ACP-AAD-v1";
const MAX_FIELD_BYTES: usize = 4096;
const MAX_DIGEST_HEX_BYTES: usize = 128;

/// ACP agent-conversation payload classes that must be sealed before relay.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SecureMeshAcpPayloadClass {
    Prompt,
    Update,
    Reasoning,
    ToolArguments,
    ToolResult,
    FilesystemContent,
    TerminalContent,
    PermissionPayload,
    ApprovalDecision,
    Artifact,
    ArchiveLayer,
    Receipt,
}

impl SecureMeshAcpPayloadClass {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Prompt => "prompt",
            Self::Update => "update",
            Self::Reasoning => "reasoning",
            Self::ToolArguments => "tool_arguments",
            Self::ToolResult => "tool_result",
            Self::FilesystemContent => "filesystem_content",
            Self::TerminalContent => "terminal_content",
            Self::PermissionPayload => "permission_payload",
            Self::ApprovalDecision => "approval_decision",
            Self::Artifact => "artifact",
            Self::ArchiveLayer => "archive_layer",
            Self::Receipt => "receipt",
        }
    }

    pub fn is_protected(self) -> bool {
        !matches!(self, Self::Receipt)
    }

    pub fn content_type(self) -> &'static str {
        match self {
            Self::Prompt => "application/licomesh.secure-mesh.acp.prompt.v1",
            Self::Update => "application/licomesh.secure-mesh.acp.update.v1",
            Self::Reasoning => "application/licomesh.secure-mesh.acp.reasoning.v1",
            Self::ToolArguments => "application/licomesh.secure-mesh.acp.tool-arguments.v1",
            Self::ToolResult => "application/licomesh.secure-mesh.acp.tool-result.v1",
            Self::FilesystemContent => "application/licomesh.secure-mesh.acp.filesystem.v1",
            Self::TerminalContent => "application/licomesh.secure-mesh.acp.terminal.v1",
            Self::PermissionPayload => "application/licomesh.secure-mesh.acp.permission.v1",
            Self::ApprovalDecision => "application/licomesh.secure-mesh.acp.approval.v1",
            Self::Artifact => "application/licomesh.secure-mesh.acp.artifact.v1",
            Self::ArchiveLayer => "application/licomesh.secure-mesh.acp.archive.v1",
            Self::Receipt => "application/licomesh.secure-mesh.acp.receipt.v1",
        }
    }

    fn from_str(value: &str) -> Result<Self> {
        match value {
            "prompt" => Ok(Self::Prompt),
            "update" => Ok(Self::Update),
            "reasoning" => Ok(Self::Reasoning),
            "tool_arguments" => Ok(Self::ToolArguments),
            "tool_result" => Ok(Self::ToolResult),
            "filesystem_content" => Ok(Self::FilesystemContent),
            "terminal_content" => Ok(Self::TerminalContent),
            "permission_payload" => Ok(Self::PermissionPayload),
            "approval_decision" => Ok(Self::ApprovalDecision),
            "artifact" => Ok(Self::Artifact),
            "archive_layer" => Ok(Self::ArchiveLayer),
            "receipt" => Ok(Self::Receipt),
            _ => bail!("secure mesh ACP payload class is unsupported"),
        }
    }
}

/// Cryptographic AAD binding for ACP protected envelopes (REQ-E2EE-014).
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SecureMeshAcpEnvelopeBinding {
    pub payload_class: SecureMeshAcpPayloadClass,
    pub source_endpoint_id: String,
    pub target_endpoint_id: String,
    pub acp_session_id: String,
    pub relay_session_id: String,
    pub relay_turn_id: String,
    pub message_sequence: u64,
    pub operation_id: Option<String>,
    pub mcp_child_operation_id: Option<String>,
    pub tool_call_id: Option<String>,
    pub permission_request_id: Option<String>,
    pub artifact_id: Option<String>,
    pub terminal_request_id: Option<String>,
    pub idempotency_key: Option<String>,
    pub retry_counter: Option<u32>,
    pub expires_at: Option<String>,
    pub policy_revision: Option<String>,
    pub grant_id: Option<String>,
    pub parent_transcript_digest: Option<String>,
    pub previous_envelope_digest: Option<String>,
}

impl SecureMeshAcpEnvelopeBinding {
    pub fn new(
        payload_class: SecureMeshAcpPayloadClass,
        source_endpoint_id: impl Into<String>,
        target_endpoint_id: impl Into<String>,
        acp_session_id: impl Into<String>,
        relay_session_id: impl Into<String>,
        relay_turn_id: impl Into<String>,
        message_sequence: u64,
    ) -> Result<Self> {
        let binding = Self {
            payload_class,
            source_endpoint_id: source_endpoint_id.into(),
            target_endpoint_id: target_endpoint_id.into(),
            acp_session_id: acp_session_id.into(),
            relay_session_id: relay_session_id.into(),
            relay_turn_id: relay_turn_id.into(),
            message_sequence,
            operation_id: None,
            mcp_child_operation_id: None,
            tool_call_id: None,
            permission_request_id: None,
            artifact_id: None,
            terminal_request_id: None,
            idempotency_key: None,
            retry_counter: None,
            expires_at: None,
            policy_revision: None,
            grant_id: None,
            parent_transcript_digest: None,
            previous_envelope_digest: None,
        };
        binding.validate()?;
        Ok(binding)
    }

    pub fn validate(&self) -> Result<()> {
        require_text("source_endpoint_id", &self.source_endpoint_id)?;
        require_text("target_endpoint_id", &self.target_endpoint_id)?;
        require_text("acp_session_id", &self.acp_session_id)?;
        require_text("relay_session_id", &self.relay_session_id)?;
        require_text("relay_turn_id", &self.relay_turn_id)?;
        optional_text("operation_id", self.operation_id.as_deref())?;
        optional_text(
            "mcp_child_operation_id",
            self.mcp_child_operation_id.as_deref(),
        )?;
        optional_text("tool_call_id", self.tool_call_id.as_deref())?;
        optional_text(
            "permission_request_id",
            self.permission_request_id.as_deref(),
        )?;
        optional_text("artifact_id", self.artifact_id.as_deref())?;
        optional_text("terminal_request_id", self.terminal_request_id.as_deref())?;
        optional_text("idempotency_key", self.idempotency_key.as_deref())?;
        optional_text("expires_at", self.expires_at.as_deref())?;
        optional_text("policy_revision", self.policy_revision.as_deref())?;
        optional_text("grant_id", self.grant_id.as_deref())?;
        optional_digest(
            "parent_transcript_digest",
            self.parent_transcript_digest.as_deref(),
        )?;
        optional_digest(
            "previous_envelope_digest",
            self.previous_envelope_digest.as_deref(),
        )?;
        Ok(())
    }
}

/// Encode the ACP envelope AAD binding used as additional AEAD associated data.
pub fn encode_acp_envelope_aad(binding: &SecureMeshAcpEnvelopeBinding) -> Result<Vec<u8>> {
    binding.validate()?;
    let mut out = Vec::new();
    out.extend_from_slice(ACP_AAD_MAGIC);
    append_len_prefixed_bytes(&mut out, SECURE_MESH_ACP_ENVELOPE_PROTOCOL.as_bytes())?;
    append_len_prefixed_bytes(&mut out, SECURE_MESH_PROTOCOL_VERSION.as_bytes())?;
    append_len_prefixed_bytes(&mut out, binding.payload_class.as_str().as_bytes())?;
    append_len_prefixed_bytes(&mut out, binding.source_endpoint_id.as_bytes())?;
    append_len_prefixed_bytes(&mut out, binding.target_endpoint_id.as_bytes())?;
    append_len_prefixed_bytes(&mut out, binding.acp_session_id.as_bytes())?;
    append_len_prefixed_bytes(&mut out, binding.relay_session_id.as_bytes())?;
    append_len_prefixed_bytes(&mut out, binding.relay_turn_id.as_bytes())?;
    out.extend_from_slice(&binding.message_sequence.to_be_bytes());
    append_optional_text(&mut out, binding.operation_id.as_deref())?;
    append_optional_text(&mut out, binding.mcp_child_operation_id.as_deref())?;
    append_optional_text(&mut out, binding.tool_call_id.as_deref())?;
    append_optional_text(&mut out, binding.permission_request_id.as_deref())?;
    append_optional_text(&mut out, binding.artifact_id.as_deref())?;
    append_optional_text(&mut out, binding.terminal_request_id.as_deref())?;
    append_optional_text(&mut out, binding.idempotency_key.as_deref())?;
    match binding.retry_counter {
        Some(value) => {
            out.push(1);
            out.extend_from_slice(&value.to_be_bytes());
        }
        None => out.push(0),
    }
    append_optional_text(&mut out, binding.expires_at.as_deref())?;
    append_optional_text(&mut out, binding.policy_revision.as_deref())?;
    append_optional_text(&mut out, binding.grant_id.as_deref())?;
    append_optional_text(&mut out, binding.parent_transcript_digest.as_deref())?;
    append_optional_text(&mut out, binding.previous_envelope_digest.as_deref())?;
    Ok(out)
}

pub fn acp_envelope_aad_digest(binding: &SecureMeshAcpEnvelopeBinding) -> Result<String> {
    let encoded = encode_acp_envelope_aad(binding)?;
    Ok(hex_sha256(&encoded))
}

/// Production ACP protected payloads must never be relayed as plaintext.
pub fn reject_plaintext_acp_protected_payload_relay(
    payload_class: SecureMeshAcpPayloadClass,
) -> Result<()> {
    ensure!(
        payload_class.is_protected(),
        "secure mesh ACP receipt class is metadata-only and must not use the protected-payload seal path"
    );
    bail!(
        "secure mesh ACP plaintext protected-payload relay is not a production path for class {}",
        payload_class.as_str()
    );
}

pub fn classify_acp_payload_class(label: &str) -> Result<SecureMeshAcpPayloadClass> {
    SecureMeshAcpPayloadClass::from_str(label)
}

pub fn seal_acp_protected_payload(
    session: &mut SecureMeshPairwiseSession,
    context: &SecureMeshContentContext,
    binding: &SecureMeshAcpEnvelopeBinding,
    body: &[u8],
) -> Result<LicoArcRelayEnvelope> {
    ensure!(
        binding.payload_class.is_protected(),
        "secure mesh ACP receipt class cannot be sealed as a protected payload"
    );
    ensure!(
        context.sender_endpoint_id == binding.source_endpoint_id,
        "secure mesh ACP source endpoint mismatch"
    );
    ensure!(
        context.recipient_endpoint_id == binding.target_endpoint_id,
        "secure mesh ACP target endpoint mismatch"
    );
    let plaintext = SecureMeshPlaintext::new(SecureMeshPayloadKind::ServiceAction, body)
        .with_content_type(binding.payload_class.content_type());
    let acp_aad = encode_acp_envelope_aad(binding)?;
    session.seal_payload_envelope_with_extra_aad(context, &plaintext, &acp_aad)
}

pub fn open_acp_protected_payload(
    session: &mut SecureMeshPairwiseSession,
    context: &SecureMeshContentContext,
    envelope: &LicoArcRelayEnvelope,
    binding: &SecureMeshAcpEnvelopeBinding,
) -> Result<OpenedSecureMeshPayload> {
    ensure!(
        binding.payload_class.is_protected(),
        "secure mesh ACP receipt class cannot be opened as a protected payload"
    );
    ensure!(
        context.sender_endpoint_id == binding.source_endpoint_id,
        "secure mesh ACP source endpoint mismatch"
    );
    ensure!(
        context.recipient_endpoint_id == binding.target_endpoint_id,
        "secure mesh ACP target endpoint mismatch"
    );
    let acp_aad = encode_acp_envelope_aad(binding)?;
    let opened = session.open_payload_envelope_with_extra_aad(
        envelope,
        SecureMeshPayloadKind::ServiceAction,
        &acp_aad,
    )?;
    ensure!(
        opened.content_type.as_deref() == Some(binding.payload_class.content_type()),
        "secure mesh ACP content type mismatch"
    );
    Ok(opened)
}

fn require_text(label: &str, value: &str) -> Result<()> {
    ensure!(!value.trim().is_empty(), "secure mesh ACP {label} is empty");
    ensure!(
        value.len() <= MAX_FIELD_BYTES,
        "secure mesh ACP {label} is too large"
    );
    Ok(())
}

fn optional_text(label: &str, value: Option<&str>) -> Result<()> {
    if let Some(text) = value {
        require_text(label, text)?;
    }
    Ok(())
}

fn optional_digest(label: &str, value: Option<&str>) -> Result<()> {
    if let Some(text) = value {
        require_text(label, text)?;
        ensure!(
            text.len() <= MAX_DIGEST_HEX_BYTES,
            "secure mesh ACP {label} digest is too large"
        );
    }
    Ok(())
}

fn append_optional_text(out: &mut Vec<u8>, value: Option<&str>) -> Result<()> {
    match value {
        Some(text) => {
            out.push(1);
            append_len_prefixed_bytes(out, text.as_bytes())?;
        }
        None => out.push(0),
    }
    Ok(())
}

fn append_len_prefixed_bytes(out: &mut Vec<u8>, value: &[u8]) -> Result<()> {
    let len =
        u32::try_from(value.len()).map_err(|_| anyhow!("secure mesh ACP field is too large"))?;
    out.extend_from_slice(&len.to_be_bytes());
    out.extend_from_slice(value);
    Ok(())
}

fn hex_sha256(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

#[cfg(test)]
mod tests;
