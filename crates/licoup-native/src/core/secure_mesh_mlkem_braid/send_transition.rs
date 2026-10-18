use anyhow::{Result, anyhow, bail};
use libcrux_ml_kem::{KEY_GENERATION_SEED_SIZE, mlkem1024::incremental};
use rand_core::{CryptoRng, RngCore};
use zeroize::{Zeroize, Zeroizing};

use super::{
    constants::{ML_KEM_BRAID_EK_BYTES, ML_KEM_BRAID_HEADER_BYTES},
    encapsulation_kdf::derive_output_key,
    erasure_encoder::ErasureEncoder,
    output::MlKemBraidSend,
    protocol_state::ProtocolState,
    secret::SecretBytes,
    transition::send_output,
    wire::{MlKemBraidMessage, MlKemBraidMessageType},
};

pub(super) fn send_state<R>(
    state: ProtocolState,
    rng: &mut R,
) -> Result<(ProtocolState, MlKemBraidSend)>
where
    R: RngCore + CryptoRng,
{
    match state {
        ProtocolState::KeysUnsampled { epoch, auth } => {
            let mut key_seed = Zeroizing::new([0u8; KEY_GENERATION_SEED_SIZE]);
            rng.fill_bytes(key_seed.as_mut());
            let mut key_pair = Zeroizing::new([0u8; incremental::COMPRESSED_KEYPAIR_LEN]);
            incremental::generate_key_pair_compressed(*key_seed, &mut *key_pair);
            let ek_offset = incremental::pk2_len();
            let header_offset = ek_offset * 2;
            let header =
                key_pair[header_offset..header_offset + ML_KEM_BRAID_HEADER_BYTES].to_vec();
            let ek_vector = key_pair[ek_offset..ek_offset + ML_KEM_BRAID_EK_BYTES].to_vec();
            incremental::validate_pk_bytes(&header, &ek_vector)
                .map_err(|_| anyhow!("ML-KEM Braid generated key is invalid"))?;
            let mac = auth.mac_header(epoch, &header)?;
            let mut header_with_mac = header;
            header_with_mac.extend_from_slice(&mac);
            let mut header_encoder = ErasureEncoder::new(&header_with_mac)?;
            let chunk = header_encoder.next_chunk()?;
            let output = send_output(
                MlKemBraidMessage::payload(epoch, MlKemBraidMessageType::Hdr, chunk),
                epoch,
                None,
            )?;
            // Transition (1).
            Ok((
                ProtocolState::KeysSampled {
                    epoch,
                    auth,
                    key_seed: SecretBytes::new(key_seed.to_vec()),
                    ek_vector,
                    header_encoder,
                },
                output,
            ))
        }
        ProtocolState::KeysSampled {
            epoch,
            auth,
            key_seed,
            ek_vector,
            mut header_encoder,
        } => {
            let chunk = header_encoder.next_chunk()?;
            let output = send_output(
                MlKemBraidMessage::payload(epoch, MlKemBraidMessageType::Hdr, chunk),
                epoch,
                None,
            )?;
            Ok((
                ProtocolState::KeysSampled {
                    epoch,
                    auth,
                    key_seed,
                    ek_vector,
                    header_encoder,
                },
                output,
            ))
        }
        ProtocolState::HeaderSent {
            epoch,
            auth,
            key_seed,
            ct1_decoder,
            mut ek_encoder,
        } => {
            let chunk = ek_encoder.next_chunk()?;
            let output = send_output(
                MlKemBraidMessage::payload(epoch, MlKemBraidMessageType::Ek, chunk),
                epoch,
                None,
            )?;
            Ok((
                ProtocolState::HeaderSent {
                    epoch,
                    auth,
                    key_seed,
                    ct1_decoder,
                    ek_encoder,
                },
                output,
            ))
        }
        ProtocolState::Ct1Received {
            epoch,
            auth,
            key_seed,
            ct1,
            mut ek_encoder,
        } => {
            let chunk = ek_encoder.next_chunk()?;
            let output = send_output(
                MlKemBraidMessage::payload(epoch, MlKemBraidMessageType::EkCt1Ack, chunk),
                epoch,
                None,
            )?;
            Ok((
                ProtocolState::Ct1Received {
                    epoch,
                    auth,
                    key_seed,
                    ct1,
                    ek_encoder,
                },
                output,
            ))
        }
        ProtocolState::EkSentCt1Received {
            epoch,
            auth,
            key_seed,
            ct1,
            ct2_decoder,
        } => {
            let output = send_output(
                MlKemBraidMessage::empty(epoch, MlKemBraidMessageType::None),
                epoch,
                None,
            )?;
            Ok((
                ProtocolState::EkSentCt1Received {
                    epoch,
                    auth,
                    key_seed,
                    ct1,
                    ct2_decoder,
                },
                output,
            ))
        }
        ProtocolState::NoHeaderReceived {
            epoch,
            auth,
            header_decoder,
        } => {
            let output = send_output(
                MlKemBraidMessage::empty(epoch, MlKemBraidMessageType::None),
                epoch,
                None,
            )?;
            Ok((
                ProtocolState::NoHeaderReceived {
                    epoch,
                    auth,
                    header_decoder,
                },
                output,
            ))
        }
        ProtocolState::HeaderReceived {
            epoch,
            mut auth,
            header,
            ek_decoder,
        } => {
            let mut randomness = [0u8; 32];
            rng.fill_bytes(&mut randomness);
            let mut encaps_state = Zeroizing::new(vec![0u8; incremental::encaps_state_len()]);
            let mut raw_shared_secret = Zeroizing::new([0u8; 32]);
            let ciphertext1 = incremental::encapsulate1(
                &header,
                randomness,
                encaps_state.as_mut_slice(),
                raw_shared_secret.as_mut(),
            )
            .map_err(|_| anyhow!("ML-KEM Braid Encaps1 failed"))?;
            randomness.zeroize();
            let output_key = derive_output_key(&raw_shared_secret[..], epoch)?;
            auth.update(epoch, output_key.key())?;
            let ct1 = ciphertext1.value.to_vec();
            let mut ct1_encoder = ErasureEncoder::new(&ct1)?;
            let chunk = ct1_encoder.next_chunk()?;
            let output = send_output(
                MlKemBraidMessage::payload(epoch, MlKemBraidMessageType::Ct1, chunk),
                epoch,
                Some(output_key),
            )?;
            // Transition (7).
            Ok((
                ProtocolState::Ct1Sampled {
                    epoch,
                    auth,
                    header,
                    encaps_state: SecretBytes::new(encaps_state.to_vec()),
                    ct1,
                    ct1_encoder,
                    ek_decoder,
                },
                output,
            ))
        }
        ProtocolState::Ct1Sampled {
            epoch,
            auth,
            header,
            encaps_state,
            ct1,
            mut ct1_encoder,
            ek_decoder,
        } => {
            let chunk = ct1_encoder.next_chunk()?;
            let output = send_output(
                MlKemBraidMessage::payload(epoch, MlKemBraidMessageType::Ct1, chunk),
                epoch,
                None,
            )?;
            Ok((
                ProtocolState::Ct1Sampled {
                    epoch,
                    auth,
                    header,
                    encaps_state,
                    ct1,
                    ct1_encoder,
                    ek_decoder,
                },
                output,
            ))
        }
        ProtocolState::EkReceivedCt1Sampled {
            epoch,
            auth,
            encaps_state,
            ct1,
            ek_vector,
            mut ct1_encoder,
        } => {
            let chunk = ct1_encoder.next_chunk()?;
            let output = send_output(
                MlKemBraidMessage::payload(epoch, MlKemBraidMessageType::Ct1, chunk),
                epoch,
                None,
            )?;
            Ok((
                ProtocolState::EkReceivedCt1Sampled {
                    epoch,
                    auth,
                    encaps_state,
                    ct1,
                    ek_vector,
                    ct1_encoder,
                },
                output,
            ))
        }
        ProtocolState::Ct1Acknowledged {
            epoch,
            auth,
            header,
            encaps_state,
            ct1,
            ek_decoder,
        } => {
            let output = send_output(
                MlKemBraidMessage::empty(epoch, MlKemBraidMessageType::None),
                epoch,
                None,
            )?;
            Ok((
                ProtocolState::Ct1Acknowledged {
                    epoch,
                    auth,
                    header,
                    encaps_state,
                    ct1,
                    ek_decoder,
                },
                output,
            ))
        }
        ProtocolState::Ct2Sampled {
            epoch,
            auth,
            mut ct2_encoder,
        } => {
            let chunk = ct2_encoder.next_chunk()?;
            let output = send_output(
                MlKemBraidMessage::payload(epoch, MlKemBraidMessageType::Ct2, chunk),
                epoch,
                None,
            )?;
            Ok((
                ProtocolState::Ct2Sampled {
                    epoch,
                    auth,
                    ct2_encoder,
                },
                output,
            ))
        }
        ProtocolState::Poisoned { .. } => bail!("ML-KEM Braid session is poisoned"),
    }
}
