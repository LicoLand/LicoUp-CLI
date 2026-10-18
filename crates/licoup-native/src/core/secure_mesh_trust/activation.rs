//! The durable record of which device identity is active for a subject.
//!
//! [`super::policy`] is stateless and receives caller-controlled JSON, so it
//! cannot establish a trust root (`policy.rs:38-40`): it can only *reduce*
//! access. Positive authorization for a device identity therefore lives here, in
//! a locally persisted record that names the authority epoch it was accepted at
//! and the SDK-verified authority it descends from.
//!
//! # Why a new record, and why it is not a second trust dialect
//!
//! [`DeviceTrustRecord`](super::DeviceTrustRecord) is the per-peer signed trust
//! record, and it stays the only encoding of *peer trust*; nothing here re-encodes
//! it. What did not exist is the per-device answer this module owns, written by
//! `active`/`revoked` together with the authority epoch and the SDK-verified
//! authority state the admission descends from. It is kept in its own file under
//! `secure_mesh_device_trust_*` tables rather than inside the multi-peer
//! `mobileRelayE2ee.peerTrustAuthority` document, for two reasons the node's
//! scope states directly:
//!
//! * **activation and revocation are independent of peer trust.** A lost device
//!   is replaced on accepted authority with no acknowledgement and no available
//!   history from it, so the record must be writable without any peer agreeing.
//! * **a revocation is not a cleanup.** Revocation withdraws the identity from
//!   new admission; erasing what the device left behind is a separate, separately
//!   authorized intent. The table keeps them as separate columns so neither is
//!   inferred from the other.
//!
//! # What the store refuses
//!
//! * an activation of a destination identity that is already active, and an
//!   activation whose destination is the source identity itself, so new endpoint
//!   keys never derive from an imported source identity;
//! * any transition that does not advance past the epoch already persisted, so a
//!   superseded roster can never become current again;
//! * the same epoch with different content, which is a fork and not a second
//!   activation;
//! * writing through an epoch that is not the one the caller read, so two writers
//!   cannot both claim the same successor.
//!
//! The store performs no device replacement, no revocation of another device and
//! no erase. It records what an authorized flow decided, and it records a cleanup
//! intent as an intent.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow, ensure};
use rusqlite::{Connection, OptionalExtension};
use serde_json::{Value, json};
use time::{OffsetDateTime, format_description::well_known::Rfc3339};

use super::codec::hash_bytes;
use zeroize::Zeroizing;

/// The schema generation this store writes and reads.
///
/// A file written by a different generation is reset rather than migrated: the
/// record is a cache of a decision the caller can re-derive from the authority,
/// so a reset loses no authority and keeps one current format.
pub const SECURE_MESH_DEVICE_ACTIVATION_SCHEMA_VERSION: u32 = 1;

/// The protocol line identity of the records this store writes.
pub const SECURE_MESH_DEVICE_ACTIVATION_RECORD: &str = "licomesh.secure-mesh.device-activation.v1";

/// Largest number of distinct destinations one subject's ledger holds.
pub const MAX_SECURE_MESH_DEVICE_ACTIVATIONS: usize = 256;

/// Largest byte length of one persisted SDK-verified authority state.
///
/// The SDK already bounds an authority record at 65 536 bytes
/// (`src/identity.rs:687-693`); this is the same order and is checked before the
/// value is written, so a store can never hold a record the SDK would refuse.
pub const MAX_SECURE_MESH_AUTHORITY_STATE_BYTES: usize = 65_536;

/// Whether a destination identity is admitted by this ledger.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DeviceActivationState {
    /// The destination may be admitted to new sessions.
    Active,
    /// The destination is withdrawn from new admission. A result that was already
    /// authenticated for it keeps standing: a revocation governs new admissions
    /// only.
    Revoked,
}

impl DeviceActivationState {
    /// The stable, non-secret label persisted for this state.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Revoked => "revoked",
        }
    }

    /// Reads the label back, refusing anything this store did not write.
    pub fn from_str(value: &str) -> Result<Self> {
        match value {
            "active" => Ok(Self::Active),
            "revoked" => Ok(Self::Revoked),
            _ => Err(anyhow!(
                "secure mesh device activation state is not one this store writes"
            )),
        }
    }
}

/// The epoch pair one record descends from and was accepted at.
///
/// `superseded_epoch` is the epoch of the authority this record succeeds, or
/// `None` for the record that had no predecessor. Keeping both is what makes the
/// replay rule checkable later without re-reading a chain.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AuthorityPosition {
    /// The epoch this record was accepted at.
    pub accepted_epoch: u64,
    /// The epoch it supersedes, when it supersedes one.
    pub superseded_epoch: Option<u64>,
}

/// One cleanup intent recorded beside a revocation.
///
/// It is an intent, never a performed action. `requested` says the subject asked
/// for the device's leavings to be erased; `completed_at` is set only by the
/// separately authorized cleanup flow, and this store never sets it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DeviceCleanupIntent {
    /// Whether a cleanup was requested for the revoked destination.
    pub requested: bool,
    /// When the separately authorized cleanup flow completed, if it did.
    pub completed_at: Option<String>,
}

impl DeviceCleanupIntent {
    /// No cleanup was requested.
    #[must_use]
    pub const fn none() -> Self {
        Self {
            requested: false,
            completed_at: None,
        }
    }

    /// A cleanup was requested, and nothing has performed it yet.
    #[must_use]
    pub const fn requested() -> Self {
        Self {
            requested: true,
            completed_at: None,
        }
    }

    /// Whether anything is still owed for this revocation.
    #[must_use]
    pub const fn is_outstanding(&self) -> bool {
        self.requested && self.completed_at.is_none()
    }
}

/// One durable device activation or revocation for one subject.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DurableDeviceActivation {
    /// The subject whose device ledger this row belongs to.
    pub subject_identity_ref: String,
    /// The destination endpoint identity this record is about.
    pub endpoint_identity_ref: String,
    /// The endpoint identity this record replaced, when it replaced one.
    pub source_endpoint_identity_ref: Option<String>,
    /// Whether the destination is admitted.
    pub state: DeviceActivationState,
    /// The authority position this record was accepted at.
    pub position: AuthorityPosition,
    /// The SDK's digest of the authority state this record descends from.
    pub authority_state_digest: String,
    /// The SDK-verified authority state, verbatim.
    ///
    /// It is the predecessor a later transition must name. It carries public
    /// keys, endpoint references, epochs and signatures only.
    pub authority_state: Value,
    /// The RFC 3339 instant this record was written.
    pub updated_at: String,
    /// The cleanup intent recorded beside a revocation.
    pub cleanup: DeviceCleanupIntent,
    /// Monotonic per-row version, advanced by every write.
    pub state_version: u64,
}

impl DurableDeviceActivation {
    /// Whether this record admits the destination to new sessions.
    #[must_use]
    pub const fn admits_new_sessions(&self) -> bool {
        matches!(self.state, DeviceActivationState::Active)
    }

    /// Whether the destination is withdrawn from new admission.
    #[must_use]
    pub const fn is_revoked(&self) -> bool {
        matches!(self.state, DeviceActivationState::Revoked)
    }

    /// The predecessor a later transition must name, as an SDK-shaped value.
    ///
    /// The store hands back exactly the bytes it persisted, so a caller never
    /// rebuilds an authority record the SDK verified.
    #[must_use]
    pub const fn predecessor_state(&self) -> &Value {
        &self.authority_state
    }

    /// The project's own JSON projection of this record.
    ///
    /// The signed-material columns are reported as a digest, so a projection
    /// cannot become a way to re-inject an authority record the SDK never saw.
    #[must_use]
    pub fn to_json(&self) -> Value {
        json!({
            "record": SECURE_MESH_DEVICE_ACTIVATION_RECORD,
            "subjectIdentityRef": self.subject_identity_ref,
            "endpointIdentityRef": self.endpoint_identity_ref,
            "sourceEndpointIdentityRef": self.source_endpoint_identity_ref,
            "state": self.state.as_str(),
            "acceptedAuthorityEpoch": self.position.accepted_epoch,
            "supersededAuthorityEpoch": self.position.superseded_epoch,
            "authorityStateDigest": self.authority_state_digest,
            "updatedAt": self.updated_at,
            "stateVersion": self.state_version,
            "cleanup": {
                "requested": self.cleanup.requested,
                "completedAt": self.cleanup.completed_at,
                "outstanding": self.cleanup.is_outstanding(),
            }
        })
    }
}

/// What one activation or revocation attempt asked for.
///
/// `expected_epoch` is the epoch the caller read before it decided; a write only
/// succeeds while the ledger still stands there, so two writers cannot both claim
/// the same successor.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DeviceActivationRequest {
    /// The subject whose ledger is written.
    pub subject_identity_ref: String,
    /// The destination endpoint identity.
    pub endpoint_identity_ref: String,
    /// The endpoint identity being replaced, when one is.
    pub source_endpoint_identity_ref: Option<String>,
    /// Whether this activation admits or revokes the destination.
    pub state: DeviceActivationState,
    /// The authority position this record claims.
    pub position: AuthorityPosition,
    /// The SDK's digest of the authority state.
    pub authority_state_digest: String,
    /// The SDK-verified authority state, verbatim.
    pub authority_state: Value,
    /// The cleanup intent recorded beside a revocation.
    pub cleanup: DeviceCleanupIntent,
    /// The epoch the caller read before deciding.
    pub expected_epoch: Option<u64>,
}

/// Why the store refused to write.
///
/// It is a bounded, non-secret classification: no variant carries key material, a
/// signature, a credential or a payload.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DeviceActivationRefusal {
    /// The destination identity the activation names is already active.
    DestinationAlreadyActive,
    /// An activation's destination identity is the source identity itself.
    DestinationIsSource,
    /// The transition does not advance past the epoch already persisted.
    StaleEpoch,
    /// The presented epoch already exists with different content: a fork.
    EpochFork,
    /// The ledger did not stand at the epoch the caller read.
    ConcurrentWrite,
    /// The record is not readable in the shape this store writes.
    UnreadableRecord,
    /// A bound this store enforces would be exceeded.
    BoundExceeded,
}

impl DeviceActivationRefusal {
    /// The stable, non-secret code reported for this refusal.
    #[must_use]
    pub const fn code(self) -> &'static str {
        match self {
            Self::DestinationAlreadyActive => "destination_already_active",
            Self::DestinationIsSource => "destination_is_source",
            Self::StaleEpoch => "stale_epoch",
            Self::EpochFork => "epoch_fork",
            Self::ConcurrentWrite => "concurrent_write",
            Self::UnreadableRecord => "unreadable_record",
            Self::BoundExceeded => "bound_exceeded",
        }
    }

    /// The fixed, non-secret explanation reported to a person.
    #[must_use]
    pub const fn reason(self) -> &'static str {
        match self {
            Self::DestinationAlreadyActive => {
                "the destination endpoint identity is already active for this subject"
            }
            Self::DestinationIsSource => {
                "an activation may not name the source identity as its own destination"
            }
            Self::StaleEpoch => {
                "the transition does not advance past the authority epoch already persisted"
            }
            Self::EpochFork => "the presented epoch already exists with different content",
            Self::ConcurrentWrite => {
                "the device ledger did not stand at the epoch this write expected"
            }
            Self::UnreadableRecord => "the record is not readable in the shape this store writes",
            Self::BoundExceeded => "a bound this store enforces would be exceeded",
        }
    }

    /// Whether nothing at all was written by the refused call.
    ///
    /// Always `true`: every refusal is decided before the transaction commits.
    #[must_use]
    pub const fn wrote_nothing(self) -> bool {
        true
    }
}

impl core::fmt::Display for DeviceActivationRefusal {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(formatter, "{}: {}", self.code(), self.reason())
    }
}

impl std::error::Error for DeviceActivationRefusal {}

/// What one read of the ledger found.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DeviceActivationLookup {
    /// The subject's ledger holds this destination.
    Found(Box<DurableDeviceActivation>),
    /// The subject's ledger holds no record for this destination.
    Absent,
    /// The subject has written nothing at all yet.
    EmptyLedger,
}

impl DeviceActivationLookup {
    /// The record, when one was found.
    #[must_use]
    pub fn record(&self) -> Option<&DurableDeviceActivation> {
        match self {
            Self::Found(record) => Some(record),
            Self::Absent | Self::EmptyLedger => None,
        }
    }

    /// Whether a record for this destination exists at all.
    #[must_use]
    pub const fn is_present(&self) -> bool {
        matches!(self, Self::Found(_))
    }

    /// Whether the destination is currently admitted to new sessions.
    ///
    /// An absent or empty ledger admits nothing: a device identity is authorized
    /// by a record, never by the absence of one.
    #[must_use]
    pub fn admits_new_sessions(&self) -> bool {
        matches!(self, Self::Found(record) if record.admits_new_sessions())
    }
}

/// The path the device activation ledger lives at for one data root.
///
/// It sits beside the other secure-mesh state under the caller's own portable
/// root, so the record travels with the client state it describes.
#[must_use]
pub fn device_activation_ledger_path(root: &Path) -> PathBuf {
    root.join("mobile-relay").join("device-activation.sqlite3")
}

/// The durable device activation and revocation ledger.
///
/// One connection, opened per operation and dropped after, exactly like the other
/// secure-mesh durable stores. There is no process-global handle.
pub struct SecureMeshDeviceActivationStore {
    connection: Connection,
}

impl SecureMeshDeviceActivationStore {
    /// Opens the ledger at `path`, resetting an incompatible generation.
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .context("secure mesh device activation state directory creation failed")?;
        }
        let connection =
            Connection::open(path).context("secure mesh device activation store open failed")?;
        let store = Self { connection };
        store.initialize()?;
        Ok(store)
    }

    /// Opens an in-memory ledger. It persists nothing and is used by tests that
    /// exercise the store's own contracts.
    pub fn open_in_memory() -> Result<Self> {
        let connection = Connection::open_in_memory()
            .context("secure mesh device activation store open failed")?;
        let store = Self { connection };
        store.initialize()?;
        Ok(store)
    }

    /// Creates the tables, or resets a file another generation wrote.
    fn initialize(&self) -> Result<()> {
        self.connection
            .execute_batch("PRAGMA secure_delete = ON;")
            .context("secure mesh device activation secure-delete enable failed")?;
        let schema_version: u32 = self
            .connection
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .context("secure mesh device activation schema version read failed")?;
        let tables_exist: bool = self
            .connection
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type = 'table' \
                 AND name LIKE 'secure_mesh_device_trust_%')",
                [],
                |row| row.get(0),
            )
            .context("secure mesh device activation schema existence check failed")?;
        if tables_exist && schema_version != SECURE_MESH_DEVICE_ACTIVATION_SCHEMA_VERSION {
            self.connection
                .execute_batch(
                    r#"
                    DROP TABLE IF EXISTS secure_mesh_device_trust_activations;
                    DROP TABLE IF EXISTS secure_mesh_device_trust_epochs;
                    PRAGMA user_version = 0;
                    "#,
                )
                .context("secure mesh device activation incompatible state reset failed")?;
        }
        self.connection
            .execute_batch(&format!(
                r#"
                CREATE TABLE IF NOT EXISTS secure_mesh_device_trust_activations (
                    subject_identity_ref TEXT NOT NULL,
                    endpoint_identity_ref TEXT NOT NULL,
                    source_endpoint_identity_ref TEXT,
                    state TEXT NOT NULL,
                    accepted_authority_epoch INTEGER NOT NULL,
                    superseded_authority_epoch INTEGER,
                    authority_state_digest TEXT NOT NULL,
                    authority_state_json TEXT NOT NULL,
                    cleanup_requested INTEGER NOT NULL,
                    cleanup_completed_at TEXT,
                    state_version INTEGER NOT NULL,
                    updated_at TEXT NOT NULL,
                    PRIMARY KEY (subject_identity_ref, endpoint_identity_ref)
                );
                CREATE INDEX IF NOT EXISTS secure_mesh_device_trust_activations_state_idx
                    ON secure_mesh_device_trust_activations(
                        subject_identity_ref, state, accepted_authority_epoch
                    );
                CREATE TABLE IF NOT EXISTS secure_mesh_device_trust_epochs (
                    subject_identity_ref TEXT NOT NULL,
                    accepted_authority_epoch INTEGER NOT NULL,
                    endpoint_identity_ref TEXT NOT NULL,
                    authority_state_digest TEXT NOT NULL,
                    PRIMARY KEY (subject_identity_ref, accepted_authority_epoch)
                );
                PRAGMA user_version = {SECURE_MESH_DEVICE_ACTIVATION_SCHEMA_VERSION};
                "#
            ))
            .context("secure mesh device activation schema creation failed")?;
        Ok(())
    }

    /// Reads one destination's record.
    pub fn lookup(
        &self,
        subject_identity_ref: &str,
        endpoint_identity_ref: &str,
    ) -> Result<DeviceActivationLookup> {
        ensure!(
            !subject_identity_ref.trim().is_empty(),
            "secure mesh device activation subject is required"
        );
        ensure!(
            !endpoint_identity_ref.trim().is_empty(),
            "secure mesh device activation endpoint is required"
        );
        let row = self
            .connection
            .query_row(
                &lookup_sql(),
                (subject_identity_ref, endpoint_identity_ref),
                |row| Ok(read_row(row, 0)),
            )
            .optional()
            .context("secure mesh device activation read failed")?;
        let Some(record) = row
            .transpose()
            .context("secure mesh device activation row is invalid")?
        else {
            return Ok(self.empty_or_absent(subject_identity_ref)?);
        };
        Ok(DeviceActivationLookup::Found(Box::new(
            record.with_subject(subject_identity_ref, endpoint_identity_ref),
        )))
    }

    /// The highest authority epoch this subject's ledger holds.
    pub fn current_epoch(&self, subject_identity_ref: &str) -> Result<Option<u64>> {
        self.connection
            .query_row(
                "SELECT MAX(accepted_authority_epoch) FROM secure_mesh_device_trust_epochs \
                 WHERE subject_identity_ref = ?1",
                (subject_identity_ref,),
                |row| row.get::<_, Option<u64>>(0),
            )
            .context("secure mesh device activation epoch read failed")
    }

    /// Every record this subject's ledger holds, ordered by destination.
    pub fn records(&self, subject_identity_ref: &str) -> Result<Vec<DurableDeviceActivation>> {
        let mut statement = self
            .connection
            .prepare(&format!(
                "SELECT endpoint_identity_ref, {RECORD_COLUMNS} \
                 FROM secure_mesh_device_trust_activations \
                 WHERE subject_identity_ref = ?1 ORDER BY endpoint_identity_ref"
            ))
            .context("secure mesh device activation list preparation failed")?;
        let rows = statement
            .query_map((subject_identity_ref,), |row| {
                Ok((row.get::<_, String>(0)?, read_row(row, 1)))
            })
            .context("secure mesh device activation list failed")?;
        let mut records = Vec::new();
        for row in rows {
            let (endpoint, record) =
                row.context("secure mesh device activation row read failed")?;
            let record = record.context("secure mesh device activation row is invalid")?;
            records.push(record.with_subject(subject_identity_ref, &endpoint));
        }
        Ok(records)
    }

    fn empty_or_absent(&self, subject_identity_ref: &str) -> Result<DeviceActivationLookup> {
        let count: u64 = self
            .connection
            .query_row(
                "SELECT COUNT(*) FROM secure_mesh_device_trust_activations \
                 WHERE subject_identity_ref = ?1",
                (subject_identity_ref,),
                |row| row.get(0),
            )
            .context("secure mesh device activation emptiness read failed")?;
        Ok(if count == 0 {
            DeviceActivationLookup::EmptyLedger
        } else {
            DeviceActivationLookup::Absent
        })
    }

    /// Writes one activation or revocation, or refuses without writing.
    ///
    /// The whole decision and the write are one transaction: a refusal commits
    /// nothing, so a refused call is reportable as having written nothing.
    pub fn apply(
        &mut self,
        request: &DeviceActivationRequest,
    ) -> Result<DurableDeviceActivation, DeviceActivationRefusal> {
        let transaction = self
            .connection
            .transaction()
            .map_err(|_error| DeviceActivationRefusal::ConcurrentWrite)?;
        let subject = request.subject_identity_ref.trim();
        let destination = request.endpoint_identity_ref.trim();
        if subject.is_empty()
            || destination.is_empty()
            || request.authority_state_digest.trim().is_empty()
        {
            return Err(DeviceActivationRefusal::UnreadableRecord);
        }
        if request.position.accepted_epoch == 0 && request.position.superseded_epoch.is_some() {
            return Err(DeviceActivationRefusal::UnreadableRecord);
        }
        let state_json = Zeroizing::new(
            serde_json::to_string(&request.authority_state)
                .map_err(|_error| DeviceActivationRefusal::UnreadableRecord)?,
        );
        if state_json.len() > MAX_SECURE_MESH_AUTHORITY_STATE_BYTES {
            return Err(DeviceActivationRefusal::BoundExceeded);
        }
        if request.state == DeviceActivationState::Active
            && request
                .source_endpoint_identity_ref
                .as_deref()
                .is_some_and(|source| source == destination)
        {
            return Err(DeviceActivationRefusal::DestinationIsSource);
        }

        let existing = read_existing(&transaction, subject, destination)
            .map_err(|_error| DeviceActivationRefusal::UnreadableRecord)?;
        let current = current_epoch(&transaction, subject)
            .map_err(|_error| DeviceActivationRefusal::UnreadableRecord)?;
        if current != request.expected_epoch {
            return Err(DeviceActivationRefusal::ConcurrentWrite);
        }

        if let Some(existing) = existing.as_ref() {
            if existing.state == DeviceActivationState::Active
                && request.state == DeviceActivationState::Active
            {
                return Err(DeviceActivationRefusal::DestinationAlreadyActive);
            }
            if request.position.accepted_epoch < existing.position.accepted_epoch {
                return Err(DeviceActivationRefusal::StaleEpoch);
            }
            if request.position.accepted_epoch == existing.position.accepted_epoch
                && (existing.authority_state_digest != request.authority_state_digest
                    || existing.state != request.state)
            {
                return Err(DeviceActivationRefusal::EpochFork);
            }
        }
        if let Some(current) = current {
            if request.position.accepted_epoch < current {
                return Err(DeviceActivationRefusal::StaleEpoch);
            }
        }

        let row_count: u64 = transaction
            .query_row(
                "SELECT COUNT(*) FROM secure_mesh_device_trust_activations \
                 WHERE subject_identity_ref = ?1",
                (subject,),
                |row| row.get(0),
            )
            .map_err(|_error| DeviceActivationRefusal::UnreadableRecord)?;
        if row_count as usize >= MAX_SECURE_MESH_DEVICE_ACTIVATIONS && existing.is_none() {
            return Err(DeviceActivationRefusal::BoundExceeded);
        }

        let epoch_row = transaction
            .query_row(
                "SELECT endpoint_identity_ref, authority_state_digest \
                 FROM secure_mesh_device_trust_epochs \
                 WHERE subject_identity_ref = ?1 AND accepted_authority_epoch = ?2",
                (subject, request.position.accepted_epoch as i64),
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
            )
            .optional()
            .map_err(|_error| DeviceActivationRefusal::UnreadableRecord)?;
        if let Some((epoch_endpoint, epoch_digest)) = epoch_row {
            if epoch_endpoint != destination || epoch_digest != request.authority_state_digest {
                return Err(DeviceActivationRefusal::EpochFork);
            }
        }

        let updated_at = OffsetDateTime::now_utc()
            .format(&Rfc3339)
            .map_err(|_error| DeviceActivationRefusal::UnreadableRecord)?;
        let state_version = existing
            .as_ref()
            .map_or(1, |existing| existing.state_version.saturating_add(1));
        transaction
            .execute(
                "INSERT INTO secure_mesh_device_trust_activations (
                    subject_identity_ref, endpoint_identity_ref, source_endpoint_identity_ref,
                    state, accepted_authority_epoch, superseded_authority_epoch,
                    authority_state_digest, authority_state_json, cleanup_requested,
                    cleanup_completed_at, state_version, updated_at
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)
                 ON CONFLICT(subject_identity_ref, endpoint_identity_ref) DO UPDATE SET
                    source_endpoint_identity_ref = excluded.source_endpoint_identity_ref,
                    state = excluded.state,
                    accepted_authority_epoch = excluded.accepted_authority_epoch,
                    superseded_authority_epoch = excluded.superseded_authority_epoch,
                    authority_state_digest = excluded.authority_state_digest,
                    authority_state_json = excluded.authority_state_json,
                    cleanup_requested = excluded.cleanup_requested,
                    cleanup_completed_at = excluded.cleanup_completed_at,
                    state_version = excluded.state_version,
                    updated_at = excluded.updated_at",
                rusqlite::params![
                    subject,
                    destination,
                    request.source_endpoint_identity_ref.as_deref(),
                    request.state.as_str(),
                    request.position.accepted_epoch as i64,
                    request.position.superseded_epoch.map(|epoch| epoch as i64),
                    request.authority_state_digest.trim(),
                    state_json.as_str(),
                    i64::from(request.cleanup.requested),
                    request.cleanup.completed_at.as_deref(),
                    state_version as i64,
                    updated_at,
                ],
            )
            .map_err(|_error| DeviceActivationRefusal::ConcurrentWrite)?;
        transaction
            .execute(
                "INSERT INTO secure_mesh_device_trust_epochs (
                    subject_identity_ref, accepted_authority_epoch, endpoint_identity_ref,
                    authority_state_digest
                 ) VALUES (?1, ?2, ?3, ?4)
                 ON CONFLICT(subject_identity_ref, accepted_authority_epoch) DO NOTHING",
                rusqlite::params![
                    subject,
                    request.position.accepted_epoch as i64,
                    destination,
                    request.authority_state_digest.trim(),
                ],
            )
            .map_err(|_error| DeviceActivationRefusal::ConcurrentWrite)?;
        transaction
            .commit()
            .map_err(|_error| DeviceActivationRefusal::ConcurrentWrite)?;

        Ok(DurableDeviceActivation {
            subject_identity_ref: subject.to_owned(),
            endpoint_identity_ref: destination.to_owned(),
            source_endpoint_identity_ref: request.source_endpoint_identity_ref.clone(),
            state: request.state,
            position: request.position,
            authority_state_digest: request.authority_state_digest.trim().to_owned(),
            authority_state: request.authority_state.clone(),
            updated_at,
            cleanup: request.cleanup.clone(),
            state_version,
        })
    }
}

/// The digest a projection reports for the signed-material column.
///
/// It reuses the device-identity codec's own hash, so a projection of this
/// record hashes the same way every other device-identity projection does.
#[must_use]
pub fn activation_authority_digest(authority_state: &Value) -> Result<String> {
    let encoded = serde_json::to_vec(authority_state)
        .context("secure mesh device activation authority state encoding failed")?;
    Ok(hash_bytes(&encoded))
}

/// The one column order every read of this table uses.
///
/// It is a constant so a caller that consumes a leading column and a caller that
/// does not cannot disagree about where the record starts.
const RECORD_COLUMNS: &str = "source_endpoint_identity_ref, state, accepted_authority_epoch, \
     superseded_authority_epoch, authority_state_digest, authority_state_json, \
     cleanup_requested, cleanup_completed_at, state_version, updated_at";

fn lookup_sql() -> String {
    format!(
        "SELECT {RECORD_COLUMNS} FROM secure_mesh_device_trust_activations \
         WHERE subject_identity_ref = ?1 AND endpoint_identity_ref = ?2"
    )
}

/// Decodes one record, whose first `offset` columns the caller already consumed.
///
/// The decode itself is a plain `anyhow` result, so a malformed persisted row and
/// a database failure are distinguished by their context rather than by nesting.
fn read_row(row: &rusqlite::Row<'_>, offset: usize) -> anyhow::Result<DurableDeviceActivation> {
    let source: Option<String> = row.get(offset)?;
    let state: String = row.get(offset + 1)?;
    let accepted: i64 = row.get(offset + 2)?;
    let superseded: Option<i64> = row.get(offset + 3)?;
    let digest: String = row.get(offset + 4)?;
    let state_json: String = row.get(offset + 5)?;
    let cleanup_requested: i64 = row.get(offset + 6)?;
    let cleanup_completed_at: Option<String> = row.get(offset + 7)?;
    let state_version: i64 = row.get(offset + 8)?;
    let updated_at: String = row.get(offset + 9)?;
    let authority_state: Value = serde_json::from_str(&state_json)
        .context("secure mesh device activation authority state is invalid")?;
    Ok(DurableDeviceActivation {
        subject_identity_ref: String::new(),
        endpoint_identity_ref: String::new(),
        source_endpoint_identity_ref: source,
        state: DeviceActivationState::from_str(&state)?,
        position: AuthorityPosition {
            accepted_epoch: u64::try_from(accepted)
                .context("secure mesh device activation epoch is negative")?,
            superseded_epoch: superseded
                .map(u64::try_from)
                .transpose()
                .context("secure mesh device activation superseded epoch is negative")?,
        },
        authority_state_digest: digest,
        authority_state,
        updated_at,
        cleanup: DeviceCleanupIntent {
            requested: cleanup_requested != 0,
            completed_at: cleanup_completed_at,
        },
        state_version: u64::try_from(state_version)
            .context("secure mesh device activation state version is negative")?,
    })
}

impl DurableDeviceActivation {
    fn with_subject(mut self, subject_identity_ref: &str, endpoint_identity_ref: &str) -> Self {
        self.subject_identity_ref = subject_identity_ref.to_owned();
        self.endpoint_identity_ref = endpoint_identity_ref.to_owned();
        self
    }
}

fn read_existing<C: std::ops::Deref<Target = Connection>>(
    connection: &C,
    subject: &str,
    destination: &str,
) -> Result<Option<DurableDeviceActivation>> {
    let row = connection
        .query_row(&lookup_sql(), (subject, destination), |row| {
            Ok(read_row(row, 0))
        })
        .optional()?;
    let Some(record) = row else {
        return Ok(None);
    };
    Ok(Some(record?.with_subject(subject, destination)))
}

fn current_epoch<C: std::ops::Deref<Target = Connection>>(
    connection: &C,
    subject: &str,
) -> Result<Option<u64>> {
    connection
        .query_row(
            "SELECT MAX(accepted_authority_epoch) FROM secure_mesh_device_trust_epochs \
             WHERE subject_identity_ref = ?1",
            (subject,),
            |row| row.get::<_, Option<u64>>(0),
        )
        .context("secure mesh device activation epoch read failed")
}

/// Reads one activation or revocation request out of caller-supplied JSON.
///
/// The shape is one object, so a caller sends the whole decision at once and a
/// store write either happens or does not. Every value is read from the caller's
/// own evidence; the store then checks it against the ledger it holds, so a
/// malformed or inconsistent request is refused there rather than here.
pub fn device_activation_request_from_json(value: &Value) -> Result<DeviceActivationRequest> {
    let object = value
        .as_object()
        .ok_or_else(|| anyhow!("secure mesh device activation request must be an object"))?;
    let subject_identity_ref = require_json_text(object, &["subjectIdentityRef", "subject"])?;
    let endpoint_identity_ref = require_json_text(object, &["endpointIdentityRef", "endpoint"])?;
    let source_endpoint_identity_ref =
        optional_json_text(object, &["sourceEndpointIdentityRef", "sourceEndpoint"])?;
    let state = DeviceActivationState::from_str(&require_json_text(
        object,
        &["state", "activationState"],
    )?)?;
    let accepted_epoch = require_json_u64(object, &["acceptedAuthorityEpoch", "acceptedEpoch"])?;
    let superseded_epoch =
        optional_json_u64(object, &["supersededAuthorityEpoch", "supersededEpoch"])?;
    let authority_state = object
        .get("authorityState")
        .cloned()
        .ok_or_else(|| anyhow!("secure mesh device activation authority state is required"))?;
    // A caller may state the digest it read; when it does not, the store derives
    // it from the authority state, so the two can never disagree silently.
    let authority_state_digest =
        optional_json_text(object, &["authorityStateDigest", "authorityDigest"])?
            .unwrap_or_else(|| activation_authority_digest(&authority_state).unwrap_or_default());
    let expected_epoch = optional_json_u64(object, &["expectedAuthorityEpoch", "expectedEpoch"])?;
    let cleanup = match object.get("cleanup") {
        None | Some(Value::Null) => DeviceCleanupIntent::none(),
        Some(Value::Bool(requested)) => cleanup_intent(*requested, None),
        Some(Value::Object(cleanup)) => cleanup_intent(
            cleanup
                .get("requested")
                .and_then(Value::as_bool)
                .unwrap_or(false),
            cleanup
                .get("completedAt")
                .and_then(Value::as_str)
                .map(str::to_owned),
        ),
        Some(_) => return Err(anyhow!("secure mesh device activation cleanup is invalid")),
    };
    Ok(DeviceActivationRequest {
        subject_identity_ref,
        endpoint_identity_ref,
        source_endpoint_identity_ref,
        state,
        position: AuthorityPosition {
            accepted_epoch,
            superseded_epoch,
        },
        authority_state_digest,
        authority_state,
        cleanup,
        expected_epoch,
    })
}

fn cleanup_intent(requested: bool, completed_at: Option<String>) -> DeviceCleanupIntent {
    match completed_at {
        Some(completed_at) => DeviceCleanupIntent {
            requested,
            completed_at: Some(completed_at),
        },
        None => {
            if requested {
                DeviceCleanupIntent::requested()
            } else {
                DeviceCleanupIntent::none()
            }
        }
    }
}

fn require_json_text(object: &serde_json::Map<String, Value>, keys: &[&str]) -> Result<String> {
    optional_json_text(object, keys)?
        .ok_or_else(|| anyhow!("secure mesh device activation field is required"))
}

fn optional_json_text(
    object: &serde_json::Map<String, Value>,
    keys: &[&str],
) -> Result<Option<String>> {
    for key in keys {
        if let Some(value) = object.get(*key) {
            if value.is_null() {
                continue;
            }
            let text = value
                .as_str()
                .ok_or_else(|| anyhow!("secure mesh device activation field must be text"))?;
            return Ok(Some(text.to_owned()));
        }
    }
    Ok(None)
}

fn require_json_u64(object: &serde_json::Map<String, Value>, keys: &[&str]) -> Result<u64> {
    optional_json_u64(object, keys)?
        .ok_or_else(|| anyhow!("secure mesh device activation field is required"))
}

fn optional_json_u64(
    object: &serde_json::Map<String, Value>,
    keys: &[&str],
) -> Result<Option<u64>> {
    for key in keys {
        if let Some(value) = object.get(*key) {
            if value.is_null() {
                continue;
            }
            let number = value
                .as_u64()
                .ok_or_else(|| anyhow!("secure mesh device activation field must be a number"))?;
            return Ok(Some(number));
        }
    }
    Ok(None)
}

#[cfg(test)]
#[path = "activation_tests.rs"]
mod activation_tests;
