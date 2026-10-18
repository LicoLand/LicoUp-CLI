use anyhow::{Result, anyhow};

use super::{
    output::{MlKemBraidOutputKey, MlKemBraidReceive, MlKemBraidSend},
    wire::{MlKemBraidChunk, MlKemBraidMessage, MlKemBraidMessageType},
};

pub(super) fn send_output(
    message: MlKemBraidMessage,
    state_epoch: u64,
    output_key: Option<MlKemBraidOutputKey>,
) -> Result<MlKemBraidSend> {
    Ok(MlKemBraidSend {
        message,
        sending_epoch: previous_epoch(state_epoch)?,
        output_key,
    })
}

pub(super) fn receive_output(
    state_epoch: u64,
    output_key: Option<MlKemBraidOutputKey>,
) -> Result<MlKemBraidReceive> {
    Ok(MlKemBraidReceive {
        receiving_epoch: previous_epoch(state_epoch)?,
        output_key,
    })
}

pub(super) fn previous_epoch(epoch: u64) -> Result<u64> {
    epoch
        .checked_sub(1)
        .ok_or_else(|| anyhow!("ML-KEM Braid epoch underflow"))
}

pub(super) fn checked_next_epoch(epoch: u64) -> Result<u64> {
    epoch
        .checked_add(1)
        .ok_or_else(|| anyhow!("ML-KEM Braid epoch exhausted"))
}

pub(super) fn is_payload(
    message: &MlKemBraidMessage,
    epoch: u64,
    message_type: MlKemBraidMessageType,
) -> bool {
    message.epoch == epoch && message.message_type == message_type
}

pub(super) fn required_data(message: &MlKemBraidMessage) -> Result<&MlKemBraidChunk> {
    message
        .data
        .as_ref()
        .ok_or_else(|| anyhow!("ML-KEM Braid message data is missing"))
}
