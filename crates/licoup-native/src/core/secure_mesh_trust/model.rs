#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DeviceTrustState {
    Unverified,
    Verified,
    CrossSigned,
    KeyChanged,
    Revoked,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DeviceCrossSignature {
    pub protocol_version: String,
    pub signer_endpoint_id: String,
    pub subject_endpoint_id: String,
    pub subject_fingerprint: String,
    pub roster_epoch: u64,
    pub signature: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DeviceTrustRecord {
    pub protocol_version: String,
    pub signer_endpoint_id: String,
    pub peer_endpoint_id: String,
    pub peer_fingerprint: String,
    pub trust_state: DeviceTrustState,
    pub roster_epoch: u64,
    pub verification_method: String,
    pub issued_at_epoch_seconds: u64,
    pub expires_at_epoch_seconds: u64,
    pub signature: String,
}
