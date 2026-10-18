use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};
use zeroize::Zeroize;

#[derive(Clone, Serialize, Deserialize)]
#[serde(transparent)]
pub(super) struct SecretBytes(pub(super) Vec<u8>);

impl SecretBytes {
    pub(super) fn new(value: Vec<u8>) -> Self {
        Self(value)
    }

    pub(super) fn as_slice(&self) -> &[u8] {
        &self.0
    }

    pub(super) fn ensure_len(&self, expected: usize) -> Result<()> {
        ensure!(
            self.0.len() == expected,
            "persisted ML-KEM Braid secret length is invalid"
        );
        Ok(())
    }
}

impl Drop for SecretBytes {
    fn drop(&mut self) {
        self.0.zeroize();
    }
}
