//! The appearance store: one published document shape and the one move between versions.
//!
//! `appearance-presentation` is the client-state store at
//! `client-state/appearance-preferences.json`. It carries the appearance slice of the
//! persisted state — the appearance preset the user selected, the font preference and the
//! locale preference — and it is the store the appearance resource package's converter
//! owns. This module is that store's owner inside the migration coordinator: the physical
//! path, the published shape and the 0→1 move live here and nowhere else.
//!
//! Two shapes are published, and nothing is guessed between them:
//!
//! - **Version 0** is the document the last published release wrote: a JSON object that
//!   records no schema version. An absent file is version 0 as well; the durable domain
//!   marker is what keeps "no store" distinct from "a store that records no version".
//! - **Version 1** is the current shape: the same object with `"schemaVersion": 1`.
//!
//! The move records the version and changes nothing else. Every key the user's client
//! wrote is preserved under the identity it was written with, which is what a preference
//! document is for: a conversion that dropped a preset or a locale would be a data loss
//! the user never asked for.
//!
//! A version above the target is `state_newer_than_binary`; any other shape — a document
//! that is not an object, or a version no release ever published — is
//! `unsupported_state_shape`, refused before anything is written. There is deliberately no
//! compatibility ladder: one source shape, one edge, one target, and the target comes from
//! the embedded frontier rather than from a second constant here.
//!
//! Nothing in this module truncates or replaces the source before the target commits. The
//! move is a single atomic private write of the whole document, performed by
//! [`migrate`] only after [`probe`] has read the shape, so an interrupted run leaves the
//! source readable and the next admission reconciles the committed store instead of
//! applying the edge a second time.

use anyhow::{Context, Result, anyhow, bail, ensure};
use serde_json::Value;
use std::fs;
use std::path::{Path, PathBuf};

use super::stores::regular_file_present;
use super::{AuthoritativeProbe, MAX_MIGRATION_JSON_BYTES, write_json_atomic};

/// The migration domain this store answers to.
pub(super) const DOMAIN_ID: &str = "appearance-presentation";

/// The store's path inside a data root.
pub(super) const STORE_PATH: &str = "client-state/appearance-preferences.json";

/// The field the current shape records its version in.
const SCHEMA_VERSION_FIELD: &str = "schemaVersion";

pub(super) fn store_path(root: &Path) -> PathBuf {
    root.join(STORE_PATH)
}

/// Read the shape of the appearance store, exactly as the admission resolves it.
///
/// `target` is the version the embedded frontier declares for this domain, so "current"
/// has one authority and this module cannot drift from the catalogue. A store that
/// records no version answers version 0 without being rewritten: reading a shape is not
/// converting it.
pub(super) fn probe(root: &Path, target: u32) -> Result<AuthoritativeProbe> {
    let path = store_path(root);
    if !regular_file_present(&path)? {
        return Ok(AuthoritativeProbe {
            version: 0,
            present: false,
        });
    }
    let value = read_document(&path)?;
    match recorded_version(&value) {
        Some(version) if version == u64::from(target) => Ok(AuthoritativeProbe {
            version: 1,
            present: true,
        }),
        Some(version) if version > u64::from(target) => bail!("state_newer_than_binary"),
        Some(_) => bail!("unsupported_state_shape"),
        None => Ok(AuthoritativeProbe {
            version: 0,
            present: true,
        }),
    }
}

/// Perform the one move: record the target version, preserve every other field.
///
/// An absent store is not created. The domain's authoritative version is the store's
/// shape, and inventing an empty document would claim a preference the user never made.
pub(super) fn migrate(root: &Path, target: u32) -> Result<()> {
    let path = store_path(root);
    if !path.exists() {
        return Ok(());
    }
    let mut value = read_document(&path)?;
    match recorded_version(&value) {
        Some(version) if version == u64::from(target) => return Ok(()),
        Some(version) if version > u64::from(target) => bail!("state_newer_than_binary"),
        Some(_) => bail!("unsupported_state_shape"),
        None => {}
    }
    let object = value
        .as_object_mut()
        .ok_or_else(|| anyhow!("unsupported_state_shape"))?;
    object.insert(SCHEMA_VERSION_FIELD.to_owned(), Value::from(target));
    write_json_atomic(&path, &value).context("migration_step_failed")
}

/// The bounded, object-shaped document this store is.
fn read_document(path: &Path) -> Result<Value> {
    let raw = fs::read(path).context("unsupported_state_shape")?;
    ensure!(
        raw.len() <= MAX_MIGRATION_JSON_BYTES,
        "unsupported_state_shape"
    );
    let value: Value = serde_json::from_slice(&raw).context("unsupported_state_shape")?;
    ensure!(value.is_object(), "unsupported_state_shape");
    Ok(value)
}

/// The version the document records as a schema version, when it records one.
///
/// A field that is present but not a version number records no version either: the
/// published source shape is "a document that records none", and a writer that left a
/// string where a number belongs did not publish a format this store can walk. The
/// document is still read as legacy rather than invented into a newer shape, and the
/// move preserves the field's value — the target version replaces it only because
/// recording the version *is* the move.
fn recorded_version(value: &Value) -> Option<u64> {
    value.get(SCHEMA_VERSION_FIELD).and_then(Value::as_u64)
}
