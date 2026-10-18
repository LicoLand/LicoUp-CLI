//! Sparse post-quantum ratchet built on the ML-KEM Braid SCKA.
//!
//! This module follows the public-domain Signal Sparse Post-Quantum Ratchet
//! construction. It produces one post-quantum message key per application
//! message; the pairwise Triple Ratchet combines it with the classical Double
//! Ratchet message key before payload encryption.

use std::collections::BTreeMap;
use std::fmt;

use anyhow::{Context, Result, anyhow, ensure};
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use hkdf::Hkdf;
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use zeroize::{Zeroize, Zeroizing};

use crate::core::secure_mesh_mlkem_braid::{
    MlKemBraidMessage, MlKemBraidOutputKey, MlKemBraidSession,
};

const SPQR_PERSISTENCE_REVISION: u8 = 2;
const SPQR_KEY_BYTES: usize = 32;
const SPQR_MAX_SKIPPED_KEYS: usize = 32;
const SPQR_MAX_PERSISTED_STATE_BYTES: usize = 1024 * 1024;
const SPQR_KEY_BASE64URL_BYTES: usize = (SPQR_KEY_BYTES * 8 + 5) / 6;
const SPQR_MAX_ENCODED_BRAID_BYTES: usize = (512 * 1024 * 8 + 5) / 6;
const SPQR_PROTOCOL_INFO: &[u8] = b"licomesh.secure-mesh.spqr.mlkem1024-braid.v1";
const SPQR_CHAIN_START_LABEL: &[u8] = b" Chain Start";
const SPQR_CHAIN_ADD_EPOCH_LABEL: &[u8] = b" Chain Add Epoch";
const SPQR_CHAIN_STEP_LABEL: &[u8] = b" Chain Step";
const TRIPLE_RATCHET_PROTOCOL_INFO: &[u8] =
    b"licomesh.secure-mesh.triple-ratchet.pqxdh-mlkem1024.v1";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum SecureMeshSparsePqRole {
    Initiator,
    Responder,
}

impl SecureMeshSparsePqRole {
    fn as_str(self) -> &'static str {
        match self {
            Self::Initiator => "initiator",
            Self::Responder => "responder",
        }
    }

    fn from_str(value: &str) -> Result<Self> {
        match value {
            "initiator" => Ok(Self::Initiator),
            "responder" => Ok(Self::Responder),
            _ => Err(anyhow!("sparse PQ ratchet role is unsupported")),
        }
    }
}

#[derive(Clone, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct SecureMeshSparsePqHeader {
    pub braid_message: MlKemBraidMessage,
    pub message_number: u64,
    pub previous_chain_length: u64,
}

impl fmt::Debug for SecureMeshSparsePqHeader {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SecureMeshSparsePqHeader")
            .field("braid_message", &"<redacted>")
            .field("message_number", &self.message_number)
            .field("previous_chain_length", &self.previous_chain_length)
            .finish()
    }
}

pub(crate) struct SecureMeshSparsePqSend {
    pub header: SecureMeshSparsePqHeader,
    pub message_key: Zeroizing<[u8; SPQR_KEY_BYTES]>,
}

#[derive(Clone)]
struct KdfChain {
    key: Zeroizing<[u8; SPQR_KEY_BYTES]>,
    counter: u64,
}

#[derive(Clone)]
struct EpochChains {
    send: Option<KdfChain>,
    receive: Option<KdfChain>,
}

#[derive(Clone)]
struct SkippedMessageKey {
    epoch: u64,
    message_number: u64,
    key: Zeroizing<[u8; SPQR_KEY_BYTES]>,
}

/// Client-only state. It deliberately has no `Debug` or direct serde support.
pub(crate) struct SecureMeshSparsePqRatchet {
    role: SecureMeshSparsePqRole,
    root_key: Zeroizing<[u8; SPQR_KEY_BYTES]>,
    epoch: u64,
    sending_epoch: u64,
    receiving_epoch: u64,
    previous_sending_chain_length: u64,
    chains: BTreeMap<u64, EpochChains>,
    skipped: Vec<SkippedMessageKey>,
    braid: MlKemBraidSession,
}

impl SecureMeshSparsePqRatchet {
    pub(crate) fn new_initiator(shared_secret: &[u8; SPQR_KEY_BYTES]) -> Result<Self> {
        Self::new(SecureMeshSparsePqRole::Initiator, shared_secret)
    }

    pub(crate) fn new_responder(shared_secret: &[u8; SPQR_KEY_BYTES]) -> Result<Self> {
        Self::new(SecureMeshSparsePqRole::Responder, shared_secret)
    }

    fn new(role: SecureMeshSparsePqRole, shared_secret: &[u8; SPQR_KEY_BYTES]) -> Result<Self> {
        let (root_key, a_to_b, b_to_a) = derive_initial_chains(shared_secret)?;
        let (send, receive) = match role {
            SecureMeshSparsePqRole::Initiator => (a_to_b, b_to_a),
            SecureMeshSparsePqRole::Responder => (b_to_a, a_to_b),
        };
        let braid = match role {
            SecureMeshSparsePqRole::Initiator => MlKemBraidSession::new_initiator(shared_secret)?,
            SecureMeshSparsePqRole::Responder => MlKemBraidSession::new_responder(shared_secret)?,
        };
        let mut chains = BTreeMap::new();
        chains.insert(
            0,
            EpochChains {
                send: Some(KdfChain {
                    key: Zeroizing::new(send),
                    counter: 0,
                }),
                receive: Some(KdfChain {
                    key: Zeroizing::new(receive),
                    counter: 0,
                }),
            },
        );
        Ok(Self {
            role,
            root_key: Zeroizing::new(root_key),
            epoch: 0,
            sending_epoch: 0,
            receiving_epoch: 0,
            previous_sending_chain_length: 0,
            chains,
            skipped: Vec::new(),
            braid,
        })
    }

    #[cfg(test)]
    pub(crate) fn epoch(&self) -> u64 {
        self.epoch
    }

    pub(crate) fn is_poisoned(&self) -> bool {
        self.braid.is_poisoned()
    }

    pub(crate) fn destroy(&mut self) {
        self.root_key.as_mut().fill(0);
        for chains in self.chains.values_mut() {
            if let Some(chain) = chains.send.as_mut() {
                chain.key.as_mut().fill(0);
            }
            if let Some(chain) = chains.receive.as_mut() {
                chain.key.as_mut().fill(0);
            }
        }
        for skipped in &mut self.skipped {
            skipped.key.as_mut().fill(0);
        }
        self.skipped.clear();
        self.braid.destroy();
    }

    pub(crate) fn send_key(&mut self) -> Result<SecureMeshSparsePqSend> {
        ensure!(!self.is_poisoned(), "sparse PQ ratchet is poisoned");
        let scka = self.braid.send()?;
        if let Some(output) = scka.output_key {
            self.add_epoch(output)?;
        }
        ensure!(
            scka.sending_epoch == self.sending_epoch
                || scka.sending_epoch.checked_sub(1) == Some(self.sending_epoch),
            "sparse PQ ratchet sending epoch is non-contiguous"
        );
        if scka.sending_epoch > self.sending_epoch {
            let previous = self
                .chains
                .get_mut(&self.sending_epoch)
                .and_then(|chains| chains.send.take())
                .ok_or_else(|| {
                    anyhow!("sparse PQ ratchet previous sending chain is unavailable")
                })?;
            self.previous_sending_chain_length = previous.counter;
            self.sending_epoch = scka.sending_epoch;
        }
        let chain = self
            .chains
            .get_mut(&self.sending_epoch)
            .and_then(|chains| chains.send.as_mut())
            .ok_or_else(|| anyhow!("sparse PQ ratchet sending chain is unavailable"))?;
        let message_number = chain
            .counter
            .checked_add(1)
            .ok_or_else(|| anyhow!("sparse PQ ratchet sending counter exhausted"))?;
        let (next_chain, message_key) = derive_chain_step(&chain.key, message_number)?;
        chain.key = next_chain;
        chain.counter = message_number;
        self.prune_old_epochs();
        Ok(SecureMeshSparsePqSend {
            header: SecureMeshSparsePqHeader {
                braid_message: scka.message,
                message_number,
                previous_chain_length: self.previous_sending_chain_length,
            },
            message_key,
        })
    }

    pub(crate) fn receive_key(
        &mut self,
        header: &SecureMeshSparsePqHeader,
    ) -> Result<Zeroizing<[u8; SPQR_KEY_BYTES]>> {
        ensure!(!self.is_poisoned(), "sparse PQ ratchet is poisoned");
        ensure!(
            header.message_number > 0,
            "sparse PQ ratchet message number is invalid"
        );
        let message_epoch = header
            .braid_message
            .epoch()
            .checked_sub(1)
            .ok_or_else(|| anyhow!("sparse PQ ratchet message epoch is invalid"))?;
        let scka = self.braid.receive(&header.braid_message)?;
        if let Some(output) = scka.output_key {
            self.add_epoch(output)?;
        }
        ensure!(
            scka.receiving_epoch <= self.receiving_epoch
                || scka.receiving_epoch.checked_sub(1) == Some(self.receiving_epoch),
            "sparse PQ ratchet receiving epoch is non-contiguous"
        );
        if scka.receiving_epoch > self.receiving_epoch {
            ensure!(
                scka.receiving_epoch.checked_sub(1) == Some(self.receiving_epoch),
                "sparse PQ ratchet receiving epoch skipped a chain"
            );
            self.skip_until(self.receiving_epoch, header.previous_chain_length)?;
            let previous = self.chains.get_mut(&self.receiving_epoch).ok_or_else(|| {
                anyhow!("sparse PQ ratchet previous receiving epoch is unavailable")
            })?;
            previous.receive = None;
            self.receiving_epoch = scka.receiving_epoch;
        }
        ensure!(
            message_epoch == self.receiving_epoch
                || message_epoch.checked_add(1) == Some(self.receiving_epoch),
            "sparse PQ ratchet message epoch is outside the retained window"
        );
        if let Some(position) = self.skipped.iter().position(|candidate| {
            candidate.epoch == message_epoch && candidate.message_number == header.message_number
        }) {
            return Ok(self.skipped.remove(position).key);
        }
        ensure!(
            message_epoch == self.receiving_epoch,
            "sparse PQ ratchet delayed message key is unavailable"
        );
        let preceding_message_number = header
            .message_number
            .checked_sub(1)
            .ok_or_else(|| anyhow!("sparse PQ ratchet message number underflow"))?;
        self.skip_until(message_epoch, preceding_message_number)?;
        let chain = self
            .chains
            .get_mut(&message_epoch)
            .and_then(|chains| chains.receive.as_mut())
            .ok_or_else(|| anyhow!("sparse PQ ratchet receiving chain is unavailable"))?;
        ensure!(
            chain.counter.checked_add(1) == Some(header.message_number),
            "sparse PQ ratchet message key is stale or unavailable"
        );
        let (next_chain, message_key) = derive_chain_step(&chain.key, header.message_number)?;
        chain.key = next_chain;
        chain.counter = header.message_number;
        self.prune_old_epochs();
        Ok(message_key)
    }

    fn add_epoch(&mut self, output: MlKemBraidOutputKey) -> Result<()> {
        let expected_epoch = self
            .epoch
            .checked_add(1)
            .ok_or_else(|| anyhow!("sparse PQ ratchet epoch exhausted"))?;
        ensure!(
            output.epoch() == expected_epoch,
            "sparse PQ ratchet output epoch is not contiguous"
        );
        let (root_key, a_to_b, b_to_a) = derive_added_epoch(&self.root_key, output.key())?;
        let (send, receive) = match self.role {
            SecureMeshSparsePqRole::Initiator => (a_to_b, b_to_a),
            SecureMeshSparsePqRole::Responder => (b_to_a, a_to_b),
        };
        ensure!(
            self.chains
                .insert(
                    expected_epoch,
                    EpochChains {
                        send: Some(KdfChain {
                            key: Zeroizing::new(send),
                            counter: 0,
                        }),
                        receive: Some(KdfChain {
                            key: Zeroizing::new(receive),
                            counter: 0,
                        }),
                    },
                )
                .is_none(),
            "sparse PQ ratchet epoch already exists"
        );
        self.root_key = Zeroizing::new(root_key);
        self.epoch = expected_epoch;
        self.prune_old_epochs();
        Ok(())
    }

    fn skip_until(&mut self, epoch: u64, until: u64) -> Result<()> {
        let current = self
            .chains
            .get(&epoch)
            .and_then(|chains| chains.receive.as_ref())
            .map(|chain| chain.counter)
            .ok_or_else(|| anyhow!("sparse PQ ratchet receiving chain is unavailable"))?;
        ensure!(
            until >= current,
            "sparse PQ ratchet previous chain length regressed"
        );
        let missing = until - current;
        ensure!(
            usize::try_from(missing).is_ok_and(|missing| {
                missing <= SPQR_MAX_SKIPPED_KEYS.saturating_sub(self.skipped.len())
            }),
            "sparse PQ ratchet skipped-key limit exceeded"
        );
        while self
            .chains
            .get(&epoch)
            .and_then(|chains| chains.receive.as_ref())
            .is_some_and(|chain| chain.counter < until)
        {
            let chain = self
                .chains
                .get_mut(&epoch)
                .and_then(|chains| chains.receive.as_mut())
                .ok_or_else(|| anyhow!("sparse PQ ratchet receiving chain is unavailable"))?;
            let message_number = chain
                .counter
                .checked_add(1)
                .ok_or_else(|| anyhow!("sparse PQ ratchet receiving counter exhausted"))?;
            let (next_chain, message_key) = derive_chain_step(&chain.key, message_number)?;
            chain.key = next_chain;
            chain.counter = message_number;
            self.skipped.push(SkippedMessageKey {
                epoch,
                message_number,
                key: message_key,
            });
        }
        Ok(())
    }

    fn prune_old_epochs(&mut self) {
        let oldest = self.epoch.saturating_sub(1);
        self.chains.retain(|epoch, _| *epoch >= oldest);
        self.skipped.retain(|key| key.epoch >= oldest);
    }

    pub(crate) fn persist(&self) -> Result<Zeroizing<Vec<u8>>> {
        let braid = self.braid.persist()?;
        let persisted = PersistedSparsePqRatchet {
            revision: SPQR_PERSISTENCE_REVISION,
            role: self.role.as_str().to_string(),
            root_key: SecretString::new(encode_key(&self.root_key)),
            epoch: self.epoch,
            sending_epoch: self.sending_epoch,
            receiving_epoch: self.receiving_epoch,
            previous_sending_chain_length: self.previous_sending_chain_length,
            chains: self
                .chains
                .iter()
                .map(|(epoch, chains)| PersistedEpochChains {
                    epoch: *epoch,
                    send: chains.send.as_ref().map(PersistedKdfChain::from),
                    receive: chains.receive.as_ref().map(PersistedKdfChain::from),
                })
                .collect(),
            skipped: self
                .skipped
                .iter()
                .map(|key| PersistedSkippedMessageKey {
                    epoch: key.epoch,
                    message_number: key.message_number,
                    key: SecretString::new(encode_key(&key.key)),
                })
                .collect(),
            braid: SecretString::new(URL_SAFE_NO_PAD.encode(braid.as_slice())),
        };
        let encoded = serde_json::to_vec(&persisted)
            .context("sparse PQ ratchet state serialization failed")?;
        Ok(Zeroizing::new(encoded))
    }

    pub(crate) fn restore(encoded: &[u8]) -> Result<Self> {
        ensure!(
            encoded.len() <= SPQR_MAX_PERSISTED_STATE_BYTES,
            "sparse PQ ratchet persisted state exceeds the resource limit"
        );
        let persisted: PersistedSparsePqRatchet = serde_json::from_slice(encoded)
            .context("sparse PQ ratchet persisted state is invalid")?;
        ensure!(
            persisted.revision == SPQR_PERSISTENCE_REVISION,
            "sparse PQ ratchet persisted revision is unsupported"
        );
        let role = SecureMeshSparsePqRole::from_str(&persisted.role)?;
        ensure!(
            persisted.sending_epoch <= persisted.epoch
                && persisted.receiving_epoch <= persisted.epoch,
            "sparse PQ ratchet persisted epochs are inconsistent"
        );
        ensure!(
            persisted.chains.len() <= 2 && persisted.skipped.len() <= SPQR_MAX_SKIPPED_KEYS,
            "sparse PQ ratchet persisted resources exceed bounds"
        );
        let mut chains = BTreeMap::new();
        for value in persisted.chains {
            ensure!(
                value.epoch >= persisted.epoch.saturating_sub(1) && value.epoch <= persisted.epoch,
                "sparse PQ ratchet persisted chain epoch is outside bounds"
            );
            let send = value.send.map(KdfChain::try_from).transpose()?;
            let receive = value.receive.map(KdfChain::try_from).transpose()?;
            ensure!(
                chains
                    .insert(value.epoch, EpochChains { send, receive })
                    .is_none(),
                "sparse PQ ratchet persisted chain epoch is duplicated"
            );
        }
        ensure!(
            chains
                .get(&persisted.sending_epoch)
                .and_then(|chains| chains.send.as_ref())
                .is_some()
                && chains
                    .get(&persisted.receiving_epoch)
                    .and_then(|chains| chains.receive.as_ref())
                    .is_some(),
            "sparse PQ ratchet current persisted chains are unavailable"
        );
        let skipped = persisted
            .skipped
            .into_iter()
            .map(|value| {
                ensure!(
                    value.epoch >= persisted.epoch.saturating_sub(1)
                        && value.epoch <= persisted.epoch
                        && value.message_number > 0,
                    "sparse PQ ratchet persisted skipped key is invalid"
                );
                Ok(SkippedMessageKey {
                    epoch: value.epoch,
                    message_number: value.message_number,
                    key: Zeroizing::new(decode_key(value.key.as_str())?),
                })
            })
            .collect::<Result<Vec<_>>>()?;
        ensure!(
            persisted.braid.as_str().len() <= SPQR_MAX_ENCODED_BRAID_BYTES,
            "sparse PQ ratchet persisted Braid state exceeds the resource limit"
        );
        let braid_bytes = Zeroizing::new(
            URL_SAFE_NO_PAD
                .decode(persisted.braid.as_str())
                .context("sparse PQ ratchet persisted Braid state is not base64url")?,
        );
        let canonical_braid = Zeroizing::new(URL_SAFE_NO_PAD.encode(braid_bytes.as_slice()));
        ensure!(
            canonical_braid.as_str() == persisted.braid.as_str(),
            "sparse PQ ratchet persisted Braid state encoding is non-canonical"
        );
        let braid = MlKemBraidSession::restore(braid_bytes.as_slice())?;
        ensure!(
            braid.epoch() == persisted.epoch
                || braid.epoch().checked_sub(1) == Some(persisted.epoch),
            "sparse PQ ratchet and Braid epochs are inconsistent"
        );
        Ok(Self {
            role,
            root_key: Zeroizing::new(decode_key(persisted.root_key.as_str())?),
            epoch: persisted.epoch,
            sending_epoch: persisted.sending_epoch,
            receiving_epoch: persisted.receiving_epoch,
            previous_sending_chain_length: persisted.previous_sending_chain_length,
            chains,
            skipped,
            braid,
        })
    }

    pub(crate) fn try_clone(&self) -> Result<Self> {
        Ok(Self {
            role: self.role,
            root_key: self.root_key.clone(),
            epoch: self.epoch,
            sending_epoch: self.sending_epoch,
            receiving_epoch: self.receiving_epoch,
            previous_sending_chain_length: self.previous_sending_chain_length,
            chains: self.chains.clone(),
            skipped: self.skipped.clone(),
            braid: self.braid.try_clone(),
        })
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct PersistedSparsePqRatchet {
    revision: u8,
    role: String,
    root_key: SecretString,
    epoch: u64,
    sending_epoch: u64,
    receiving_epoch: u64,
    previous_sending_chain_length: u64,
    chains: Vec<PersistedEpochChains>,
    skipped: Vec<PersistedSkippedMessageKey>,
    braid: SecretString,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct PersistedEpochChains {
    epoch: u64,
    send: Option<PersistedKdfChain>,
    receive: Option<PersistedKdfChain>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct PersistedKdfChain {
    key: SecretString,
    counter: u64,
}

impl From<&KdfChain> for PersistedKdfChain {
    fn from(value: &KdfChain) -> Self {
        Self {
            key: SecretString::new(encode_key(&value.key)),
            counter: value.counter,
        }
    }
}

impl TryFrom<PersistedKdfChain> for KdfChain {
    type Error = anyhow::Error;

    fn try_from(value: PersistedKdfChain) -> Result<Self> {
        Ok(Self {
            key: Zeroizing::new(decode_key(value.key.as_str())?),
            counter: value.counter,
        })
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct PersistedSkippedMessageKey {
    epoch: u64,
    message_number: u64,
    key: SecretString,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(transparent)]
struct SecretString(String);

impl SecretString {
    fn new(value: String) -> Self {
        Self(value)
    }

    fn as_str(&self) -> &str {
        &self.0
    }
}

impl Drop for SecretString {
    fn drop(&mut self) {
        self.0.zeroize();
    }
}

pub fn derive_hybrid_message_key(
    classical_message_key: &[u8; SPQR_KEY_BYTES],
    post_quantum_message_key: &[u8; SPQR_KEY_BYTES],
    session_binding: &[u8],
) -> Result<Zeroizing<[u8; SPQR_KEY_BYTES]>> {
    ensure!(
        !session_binding.is_empty(),
        "Triple Ratchet session binding is required"
    );
    let mut info =
        Vec::with_capacity(TRIPLE_RATCHET_PROTOCOL_INFO.len() + session_binding.len() + 4);
    info.extend_from_slice(TRIPLE_RATCHET_PROTOCOL_INFO);
    append_len_prefixed(&mut info, session_binding)?;
    let mut output = Zeroizing::new([0u8; SPQR_KEY_BYTES]);
    Hkdf::<Sha256>::new(Some(post_quantum_message_key), classical_message_key)
        .expand(&info, output.as_mut())
        .map_err(|_| anyhow!("Triple Ratchet hybrid message-key derivation failed"))?;
    Ok(output)
}

fn derive_initial_chains(
    shared_secret: &[u8; SPQR_KEY_BYTES],
) -> Result<([u8; 32], [u8; 32], [u8; 32])> {
    derive_three_keys(&[0u8; 32], shared_secret, SPQR_CHAIN_START_LABEL)
}

fn derive_added_epoch(
    root_key: &[u8; SPQR_KEY_BYTES],
    output_key: &[u8; SPQR_KEY_BYTES],
) -> Result<([u8; 32], [u8; 32], [u8; 32])> {
    derive_three_keys(root_key, output_key, SPQR_CHAIN_ADD_EPOCH_LABEL)
}

fn derive_three_keys(
    salt: &[u8; SPQR_KEY_BYTES],
    input: &[u8; SPQR_KEY_BYTES],
    label: &[u8],
) -> Result<([u8; 32], [u8; 32], [u8; 32])> {
    let mut info = Vec::with_capacity(SPQR_PROTOCOL_INFO.len() + label.len());
    info.extend_from_slice(SPQR_PROTOCOL_INFO);
    info.extend_from_slice(label);
    let mut output = Zeroizing::new([0u8; 96]);
    Hkdf::<Sha256>::new(Some(salt), input)
        .expand(&info, output.as_mut())
        .map_err(|_| anyhow!("sparse PQ ratchet root derivation failed"))?;
    let mut root = [0u8; 32];
    let mut a_to_b = [0u8; 32];
    let mut b_to_a = [0u8; 32];
    root.copy_from_slice(&output[..32]);
    a_to_b.copy_from_slice(&output[32..64]);
    b_to_a.copy_from_slice(&output[64..]);
    Ok((root, a_to_b, b_to_a))
}

fn derive_chain_step(
    chain_key: &[u8; SPQR_KEY_BYTES],
    counter: u64,
) -> Result<(
    Zeroizing<[u8; SPQR_KEY_BYTES]>,
    Zeroizing<[u8; SPQR_KEY_BYTES]>,
)> {
    let mut info = Vec::with_capacity(SPQR_PROTOCOL_INFO.len() + SPQR_CHAIN_STEP_LABEL.len() + 8);
    info.extend_from_slice(SPQR_PROTOCOL_INFO);
    info.extend_from_slice(SPQR_CHAIN_STEP_LABEL);
    info.extend_from_slice(&counter.to_be_bytes());
    let mut output = Zeroizing::new([0u8; 64]);
    Hkdf::<Sha256>::new(Some(&[0u8; 32]), chain_key)
        .expand(&info, output.as_mut())
        .map_err(|_| anyhow!("sparse PQ ratchet chain derivation failed"))?;
    let mut next = [0u8; 32];
    let mut message = [0u8; 32];
    next.copy_from_slice(&output[..32]);
    message.copy_from_slice(&output[32..]);
    Ok((Zeroizing::new(next), Zeroizing::new(message)))
}

fn encode_key(key: &[u8; SPQR_KEY_BYTES]) -> String {
    URL_SAFE_NO_PAD.encode(key)
}

fn decode_key(value: &str) -> Result<[u8; SPQR_KEY_BYTES]> {
    ensure!(
        value.len() == SPQR_KEY_BASE64URL_BYTES,
        "sparse PQ ratchet key length is invalid"
    );
    let mut key = [0u8; SPQR_KEY_BYTES];
    let decoded_len = URL_SAFE_NO_PAD
        .decode_slice(value.as_bytes(), &mut key)
        .context("sparse PQ ratchet key is not base64url")?;
    ensure!(
        decoded_len == SPQR_KEY_BYTES,
        "sparse PQ ratchet key length is invalid"
    );
    let canonical = Zeroizing::new(URL_SAFE_NO_PAD.encode(key));
    ensure!(
        canonical.as_str() == value,
        "sparse PQ ratchet key encoding is non-canonical"
    );
    Ok(key)
}

fn append_len_prefixed(out: &mut Vec<u8>, value: &[u8]) -> Result<()> {
    let length =
        u32::try_from(value.len()).map_err(|_| anyhow!("Triple Ratchet context is too large"))?;
    out.extend_from_slice(&length.to_be_bytes());
    out.extend_from_slice(value);
    Ok(())
}

#[cfg(test)]
mod tests;
