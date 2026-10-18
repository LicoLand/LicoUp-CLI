use anyhow::{Context, Result, anyhow, ensure};
use serde::{Deserialize, Serialize};
use zeroize::Zeroizing;

use super::{
    constants::{MAX_PERSISTED_SESSION_BYTES, PERSISTENCE_REVISION},
    protocol_state::ProtocolState,
    session::MlKemBraidSession,
    validation::validate_restored_state,
};

#[derive(Serialize)]
#[serde(deny_unknown_fields)]
struct PersistedSessionRef<'a> {
    revision: u8,
    state: &'a ProtocolState,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PersistedSession {
    revision: u8,
    state: ProtocolState,
}

impl MlKemBraidSession {
    pub fn persist(&self) -> Result<Zeroizing<Vec<u8>>> {
        let encoded = serde_json::to_vec(&PersistedSessionRef {
            revision: PERSISTENCE_REVISION,
            state: &self.state,
        })
        .map_err(|_| anyhow!("ML-KEM Braid state serialization failed"))?;
        Ok(Zeroizing::new(encoded))
    }

    pub fn restore(encoded: &[u8]) -> Result<Self> {
        ensure!(
            encoded.len() <= MAX_PERSISTED_SESSION_BYTES,
            "ML-KEM Braid persisted state exceeds the resource limit"
        );
        let mut persisted: PersistedSession =
            serde_json::from_slice(encoded).context("ML-KEM Braid persisted state is invalid")?;
        ensure!(
            persisted.revision == PERSISTENCE_REVISION,
            "ML-KEM Braid persisted state revision is unsupported"
        );
        validate_restored_state(&mut persisted.state)?;
        Ok(Self {
            state: persisted.state,
        })
    }
}
