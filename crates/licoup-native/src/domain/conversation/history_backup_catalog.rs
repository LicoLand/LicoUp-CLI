//! Minimal projection from existing history owners into immutable backup items.
//!
//! Callers pass the stable IDs and canonical bytes already owned by the
//! conversation, snapshot, or selected archive subsystem. This module does not
//! infer identity from a path or create a second conversation catalog.

use std::collections::BTreeMap;

use serde_json::Value;
use thiserror::Error;

use crate::domain::history_backup::{ContentKind, ObjectId};

#[derive(Clone, Eq, PartialEq)]
pub struct RetainedHistoryItem {
    pub object_id: ObjectId,
    pub content_kind: ContentKind,
    pub canonical_bytes: Vec<u8>,
    pub semantic_value: Value,
}

#[derive(Clone, Default, Eq, PartialEq)]
pub struct RetainedHistoryCatalog {
    items: BTreeMap<ObjectId, RetainedHistoryItem>,
}

#[derive(Clone, Debug, Eq, Error, PartialEq)]
pub enum CatalogProjectionError {
    #[error("an owner identity is empty or invalid")]
    InvalidOwnerIdentity,
    #[error("one immutable retained-history identity has conflicting content")]
    ConflictingImmutableVersion(ObjectId),
}

impl RetainedHistoryCatalog {
    pub fn insert(&mut self, item: RetainedHistoryItem) -> Result<(), CatalogProjectionError> {
        if let Some(existing) = self.items.get(&item.object_id) {
            if existing != &item {
                return Err(CatalogProjectionError::ConflictingImmutableVersion(
                    item.object_id,
                ));
            }
            return Ok(());
        }
        self.items.insert(item.object_id.clone(), item);
        Ok(())
    }

    pub fn items(&self) -> &BTreeMap<ObjectId, RetainedHistoryItem> {
        &self.items
    }
}

pub fn conversation_revision(
    stable_conversation_id: &str,
    stable_revision_id: &str,
    canonical_bytes: Vec<u8>,
    semantic_value: Value,
) -> Result<RetainedHistoryItem, CatalogProjectionError> {
    retained_item(
        "conversation",
        &[stable_conversation_id, "revision", stable_revision_id],
        ContentKind::SemanticConversation,
        canonical_bytes,
        semantic_value,
    )
}

pub fn conversation_snapshot(
    stable_snapshot_id: &str,
    canonical_bytes: Vec<u8>,
    semantic_value: Value,
) -> Result<RetainedHistoryItem, CatalogProjectionError> {
    retained_item(
        "snapshot",
        &[stable_snapshot_id],
        ContentKind::ConversationSnapshot,
        canonical_bytes,
        semantic_value,
    )
}

pub fn selected_archive(
    stable_archive_id: &str,
    canonical_bytes: Vec<u8>,
    semantic_value: Value,
) -> Result<RetainedHistoryItem, CatalogProjectionError> {
    retained_item(
        "archive",
        &[stable_archive_id],
        ContentKind::SelectedArchive,
        canonical_bytes,
        semantic_value,
    )
}

fn retained_item(
    prefix: &str,
    owner_ids: &[&str],
    content_kind: ContentKind,
    canonical_bytes: Vec<u8>,
    semantic_value: Value,
) -> Result<RetainedHistoryItem, CatalogProjectionError> {
    if owner_ids
        .iter()
        .any(|value| value.is_empty() || value.contains('/') || value.chars().any(char::is_control))
    {
        return Err(CatalogProjectionError::InvalidOwnerIdentity);
    }
    let object_id = ObjectId::new(format!("{prefix}/{}", owner_ids.join("/")))
        .map_err(|_| CatalogProjectionError::InvalidOwnerIdentity)?;
    Ok(RetainedHistoryItem {
        object_id,
        content_kind,
        canonical_bytes,
        semantic_value,
    })
}

#[cfg(test)]
mod trusted_history_catalog {
    use serde_json::json;

    use super::*;

    #[test]
    fn growing_conversation_retains_each_owner_revision_and_dedupes_exact_repeats() {
        let first =
            conversation_revision("conversation-1", "event-10", b"first".to_vec(), json!(1))
                .unwrap();
        let second =
            conversation_revision("conversation-1", "event-11", b"second".to_vec(), json!(2))
                .unwrap();
        let mut catalog = RetainedHistoryCatalog::default();
        catalog.insert(first.clone()).unwrap();
        catalog.insert(first).unwrap();
        catalog.insert(second).unwrap();
        assert_eq!(
            catalog
                .items()
                .keys()
                .map(ObjectId::as_str)
                .collect::<Vec<_>>(),
            vec![
                "conversation/conversation-1/revision/event-10",
                "conversation/conversation-1/revision/event-11",
            ]
        );
    }

    #[test]
    fn same_owner_revision_with_different_content_is_a_conflict() {
        let first =
            conversation_revision("conversation-1", "event-10", b"first".to_vec(), json!(1))
                .unwrap();
        let conflicting =
            conversation_revision("conversation-1", "event-10", b"other".to_vec(), json!(2))
                .unwrap();
        let mut catalog = RetainedHistoryCatalog::default();
        catalog.insert(first).unwrap();
        assert!(matches!(
            catalog.insert(conflicting),
            Err(CatalogProjectionError::ConflictingImmutableVersion(_))
        ));
    }

    #[test]
    fn snapshot_and_selected_archive_keep_owner_ids_bytes_and_semantics() {
        let snapshot = conversation_snapshot(
            "snapshot-7",
            b"snapshot-bytes".to_vec(),
            json!({"snapshotId":"snapshot-7"}),
        )
        .unwrap();
        let archive = selected_archive(
            "archive-3",
            b"archive-bytes".to_vec(),
            json!({"archiveId":"archive-3"}),
        )
        .unwrap();

        assert_eq!(snapshot.object_id.as_str(), "snapshot/snapshot-7");
        assert_eq!(snapshot.canonical_bytes, b"snapshot-bytes");
        assert_eq!(snapshot.semantic_value, json!({"snapshotId":"snapshot-7"}));
        assert_eq!(archive.object_id.as_str(), "archive/archive-3");
        assert_eq!(archive.canonical_bytes, b"archive-bytes");
        assert_eq!(archive.semantic_value, json!({"archiveId":"archive-3"}));
    }
}
