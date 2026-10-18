use anyhow::{Result, ensure};
use libcrux_ml_kem::{KEY_GENERATION_SEED_SIZE, mlkem1024::incremental};

use super::{
    constants::{
        ML_KEM_BRAID_CT1_BYTES, ML_KEM_BRAID_CT2_BYTES, ML_KEM_BRAID_EK_BYTES,
        ML_KEM_BRAID_HEADER_BYTES, ML_KEM_BRAID_MAC_BYTES,
    },
    erasure_encoder::ErasureEncoder,
    protocol_state::ProtocolState,
};

pub(super) fn validate_encoder(encoder: &mut ErasureEncoder, expected_bytes: usize) -> Result<()> {
    encoder.rebuild_cache()?;
    ensure!(
        encoder.message_bytes() == expected_bytes,
        "persisted ML-KEM Braid encoder size is invalid"
    );
    Ok(())
}

pub(super) fn validate_restored_state(state: &mut ProtocolState) -> Result<()> {
    ensure!(
        state.epoch() != 0,
        "persisted ML-KEM Braid epoch is invalid"
    );
    match state {
        ProtocolState::KeysUnsampled { auth, .. } => auth.validate(),
        ProtocolState::KeysSampled {
            auth,
            key_seed,
            ek_vector,
            header_encoder,
            ..
        } => {
            auth.validate()?;
            key_seed.ensure_len(KEY_GENERATION_SEED_SIZE)?;
            ensure!(
                ek_vector.len() == ML_KEM_BRAID_EK_BYTES,
                "persisted ML-KEM Braid ek length is invalid"
            );
            validate_encoder(
                header_encoder,
                ML_KEM_BRAID_HEADER_BYTES + ML_KEM_BRAID_MAC_BYTES,
            )
        }
        ProtocolState::HeaderSent {
            auth,
            key_seed,
            ct1_decoder,
            ek_encoder,
            ..
        } => {
            auth.validate()?;
            key_seed.ensure_len(KEY_GENERATION_SEED_SIZE)?;
            ct1_decoder.validate_active(ML_KEM_BRAID_CT1_BYTES)?;
            validate_encoder(ek_encoder, ML_KEM_BRAID_EK_BYTES)
        }
        ProtocolState::Ct1Received {
            auth,
            key_seed,
            ct1,
            ek_encoder,
            ..
        } => {
            auth.validate()?;
            key_seed.ensure_len(KEY_GENERATION_SEED_SIZE)?;
            ensure!(
                ct1.len() == ML_KEM_BRAID_CT1_BYTES,
                "persisted ML-KEM Braid ct1 length is invalid"
            );
            validate_encoder(ek_encoder, ML_KEM_BRAID_EK_BYTES)
        }
        ProtocolState::EkSentCt1Received {
            auth,
            key_seed,
            ct1,
            ct2_decoder,
            ..
        } => {
            auth.validate()?;
            key_seed.ensure_len(KEY_GENERATION_SEED_SIZE)?;
            ensure!(
                ct1.len() == ML_KEM_BRAID_CT1_BYTES,
                "persisted ML-KEM Braid ct1 length is invalid"
            );
            ct2_decoder.validate_active(ML_KEM_BRAID_CT2_BYTES + ML_KEM_BRAID_MAC_BYTES)
        }
        ProtocolState::NoHeaderReceived {
            auth,
            header_decoder,
            ..
        } => {
            auth.validate()?;
            header_decoder.validate_active(ML_KEM_BRAID_HEADER_BYTES + ML_KEM_BRAID_MAC_BYTES)
        }
        ProtocolState::HeaderReceived {
            auth,
            header,
            ek_decoder,
            ..
        } => {
            auth.validate()?;
            ensure!(
                header.len() == ML_KEM_BRAID_HEADER_BYTES,
                "persisted ML-KEM Braid header length is invalid"
            );
            ek_decoder.validate_active(ML_KEM_BRAID_EK_BYTES)
        }
        ProtocolState::Ct1Sampled {
            auth,
            header,
            encaps_state,
            ct1,
            ct1_encoder,
            ek_decoder,
            ..
        } => {
            auth.validate()?;
            ensure!(
                header.len() == ML_KEM_BRAID_HEADER_BYTES && ct1.len() == ML_KEM_BRAID_CT1_BYTES,
                "persisted ML-KEM Braid sampled state length is invalid"
            );
            encaps_state.ensure_len(incremental::encaps_state_len())?;
            validate_encoder(ct1_encoder, ML_KEM_BRAID_CT1_BYTES)?;
            ek_decoder.validate_active(ML_KEM_BRAID_EK_BYTES)
        }
        ProtocolState::EkReceivedCt1Sampled {
            auth,
            encaps_state,
            ct1,
            ek_vector,
            ct1_encoder,
            ..
        } => {
            auth.validate()?;
            encaps_state.ensure_len(incremental::encaps_state_len())?;
            ensure!(
                ct1.len() == ML_KEM_BRAID_CT1_BYTES && ek_vector.len() == ML_KEM_BRAID_EK_BYTES,
                "persisted ML-KEM Braid sampled state length is invalid"
            );
            validate_encoder(ct1_encoder, ML_KEM_BRAID_CT1_BYTES)
        }
        ProtocolState::Ct1Acknowledged {
            auth,
            header,
            encaps_state,
            ct1,
            ek_decoder,
            ..
        } => {
            auth.validate()?;
            encaps_state.ensure_len(incremental::encaps_state_len())?;
            ensure!(
                header.len() == ML_KEM_BRAID_HEADER_BYTES && ct1.len() == ML_KEM_BRAID_CT1_BYTES,
                "persisted ML-KEM Braid acknowledged state length is invalid"
            );
            ek_decoder.validate_active(ML_KEM_BRAID_EK_BYTES)
        }
        ProtocolState::Ct2Sampled {
            auth, ct2_encoder, ..
        } => {
            auth.validate()?;
            validate_encoder(ct2_encoder, ML_KEM_BRAID_CT2_BYTES + ML_KEM_BRAID_MAC_BYTES)
        }
        ProtocolState::Poisoned { .. } => Ok(()),
    }
}
