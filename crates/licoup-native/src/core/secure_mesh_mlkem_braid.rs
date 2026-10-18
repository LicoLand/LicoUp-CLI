//! Signal ML-KEM Braid Revision 1 SCKA for the secure-mesh client.
//!
//! The relay transports only protocol messages and never receives state or
//! output keys. Protocol internals live in focused, bounded modules.

mod authenticator;
mod constants;
mod encapsulation_kdf;
mod erasure_decoder;
mod erasure_encoder;
mod erasure_gf;
mod output;
mod persistence;
mod protocol_state;
mod receive_transition;
mod secret;
mod send_transition;
mod session;
mod transition;
mod validation;
mod wire;

pub use constants::{
    ML_KEM_BRAID_CHUNK_BYTES, ML_KEM_BRAID_CT1_BYTES, ML_KEM_BRAID_CT2_BYTES,
    ML_KEM_BRAID_EK_BYTES, ML_KEM_BRAID_HEADER_BYTES, ML_KEM_BRAID_MAC_BYTES,
    ML_KEM_BRAID_TRANSITION_COUNT,
};
pub use wire::{MlKemBraidChunk, MlKemBraidMessage, MlKemBraidMessageType};

pub(crate) use output::{MlKemBraidOutputKey, MlKemBraidReceive, MlKemBraidSend};
pub(crate) use session::MlKemBraidSession;

#[cfg(test)]
mod tests;
