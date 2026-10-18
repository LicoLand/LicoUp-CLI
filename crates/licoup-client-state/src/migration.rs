use std::{fs, path::Path};

use anyhow::{Context, Result, bail, ensure};
use serde_json::{Value, json};

use super::{collections::validate_current_collection_document, paths, policy, serialization};

pub fn probe_collections(portable_root: &Path) -> Result<(u32, bool)> {
    let root = portable_root.join(policy::CLIENT_STATE_DIR);
    let mut found = false;
    let mut legacy = false;
    for collection in policy::COLLECTIONS {
        let path = paths::collection_path(&root, collection);
        match fs::symlink_metadata(&path) {
            Ok(metadata) => ensure!(
                metadata.is_file() && !metadata.file_type().is_symlink(),
                "client state collection must be a regular file"
            ),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => return Err(error.into()),
        }
        found = true;
        let document = read_collection(&path)?;
        match document.get("schemaVersion").and_then(Value::as_str) {
            Some(policy::STATE_SCHEMA_VERSION) => {
                validate_current_collection_document(collection, &document)?;
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
    for collection in policy::COLLECTIONS {
        let path = paths::collection_path(&root, collection);
        if !path.exists() {
            continue;
        }
        let mut document = read_collection(&path)?;
        match document.get("schemaVersion").and_then(Value::as_str) {
            Some(policy::STATE_SCHEMA_VERSION) => {
                validate_current_collection_document(collection, &document)?;
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
        serialization::atomic_write_json(&path, &document, policy::MAX_COLLECTION_DOCUMENT_BYTES)?;
        validate_current_collection_document(collection, &document)?;
    }
    Ok(())
}

fn read_collection(path: &Path) -> Result<Value> {
    let metadata = fs::symlink_metadata(path).context("client state collection metadata failed")?;
    ensure!(
        metadata.is_file() && !metadata.file_type().is_symlink(),
        "client state collection must be a regular file"
    );
    let raw = fs::read(path).context("client state collection read failed")?;
    ensure!(
        !raw.is_empty() && raw.len() <= policy::MAX_COLLECTION_DOCUMENT_BYTES,
        "client state collection size is invalid"
    );
    let document: Value =
        serde_json::from_slice(&raw).context("client state collection JSON is invalid")?;
    ensure!(
        document.is_object(),
        "client state collection is not an object"
    );
    Ok(document)
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
