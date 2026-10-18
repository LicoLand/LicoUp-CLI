use std::collections::BTreeSet;
use std::num::NonZeroUsize;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use thiserror::Error;

use super::{
    policy::{HistoryKeySource, ObjectOpenError, StorageMode, open_history_object},
    recovery_material::{RecoveryMaterialError, RecoveryPackage, RecoverySecret},
    store::{
        AuthorizedHistoryStore, ContentKind, HistoryStoreError, ManifestEntry, ObjectId,
        freeze_inventory,
    },
};

/// One decoded retained-history object on its way to a destination owner.
///
/// The identity travels with the page: an owner that stages a bounded page has
/// to know which retained object it is writing, because the destination is
/// verified by reading those objects back by identity.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RecoveredHistoryObject {
    pub object_id: ObjectId,
    pub content_kind: ContentKind,
    pub storage_mode: StorageMode,
    pub canonical_bytes: Vec<u8>,
    pub semantic_value: Value,
}

pub trait RetainedContentDecoder: Sync {
    fn decode_semantics(&self, kind: ContentKind, canonical_bytes: &[u8]) -> Option<Value>;
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FreshSessionRequirement {
    Required,
}

/// One destination owner whose part of a replacement is staged on its own.
///
/// The filesystem payload, the database stores and the platform credentials are
/// three owners with three separate durability domains. They are ordered because
/// each one reads what the previous one wrote, and they are *not* one
/// transaction: an interruption between them is an ordinary outcome that the
/// next run resumes, not a rollback.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum RecoveryOwner {
    /// The extracted payload staged on the destination filesystem.
    FilesystemPayload,
    /// The database stores opened and read back from the staged payload.
    DatabaseStores,
    /// The credential material this device's own custody must hold.
    PlatformCredentials,
}

impl RecoveryOwner {
    /// Every required owner, in the order they must verify.
    pub const ALL: [Self; 3] = [
        Self::FilesystemPayload,
        Self::DatabaseStores,
        Self::PlatformCredentials,
    ];

    /// Whether this owner consumes the retained history objects themselves.
    #[must_use]
    pub const fn consumes_history(self) -> bool {
        matches!(self, Self::FilesystemPayload | Self::DatabaseStores)
    }

    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::FilesystemPayload => "filesystem-payload",
            Self::DatabaseStores => "database-stores",
            Self::PlatformCredentials => "platform-credentials",
        }
    }
}

/// The owners a target recorded as verified before it last advanced.
///
/// It is recoverable progress, not authority: a target that lost this record
/// re-verifies from the first owner, and a record that is not a prefix of
/// [`RecoveryOwner::ALL`] is refused rather than trusted.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RecoveryProgress {
    verified: BTreeSet<RecoveryOwner>,
}

impl RecoveryProgress {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    #[must_use]
    pub fn is_verified(&self, owner: RecoveryOwner) -> bool {
        self.verified.contains(&owner)
    }

    /// Whether this destination recorded nothing at all, which is a first run
    /// rather than a resume.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.verified.is_empty()
    }

    /// The next owner that still has to verify, or `None` when all of them did.
    #[must_use]
    pub fn next_owner(&self) -> Option<RecoveryOwner> {
        RecoveryOwner::ALL
            .into_iter()
            .find(|owner| !self.is_verified(*owner))
    }

    /// Whether the record is a prefix of the required order. A gap, or an owner
    /// recorded out of order, means the record cannot be resumed from.
    #[must_use]
    pub fn is_resumable(&self) -> bool {
        let mut prefix = true;
        for owner in RecoveryOwner::ALL {
            if self.is_verified(owner) {
                if !prefix {
                    return false;
                }
            } else {
                prefix = false;
            }
        }
        true
    }

    /// Records `owner` as verified. The caller persists the record before the
    /// next owner runs, so an interruption resumes here instead of starting over.
    pub fn record(&mut self, owner: RecoveryOwner) {
        self.verified.insert(owner);
    }

    #[must_use]
    pub fn verified_owners(&self) -> impl Iterator<Item = RecoveryOwner> + '_ {
        self.verified.iter().copied()
    }
}

/// Caller-owned bridge to the pinned LicoArc authority implementation and to the
/// destination's own owners.
///
/// There is deliberately no method here that commits the filesystem, the
/// databases and the credentials together, and none that reports an earlier
/// cross-store write as reversible. Each owner is staged and verified on its own;
/// the target persists recoverable progress before the caller advances; and the
/// single commit boundary at the end selects the verified destination and the
/// prepared identity. An interruption is resumed from the recorded owner, and a
/// destination that cannot continue discards exactly the staging that record does
/// not cover, never the verified work behind it and never rolled back as a whole.
///
/// The staging a recorded owner covers is therefore durable state, not scratch
/// space: the resume path skips verified owners instead of staging them again, so
/// a later owner's failure must not destroy what an earlier one already staged,
/// verified and recorded. A target that discarded all of it would leave the
/// record naming content that no longer exists, and the commit boundary would
/// publish a destination the record claims is complete.
///
/// Preparing identity must not publish live state, and it must derive fresh
/// replacement-Endpoint keys: an imported source identity is never the new
/// endpoint's identity.
pub trait StagedRecoveryTarget {
    type PreparedIdentity;

    /// Whether this destination may receive a replacement at all.
    ///
    /// A destination that already holds an active identity refuses here, before
    /// the provider is consulted and before any owner stages anything, so an
    /// already-replaced device cannot be overwritten into a second activation.
    fn admit_destination(&mut self) -> Result<(), RecoveryError>;

    fn prepare_identity(
        &self,
        protected_authority_material: &[u8],
    ) -> Result<Self::PreparedIdentity, RecoveryError>;

    /// The owners this destination recorded as verified, as they were persisted.
    fn recorded_progress(&self) -> RecoveryProgress;

    /// Stages one bounded page of history into `owner`'s part of the
    /// destination. Pages of one owner arrive in inventory order.
    fn stage_history_page(
        &mut self,
        owner: RecoveryOwner,
        page: &[RecoveredHistoryObject],
    ) -> Result<(), RecoveryError>;

    /// Verifies `owner`'s staged part by reading it back, and reports the owner
    /// it could not verify. The caller records the owner only after `Ok`.
    fn verify_owner(&mut self, owner: RecoveryOwner) -> Result<(), RecoveryError>;

    /// Persists `progress` durably. The caller advances to the next owner only
    /// after this returns; a target that cannot persist progress cannot resume,
    /// so refusing here is the honest result.
    fn record_progress(&mut self, progress: &RecoveryProgress) -> Result<(), RecoveryError>;

    /// Selects the verified destination and the prepared identity under fresh
    /// sessions. It makes no earlier cross-store write atomically reversible.
    fn commit_verified(
        &mut self,
        prepared_identity: Self::PreparedIdentity,
        progress: &RecoveryProgress,
        fresh_sessions: FreshSessionRequirement,
    ) -> Result<(), RecoveryError>;

    /// Discards the staged destination that the recorded progress does not
    /// cover: the in-flight owner's pages, and any staging left by an attempt
    /// whose progress was never persisted. The staged parts of owners
    /// [`Self::recorded_progress`] marks verified survive, because a resume
    /// continues from that record and [`Self::commit_verified`] publishes them.
    ///
    /// The source is never part of it.
    fn discard_staged(&mut self) -> Result<(), RecoveryError>;
}

#[derive(Clone, Debug, Eq, Error, PartialEq)]
pub enum RecoveryError {
    #[error("provider access is required")]
    AccessRequired,
    #[error("retained history is incomplete")]
    HistoryIncomplete(Option<ObjectId>),
    #[error("retained history is corrupt")]
    CorruptObject(ObjectId),
    #[error("retained history has conflicting immutable content")]
    ConflictingObject(ObjectId),
    #[error("the frozen provider inventory became stale")]
    StaleInventory,
    #[error("an encrypted history key generation is unavailable")]
    UnknownKeyGeneration(u64),
    #[error("identity recovery authority is required")]
    IdentityAuthorityRequired,
    #[error("recovery secret was rejected")]
    RecoverySecretRejected,
    #[error("identity recovery authority rejected the transition")]
    IdentityAuthorityRejected,
    #[error("the destination owner did not verify: {0:?}")]
    OwnerNotVerified(RecoveryOwner),
    #[error("the recorded recovery progress cannot be resumed from")]
    ProgressNotResumable,
    #[error("the destination already holds an active identity")]
    DestinationActive,
    #[error("the destination cannot hold this device's own credentials")]
    CredentialsUnavailable,
    #[error("the destination refused to commit the verified target")]
    CommitRefused,
    #[error("provider operation failed")]
    ProviderFailure,
    #[error("history manifest is invalid")]
    InvalidManifest,
}

/// What one replacement run delivered.
///
/// It reports how many objects the run staged for each owner that consumes
/// history, and which owner it continued from. It deliberately does not return
/// the history: the objects were staged page by page and are read back by their
/// owner, so a caller that needs them asks the destination.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RecoveryOutcome {
    pub progress: RecoveryProgress,
    pub staged_objects: usize,
    /// The owner an interrupted run continued from, or `None` when the
    /// destination had recorded nothing to resume.
    pub resumed_from: Option<RecoveryOwner>,
}

/// Read every authorized history object in bounded pages, without holding the
/// whole history in memory.
///
/// The inventory itself is metadata and is frozen once; only the objects are
/// paged. `deliver` is called once per page in inventory order.
pub fn visit_authorized_history<S, K, D, F>(
    store: &S,
    local_keys: &K,
    decoder: &D,
    page_limit: NonZeroUsize,
    mut deliver: F,
) -> Result<usize, RecoveryError>
where
    S: AuthorizedHistoryStore,
    K: HistoryKeySource,
    D: RetainedContentDecoder,
    F: FnMut(&[RecoveredHistoryObject]) -> Result<(), RecoveryError>,
{
    let (version, inventory) = freeze_inventory(store).map_err(map_inventory_error)?;
    load_history_pages(
        store,
        &version,
        &inventory,
        local_keys,
        decoder,
        page_limit,
        &mut deliver,
    )
}

/// Recover a replacement endpoint on this device.
///
/// Each destination owner is staged and verified on its own, and the recorded
/// progress is persisted before the next owner runs, so a run interrupted
/// between owners resumes at the recorded one and needs no verified owner staged
/// again. A run that cannot continue discards the staging that record does not
/// cover — the verified owners' staged content stays, because the record names
/// it — and the source, the provider inventory and the recovery package are
/// untouched, so a later run or a separately authorized lost-activation remains
/// possible.
pub fn recover_replacement_endpoint<S, D, T>(
    store: &S,
    package: &RecoveryPackage,
    secret: &RecoverySecret,
    decoder: &D,
    page_limit: NonZeroUsize,
    target: &mut T,
) -> Result<RecoveryOutcome, RecoveryError>
where
    S: AuthorizedHistoryStore,
    D: RetainedContentDecoder,
    T: StagedRecoveryTarget,
{
    // The destination decides first whether it may receive a replacement at all.
    // An already-active destination is refused here, before provider access and
    // before any owner stages a single object.
    target.admit_destination()?;

    // Provider authorization and a frozen inventory are prerequisites. Key
    // possession must never turn an inaccessible provider into an empty success.
    let (version, inventory) = freeze_inventory(store).map_err(map_inventory_error)?;
    if inventory.is_empty() {
        target.discard_staged()?;
        return Err(RecoveryError::HistoryIncomplete(None));
    }
    let progress = target.recorded_progress();
    if !progress.is_resumable() {
        target.discard_staged()?;
        return Err(RecoveryError::ProgressNotResumable);
    }
    // A first run records nothing and continues from no owner. A run that already
    // recorded verified owners continues from the first one still outstanding.
    let resumed_from = if progress.is_empty() {
        None
    } else {
        progress.next_owner()
    };

    let material = match package.open(secret).map_err(map_material_error) {
        Ok(material) => material,
        Err(error) => {
            target.discard_staged()?;
            return Err(error);
        }
    };
    let prepared_identity = match target.prepare_identity(&material.identity_authority) {
        Ok(prepared) => prepared,
        Err(error) => {
            target.discard_staged()?;
            return Err(error);
        }
    };

    // Every remaining owner runs on its own: stage what it consumes, verify it by
    // reading it back, persist the progress, and only then move on. Nothing here
    // claims that a failure inside one owner is undone by another.
    let mut progress = progress;
    let mut staged_objects = 0_usize;
    for owner in RecoveryOwner::ALL {
        if progress.is_verified(owner) {
            continue;
        }
        let staged = if owner.consumes_history() {
            match load_history_pages(
                store,
                &version,
                &inventory,
                &material,
                decoder,
                page_limit,
                &mut |page| target.stage_history_page(owner, page),
            ) {
                Ok(staged) => staged,
                Err(error) => {
                    target.discard_staged()?;
                    return Err(error);
                }
            }
        } else {
            0
        };
        if let Err(error) = target.verify_owner(owner) {
            target.discard_staged()?;
            return Err(error);
        }
        progress.record(owner);
        if let Err(error) = target.record_progress(&progress) {
            target.discard_staged()?;
            return Err(error);
        }
        staged_objects = staged_objects.max(staged);
    }

    target.commit_verified(
        prepared_identity,
        &progress,
        FreshSessionRequirement::Required,
    )?;
    Ok(RecoveryOutcome {
        progress,
        staged_objects,
        resumed_from,
    })
}

/// Deliver the frozen inventory's objects in bounded pages.
///
/// The frozen inventory is metadata and stays in memory; the objects do not.
/// One page is built at a time, so peak memory is one page rather than the whole
/// retained history.
fn load_history_pages<S, K, D, F>(
    store: &S,
    version: &super::store::InventoryVersion,
    inventory: &std::collections::BTreeMap<ObjectId, ManifestEntry>,
    keys: &K,
    decoder: &D,
    page_limit: NonZeroUsize,
    deliver: &mut F,
) -> Result<usize, RecoveryError>
where
    S: AuthorizedHistoryStore,
    K: HistoryKeySource,
    D: RetainedContentDecoder,
    F: FnMut(&[RecoveredHistoryObject]) -> Result<(), RecoveryError>,
{
    if let Some(generation) = inventory
        .values()
        .find_map(|entry| match entry.storage_mode {
            StorageMode::ProviderManaged => None,
            StorageMode::ClientEncrypted { key_generation }
                if keys.key_for_generation(key_generation).is_none() =>
            {
                Some(key_generation)
            }
            StorageMode::ClientEncrypted { .. } => None,
        })
    {
        return Err(RecoveryError::UnknownKeyGeneration(generation));
    }

    let mut page = Vec::with_capacity(page_limit.get());
    let mut delivered = 0_usize;
    for entry in inventory.values() {
        page.push(load_one(store, version, entry, keys, decoder)?);
        if page.len() == page_limit.get() {
            deliver(&page)?;
            delivered += page.len();
            page.clear();
        }
    }
    if !page.is_empty() {
        deliver(&page)?;
        delivered += page.len();
    }
    if delivered != inventory.len() {
        return Err(RecoveryError::InvalidManifest);
    }
    Ok(delivered)
}

fn load_one<S, K, D>(
    store: &S,
    version: &super::store::InventoryVersion,
    entry: &ManifestEntry,
    keys: &K,
    decoder: &D,
) -> Result<RecoveredHistoryObject, RecoveryError>
where
    S: AuthorizedHistoryStore,
    K: HistoryKeySource,
    D: RetainedContentDecoder,
{
    let object = store
        .get_immutable(&entry.object_id, version)
        .map_err(|error| map_object_store_error(error, &entry.object_id))?;
    let canonical_bytes =
        open_history_object(entry, &object, keys).map_err(|error| match error {
            ObjectOpenError::MissingKey(generation) => {
                RecoveryError::UnknownKeyGeneration(generation)
            }
            ObjectOpenError::HeaderMismatch
            | ObjectOpenError::AuthenticationFailed
            | ObjectOpenError::ContentCorrupt => {
                RecoveryError::CorruptObject(entry.object_id.clone())
            }
        })?;
    let semantic_value = decoder
        .decode_semantics(entry.content_kind, &canonical_bytes)
        .ok_or_else(|| RecoveryError::CorruptObject(entry.object_id.clone()))?;
    Ok(RecoveredHistoryObject {
        object_id: entry.object_id.clone(),
        content_kind: entry.content_kind,
        storage_mode: entry.storage_mode,
        canonical_bytes,
        semantic_value,
    })
}

fn map_inventory_error(error: HistoryStoreError) -> RecoveryError {
    match error {
        HistoryStoreError::AccessRequired => RecoveryError::AccessRequired,
        HistoryStoreError::StaleInventory => RecoveryError::StaleInventory,
        HistoryStoreError::ObjectMissing => RecoveryError::InvalidManifest,
        HistoryStoreError::ImmutableConflict => {
            // Inventory conflicts are rejected before any object is loaded.
            RecoveryError::InvalidManifest
        }
        HistoryStoreError::ManifestConflict(object_id) => {
            RecoveryError::ConflictingObject(object_id)
        }
        HistoryStoreError::InvalidManifest => RecoveryError::InvalidManifest,
        HistoryStoreError::ProviderFailure => RecoveryError::ProviderFailure,
    }
}

fn map_object_store_error(error: HistoryStoreError, object_id: &ObjectId) -> RecoveryError {
    match error {
        HistoryStoreError::AccessRequired => RecoveryError::AccessRequired,
        HistoryStoreError::StaleInventory => RecoveryError::StaleInventory,
        HistoryStoreError::ObjectMissing => {
            RecoveryError::HistoryIncomplete(Some(object_id.clone()))
        }
        HistoryStoreError::ImmutableConflict => RecoveryError::ConflictingObject(object_id.clone()),
        HistoryStoreError::ManifestConflict(conflicting_id) => {
            RecoveryError::ConflictingObject(conflicting_id)
        }
        HistoryStoreError::InvalidManifest => RecoveryError::InvalidManifest,
        HistoryStoreError::ProviderFailure => RecoveryError::ProviderFailure,
    }
}

fn map_material_error(error: RecoveryMaterialError) -> RecoveryError {
    match error {
        RecoveryMaterialError::IdentityAuthorityRequired => {
            RecoveryError::IdentityAuthorityRequired
        }
        RecoveryMaterialError::InvalidPackage | RecoveryMaterialError::SecretRejected => {
            RecoveryError::RecoverySecretRejected
        }
    }
}
