use anyhow::{Result, anyhow, ensure};

pub(super) fn append_len_prefixed_bytes(out: &mut Vec<u8>, value: &[u8]) -> Result<()> {
    let len = u32::try_from(value.len())
        .map_err(|_| anyhow!("secure mesh payload field is too large"))?;
    out.extend_from_slice(&len.to_be_bytes());
    out.extend_from_slice(value);
    Ok(())
}

pub(super) struct SliceReader<'a> {
    bytes: &'a [u8],
    offset: usize,
}

impl<'a> SliceReader<'a> {
    pub(super) fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, offset: 0 }
    }

    pub(super) fn expect_bytes(&mut self, expected: &[u8]) -> Result<()> {
        let actual = self.read_exact(expected.len())?;
        ensure!(
            actual == expected,
            "secure mesh payload plaintext magic is invalid"
        );
        Ok(())
    }

    pub(super) fn read_u8(&mut self) -> Result<u8> {
        Ok(self.read_exact(1)?[0])
    }

    pub(super) fn read_len_prefixed_bytes(&mut self) -> Result<&'a [u8]> {
        let len_bytes = self.read_exact(4)?;
        let len = u32::from_be_bytes(
            len_bytes
                .try_into()
                .map_err(|_| anyhow!("secure mesh length prefix is invalid"))?,
        ) as usize;
        self.read_exact(len)
    }

    fn read_exact(&mut self, len: usize) -> Result<&'a [u8]> {
        let end = self
            .offset
            .checked_add(len)
            .ok_or_else(|| anyhow!("secure mesh payload length overflow"))?;
        ensure!(end <= self.bytes.len(), "secure mesh payload is truncated");
        let value = &self.bytes[self.offset..end];
        self.offset = end;
        Ok(value)
    }

    pub(super) fn is_empty(&self) -> bool {
        self.offset == self.bytes.len()
    }
}
