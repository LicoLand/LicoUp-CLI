use std::fmt;

use anyhow::{Result, bail};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SecureMeshPayloadKind {
    Command,
    ResultPayload,
    Error,
    FileChunk,
    FileManifest,
    ServiceAction,
    TypingIndicator,
    ReadReceipt,
}

impl SecureMeshPayloadKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Command => "command",
            Self::ResultPayload => "result",
            Self::Error => "error",
            Self::FileChunk => "file_chunk",
            Self::FileManifest => "file_manifest",
            Self::ServiceAction => "service_action",
            Self::TypingIndicator => "typing_indicator",
            Self::ReadReceipt => "read_receipt",
        }
    }

    pub(super) fn tag(self) -> u8 {
        match self {
            Self::Command => 1,
            Self::ResultPayload => 2,
            Self::Error => 3,
            Self::FileChunk => 4,
            Self::FileManifest => 5,
            Self::ServiceAction => 6,
            Self::TypingIndicator => 7,
            Self::ReadReceipt => 8,
        }
    }

    pub(super) fn from_tag(tag: u8) -> Result<Self> {
        match tag {
            1 => Ok(Self::Command),
            2 => Ok(Self::ResultPayload),
            3 => Ok(Self::Error),
            4 => Ok(Self::FileChunk),
            5 => Ok(Self::FileManifest),
            6 => Ok(Self::ServiceAction),
            7 => Ok(Self::TypingIndicator),
            8 => Ok(Self::ReadReceipt),
            _ => bail!("secure mesh payload kind tag is unsupported"),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SecureMeshContentContext {
    pub envelope_id: String,
    pub message_id: String,
    pub opaque_mailbox_id: String,
    pub sender_endpoint_id: String,
    pub recipient_endpoint_id: String,
    pub session_id: String,
    pub created_at: String,
    pub expires_at: String,
}

impl SecureMeshContentContext {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        envelope_id: impl Into<String>,
        message_id: impl Into<String>,
        opaque_mailbox_id: impl Into<String>,
        sender_endpoint_id: impl Into<String>,
        recipient_endpoint_id: impl Into<String>,
        session_id: impl Into<String>,
        created_at: impl Into<String>,
        expires_at: impl Into<String>,
    ) -> Self {
        Self {
            envelope_id: envelope_id.into(),
            message_id: message_id.into(),
            opaque_mailbox_id: opaque_mailbox_id.into(),
            sender_endpoint_id: sender_endpoint_id.into(),
            recipient_endpoint_id: recipient_endpoint_id.into(),
            session_id: session_id.into(),
            created_at: created_at.into(),
            expires_at: expires_at.into(),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SecureMeshPlaintext {
    pub kind: SecureMeshPayloadKind,
    pub body: Vec<u8>,
    pub content_type: Option<String>,
}

impl SecureMeshPlaintext {
    pub fn new(kind: SecureMeshPayloadKind, body: impl Into<Vec<u8>>) -> Self {
        Self {
            kind,
            body: body.into(),
            content_type: None,
        }
    }

    pub fn with_content_type(mut self, content_type: impl Into<String>) -> Self {
        self.content_type = Some(content_type.into());
        self
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OpenedSecureMeshPayload {
    pub kind: SecureMeshPayloadKind,
    pub body: Vec<u8>,
    pub content_type: Option<String>,
    pub created_at: String,
    pub expires_at: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SealedSecureMeshPayload {
    pub protocol_version: String,
    pub cipher_suite: String,
    pub encrypted_header: String,
    pub ciphertext: String,
    pub ciphertext_size: usize,
}

#[derive(Clone, Eq, PartialEq)]
pub(crate) struct SealedSecureMeshPrivateContextPayload {
    pub(super) encrypted_header: String,
    pub(super) ciphertext: String,
    pub(super) ciphertext_size: usize,
}

impl SealedSecureMeshPrivateContextPayload {
    pub(crate) fn encrypted_header(&self) -> &str {
        &self.encrypted_header
    }

    pub(crate) fn ciphertext(&self) -> &str {
        &self.ciphertext
    }

    pub(crate) fn ciphertext_size(&self) -> usize {
        self.ciphertext_size
    }
}

impl fmt::Debug for SealedSecureMeshPrivateContextPayload {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SealedSecureMeshPrivateContextPayload")
            .field("encrypted_header", &"[redacted]")
            .field("ciphertext", &"[redacted]")
            .field("ciphertext_size", &self.ciphertext_size)
            .finish()
    }
}

pub(crate) struct OpenedSecureMeshPrivateContextPayload {
    pub(super) context: SecureMeshContentContext,
    pub(super) payload: OpenedSecureMeshPayload,
}

impl OpenedSecureMeshPrivateContextPayload {
    pub(crate) fn into_parts(self) -> (SecureMeshContentContext, OpenedSecureMeshPayload) {
        (self.context, self.payload)
    }
}
