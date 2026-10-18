use rand_core::{OsRng, RngCore};
use zeroize::Zeroizing;

use super::constants::CONTENT_KEY_LEN;

pub struct ContentKey {
    bytes: Zeroizing<Vec<u8>>,
}

impl ContentKey {
    pub fn generate() -> Self {
        let mut bytes = vec![0u8; CONTENT_KEY_LEN];
        OsRng.fill_bytes(&mut bytes);
        Self {
            bytes: Zeroizing::new(bytes),
        }
    }

    pub fn from_bytes(bytes: [u8; CONTENT_KEY_LEN]) -> Self {
        Self {
            bytes: Zeroizing::new(bytes.to_vec()),
        }
    }

    pub(super) fn as_slice(&self) -> &[u8] {
        self.bytes.as_slice()
    }
}
