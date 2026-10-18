//! Strict Lico Arc JSON decoding and canonical carrier validation.

use anyhow::{Context, Result, ensure};
use serde::Deserialize;

use super::carrier::decode_carrier;
use super::constants::MAX_RELAY_ENVELOPE_JSON_BYTES;
use super::envelope::LicoArcRelayEnvelope;

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub(super) struct LicoArcRelayEnvelopeWire {
    pub(in crate::licoarc_relay) contract_version: String,
    pub(in crate::licoarc_relay) envelope_id: String,
    pub(in crate::licoarc_relay) mailbox_id: String,
    pub(in crate::licoarc_relay) ciphertext: String,
    pub(in crate::licoarc_relay) expires_at: String,
}

impl LicoArcRelayEnvelope {
    pub fn from_json(wire: &str) -> Result<Self> {
        ensure!(
            wire.len() <= MAX_RELAY_ENVELOPE_JSON_BYTES,
            "Lico Arc relay envelope JSON is too large"
        );
        let wire_envelope: LicoArcRelayEnvelopeWire =
            serde_json::from_str(wire).context("Lico Arc relay envelope JSON is invalid")?;
        let envelope = Self {
            contract_version: wire_envelope.contract_version,
            envelope_id: wire_envelope.envelope_id,
            mailbox_id: wire_envelope.mailbox_id,
            ciphertext: wire_envelope.ciphertext,
            expires_at: wire_envelope.expires_at,
        };
        envelope.validate()?;
        Ok(envelope)
    }

    pub fn to_json(&self) -> Result<String> {
        self.validate()?;
        serde_json::to_string(self).context("Lico Arc relay envelope serialization failed")
    }

    pub fn validate(&self) -> Result<()> {
        self.validate_contract_fields()?;
        decode_carrier(&self.ciphertext)?;
        Ok(())
    }
}
