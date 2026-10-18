use anyhow::{Result, ensure};

use super::{
    constants::{
        MAX_ADDITIONAL_AAD_BYTES, MAX_CONTENT_BYTES, MAX_CONTENT_TYPE_BYTES,
        MAX_CONTEXT_FIELD_BYTES,
    },
    model::{SecureMeshContentContext, SecureMeshPlaintext},
};

impl SecureMeshContentContext {
    pub(super) fn validate(&self) -> Result<()> {
        validate_context_field("envelope_id", &self.envelope_id)?;
        validate_context_field("message_id", &self.message_id)?;
        validate_context_field("opaque_mailbox_id", &self.opaque_mailbox_id)?;
        validate_context_field("sender_endpoint_id", &self.sender_endpoint_id)?;
        validate_context_field("recipient_endpoint_id", &self.recipient_endpoint_id)?;
        validate_context_field("session_id", &self.session_id)?;
        validate_context_field("created_at", &self.created_at)?;
        validate_context_field("expires_at", &self.expires_at)?;
        Ok(())
    }
}

fn validate_context_field(label: &str, value: &str) -> Result<()> {
    let trimmed = value.trim();
    ensure!(
        !trimmed.is_empty(),
        "secure mesh context {label} is required"
    );
    ensure!(
        value.len() <= MAX_CONTEXT_FIELD_BYTES,
        "secure mesh context {label} is too large"
    );
    Ok(())
}

pub(super) fn validate_plaintext(plaintext: &SecureMeshPlaintext) -> Result<()> {
    ensure!(
        plaintext.body.len() <= MAX_CONTENT_BYTES,
        "secure mesh payload body is too large"
    );
    if let Some(content_type) = &plaintext.content_type {
        ensure!(
            !content_type.trim().is_empty(),
            "secure mesh payload content type is empty"
        );
        ensure!(
            content_type.len() <= MAX_CONTENT_TYPE_BYTES,
            "secure mesh payload content type is too large"
        );
    }
    Ok(())
}

pub(super) fn validate_additional_aad(additional_aad: &[u8]) -> Result<()> {
    ensure!(
        additional_aad.len() <= MAX_ADDITIONAL_AAD_BYTES,
        "secure mesh additional AAD is too large"
    );
    Ok(())
}
