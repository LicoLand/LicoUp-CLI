use super::identity::DeviceTrustPublicIdentity;
use super::model::{DeviceTrustRecord, DeviceTrustState};
use super::signature::verify_device_trust_record;
use anyhow::{Result, anyhow, ensure};

/// Protected payload classes gated by Decision D5 verify-before-send.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProtectedSendPayloadKind {
    Command,
    Result,
    File,
    Lifecycle,
    Group,
    Acp,
    Prekey,
}

impl ProtectedSendPayloadKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Command => "command",
            Self::Result => "result",
            Self::File => "file",
            Self::Lifecycle => "lifecycle",
            Self::Group => "group",
            Self::Acp => "acp",
            Self::Prekey => "prekey",
        }
    }

    pub fn all() -> [Self; 7] {
        [
            Self::Command,
            Self::Result,
            Self::File,
            Self::Lifecycle,
            Self::Group,
            Self::Acp,
            Self::Prekey,
        ]
    }
}

/// Opaque token proving a peer passed verify-before-send for one protected payload kind.
/// Constructible only through [`authorize_protected_send`].
#[derive(Debug)]
pub struct ProtectedSendAuthorization {
    payload_kind: ProtectedSendPayloadKind,
    peer_endpoint_id: String,
}

impl ProtectedSendAuthorization {
    pub fn payload_kind(&self) -> ProtectedSendPayloadKind {
        self.payload_kind
    }

    pub fn peer_endpoint_id(&self) -> &str {
        &self.peer_endpoint_id
    }
}

/// Single enforcement point for Decision D5 verify-before-send.
/// Observation, relay-supplied trust labels, and cross-signing alone never authorize.
pub fn authorize_protected_send(
    peer_endpoint_id: &str,
    trust_state: &DeviceTrustState,
    payload_kind: ProtectedSendPayloadKind,
) -> Result<ProtectedSendAuthorization> {
    ensure!(
        !peer_endpoint_id.trim().is_empty(),
        "secure mesh protected send requires a peer endpoint id"
    );
    match trust_state {
        DeviceTrustState::Verified => Ok(ProtectedSendAuthorization {
            payload_kind,
            peer_endpoint_id: peer_endpoint_id.trim().to_string(),
        }),
        DeviceTrustState::Unverified => Err(anyhow!(
            "secure mesh protected {} send blocked: verification_required",
            payload_kind.as_str()
        )),
        DeviceTrustState::KeyChanged => Err(anyhow!(
            "secure mesh protected {} send blocked: identity_key_changed",
            payload_kind.as_str()
        )),
        DeviceTrustState::Revoked => Err(anyhow!(
            "secure mesh protected {} send blocked: device_revoked",
            payload_kind.as_str()
        )),
        DeviceTrustState::CrossSigned => Err(anyhow!(
            "secure mesh protected {} send blocked: cross_signature_requires_durable_epoch_validation",
            payload_kind.as_str()
        )),
    }
}

pub fn authorize_protected_send_from_trust_record(
    signer_identity: &DeviceTrustPublicIdentity,
    peer_identity: &DeviceTrustPublicIdentity,
    record: &DeviceTrustRecord,
    now_epoch_seconds: u64,
    payload_kind: ProtectedSendPayloadKind,
) -> Result<ProtectedSendAuthorization> {
    let trust_state =
        verify_device_trust_record(signer_identity, peer_identity, record, now_epoch_seconds)?;
    authorize_protected_send(&peer_identity.endpoint_id, &trust_state, payload_kind)
}
