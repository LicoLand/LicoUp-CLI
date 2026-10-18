use anyhow::{Result, bail};

use super::{
    constants::{
        ML_KEM_BRAID_CT1_BYTES, ML_KEM_BRAID_CT2_BYTES, ML_KEM_BRAID_EK_BYTES,
        ML_KEM_BRAID_HEADER_BYTES, ML_KEM_BRAID_MAC_BYTES,
    },
    encapsulation_kdf::{
        complete_encapsulation, decapsulate, derive_output_key, validate_encapsulation_key,
    },
    erasure_decoder::ErasureDecoder,
    erasure_encoder::ErasureEncoder,
    output::MlKemBraidReceive,
    protocol_state::ProtocolState,
    transition::{checked_next_epoch, is_payload, previous_epoch, receive_output, required_data},
    wire::{MlKemBraidMessage, MlKemBraidMessageType},
};

pub(super) fn receive_state(
    state: ProtocolState,
    message: &MlKemBraidMessage,
) -> Result<(ProtocolState, MlKemBraidReceive)> {
    match state {
        ProtocolState::KeysUnsampled { epoch, auth } => Ok((
            ProtocolState::KeysUnsampled { epoch, auth },
            receive_output(epoch, None)?,
        )),
        ProtocolState::KeysSampled {
            epoch,
            auth,
            key_seed,
            ek_vector,
            header_encoder,
        } => {
            if is_payload(message, epoch, MlKemBraidMessageType::Ct1) {
                let mut ct1_decoder = ErasureDecoder::new(ML_KEM_BRAID_CT1_BYTES)?;
                ct1_decoder.add_chunk(required_data(message)?)?;
                let ek_encoder = ErasureEncoder::new(&ek_vector)?;
                // Transition (2).
                Ok((
                    ProtocolState::HeaderSent {
                        epoch,
                        auth,
                        key_seed,
                        ct1_decoder,
                        ek_encoder,
                    },
                    receive_output(epoch, None)?,
                ))
            } else {
                Ok((
                    ProtocolState::KeysSampled {
                        epoch,
                        auth,
                        key_seed,
                        ek_vector,
                        header_encoder,
                    },
                    receive_output(epoch, None)?,
                ))
            }
        }
        ProtocolState::HeaderSent {
            epoch,
            auth,
            key_seed,
            mut ct1_decoder,
            ek_encoder,
        } => {
            if is_payload(message, epoch, MlKemBraidMessageType::Ct1) {
                ct1_decoder.add_chunk(required_data(message)?)?;
                if ct1_decoder.has_message() {
                    let ct1 = ct1_decoder.take_message()?;
                    // Transition (3).
                    return Ok((
                        ProtocolState::Ct1Received {
                            epoch,
                            auth,
                            key_seed,
                            ct1,
                            ek_encoder,
                        },
                        receive_output(epoch, None)?,
                    ));
                }
            }
            Ok((
                ProtocolState::HeaderSent {
                    epoch,
                    auth,
                    key_seed,
                    ct1_decoder,
                    ek_encoder,
                },
                receive_output(epoch, None)?,
            ))
        }
        ProtocolState::Ct1Received {
            epoch,
            auth,
            key_seed,
            ct1,
            ek_encoder,
        } => {
            if is_payload(message, epoch, MlKemBraidMessageType::Ct2) {
                let mut ct2_decoder =
                    ErasureDecoder::new(ML_KEM_BRAID_CT2_BYTES + ML_KEM_BRAID_MAC_BYTES)?;
                ct2_decoder.add_chunk(required_data(message)?)?;
                // Transition (4).
                Ok((
                    ProtocolState::EkSentCt1Received {
                        epoch,
                        auth,
                        key_seed,
                        ct1,
                        ct2_decoder,
                    },
                    receive_output(epoch, None)?,
                ))
            } else {
                Ok((
                    ProtocolState::Ct1Received {
                        epoch,
                        auth,
                        key_seed,
                        ct1,
                        ek_encoder,
                    },
                    receive_output(epoch, None)?,
                ))
            }
        }
        ProtocolState::EkSentCt1Received {
            epoch,
            mut auth,
            key_seed,
            ct1,
            mut ct2_decoder,
        } => {
            if is_payload(message, epoch, MlKemBraidMessageType::Ct2) {
                ct2_decoder.add_chunk(required_data(message)?)?;
                if ct2_decoder.has_message() {
                    let ct2_with_mac = ct2_decoder.take_message()?;
                    let ct2 = &ct2_with_mac[..ML_KEM_BRAID_CT2_BYTES];
                    let mac = &ct2_with_mac[ML_KEM_BRAID_CT2_BYTES..];
                    let raw_shared_secret = decapsulate(&key_seed, &ct1, ct2)?;
                    let output_key = derive_output_key(&raw_shared_secret[..], epoch)?;
                    auth.update(epoch, output_key.key())?;
                    let mut authenticated =
                        Vec::with_capacity(ML_KEM_BRAID_CT1_BYTES + ML_KEM_BRAID_CT2_BYTES);
                    authenticated.extend_from_slice(&ct1);
                    authenticated.extend_from_slice(ct2);
                    auth.verify_ciphertext(epoch, &authenticated, mac)?;
                    let next_epoch = checked_next_epoch(epoch)?;
                    // Transition (5).
                    return Ok((
                        ProtocolState::NoHeaderReceived {
                            epoch: next_epoch,
                            auth,
                            header_decoder: ErasureDecoder::new(
                                ML_KEM_BRAID_HEADER_BYTES + ML_KEM_BRAID_MAC_BYTES,
                            )?,
                        },
                        MlKemBraidReceive {
                            receiving_epoch: previous_epoch(epoch)?,
                            output_key: Some(output_key),
                        },
                    ));
                }
            }
            Ok((
                ProtocolState::EkSentCt1Received {
                    epoch,
                    auth,
                    key_seed,
                    ct1,
                    ct2_decoder,
                },
                receive_output(epoch, None)?,
            ))
        }
        ProtocolState::NoHeaderReceived {
            epoch,
            auth,
            mut header_decoder,
        } => {
            if is_payload(message, epoch, MlKemBraidMessageType::Hdr) {
                header_decoder.add_chunk(required_data(message)?)?;
                if header_decoder.has_message() {
                    let header_with_mac = header_decoder.take_message()?;
                    let header = header_with_mac[..ML_KEM_BRAID_HEADER_BYTES].to_vec();
                    auth.verify_header(
                        epoch,
                        &header,
                        &header_with_mac[ML_KEM_BRAID_HEADER_BYTES..],
                    )?;
                    // Transition (6).
                    return Ok((
                        ProtocolState::HeaderReceived {
                            epoch,
                            auth,
                            header,
                            ek_decoder: ErasureDecoder::new(ML_KEM_BRAID_EK_BYTES)?,
                        },
                        receive_output(epoch, None)?,
                    ));
                }
            }
            Ok((
                ProtocolState::NoHeaderReceived {
                    epoch,
                    auth,
                    header_decoder,
                },
                receive_output(epoch, None)?,
            ))
        }
        ProtocolState::HeaderReceived {
            epoch,
            auth,
            header,
            ek_decoder,
        } => Ok((
            ProtocolState::HeaderReceived {
                epoch,
                auth,
                header,
                ek_decoder,
            },
            receive_output(epoch, None)?,
        )),
        remaining => receive_state_after_header(remaining, message),
    }
}

fn receive_state_after_header(
    state: ProtocolState,
    message: &MlKemBraidMessage,
) -> Result<(ProtocolState, MlKemBraidReceive)> {
    match state {
        ProtocolState::Ct1Sampled {
            epoch,
            auth,
            header,
            encaps_state,
            ct1,
            ct1_encoder,
            mut ek_decoder,
        } => {
            if is_payload(message, epoch, MlKemBraidMessageType::Ek) {
                ek_decoder.add_chunk(required_data(message)?)?;
                if ek_decoder.has_message() {
                    let ek_vector = ek_decoder.take_message()?;
                    validate_encapsulation_key(&header, &ek_vector)?;
                    // Transition (10).
                    return Ok((
                        ProtocolState::EkReceivedCt1Sampled {
                            epoch,
                            auth,
                            encaps_state,
                            ct1,
                            ek_vector,
                            ct1_encoder,
                        },
                        receive_output(epoch, None)?,
                    ));
                }
            } else if is_payload(message, epoch, MlKemBraidMessageType::EkCt1Ack) {
                ek_decoder.add_chunk(required_data(message)?)?;
                if ek_decoder.has_message() {
                    let ek_vector = ek_decoder.take_message()?;
                    validate_encapsulation_key(&header, &ek_vector)?;
                    let ct2_encoder =
                        complete_encapsulation(&auth, epoch, &encaps_state, &ct1, &ek_vector)?;
                    // Transition (9).
                    return Ok((
                        ProtocolState::Ct2Sampled {
                            epoch,
                            auth,
                            ct2_encoder,
                        },
                        receive_output(epoch, None)?,
                    ));
                }
                // Transition (8).
                return Ok((
                    ProtocolState::Ct1Acknowledged {
                        epoch,
                        auth,
                        header,
                        encaps_state,
                        ct1,
                        ek_decoder,
                    },
                    receive_output(epoch, None)?,
                ));
            }
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
                receive_output(epoch, None)?,
            ))
        }
        ProtocolState::EkReceivedCt1Sampled {
            epoch,
            auth,
            encaps_state,
            ct1,
            ek_vector,
            ct1_encoder,
        } => {
            if is_payload(message, epoch, MlKemBraidMessageType::EkCt1Ack) {
                let ct2_encoder =
                    complete_encapsulation(&auth, epoch, &encaps_state, &ct1, &ek_vector)?;
                // Transition (12).
                Ok((
                    ProtocolState::Ct2Sampled {
                        epoch,
                        auth,
                        ct2_encoder,
                    },
                    receive_output(epoch, None)?,
                ))
            } else {
                Ok((
                    ProtocolState::EkReceivedCt1Sampled {
                        epoch,
                        auth,
                        encaps_state,
                        ct1,
                        ek_vector,
                        ct1_encoder,
                    },
                    receive_output(epoch, None)?,
                ))
            }
        }
        ProtocolState::Ct1Acknowledged {
            epoch,
            auth,
            header,
            encaps_state,
            ct1,
            mut ek_decoder,
        } => {
            if is_payload(message, epoch, MlKemBraidMessageType::EkCt1Ack) {
                ek_decoder.add_chunk(required_data(message)?)?;
                if ek_decoder.has_message() {
                    let ek_vector = ek_decoder.take_message()?;
                    validate_encapsulation_key(&header, &ek_vector)?;
                    let ct2_encoder =
                        complete_encapsulation(&auth, epoch, &encaps_state, &ct1, &ek_vector)?;
                    // Transition (11).
                    return Ok((
                        ProtocolState::Ct2Sampled {
                            epoch,
                            auth,
                            ct2_encoder,
                        },
                        receive_output(epoch, None)?,
                    ));
                }
            }
            Ok((
                ProtocolState::Ct1Acknowledged {
                    epoch,
                    auth,
                    header,
                    encaps_state,
                    ct1,
                    ek_decoder,
                },
                receive_output(epoch, None)?,
            ))
        }
        ProtocolState::Ct2Sampled {
            epoch,
            auth,
            ct2_encoder,
        } => {
            let next_epoch = checked_next_epoch(epoch)?;
            if message.epoch == next_epoch {
                // Transition (13).
                Ok((
                    ProtocolState::KeysUnsampled {
                        epoch: next_epoch,
                        auth,
                    },
                    MlKemBraidReceive {
                        receiving_epoch: epoch,
                        output_key: None,
                    },
                ))
            } else {
                Ok((
                    ProtocolState::Ct2Sampled {
                        epoch,
                        auth,
                        ct2_encoder,
                    },
                    receive_output(epoch, None)?,
                ))
            }
        }
        ProtocolState::Poisoned { .. } => bail!("ML-KEM Braid session is poisoned"),
        _ => bail!("ML-KEM Braid internal state dispatch failed"),
    }
}
