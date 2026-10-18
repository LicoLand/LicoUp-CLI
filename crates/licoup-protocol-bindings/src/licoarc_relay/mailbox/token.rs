//! Canonical fixed-size base64url mailbox identifier.

use std::fmt;

use anyhow::Result;

use super::super::codec::decode_exact_base64url;
use super::super::constants::MAILBOX_TOKEN_BYTES;

#[derive(Clone, Eq, PartialEq)]
pub struct SecureMeshMailboxToken {
    pub(in crate::licoarc_relay) value: String,
    pub(in crate::licoarc_relay) epoch: u64,
}

impl SecureMeshMailboxToken {
    pub fn from_base64url(value: impl Into<String>) -> Result<Self> {
        let value = value.into();
        decode_exact_base64url("mailbox token", &value, MAILBOX_TOKEN_BYTES)?;
        Ok(Self { value, epoch: 0 })
    }

    pub fn as_str(&self) -> &str {
        &self.value
    }

    #[cfg(test)]
    pub(in crate::licoarc_relay) fn epoch(&self) -> u64 {
        self.epoch
    }
}

impl fmt::Debug for SecureMeshMailboxToken {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SecureMeshMailboxToken")
            .field("value", &"[redacted]")
            .field("epoch", &"[redacted]")
            .finish()
    }
}
