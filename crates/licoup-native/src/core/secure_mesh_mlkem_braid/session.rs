use std::mem;

use anyhow::{Result, ensure};
use rand_core::{CryptoRng, OsRng, RngCore};

#[cfg(test)]
use super::protocol_state::MlKemBraidStateName;
use super::{
    MlKemBraidReceive, MlKemBraidSend,
    authenticator::RatchetedAuthenticator,
    constants::{INITIAL_EPOCH, ML_KEM_BRAID_HEADER_BYTES, ML_KEM_BRAID_MAC_BYTES},
    erasure_decoder::ErasureDecoder,
    protocol_state::ProtocolState,
    receive_transition::receive_state,
    send_transition::send_state,
    wire::MlKemBraidMessage,
};

/// Persistable client-only ML-KEM Braid session. The persisted bytes contain
/// plaintext secret state and belong exclusively in the platform secret store.
pub(crate) struct MlKemBraidSession {
    pub(super) state: ProtocolState,
}

impl MlKemBraidSession {
    pub fn new_initiator(shared_secret: &[u8; 32]) -> Result<Self> {
        Ok(Self {
            state: ProtocolState::KeysUnsampled {
                epoch: INITIAL_EPOCH,
                auth: RatchetedAuthenticator::initialize(INITIAL_EPOCH, shared_secret)?,
            },
        })
    }

    pub fn new_responder(shared_secret: &[u8; 32]) -> Result<Self> {
        Ok(Self {
            state: ProtocolState::NoHeaderReceived {
                epoch: INITIAL_EPOCH,
                auth: RatchetedAuthenticator::initialize(INITIAL_EPOCH, shared_secret)?,
                header_decoder: ErasureDecoder::new(
                    ML_KEM_BRAID_HEADER_BYTES + ML_KEM_BRAID_MAC_BYTES,
                )?,
            },
        })
    }

    #[cfg(test)]
    pub fn state_name(&self) -> MlKemBraidStateName {
        self.state.name()
    }

    pub fn epoch(&self) -> u64 {
        self.state.epoch()
    }

    pub fn is_poisoned(&self) -> bool {
        matches!(self.state, ProtocolState::Poisoned { .. })
    }

    pub fn destroy(&mut self) {
        let epoch = self.state.epoch();
        self.state = ProtocolState::Poisoned { epoch };
    }

    pub fn try_clone(&self) -> Self {
        Self {
            state: self.state.clone(),
        }
    }

    pub fn send(&mut self) -> Result<MlKemBraidSend> {
        self.send_with_rng(&mut OsRng)
    }

    pub fn send_with_rng<R>(&mut self, rng: &mut R) -> Result<MlKemBraidSend>
    where
        R: RngCore + CryptoRng,
    {
        let epoch = self.state.epoch();
        ensure!(
            !matches!(self.state, ProtocolState::Poisoned { .. }),
            "ML-KEM Braid session is poisoned"
        );
        let state = mem::replace(&mut self.state, ProtocolState::Poisoned { epoch });
        match send_state(state, rng) {
            Ok((state, output)) => {
                self.state = state;
                Ok(output)
            }
            Err(error) => Err(error),
        }
    }

    pub fn receive(&mut self, message: &MlKemBraidMessage) -> Result<MlKemBraidReceive> {
        let epoch = self.state.epoch();
        if let Err(error) = message.validate() {
            self.state = ProtocolState::Poisoned { epoch };
            return Err(error);
        }
        ensure!(
            !matches!(self.state, ProtocolState::Poisoned { .. }),
            "ML-KEM Braid session is poisoned"
        );
        let state = mem::replace(&mut self.state, ProtocolState::Poisoned { epoch });
        match receive_state(state, message) {
            Ok((state, output)) => {
                self.state = state;
                Ok(output)
            }
            Err(error) => Err(error),
        }
    }
}
