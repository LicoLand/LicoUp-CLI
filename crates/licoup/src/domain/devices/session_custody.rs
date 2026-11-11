//! The session key custody: LicoArc `KeyCustody` over the OS secret store.
//!
//! [`licoarc_client::FileCustody`] is the client's development backend: it
//! keeps raw private keys in a plain file, which is not a product keystore.
//! This module implements the same [`KeyCustodyPort`] and
//! [`adapters::CustodyHandles`] contracts with the identical old-or-new
//! semantics — one serialized custody table per endpoint — but the table lives
//! in the platform secret store (macOS Keychain, Linux Secret Service,
//! Windows DPAPI) under the same user-presence policy as the mailbox device
//! key, never in a file under the data home.
//!
//! One [`SessionCustody`] instance is bound to one endpoint directory: all
//! signing keys, staged and adopted agreement keys and the checkpoint
//! integrity key of that endpoint ride in its single store item, so `unpair`
//! or `pair cancel` removes every trace by deleting that item.

use anyhow::{Result, anyhow, ensure};
use rand_core::{OsRng, RngCore};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::cell::RefCell;
use std::collections::BTreeMap;
use std::path::Path;
use std::rc::Rc;
use std::sync::Arc;
use zeroize::Zeroizing;

use licoarc_client::endpoint::CheckpointIntegrityKey;
use licoarc_client::ports::{
    CustodyHandle, CustodyLifecycle, CustodyPurpose, KeyCustody as KeyCustodyPort, KeyMutation,
    PortFailure,
};
use licoarc_client::provider::{AgreementProvider, RustCryptoProvider, SignatureProvider};

use crate::core::secret_store::{
    SecretBytes, SecretStoreAuthorizationRequest, SecretStoreCallerChannel, SecretStoreHandle,
    SecretStoreKeyClass,
};
use crate::platform::secret_store::SecretStore;

use super::custody::session_secret_store;

const ACCOUNT_PREFIX: &str = "devices:session";
const CUSTODY_KEY: &str = "custody";

fn secret_bytes(purpose: CustodyPurpose) -> usize {
    match purpose {
        CustodyPurpose::Ed25519Signing
        | CustodyPurpose::MlDsa65Signing
        | CustodyPurpose::X25519Private
        | CustodyPurpose::MlKemEncapsulationEntropy => 32,
        CustodyPurpose::MlKem768Private => 64,
    }
}

fn purpose_name(purpose: CustodyPurpose) -> &'static str {
    match purpose {
        CustodyPurpose::Ed25519Signing => "ed25519-signing",
        CustodyPurpose::MlDsa65Signing => "ml-dsa-65-signing",
        CustodyPurpose::X25519Private => "x25519-private",
        CustodyPurpose::MlKem768Private => "ml-kem-768-private",
        CustodyPurpose::MlKemEncapsulationEntropy => "ml-kem-encapsulation-entropy",
    }
}

fn name_purpose(name: &str) -> Option<CustodyPurpose> {
    Some(match name {
        "ed25519-signing" => CustodyPurpose::Ed25519Signing,
        "ml-dsa-65-signing" => CustodyPurpose::MlDsa65Signing,
        "x25519-private" => CustodyPurpose::X25519Private,
        "ml-kem-768-private" => CustodyPurpose::MlKem768Private,
        "ml-kem-encapsulation-entropy" => CustodyPurpose::MlKemEncapsulationEntropy,
        _ => return None,
    })
}

fn name_lifecycle(name: &str) -> Option<CustodyLifecycle> {
    Some(match name {
        "adopted" => CustodyLifecycle::Adopted,
        "staged" => CustodyLifecycle::Staged,
        _ => return None,
    })
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn unhex(text: &str) -> Option<Vec<u8>> {
    if !text.len().is_multiple_of(2) {
        return None;
    }
    (0..text.len())
        .step_by(2)
        .map(|at| u8::from_str_radix(&text[at..at + 2], 16).ok())
        .collect()
}

struct Entry {
    purpose: CustodyPurpose,
    lifecycle: CustodyLifecycle,
    secret: Zeroizing<Vec<u8>>,
}

#[derive(Default)]
struct Table {
    objects: BTreeMap<u128, Entry>,
    integrity_key: Option<[u8; 32]>,
}

impl Table {
    fn clone_objects(&self) -> Self {
        Self {
            objects: self
                .objects
                .iter()
                .map(|(token, entry)| {
                    (
                        *token,
                        Entry {
                            purpose: entry.purpose,
                            lifecycle: entry.lifecycle,
                            secret: Zeroizing::new(entry.secret.to_vec()),
                        },
                    )
                })
                .collect(),
            integrity_key: self.integrity_key,
        }
    }
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct StoredObject {
    token: String,
    purpose: String,
    lifecycle: String,
    secret: String,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct StoredTable {
    objects: Vec<StoredObject>,
    #[serde(default)]
    integrity_key: Option<String>,
}

fn encode_table(table: &Table) -> Zeroizing<Vec<u8>> {
    let objects = table
        .objects
        .iter()
        .map(|(token, entry)| StoredObject {
            token: hex(&token.to_le_bytes()),
            purpose: purpose_name(entry.purpose).to_owned(),
            lifecycle: match entry.lifecycle {
                CustodyLifecycle::Adopted => "adopted".to_owned(),
                CustodyLifecycle::Staged => "staged".to_owned(),
            },
            secret: hex(&entry.secret),
        })
        .collect();
    Zeroizing::new(
        serde_json::to_vec(&StoredTable {
            objects,
            integrity_key: table.integrity_key.map(|key| hex(&key)),
        })
        .expect("the custody table shape serializes"),
    )
}

fn decode_table(bytes: &[u8]) -> Option<Table> {
    let stored: StoredTable = serde_json::from_slice(bytes).ok()?;
    let mut table = Table::default();
    for object in stored.objects {
        let token_bytes = unhex(&object.token)?;
        let token_array = <[u8; 16]>::try_from(token_bytes.as_slice()).ok()?;
        let token = u128::from_le_bytes(token_array);
        let purpose = name_purpose(&object.purpose)?;
        let lifecycle = name_lifecycle(&object.lifecycle)?;
        let secret = unhex(&object.secret)?;
        if secret.len() != secret_bytes(purpose) {
            return None;
        }
        table.objects.insert(
            token,
            Entry {
                purpose,
                lifecycle,
                secret: Zeroizing::new(secret),
            },
        );
    }
    table.integrity_key = match stored.integrity_key {
        Some(text) => {
            let bytes = unhex(&text)?;
            Some(<[u8; 32]>::try_from(bytes.as_slice()).ok()?)
        }
        None => None,
    };
    Some(table)
}

/// One endpoint's custody backend over the OS secret store.
///
/// `Clone` shares one in-memory table view with the coupled store, exactly as
/// `FileCustody` clones share the index-backed table, so the custody adapter
/// and the coupled `AtomicState` see the same objects the protocol commits
/// mutations against.
pub(crate) struct SessionCustody {
    table: Rc<RefCell<Table>>,
    store: Arc<dyn SecretStore>,
    handle: SecretStoreHandle,
    provider: RustCryptoProvider,
}

impl Clone for SessionCustody {
    fn clone(&self) -> Self {
        Self {
            table: Rc::clone(&self.table),
            store: Arc::clone(&self.store),
            handle: self.handle.clone(),
            provider: RustCryptoProvider,
        }
    }
}

impl SessionCustody {
    /// Opens the custody table for one endpoint directory.
    ///
    /// The store item's namespace is derived from the directory's digest, so
    /// two endpoints never share secrets; the data home itself only ever holds
    /// the opaque handle. A malformed stored table fails closed.
    pub(crate) fn open(dir: &Path) -> Result<Self> {
        let digest = Sha256::digest(dir.to_string_lossy().as_bytes());
        let namespace = format!("{ACCOUNT_PREFIX}:{}", hex(&digest[..12]));
        let handle = SecretStoreHandle::new(namespace, CUSTODY_KEY)
            .map_err(|_| anyhow!("devices_session_custody_unavailable"))?;
        let store = session_secret_store();
        ensure!(store.supported(), "devices_session_custody_unsupported");
        let table = match store
            .get_secret(&handle)
            .map_err(|_| anyhow!("devices_session_custody_unavailable"))?
        {
            Some(secret) => decode_table(secret.expose_bytes())
                .ok_or_else(|| anyhow!("devices_session_custody_invalid"))?,
            None => Table::default(),
        };
        Ok(Self {
            table: Rc::new(RefCell::new(table)),
            store,
            handle,
            provider: RustCryptoProvider,
        })
    }

    fn request(reason: &str, operations: usize) -> SecretStoreAuthorizationRequest {
        SecretStoreAuthorizationRequest::for_scope(
            reason,
            operations,
            super::custody::session_user_presence_required(),
            SecretStoreKeyClass::DeviceIdentity,
            SecretStoreCallerChannel::NativeCli,
        )
    }

    /// Atomically publishes `table` into the secret store item.
    fn persist(&self, table: &Table) -> Result<(), PortFailure> {
        let session = self
            .store
            .begin_authorized_session(&Self::request(
                "Use this device's LicoUp session keys",
                table.objects.len().max(1),
            ))
            .map_err(|_| PortFailure::Unavailable)?;
        let secret = SecretBytes::try_from_bytes(encode_table(table).to_vec())
            .map_err(|_| PortFailure::Unavailable)?;
        self.store
            .set_secret_with_session(&session, &self.handle, secret)
            .map_err(|_| PortFailure::Unavailable)
    }

    /// Inserts one fresh adopted secret of `purpose` (signing identities,
    /// encapsulation entropy).
    pub(crate) fn adopt(&mut self, purpose: CustodyPurpose) -> Result<CustodyHandle> {
        let staged = matches!(
            purpose,
            CustodyPurpose::X25519Private | CustodyPurpose::MlKem768Private
        );
        ensure!(!staged, "devices_session_custody_refused");
        self.insert(purpose, CustodyLifecycle::Adopted)
    }

    /// Inserts one fresh staged private secret (X25519 or ML-KEM-768).
    pub(crate) fn stage(&mut self, purpose: CustodyPurpose) -> Result<CustodyHandle> {
        let adoptable = matches!(
            purpose,
            CustodyPurpose::X25519Private | CustodyPurpose::MlKem768Private
        );
        ensure!(adoptable, "devices_session_custody_refused");
        self.insert(purpose, CustodyLifecycle::Staged)
    }

    fn insert(
        &mut self,
        purpose: CustodyPurpose,
        lifecycle: CustodyLifecycle,
    ) -> Result<CustodyHandle> {
        let mut secret = Zeroizing::new(vec![0_u8; secret_bytes(purpose)]);
        OsRng.fill_bytes(&mut secret[..]);
        let mut token_bytes = [0_u8; 16];
        OsRng.fill_bytes(&mut token_bytes);
        let token = u128::from_le_bytes(token_bytes);
        let handle = match lifecycle {
            CustodyLifecycle::Adopted => CustodyHandle::adopted(token, purpose),
            CustodyLifecycle::Staged => CustodyHandle::staged(token, purpose),
        };
        let mut next = self.table.borrow().clone_objects();
        next.objects.insert(
            token,
            Entry {
                purpose,
                lifecycle,
                secret,
            },
        );
        self.persist(&next)
            .map_err(|_| anyhow!("devices_session_custody_unavailable"))?;
        self.table.borrow_mut().objects = next.objects;
        Ok(handle)
    }

    /// Applies one commit's custody mutations atomically; the coupled store
    /// calls this inside `compare_and_swap`.
    pub(crate) fn apply_mutations(&mut self, mutations: &[KeyMutation]) -> Result<(), PortFailure> {
        let mut next = self.table.borrow().clone_objects();
        for mutation in mutations {
            match mutation {
                KeyMutation::Adopt(handle) => {
                    let purpose = handle.purpose();
                    if !matches!(
                        purpose,
                        CustodyPurpose::X25519Private | CustodyPurpose::MlKem768Private
                    ) {
                        return Err(PortFailure::Unavailable);
                    }
                    let entry = next
                        .objects
                        .get_mut(&handle.custody_token())
                        .filter(|entry| {
                            entry.purpose == purpose && entry.lifecycle == CustodyLifecycle::Staged
                        })
                        .ok_or(PortFailure::Unavailable)?;
                    entry.lifecycle = CustodyLifecycle::Adopted;
                }
                KeyMutation::Delete(handle) => {
                    let purpose = handle.purpose();
                    if !matches!(
                        purpose,
                        CustodyPurpose::X25519Private
                            | CustodyPurpose::MlKem768Private
                            | CustodyPurpose::MlKemEncapsulationEntropy
                    ) {
                        return Err(PortFailure::Unavailable);
                    }
                    match next.objects.get(&handle.custody_token()) {
                        Some(entry)
                            if entry.purpose == purpose
                                && entry.lifecycle == CustodyLifecycle::Adopted =>
                        {
                            next.objects.remove(&handle.custody_token());
                        }
                        _ => return Err(PortFailure::Unavailable),
                    }
                }
            }
        }
        self.persist(&next)?;
        self.table.borrow_mut().objects = next.objects;
        Ok(())
    }

    /// The checkpoint integrity key this endpoint's [`super::session_state`]
    /// authenticates its sealed state with. It lives inside the same store
    /// item as the custody objects, so it never sits in a plain file either.
    pub(crate) fn integrity_key(
        &mut self,
        checkpoint_present: bool,
    ) -> Result<CheckpointIntegrityKey, PortFailure> {
        if let Some(key) = self.table.borrow().integrity_key {
            return Ok(CheckpointIntegrityKey::from_secret_bytes(key));
        }
        if checkpoint_present {
            return Err(PortFailure::Unavailable);
        }
        let mut key = [0_u8; 32];
        OsRng.fill_bytes(&mut key);
        let mut next = self.table.borrow().clone_objects();
        next.integrity_key = Some(key);
        self.persist(&next)?;
        self.table.borrow_mut().integrity_key = Some(key);
        Ok(CheckpointIntegrityKey::from_secret_bytes(key))
    }

    /// Removes the whole custody item; all staged and adopted secrets of this
    /// endpoint are gone.
    pub(crate) fn destroy(&self) -> Result<()> {
        let session = self.store.begin_authorized_session(&Self::request(
            "Remove this device's retired LicoUp session keys",
            1,
        ))?;
        self.store
            .delete_secret_with_session(&session, &self.handle)
            .map_err(|_| anyhow!("devices_session_custody_unavailable"))
    }

    fn secret(
        &self,
        handle: CustodyHandle,
        purpose: CustodyPurpose,
        lifecycle: CustodyLifecycle,
    ) -> Result<Zeroizing<Vec<u8>>, PortFailure> {
        let table = self.table.borrow();
        let entry = table
            .objects
            .get(&handle.custody_token())
            .filter(|entry| entry.purpose == purpose && entry.lifecycle == lifecycle)
            .filter(|_| handle.purpose() == purpose && handle.lifecycle() == lifecycle)
            .ok_or(PortFailure::Unavailable)?;
        Ok(entry.secret.clone())
    }

    fn private(
        &self,
        handle: CustodyHandle,
        purpose: CustodyPurpose,
    ) -> Result<Zeroizing<Vec<u8>>, PortFailure> {
        self.secret(handle, purpose, handle.lifecycle())
    }

    fn adopted_secret(
        &self,
        handle: CustodyHandle,
        purpose: CustodyPurpose,
    ) -> Result<Zeroizing<Vec<u8>>, PortFailure> {
        self.secret(handle, purpose, CustodyLifecycle::Adopted)
    }

    fn abort(&mut self, staged: CustodyHandle, purpose: CustodyPurpose) {
        let token = staged.custody_token();
        let mut next = self.table.borrow().clone_objects();
        let staged_present = next.objects.get(&token).is_some_and(|entry| {
            entry.purpose == purpose && entry.lifecycle == CustodyLifecycle::Staged
        });
        if !staged_present {
            return;
        }
        next.objects.remove(&token);
        if self.persist(&next).is_ok() {
            self.table.borrow_mut().objects = next.objects;
        }
    }
}

impl licoarc_client::adapters::CustodyHandles for SessionCustody {
    fn custody_handle(
        &self,
        token: u128,
        purpose: CustodyPurpose,
        lifecycle: CustodyLifecycle,
    ) -> Result<CustodyHandle, PortFailure> {
        let table = self.table.borrow();
        let entry = table.objects.get(&token).ok_or(PortFailure::Unavailable)?;
        if entry.purpose != purpose || entry.lifecycle != lifecycle {
            return Err(PortFailure::Unavailable);
        }
        Ok(match lifecycle {
            CustodyLifecycle::Adopted => CustodyHandle::adopted(token, purpose),
            CustodyLifecycle::Staged => CustodyHandle::staged(token, purpose),
        })
    }
}

impl KeyCustodyPort for SessionCustody {
    fn ed25519_public(&self, handle: CustodyHandle) -> Result<[u8; 32], PortFailure> {
        let seed = self.adopted_secret(handle, CustodyPurpose::Ed25519Signing)?;
        let seed: Zeroizing<[u8; 32]> = Zeroizing::new(
            seed.as_slice()
                .try_into()
                .map_err(|_| PortFailure::Unavailable)?,
        );
        Ok(self.provider.ed25519_public(&seed))
    }

    fn ed25519_sign(&self, handle: CustodyHandle, message: &[u8]) -> Result<[u8; 64], PortFailure> {
        let seed = self.adopted_secret(handle, CustodyPurpose::Ed25519Signing)?;
        let seed: Zeroizing<[u8; 32]> = Zeroizing::new(
            seed.as_slice()
                .try_into()
                .map_err(|_| PortFailure::Unavailable)?,
        );
        Ok(self.provider.ed25519_sign(&seed, message))
    }

    fn ml_dsa_65_public(&self, handle: CustodyHandle) -> Result<Vec<u8>, PortFailure> {
        let seed = self.adopted_secret(handle, CustodyPurpose::MlDsa65Signing)?;
        let seed: Zeroizing<[u8; 32]> = Zeroizing::new(
            seed.as_slice()
                .try_into()
                .map_err(|_| PortFailure::Unavailable)?,
        );
        Ok(self.provider.ml_dsa_65_public(&seed))
    }

    fn ml_dsa_65_sign(
        &self,
        handle: CustodyHandle,
        message: &[u8],
    ) -> Result<Vec<u8>, PortFailure> {
        let seed = self.adopted_secret(handle, CustodyPurpose::MlDsa65Signing)?;
        let seed: Zeroizing<[u8; 32]> = Zeroizing::new(
            seed.as_slice()
                .try_into()
                .map_err(|_| PortFailure::Unavailable)?,
        );
        Ok(self.provider.ml_dsa_65_sign(&seed, message))
    }

    fn x25519_public(&self, handle: CustodyHandle) -> Result<[u8; 32], PortFailure> {
        let private = self.private(handle, CustodyPurpose::X25519Private)?;
        let private: Zeroizing<[u8; 32]> = Zeroizing::new(
            private
                .as_slice()
                .try_into()
                .map_err(|_| PortFailure::Unavailable)?,
        );
        Ok(self.provider.x25519_public(&private))
    }

    fn x25519(&self, handle: CustodyHandle, public: &[u8; 32]) -> Result<[u8; 32], PortFailure> {
        let private = self.private(handle, CustodyPurpose::X25519Private)?;
        let private: Zeroizing<[u8; 32]> = Zeroizing::new(
            private
                .as_slice()
                .try_into()
                .map_err(|_| PortFailure::Unavailable)?,
        );
        self.provider
            .x25519(&private, public)
            .map_err(|_| PortFailure::Unavailable)
    }

    fn ml_kem_768_public(&self, handle: CustodyHandle) -> Result<Vec<u8>, PortFailure> {
        let seed = self.private(handle, CustodyPurpose::MlKem768Private)?;
        let seed: Zeroizing<[u8; 64]> = Zeroizing::new(
            seed.as_slice()
                .try_into()
                .map_err(|_| PortFailure::Unavailable)?,
        );
        Ok(self.provider.ml_kem_768_public(&seed))
    }

    fn ml_kem_768_encapsulate(
        &self,
        public: &[u8],
        entropy: CustodyHandle,
    ) -> Result<(Vec<u8>, [u8; 32]), PortFailure> {
        let seed = self.adopted_secret(entropy, CustodyPurpose::MlKemEncapsulationEntropy)?;
        let seed: Zeroizing<[u8; 32]> = Zeroizing::new(
            seed.as_slice()
                .try_into()
                .map_err(|_| PortFailure::Unavailable)?,
        );
        self.provider
            .ml_kem_768_encapsulate(public, &seed)
            .map_err(|_| PortFailure::Unavailable)
    }

    fn ml_kem_768_decapsulate(
        &self,
        handle: CustodyHandle,
        ciphertext: &[u8],
    ) -> Result<[u8; 32], PortFailure> {
        let seed = self.private(handle, CustodyPurpose::MlKem768Private)?;
        let seed: Zeroizing<[u8; 64]> = Zeroizing::new(
            seed.as_slice()
                .try_into()
                .map_err(|_| PortFailure::Unavailable)?,
        );
        self.provider
            .ml_kem_768_decapsulate(&seed, ciphertext)
            .map_err(|_| PortFailure::Unavailable)
    }

    fn abort_x25519(&mut self, staged: CustodyHandle) {
        self.abort(staged, CustodyPurpose::X25519Private);
    }

    fn abort_ml_kem_768(&mut self, staged: CustodyHandle) {
        self.abort(staged, CustodyPurpose::MlKem768Private);
    }
}
