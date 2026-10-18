//! Fixed-size zeroizing private-header frame and zero-padding validation.

use anyhow::{Result, anyhow, ensure};
use zeroize::Zeroizing;

use super::constants::{
    MAX_RELAY_PRIVATE_HEADER_BYTES, RELAY_HEADER_FRAME_BYTES, RELAY_HEADER_FRAME_MAGIC,
    RELAY_HEADER_LENGTH_BYTES,
};

pub(in crate::licoarc_relay) fn encode_private_relay_header_frame(
    private_header: &[u8],
) -> Result<Zeroizing<Vec<u8>>> {
    ensure!(
        private_header.len() <= MAX_RELAY_PRIVATE_HEADER_BYTES,
        "secure mesh private relay header payload is too large"
    );
    let payload_length = u32::try_from(private_header.len())
        .map_err(|_| anyhow!("secure mesh private relay header payload length is invalid"))?;
    let mut frame = Zeroizing::new(vec![0u8; RELAY_HEADER_FRAME_BYTES]);
    let mut offset = 0usize;
    frame[offset..offset + RELAY_HEADER_FRAME_MAGIC.len()]
        .copy_from_slice(RELAY_HEADER_FRAME_MAGIC);
    offset += RELAY_HEADER_FRAME_MAGIC.len();
    frame[offset..offset + RELAY_HEADER_LENGTH_BYTES]
        .copy_from_slice(&payload_length.to_be_bytes());
    offset += RELAY_HEADER_LENGTH_BYTES;
    frame[offset..offset + private_header.len()].copy_from_slice(private_header);
    Ok(frame)
}

pub(in crate::licoarc_relay) fn decode_private_relay_header_frame(
    frame: Zeroizing<Vec<u8>>,
) -> Result<Zeroizing<Vec<u8>>> {
    ensure!(
        frame.len() == RELAY_HEADER_FRAME_BYTES && frame.starts_with(RELAY_HEADER_FRAME_MAGIC),
        "secure mesh private relay header frame is invalid"
    );
    let length_start = RELAY_HEADER_FRAME_MAGIC.len();
    let payload_start = length_start + RELAY_HEADER_LENGTH_BYTES;
    let payload_length = u32::from_be_bytes(
        frame[length_start..payload_start]
            .try_into()
            .map_err(|_| anyhow!("secure mesh private relay header length is invalid"))?,
    );
    let payload_length = usize::try_from(payload_length)
        .map_err(|_| anyhow!("secure mesh private relay header length is invalid"))?;
    ensure!(
        payload_length <= MAX_RELAY_PRIVATE_HEADER_BYTES,
        "secure mesh private relay header length is outside bounds"
    );
    let payload_end = payload_start
        .checked_add(payload_length)
        .ok_or_else(|| anyhow!("secure mesh private relay header length overflow"))?;
    ensure!(
        payload_end <= frame.len() && frame[payload_end..].iter().all(|byte| *byte == 0),
        "secure mesh private relay header padding is invalid"
    );
    Ok(Zeroizing::new(frame[payload_start..payload_end].to_vec()))
}
