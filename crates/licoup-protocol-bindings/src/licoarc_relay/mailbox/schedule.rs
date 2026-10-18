//! HKDF-SHA256 mailbox rotation with fixed current/previous acceptance windows.

use std::fmt;

use anyhow::{Result, anyhow, ensure};
use base64::{Engine as _, engine::general_purpose};
use hkdf::Hkdf;
use sha2::Sha256;

use super::super::codec::{append_len_prefixed, decode_exact_base64url};
use super::super::constant_time::constant_time_equal;
use super::super::constants::{
    JSON_SAFE_INTEGER_MAX, LICOARC_RELAY_CONTRACT_VERSION, MAILBOX_HKDF_INFO, MAILBOX_HKDF_SALT,
    MAILBOX_TOKEN_BYTES, SECURE_MESH_MAILBOX_PREVIOUS_WINDOW_COUNT,
    SECURE_MESH_MAILBOX_ROTATION_WINDOW_SECONDS,
};
use super::super::delivery::{SecureMeshDeliverySecret, SecureMeshRelayChannelBinding};
use super::super::envelope::LicoArcRelayEnvelope;
use super::{SecureMeshMailboxDirection, SecureMeshMailboxToken};

pub struct SecureMeshMailboxSchedule {
    delivery_secret: SecureMeshDeliverySecret,
    direction: SecureMeshMailboxDirection,
    channel_binding: SecureMeshRelayChannelBinding,
}

impl SecureMeshMailboxSchedule {
    pub fn new(
        delivery_secret: SecureMeshDeliverySecret,
        direction: SecureMeshMailboxDirection,
        channel_binding: SecureMeshRelayChannelBinding,
    ) -> Self {
        Self {
            delivery_secret,
            direction,
            channel_binding,
        }
    }

    pub fn token_for_unix_seconds(&self, unix_seconds: u64) -> Result<SecureMeshMailboxToken> {
        let epoch = mailbox_epoch(unix_seconds)?;
        self.token_for_epoch(epoch)
    }

    pub fn accepted_tokens_for_unix_seconds(
        &self,
        unix_seconds: u64,
    ) -> Result<Vec<SecureMeshMailboxToken>> {
        let current_epoch = mailbox_epoch(unix_seconds)?;
        let epochs = accepted_mailbox_epochs(current_epoch)?;
        let mut tokens = Vec::with_capacity(epochs.len());
        for epoch in epochs {
            tokens.push(self.token_for_epoch(epoch)?);
        }
        Ok(tokens)
    }

    pub fn validate_token_for_unix_seconds(
        &self,
        observed_token: &str,
        unix_seconds: u64,
    ) -> Result<SecureMeshMailboxToken> {
        let observed =
            decode_exact_base64url("mailbox token", observed_token, MAILBOX_TOKEN_BYTES)?;
        let current_epoch = mailbox_epoch(unix_seconds)?;
        let mut matched_epoch = None;
        for epoch in accepted_mailbox_epochs(current_epoch)? {
            let expected = self.derive_token_bytes(epoch)?;
            if constant_time_equal(&observed, &expected) && matched_epoch.is_none() {
                matched_epoch = Some(epoch);
            }
        }
        ensure!(
            matched_epoch.is_some(),
            "secure mesh mailbox token is outside the accepted rotation window"
        );
        Ok(SecureMeshMailboxToken {
            value: observed_token.to_string(),
            epoch: matched_epoch.unwrap_or(current_epoch),
        })
    }

    pub fn validate_envelope_for_unix_seconds(
        &self,
        envelope: &LicoArcRelayEnvelope,
        unix_seconds: u64,
    ) -> Result<SecureMeshMailboxToken> {
        envelope.validate()?;
        self.validate_token_for_unix_seconds(envelope.mailbox_id(), unix_seconds)
    }

    fn token_for_epoch(&self, epoch: u64) -> Result<SecureMeshMailboxToken> {
        let bytes = self.derive_token_bytes(epoch)?;
        Ok(SecureMeshMailboxToken {
            value: general_purpose::URL_SAFE_NO_PAD.encode(bytes),
            epoch,
        })
    }

    fn derive_token_bytes(&self, epoch: u64) -> Result<[u8; MAILBOX_TOKEN_BYTES]> {
        ensure!(
            epoch <= JSON_SAFE_INTEGER_MAX,
            "secure mesh mailbox epoch is outside the supported integer range"
        );
        let mut info = Vec::with_capacity(256);
        append_len_prefixed(&mut info, MAILBOX_HKDF_INFO)?;
        append_len_prefixed(&mut info, LICOARC_RELAY_CONTRACT_VERSION.as_bytes())?;
        append_len_prefixed(&mut info, self.direction.stable_label())?;
        append_len_prefixed(&mut info, self.channel_binding.as_bytes())?;
        info.extend_from_slice(&SECURE_MESH_MAILBOX_ROTATION_WINDOW_SECONDS.to_be_bytes());
        info.extend_from_slice(&epoch.to_be_bytes());
        let hkdf = Hkdf::<Sha256>::new(Some(MAILBOX_HKDF_SALT), self.delivery_secret.as_bytes());
        let mut output = [0u8; MAILBOX_TOKEN_BYTES];
        hkdf.expand(&info, &mut output)
            .map_err(|_| anyhow!("secure mesh mailbox HKDF expansion failed"))?;
        Ok(output)
    }
}

impl fmt::Debug for SecureMeshMailboxSchedule {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SecureMeshMailboxSchedule")
            .field("delivery_secret", &"[redacted]")
            .field("direction", &self.direction)
            .field("channel_binding", &"[redacted]")
            .finish()
    }
}

fn mailbox_epoch(unix_seconds: u64) -> Result<u64> {
    ensure!(
        unix_seconds <= JSON_SAFE_INTEGER_MAX,
        "secure mesh mailbox time is outside the supported integer range"
    );
    Ok(unix_seconds / SECURE_MESH_MAILBOX_ROTATION_WINDOW_SECONDS)
}

fn accepted_mailbox_epochs(current_epoch: u64) -> Result<Vec<u64>> {
    let mut epochs = Vec::with_capacity(1 + SECURE_MESH_MAILBOX_PREVIOUS_WINDOW_COUNT);
    epochs.push(current_epoch);
    for distance in 1..=SECURE_MESH_MAILBOX_PREVIOUS_WINDOW_COUNT {
        let distance = u64::try_from(distance)
            .map_err(|_| anyhow!("secure mesh mailbox overlap is outside bounds"))?;
        let Some(epoch) = current_epoch.checked_sub(distance) else {
            break;
        };
        epochs.push(epoch);
    }
    Ok(epochs)
}
