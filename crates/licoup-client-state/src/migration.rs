use std::path::Path;

use anyhow::{Context, Result, bail, ensure};
use collection_store::CollectionStore;
use licoup_foundation::platform::file_security::{
    read_existing_private_text_bounded, sync_directory,
};
use serde_json::{Value, json};

use super::collections::validate_collection_document;
use super::{paths, policy};

pub fn probe_collections(portable_root: &Path) -> Result<(u32, bool)> {
    let root = portable_root.join(policy::CLIENT_STATE_DIR);
    let mut found = false;
    let mut legacy = false;
    for collection in policy::COLLECTIONS {
        let path = paths::collection_path(&root, collection);
        let Some(document) = read_collection_document(&path)? else {
            continue;
        };
        found = true;
        match document.get("schemaVersion").and_then(Value::as_str) {
            Some(policy::STATE_SCHEMA_VERSION) => {
                validate_collection_document(collection, &document)?;
            }
            None => {
                validate_legacy_owner(collection, &document)?;
                legacy = true;
            }
            Some(_) => bail!("unsupported client state collection schema"),
        }
    }
    Ok((u32::from(found && !legacy), found))
}

pub fn migrate_collections(portable_root: &Path) -> Result<()> {
    let root = portable_root.join(policy::CLIENT_STATE_DIR);
    let store = open_existing_store(&root)?;
    for collection in policy::COLLECTIONS {
        let path = paths::collection_path(&root, collection);
        let Some(mut document) = read_collection_document(&path)? else {
            continue;
        };
        match document.get("schemaVersion").and_then(Value::as_str) {
            Some(policy::STATE_SCHEMA_VERSION) => {
                validate_collection_document(collection, &document)?;
                continue;
            }
            None => validate_legacy_owner(collection, &document)?,
            Some(_) => bail!("unsupported client state collection schema"),
        }
        let object = document
            .as_object_mut()
            .ok_or_else(|| anyhow::anyhow!("client state collection is not an object"))?;
        object.insert(
            "schemaVersion".to_owned(),
            json!(policy::STATE_SCHEMA_VERSION),
        );
        object
            .entry("collection".to_owned())
            .or_insert_with(|| json!(collection));
        let store = store
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("client state store is unavailable"))?;
        store.write_collection(collection, document)?;
        // The store syncs the staged file before its rename; the parent sync
        // is what makes the published rename durable.
        sync_directory(&root)?;
    }
    Ok(())
}

/// Open the published store only when the collection directory already exists;
/// probing or migrating an absent data home must not create it.
fn open_existing_store(root: &Path) -> Result<Option<CollectionStore>> {
    if !root.is_dir() {
        return Ok(None);
    }
    Ok(Some(CollectionStore::new(root.to_path_buf())?))
}

fn read_collection_document(path: &Path) -> Result<Option<Value>> {
    let Some(raw) =
        read_existing_private_text_bounded(path, policy::MAX_COLLECTION_DOCUMENT_BYTES)?
    else {
        return Ok(None);
    };
    ensure!(
        !raw.trim().is_empty(),
        "client state collection size is invalid"
    );
    let document: Value =
        serde_json::from_str(&raw).context("client state collection JSON is invalid")?;
    ensure!(
        document.is_object(),
        "client state collection is not an object"
    );
    Ok(Some(document))
}

fn validate_legacy_owner(collection: &str, document: &Value) -> Result<()> {
    if let Some(owner) = document.get("collection") {
        ensure!(
            owner.as_str() == Some(collection),
            "client state collection owner mismatch"
        );
    }
    Ok(())
}
