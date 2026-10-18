use serde::{Deserialize, Serialize};

use super::{
    authenticator::RatchetedAuthenticator, erasure_decoder::ErasureDecoder,
    erasure_encoder::ErasureEncoder, secret::SecretBytes,
};

#[cfg(test)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum MlKemBraidStateName {
    KeysUnsampled,
    KeysSampled,
    HeaderSent,
    Ct1Received,
    EkSentCt1Received,
    NoHeaderReceived,
    HeaderReceived,
    Ct1Sampled,
    EkReceivedCt1Sampled,
    Ct1Acknowledged,
    Ct2Sampled,
    Poisoned,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "camelCase", deny_unknown_fields)]
pub(super) enum ProtocolState {
    KeysUnsampled {
        epoch: u64,
        auth: RatchetedAuthenticator,
    },
    KeysSampled {
        epoch: u64,
        auth: RatchetedAuthenticator,
        key_seed: SecretBytes,
        ek_vector: Vec<u8>,
        header_encoder: ErasureEncoder,
    },
    HeaderSent {
        epoch: u64,
        auth: RatchetedAuthenticator,
        key_seed: SecretBytes,
        ct1_decoder: ErasureDecoder,
        ek_encoder: ErasureEncoder,
    },
    Ct1Received {
        epoch: u64,
        auth: RatchetedAuthenticator,
        key_seed: SecretBytes,
        ct1: Vec<u8>,
        ek_encoder: ErasureEncoder,
    },
    EkSentCt1Received {
        epoch: u64,
        auth: RatchetedAuthenticator,
        key_seed: SecretBytes,
        ct1: Vec<u8>,
        ct2_decoder: ErasureDecoder,
    },
    NoHeaderReceived {
        epoch: u64,
        auth: RatchetedAuthenticator,
        header_decoder: ErasureDecoder,
    },
    HeaderReceived {
        epoch: u64,
        auth: RatchetedAuthenticator,
        header: Vec<u8>,
        ek_decoder: ErasureDecoder,
    },
    Ct1Sampled {
        epoch: u64,
        auth: RatchetedAuthenticator,
        header: Vec<u8>,
        encaps_state: SecretBytes,
        ct1: Vec<u8>,
        ct1_encoder: ErasureEncoder,
        ek_decoder: ErasureDecoder,
    },
    EkReceivedCt1Sampled {
        epoch: u64,
        auth: RatchetedAuthenticator,
        encaps_state: SecretBytes,
        ct1: Vec<u8>,
        ek_vector: Vec<u8>,
        ct1_encoder: ErasureEncoder,
    },
    Ct1Acknowledged {
        epoch: u64,
        auth: RatchetedAuthenticator,
        header: Vec<u8>,
        encaps_state: SecretBytes,
        ct1: Vec<u8>,
        ek_decoder: ErasureDecoder,
    },
    Ct2Sampled {
        epoch: u64,
        auth: RatchetedAuthenticator,
        ct2_encoder: ErasureEncoder,
    },
    Poisoned {
        epoch: u64,
    },
}

impl ProtocolState {
    pub(super) fn epoch(&self) -> u64 {
        match self {
            Self::KeysUnsampled { epoch, .. }
            | Self::KeysSampled { epoch, .. }
            | Self::HeaderSent { epoch, .. }
            | Self::Ct1Received { epoch, .. }
            | Self::EkSentCt1Received { epoch, .. }
            | Self::NoHeaderReceived { epoch, .. }
            | Self::HeaderReceived { epoch, .. }
            | Self::Ct1Sampled { epoch, .. }
            | Self::EkReceivedCt1Sampled { epoch, .. }
            | Self::Ct1Acknowledged { epoch, .. }
            | Self::Ct2Sampled { epoch, .. }
            | Self::Poisoned { epoch } => *epoch,
        }
    }

    #[cfg(test)]
    pub(super) fn name(&self) -> MlKemBraidStateName {
        match self {
            Self::KeysUnsampled { .. } => MlKemBraidStateName::KeysUnsampled,
            Self::KeysSampled { .. } => MlKemBraidStateName::KeysSampled,
            Self::HeaderSent { .. } => MlKemBraidStateName::HeaderSent,
            Self::Ct1Received { .. } => MlKemBraidStateName::Ct1Received,
            Self::EkSentCt1Received { .. } => MlKemBraidStateName::EkSentCt1Received,
            Self::NoHeaderReceived { .. } => MlKemBraidStateName::NoHeaderReceived,
            Self::HeaderReceived { .. } => MlKemBraidStateName::HeaderReceived,
            Self::Ct1Sampled { .. } => MlKemBraidStateName::Ct1Sampled,
            Self::EkReceivedCt1Sampled { .. } => MlKemBraidStateName::EkReceivedCt1Sampled,
            Self::Ct1Acknowledged { .. } => MlKemBraidStateName::Ct1Acknowledged,
            Self::Ct2Sampled { .. } => MlKemBraidStateName::Ct2Sampled,
            Self::Poisoned { .. } => MlKemBraidStateName::Poisoned,
        }
    }
}
