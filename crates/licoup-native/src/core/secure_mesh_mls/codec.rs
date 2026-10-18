use anyhow::{Context, Result, anyhow, ensure};
use openmls::prelude::{MlsMessageIn, ProtocolMessage, tls_codec::Deserialize as TlsDeserialize};
use sha2::{Digest, Sha256};

pub(super) fn deserialize_protocol_message(
    message: &[u8],
    context: &'static str,
) -> Result<ProtocolMessage> {
    MlsMessageIn::tls_deserialize_exact(message)
        .context(context)?
        .try_into_protocol_message()
        .map_err(|_| anyhow!("secure mesh MLS message is not a protocol message"))
}

pub(super) fn append_mls_len_prefixed_bytes(out: &mut Vec<u8>, value: &[u8]) -> Result<()> {
    let len = u32::try_from(value.len())
        .map_err(|_| anyhow!("secure mesh MLS payload field is too large"))?;
    out.extend_from_slice(&len.to_be_bytes());
    out.extend_from_slice(value);
    Ok(())
}

pub(super) struct MlsPayloadReader<'a> {
    bytes: &'a [u8],
    offset: usize,
}

impl<'a> MlsPayloadReader<'a> {
    pub(super) fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, offset: 0 }
    }

    pub(super) fn expect_bytes(&mut self, expected: &[u8]) -> Result<()> {
        let actual = self.read_exact(expected.len())?;
        ensure!(
            actual == expected,
            "secure mesh MLS sealed payload magic is invalid"
        );
        Ok(())
    }

    pub(super) fn read_string(&mut self, label: &str) -> Result<String> {
        let bytes = self.read_len_prefixed_bytes()?;
        String::from_utf8(bytes.to_vec())
            .map_err(|_| anyhow!("secure mesh MLS sealed payload {label} is not valid UTF-8"))
    }

    pub(super) fn read_u64(&mut self) -> Result<u64> {
        let bytes = self.read_exact(8)?;
        Ok(u64::from_be_bytes(bytes.try_into().map_err(|_| {
            anyhow!("secure mesh MLS sealed payload integer is invalid")
        })?))
    }

    pub(super) fn read_len_prefixed_bytes(&mut self) -> Result<&'a [u8]> {
        let len_bytes = self.read_exact(4)?;
        let len = u32::from_be_bytes(
            len_bytes
                .try_into()
                .map_err(|_| anyhow!("secure mesh MLS sealed payload length is invalid"))?,
        ) as usize;
        self.read_exact(len)
    }

    pub(super) fn read_exact(&mut self, len: usize) -> Result<&'a [u8]> {
        let end = self
            .offset
            .checked_add(len)
            .ok_or_else(|| anyhow!("secure mesh MLS sealed payload length overflow"))?;
        ensure!(
            end <= self.bytes.len(),
            "secure mesh MLS sealed payload is truncated"
        );
        let value = &self.bytes[self.offset..end];
        self.offset = end;
        Ok(value)
    }

    pub(super) fn is_empty(&self) -> bool {
        self.offset == self.bytes.len()
    }
}

pub(super) fn hash_bytes(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut hex = [0u8; 64];
    for (index, byte) in digest.iter().enumerate() {
        hex[index * 2] = HEX[usize::from(byte >> 4)];
        hex[index * 2 + 1] = HEX[usize::from(byte & 0x0f)];
    }
    let mut out = String::with_capacity(71);
    out.push_str("sha256:");
    out.push_str(std::str::from_utf8(&hex).expect("hex digits are ASCII"));
    out
}
