use zeroize::Zeroizing;

use super::wire::MlKemBraidMessage;

/// Newly emitted SCKA output. Secrets deliberately have no Debug or serde.
pub(crate) struct MlKemBraidOutputKey {
    pub(super) epoch: u64,
    pub(super) key: Zeroizing<[u8; 32]>,
}

impl MlKemBraidOutputKey {
    pub fn epoch(&self) -> u64 {
        self.epoch
    }

    pub fn key(&self) -> &[u8; 32] {
        &self.key
    }
}

pub(crate) struct MlKemBraidSend {
    pub message: MlKemBraidMessage,
    pub sending_epoch: u64,
    pub output_key: Option<MlKemBraidOutputKey>,
}

pub(crate) struct MlKemBraidReceive {
    pub receiving_epoch: u64,
    pub output_key: Option<MlKemBraidOutputKey>,
}
