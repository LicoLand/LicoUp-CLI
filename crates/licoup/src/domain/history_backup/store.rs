use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use thiserror::Error;

use super::policy::{StorageMode, StoredHistoryObject, validate_stored_object_header};

#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ObjectId(String);

impl ObjectId {
    pub fn new(value: impl Into<String>) -> Result<Self, HistoryStoreError> {
        let value = value.into();
        if value.is_empty() || value.chars().any(char::is_control) {
            return Err(HistoryStoreError::InvalidManifest);
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

macro_rules! opaque_id {
    ($name:ident) => {
        #[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Serialize, Deserialize)]
        #[serde(transparent)]
        pub struct $name(String);

        impl $name {
            pub fn new(value: impl Into<String>) -> Result<Self, HistoryStoreError> {
                let value = value.into();
                if value.is_empty() || value.chars().any(char::is_control) {
                    return Err(HistoryStoreError::InvalidManifest);
                }
                Ok(Self(value))
            }

            pub fn as_str(&self) -> &str {
                &self.0
            }
        }
    };
}

opaque_id!(DeviceId);
opaque_id!(SegmentId);
opaque_id!(InventoryVersion);

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ContentKind {
    SemanticConversation,
    ConversationSnapshot,
    SelectedArchive,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ManifestEntry {
    pub object_id: ObjectId,
    pub content_kind: ContentKind,
    pub storage_mode: StorageMode,
    pub canonical_length: u64,
    pub canonical_digest: [u8; 32],
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ManifestSegment {
    pub format_version: u16,
    pub device_id: DeviceId,
    pub segment_id: SegmentId,
    pub entries: Vec<ManifestEntry>,
}

impl ManifestSegment {
    pub fn new(
        device_id: DeviceId,
        segment_id: SegmentId,
        mut entries: Vec<ManifestEntry>,
    ) -> Result<Self, HistoryStoreError> {
        entries.sort_by(|left, right| left.object_id.cmp(&right.object_id));
        if entries
            .windows(2)
            .any(|pair| pair[0].object_id == pair[1].object_id)
        {
            return Err(HistoryStoreError::InvalidManifest);
        }
        Ok(Self {
            format_version: 1,
            device_id,
            segment_id,
            entries,
        })
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct InventoryPage {
    pub version: InventoryVersion,
    pub segments: Vec<ManifestSegment>,
    pub next_cursor: Option<String>,
}

#[derive(Clone, Debug, Eq, Error, PartialEq)]
pub enum HistoryStoreError {
    #[error("provider access is required")]
    AccessRequired,
    #[error("the frozen provider inventory became stale")]
    StaleInventory,
    #[error("a referenced history object is absent")]
    ObjectMissing,
    #[error("immutable provider content conflicts")]
    ImmutableConflict,
    #[error("one immutable history identity has conflicting manifest entries")]
    ManifestConflict(ObjectId),
    #[error("history manifest is invalid")]
    InvalidManifest,
    #[error("provider operation failed")]
    ProviderFailure,
}

/// A caller-supplied store whose provider authorization is already complete.
///
/// The port deliberately has no login, credential, token, Station, or vendor
/// operation. `get_immutable` must read the requested frozen inventory version
/// or return `StaleInventory`.
pub trait AuthorizedHistoryStore: Sync {
    fn list_manifest_page(&self, cursor: Option<&str>) -> Result<InventoryPage, HistoryStoreError>;

    fn get_immutable(
        &self,
        object_id: &ObjectId,
        frozen_version: &InventoryVersion,
    ) -> Result<StoredHistoryObject, HistoryStoreError>;

    fn put_immutable(&mut self, object: StoredHistoryObject) -> Result<(), HistoryStoreError>;

    fn append_manifest_segment(
        &mut self,
        segment: ManifestSegment,
    ) -> Result<(), HistoryStoreError>;
}

pub fn append_backup_segment<S: AuthorizedHistoryStore>(
    store: &mut S,
    objects: impl IntoIterator<Item = StoredHistoryObject>,
    segment: ManifestSegment,
) -> Result<(), HistoryStoreError> {
    if segment.format_version != 1
        || segment
            .entries
            .windows(2)
            .any(|pair| pair[0].object_id >= pair[1].object_id)
    {
        return Err(HistoryStoreError::InvalidManifest);
    }
    let expected = segment
        .entries
        .iter()
        .map(|entry| (entry.object_id.clone(), entry))
        .collect::<BTreeMap<_, _>>();
    let mut supplied = BTreeMap::<ObjectId, StoredHistoryObject>::new();
    for object in objects {
        let Some(entry) = expected.get(&object.header.object_id) else {
            return Err(HistoryStoreError::InvalidManifest);
        };
        validate_stored_object_header(entry, &object)
            .map_err(|_| HistoryStoreError::InvalidManifest)?;
        match supplied.get(&object.header.object_id) {
            Some(existing) if existing != &object => {
                return Err(HistoryStoreError::ImmutableConflict);
            }
            Some(_) => continue,
            None => {
                supplied.insert(object.header.object_id.clone(), object);
            }
        }
    }
    if supplied.keys().ne(expected.keys()) {
        return Err(HistoryStoreError::InvalidManifest);
    }
    for object in supplied.into_values() {
        store.put_immutable(object)?;
    }
    store.append_manifest_segment(segment)
}

pub(crate) fn freeze_inventory<S: AuthorizedHistoryStore>(
    store: &S,
) -> Result<(InventoryVersion, BTreeMap<ObjectId, ManifestEntry>), HistoryStoreError> {
    let mut cursor = None::<String>;
    let mut seen_cursors = BTreeSet::new();
    let mut version = None::<InventoryVersion>;
    let mut seen_segments = BTreeMap::<(DeviceId, SegmentId), ManifestSegment>::new();
    let mut inventory = BTreeMap::<ObjectId, ManifestEntry>::new();

    loop {
        let page = store.list_manifest_page(cursor.as_deref())?;
        match &version {
            Some(frozen) if frozen != &page.version => {
                return Err(HistoryStoreError::StaleInventory);
            }
            None => version = Some(page.version.clone()),
            _ => {}
        }

        for segment in page.segments {
            if segment.format_version != 1
                || segment
                    .entries
                    .windows(2)
                    .any(|pair| pair[0].object_id >= pair[1].object_id)
            {
                return Err(HistoryStoreError::InvalidManifest);
            }
            let segment_key = (segment.device_id.clone(), segment.segment_id.clone());
            if let Some(existing) = seen_segments.get(&segment_key) {
                if existing != &segment {
                    return Err(HistoryStoreError::ImmutableConflict);
                }
                continue;
            }
            for entry in &segment.entries {
                if let Some(existing) = inventory.get(&entry.object_id) {
                    if existing != entry {
                        return Err(HistoryStoreError::ManifestConflict(entry.object_id.clone()));
                    }
                } else {
                    inventory.insert(entry.object_id.clone(), entry.clone());
                }
            }
            seen_segments.insert(segment_key, segment);
        }

        let Some(next) = page.next_cursor else {
            break;
        };
        if next.is_empty() || !seen_cursors.insert(next.clone()) {
            return Err(HistoryStoreError::InvalidManifest);
        }
        cursor = Some(next);
    }

    Ok((
        version.ok_or(HistoryStoreError::InvalidManifest)?,
        inventory,
    ))
}
