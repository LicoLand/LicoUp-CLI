//! The durable endpoint state behind one session drive.
//!
//! [`licoarc_client::FileState`] couples a live `Versioned` snapshot to the
//! last checkpoint the SDK sealed of it; this module implements the same
//! [`AtomicStatePort`] contract over [`SessionCustody`] instead of
//! `FileCustody`, so the anti-rollback checkpoint integrity key is custody-held
//! rather than a `0600` file.
//!
//! The ordering contract matches the SDK's: `compare_and_swap` commits to the
//! live value first (revision check, then the coupled custody mutations, then
//! the snapshot advance), then the caller seals the new committed snapshot
//! with [`SessionState::save`] before reporting the operation complete. A
//! crash in that window restores the previous complete checkpoint and the
//! station's redelivery replays what the lost commit received — nothing is
//! half committed and nothing is invented.

use anyhow::{Context, Result, anyhow, ensure};
use rand_core::{OsRng, RngCore};
use std::cell::{Cell, RefCell};
use std::path::{Path, PathBuf};
use std::rc::Rc;

use licoarc_client::endpoint::{
    CheckpointIntegrityKey, Endpoint, EndpointCheckpoint, EndpointState, Role,
    decode_endpoint_checkpoint,
};
use licoarc_client::ports::{
    AtomicState as AtomicStatePort, PortFailure, Revision, StateCommit, Versioned,
};
use licoarc_client::state::{
    AtomicState as SdkAtomicState, KeyCustody as SdkKeyCustody, PendingItem as SdkPendingItem,
};
use licoarc_client::{VerifiedProtocolLine, provider::RustCryptoProvider};

use super::session_custody::SessionCustody;

const CHECKPOINT_FILE: &str = "endpoint.checkpoint";
const GENERATION_FILE: &str = "endpoint.generation";
const SCOPE_FILE: &str = "endpoint.scope";

/// Atomically publishes `bytes` at `path`: temporary sibling, fsync, rename,
/// mode `0600` on unix — the same durability boundary the SDK's own
/// `FileState` uses for the checkpoint file.
fn write_atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut temporary = path.as_os_str().to_os_string();
    temporary.push(".tmp");
    let temporary = PathBuf::from(temporary);
    {
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create(true).truncate(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(&temporary)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            file.set_permissions(std::fs::Permissions::from_mode(0o600))?;
        }
        use std::io::Write;
        file.write_all(bytes)?;
        file.sync_all()?;
    }
    std::fs::rename(&temporary, path)?;
    #[cfg(unix)]
    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
    {
        std::fs::File::open(parent)?.sync_all()?;
    }
    Ok(())
}

/// One endpoint's durable state: the live snapshot plus its sealed image.
///
/// `Clone` shares the same live snapshot — the `AtomicState` adapter sees the
/// same `Versioned` value the caller's `save` just sealed.
pub(crate) struct SessionState {
    live: Rc<RefCell<Versioned<EndpointState, SdkPendingItem>>>,
    custody: SessionCustody,
    checkpoint: PathBuf,
    generation: PathBuf,
    integrity: Rc<CheckpointIntegrityKey>,
    scope: Rc<[u8; 32]>,
    minimum: Rc<Cell<u64>>,
}

impl Clone for SessionState {
    fn clone(&self) -> Self {
        Self {
            live: Rc::clone(&self.live),
            custody: self.custody.clone(),
            checkpoint: self.checkpoint.clone(),
            generation: self.generation.clone(),
            integrity: self.integrity.clone(),
            scope: self.scope.clone(),
            minimum: self.minimum.clone(),
        }
    }
}

impl SessionState {
    /// The responder endpoint state at `dir`; without a checkpoint the state
    /// starts in the `ResponderReady` phase.
    pub(crate) fn responder(dir: &Path, line: &VerifiedProtocolLine) -> Result<Self> {
        Self::open(dir, line, EndpointState::responder)
    }

    /// The initiator endpoint state at `dir`; without a checkpoint the state
    /// starts in the `InitiatorNew` phase.
    pub(crate) fn initiator(dir: &Path, line: &VerifiedProtocolLine) -> Result<Self> {
        Self::open(dir, line, EndpointState::initiator)
    }

    fn open(dir: &Path, line: &VerifiedProtocolLine, fresh: fn() -> EndpointState) -> Result<Self> {
        let checkpoint = dir.join(CHECKPOINT_FILE);
        let bytes = match std::fs::read(&checkpoint) {
            Ok(bytes) => Some(bytes),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(_) => return Err(anyhow!("devices_session_state_unavailable")),
        };
        let mut custody =
            SessionCustody::open(dir).context("devices_session_custody_unavailable")?;
        let integrity = Rc::new(
            custody
                .integrity_key(bytes.is_some())
                .map_err(|_| anyhow!("devices_session_custody_unavailable"))?,
        );
        let scope = Rc::new(endpoint_scope(&dir.join(SCOPE_FILE), bytes.is_some())?);
        let generation = dir.join(GENERATION_FILE);
        let minimum = match bytes {
            Some(_) => generation_floor(&generation)?,
            None => 0,
        };
        let live = match bytes {
            Some(bytes) => {
                let sealed = EndpointCheckpoint::from_secret_bytes(bytes)
                    .map_err(|_| anyhow!("devices_session_state_unavailable"))?;
                let restored = decode_endpoint_checkpoint(
                    &RustCryptoProvider,
                    line,
                    &sealed,
                    &integrity,
                    &scope[..],
                )
                .map_err(|_| anyhow!("devices_session_state_unavailable"))?;
                let sealed = restored
                    .into_versioned()
                    .map_err(|_| anyhow!("devices_session_state_unavailable"))?;
                Versioned::restored(
                    Revision::from_value(sealed.revision().value()),
                    sealed.state().clone(),
                    sealed.pending().to_vec(),
                )
            }
            None => Versioned::initial(fresh()),
        };
        Ok(Self {
            live: Rc::new(RefCell::new(live)),
            custody,
            checkpoint,
            generation,
            integrity,
            scope,
            minimum: Rc::new(Cell::new(minimum)),
        })
    }

    /// The coupled custody, for the custody adapter and secret staging.
    #[must_use]
    pub(crate) const fn custody(&self) -> &SessionCustody {
        &self.custody
    }

    #[allow(dead_code)]
    pub(crate) fn custody_mut(&mut self) -> &mut SessionCustody {
        &mut self.custody
    }

    /// Seals the endpoint's committed snapshot into the checkpoint file.
    /// Call it after every state-advancing operation so a restart restores
    /// exactly the last committed snapshot.
    pub(crate) fn save<R, EP, C, S>(&self, endpoint: &Endpoint<R, EP, C, S>) -> Result<()>
    where
        R: Role,
        EP: licoarc_client::provider::Provider,
        C: SdkKeyCustody,
        S: SdkAtomicState<EndpointState>,
    {
        let checkpoint = endpoint
            .checkpoint(&self.integrity, &self.scope[..])
            .map_err(|_| anyhow!("devices_session_state_unavailable"))?;
        write_atomic(&self.checkpoint, checkpoint.expose_secret_bytes())?;
        // The high-water mark is published after the checkpoint it names.
        let generation = self
            .load()
            .map_err(|_| anyhow!("devices_session_state_unavailable"))?
            .revision()
            .value();
        write_atomic(&self.generation, &generation.to_be_bytes())?;
        self.minimum.set(generation);
        Ok(())
    }

    /// The persisted generation floor the endpoint must not resume below.
    pub(crate) fn minimum_generation(&self) -> u64 {
        self.minimum.get()
    }
}

/// Reads the endpoint-scope identifier, minting it on a fresh endpoint. The
/// scope is not a secret — the checkpoint MAC keeps it honest — but it must be
/// stable and unique per endpoint.
fn endpoint_scope(path: &PathBuf, checkpoint_present: bool) -> Result<[u8; 32]> {
    match std::fs::read(path) {
        Ok(bytes) => bytes
            .try_into()
            .map_err(|_| anyhow!("devices_session_state_unavailable")),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            ensure!(!checkpoint_present, "devices_session_state_unavailable");
            let mut bytes = [0_u8; 32];
            OsRng.fill_bytes(&mut bytes);
            write_atomic(path, &bytes)?;
            Ok(bytes)
        }
        Err(_) => Err(anyhow!("devices_session_state_unavailable")),
    }
}

/// The anti-rollback floor `Endpoint::resume` enforces; a checkpoint without
/// its mark fails closed.
fn generation_floor(path: &PathBuf) -> Result<u64> {
    match std::fs::read(path) {
        Ok(bytes) => {
            let bytes: [u8; 8] = bytes
                .try_into()
                .map_err(|_| anyhow!("devices_session_state_unavailable"))?;
            Ok(u64::from_be_bytes(bytes))
        }
        Err(_) => Err(anyhow!("devices_session_state_unavailable")),
    }
}

impl AtomicStatePort for SessionState {
    type Snapshot = EndpointState;
    type Pending = SdkPendingItem;

    fn load(&self) -> Result<Versioned<EndpointState, SdkPendingItem>, PortFailure> {
        Ok(self.live.borrow().clone())
    }

    fn compare_and_swap(
        &mut self,
        expected: Revision,
        commit: StateCommit<EndpointState, SdkPendingItem>,
    ) -> Result<Revision, PortFailure> {
        if expected != self.live.borrow().revision() {
            return Err(PortFailure::Conflict);
        }
        // Refuse the overflow before touching custody: a revision that cannot
        // publish must not leave adopted secrets behind.
        expected.successor()?;
        self.custody.apply_mutations(commit.key_mutations())?;
        let next = commit.into_versioned(expected)?;
        let revision = next.revision();
        *self.live.borrow_mut() = next;
        Ok(revision)
    }

    fn settle(
        &mut self,
        revision: Revision,
        pending: &SdkPendingItem,
    ) -> Result<Revision, PortFailure> {
        let mut current = self.live.borrow_mut();
        if revision != current.revision() {
            return Err(PortFailure::Conflict);
        }
        let mut remaining = current.pending().to_vec();
        let Some(index) = remaining.iter().position(|item| item == pending) else {
            return Err(PortFailure::Unavailable);
        };
        remaining.remove(index);
        let commit = StateCommit::new(current.state().clone(), Vec::new(), remaining);
        *current = commit.into_versioned(revision)?;
        Ok(current.revision())
    }
}
