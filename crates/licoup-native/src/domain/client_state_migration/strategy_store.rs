use anyhow::{Context, Result, anyhow, bail, ensure};
use rusqlite::{Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

use super::stores::regular_file_present;
use super::{AuthoritativeProbe, migration_failpoint, write_json_atomic};
use crate::state_machines::strategy_store_artifact;

// ---------------------------------------------------------------------------
// The released strategy-store layout and the conversion to the current one
// ---------------------------------------------------------------------------
//
// The strategy database is one file with two layouts this conversion model
// knows. Their identifiers name the `strategy_meta` version each published
// writer stamped, so a file is classified by what it physically is — its
// version row and its tables — not by what its name claims:
//
// * `strategy-store-2` — the layout the published client writes. The immutable
//   release tag v0.2.1 (db0fc4d7ae875332f8b0cab28cda3f3337ac3c9e) creates the
//   seven tables below and stamps `strategy_meta.version = '2'` in
//   `crates/licoup-native/src/domain/adaptive_flywheel/store.rs`; its frontier
//   owns `adaptive-flywheel` at domain version 1 through the immutable
//   `adaptive-flywheel.absent-to-1` step.
// * `strategy-store-3` — this binary's current layout. The owner creates the
//   same seven tables, canonicalizes the stored workflows and stamps `= '3'`;
//   its frontier owns domain version 2 through
//   `adaptive-flywheel.workflow-routing-to-2`.
//
// The rule is the one the plan states for published contracts: **a layout that
// shipped is immutable**. Both entries describe a layout that actually exists;
// a file that matches neither is refused rather than guessed at, and a file
// whose version is ahead of this binary is `state_newer_than_binary`. Nothing
// here may declare a layout, which is why the probe opens the file read-only
// and asks `PRAGMA table_info` what is actually there: inspecting an old
// database by running `CREATE TABLE IF NOT EXISTS` against it would answer the
// question with the answer we just wrote.

/// The strategy database, at the path the published writer used.
pub(super) const STRATEGY_STORE_DATABASE: &str =
    "client-state/adaptive-flywheel/strategies.sqlite3";

/// Recovery record for one strategy-store conversion.
///
/// It is written before the first store-format edge runs, one step id is
/// appended after each edge's postcondition holds, and it is marked applied at
/// the end. An interrupted conversion therefore resumes from the physical
/// format the file actually reached — the artifact states what was attempted,
/// the file states what happened, and the next run drives from the file.
pub(super) const STRATEGY_STORE_ARTIFACT: &str =
    "client-state/migrations/artifacts/adaptive-flywheel-strategy-store.json";
pub(super) const STRATEGY_STORE_ARTIFACT_SCHEMA: &str =
    "v0.0.1:strategy-store-conversion-artifact-1";

/// A layout of the strategy database, as data.
///
/// The physical layout itself is validated by the owning store
/// (`validate_published_core_layout`), not by a table or column subset here:
/// both published writers create the same seven core tables with the same keys,
/// foreign keys and uniqueness constraints, and only the `strategy_meta.version`
/// row tells them apart.
pub(super) struct StrategyStoreFormat {
    pub(super) format_id: &'static str,
    /// The `strategy_meta.version` values this layout shipped under. Both
    /// writers stamped one version; the value is part of the format identity.
    pub(super) meta_versions: &'static [&'static str],
    /// The frontier domain version this layout answers to.
    pub(super) domain_schema_version: u32,
}

/// The two layouts this conversion model knows, released first.
pub(super) const STRATEGY_STORE_FORMATS: &[StrategyStoreFormat] = &[
    StrategyStoreFormat {
        format_id: "strategy-store-2",
        meta_versions: &["2"],
        domain_schema_version: 1,
    },
    StrategyStoreFormat {
        format_id: "strategy-store-3",
        meta_versions: &["3"],
        domain_schema_version: 2,
    },
];

pub(super) struct StrategyStoreEdge {
    pub(super) step_id: &'static str,
    pub(super) from: &'static str,
    pub(super) to: &'static str,
}

/// The conversion graph over the known layouts: the one published step from the
/// released layout to this binary's current one.
pub(super) const STRATEGY_STORE_EDGES: &[StrategyStoreEdge] = &[StrategyStoreEdge {
    step_id: "adaptive-flywheel.strategy-store-workflow-routing",
    from: "strategy-store-2",
    to: "strategy-store-3",
}];

/// The layout that answers to one frontier domain version.
pub(super) fn strategy_format_for_domain_version(
    version: u32,
) -> Result<&'static StrategyStoreFormat> {
    STRATEGY_STORE_FORMATS
        .iter()
        .find(|format| format.domain_schema_version == version)
        .ok_or_else(|| anyhow!("migration_frontier_incomplete"))
}

/// The current layout: the newest one in the conversion graph.
pub(super) fn current_strategy_format() -> &'static StrategyStoreFormat {
    STRATEGY_STORE_FORMATS
        .last()
        .expect("the strategy layout list is not empty")
}

pub(super) fn strategy_format_position(format_id: &str) -> Result<usize> {
    STRATEGY_STORE_FORMATS
        .iter()
        .position(|format| format.format_id == format_id)
        .ok_or_else(|| anyhow!("migration_frontier_incomplete"))
}

/// The unique conversion path from one known layout to another.
pub(super) fn strategy_store_path(from: &str, to: &str) -> Result<Vec<&'static StrategyStoreEdge>> {
    let target = strategy_format_position(to)?;
    let mut cursor = strategy_format_position(from)?;
    let mut path = Vec::new();
    while cursor < target {
        let current = STRATEGY_STORE_FORMATS[cursor].format_id;
        let edge = STRATEGY_STORE_EDGES
            .iter()
            .find(|edge| edge.from == current)
            .ok_or_else(|| anyhow!("migration_frontier_incomplete"))?;
        path.push(edge);
        cursor = strategy_format_position(edge.to)?;
    }
    ensure!(
        cursor == target && path.iter().all(|edge| edge.to != from),
        "migration_frontier_incomplete"
    );
    Ok(path)
}

/// Read which known layout a real file holds.
///
/// Read-only, and by construction unable to declare a layout: the answer comes
/// from the file's own `strategy_meta` row and then from the owning store's
/// exact layout validation, which checks the seven core tables, their columns,
/// keys, foreign keys, uniqueness constraints and named indexes. A file that
/// matches no known version is refused, a file whose version is ahead of this
/// binary is refused as `state_newer_than_binary`, and a version row on an
/// incomplete or malformed layout is refused as `unsupported_state_shape`.
pub(super) fn read_strategy_store_format(path: &Path) -> Result<&'static StrategyStoreFormat> {
    ensure!(regular_file_present(path)?, "unsupported_state_shape");
    let connection = Connection::open_with_flags(
        path,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .context("unsupported_state_shape")?;
    read_strategy_store_format_on(&connection)
}

pub(super) fn read_strategy_store_format_on(
    connection: &Connection,
) -> Result<&'static StrategyStoreFormat> {
    let version: Option<String> = connection
        .query_row(
            "SELECT value FROM strategy_meta WHERE key='version'",
            [],
            |row| row.get(0),
        )
        .optional()
        .context("unsupported_state_shape")?;
    let version = version.ok_or_else(|| anyhow!("unsupported_state_shape"))?;
    if let Ok(numeric) = version.parse::<u32>() {
        ensure!(
            numeric <= current_strategy_meta_version(),
            "state_newer_than_binary"
        );
    }
    for format in STRATEGY_STORE_FORMATS {
        if !format.meta_versions.contains(&version.as_str()) {
            continue;
        }
        crate::domain::workflow_store::validate_published_core_layout(connection, &version)
            .context("unsupported_state_shape")?;
        return Ok(format);
    }
    bail!("unsupported_state_shape")
}

pub(super) fn current_strategy_meta_version() -> u32 {
    current_strategy_format()
        .meta_versions
        .iter()
        .filter_map(|version| version.parse::<u32>().ok())
        .max()
        .expect("the current strategy layout has a numeric version")
}

pub(super) fn probe_adaptive_flywheel(root: &Path) -> Result<AuthoritativeProbe> {
    let path = root.join(STRATEGY_STORE_DATABASE);
    if !regular_file_present(&path)? {
        return Ok(AuthoritativeProbe {
            version: 0,
            present: false,
        });
    }
    let format = read_strategy_store_format(&path)?;
    Ok(AuthoritativeProbe {
        version: format.domain_schema_version,
        present: true,
    })
}

/// The recovery record of a strategy-store conversion.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct StrategyStoreArtifact {
    pub(super) schema_version: String,
    pub(super) domain_id: String,
    pub(super) from_format: String,
    pub(super) target_format: String,
    pub(super) applied_step_ids: Vec<String>,
    pub(super) status: String,
}

pub(super) fn strategy_store_artifact_path(root: &Path) -> PathBuf {
    root.join(STRATEGY_STORE_ARTIFACT)
}

/// Load the artifact for a conversion, or start one that records this chain.
///
/// One record per store, not per step: a conversion may cross more than one
/// store-format edge, and a record that could only describe one of them would
/// lose the chain exactly when a resume needs it. The `from_format` stays where
/// the first conversion began, `applied_step_ids` accumulates, and the target
/// only ever moves forward — a store cannot be un-converted by a later record.
pub(super) fn begin_strategy_store_artifact(
    root: &Path,
    from: &StrategyStoreFormat,
    target: &StrategyStoreFormat,
) -> Result<StrategyStoreArtifact> {
    let path = strategy_store_artifact_path(root);
    let mut artifact = if regular_file_present(&path)? {
        let raw = licoup_foundation::platform::file_security::read_existing_private_text_bounded(
            &path,
            64 * 1024,
        )
        .context("migration_step_failed")?
        .ok_or_else(|| anyhow!("migration_step_failed"))?;
        let existing: StrategyStoreArtifact =
            serde_json::from_str(&raw).context("migration_step_failed")?;
        ensure!(
            existing.schema_version == STRATEGY_STORE_ARTIFACT_SCHEMA
                && existing.domain_id == STRATEGY_STORE_DOMAIN,
            "migration_step_failed"
        );
        ensure!(
            strategy_format_position(&existing.target_format)?
                <= strategy_format_position(target.format_id)?,
            "state_newer_than_binary"
        );
        existing
    } else {
        StrategyStoreArtifact {
            schema_version: STRATEGY_STORE_ARTIFACT_SCHEMA.to_owned(),
            domain_id: STRATEGY_STORE_DOMAIN.to_owned(),
            from_format: from.format_id.to_owned(),
            target_format: target.format_id.to_owned(),
            applied_step_ids: Vec::new(),
            status: strategy_store_artifact::INITIAL.as_str().to_owned(),
        }
    };
    artifact.target_format = target.format_id.to_owned();
    // Written before any edge runs, so a crash inside the conversion leaves a
    // record that claims only what is true: a step is in flight. The caller
    // marks it applied after the last edge's postcondition holds.
    artifact.status = strategy_store_artifact::transition(
        strategy_store_artifact::State::from_name(&artifact.status)
            .context("migration_step_failed")?,
        strategy_store_artifact::Event::Begin,
    )
    .context("migration_step_failed")?
    .as_str()
    .to_owned();
    if let Some(parent) = path.parent() {
        licoup_foundation::platform::file_security::ensure_private_dir(parent)
            .context("migration_step_failed")?;
    }
    write_json_atomic(&path, &artifact).context("migration_step_failed")?;
    Ok(artifact)
}

pub(super) const STRATEGY_STORE_DOMAIN: &str = "adaptive-flywheel";

/// The released writer's own migration for the one store-format edge.
///
/// The call is named by the edge it performs: the owner's canonicalization is
/// what produces the current layout and moves the version row from `2` to `3`.
/// Naming it here is what keeps this list from quietly becoming a second
/// definition of the layout.
pub(super) fn delegate_strategy_store_edge(edge: &StrategyStoreEdge, root: &Path) -> Result<()> {
    match edge.to {
        "strategy-store-3" => {
            crate::domain::workflow_store::StrategyStore::open_for_migration(root)
                .context("migration_step_failed")
                .map(|_| ())
        }
        _ => bail!("migration_frontier_incomplete"),
    }
}

/// Finish the bookkeeping of a conversion whose physical store already reached
/// its target.
///
/// A process can stop after the owner committed the new layout but before this
/// module recorded the step or marked the record applied. The next run sees a
/// store at the target and would otherwise return without touching the record,
/// leaving a pending journal forever. The store is the authority for what
/// happened, so the record is reconciled to it — and only the record is
/// written: a store already at the target is never rewritten by bookkeeping.
fn reconcile_strategy_store_artifact(
    root: &Path,
    observed: &'static StrategyStoreFormat,
) -> Result<Vec<&'static str>> {
    let path = strategy_store_artifact_path(root);
    if !regular_file_present(&path)? {
        return Ok(Vec::new());
    }
    let raw = licoup_foundation::platform::file_security::read_existing_private_text_bounded(
        &path,
        64 * 1024,
    )
    .context("migration_step_failed")?
    .ok_or_else(|| anyhow!("migration_step_failed"))?;
    let mut artifact: StrategyStoreArtifact =
        serde_json::from_str(&raw).context("migration_step_failed")?;
    ensure!(
        artifact.schema_version == STRATEGY_STORE_ARTIFACT_SCHEMA
            && artifact.domain_id == STRATEGY_STORE_DOMAIN,
        "migration_step_failed"
    );
    // The physical store has already been validated by its owner. A completed
    // receipt is retained history, not another format authority: a corrected
    // unpublished conversion may no longer have an executable path. Only an
    // unfinished receipt needs reconciliation against the current graph.
    if artifact.status == "applied" {
        return Ok(Vec::new());
    }
    ensure!(
        strategy_format_position(&artifact.target_format)?
            >= strategy_format_position(observed.format_id)?,
        "state_newer_than_binary"
    );
    let mut recorded = Vec::new();
    let mut changed = false;
    for edge in strategy_store_path(&artifact.from_format, observed.format_id)? {
        if !artifact
            .applied_step_ids
            .iter()
            .any(|id| id.as_str() == edge.step_id)
        {
            artifact.applied_step_ids.push(edge.step_id.to_owned());
            recorded.push(edge.step_id);
            changed = true;
        }
    }
    if artifact.target_format == observed.format_id && artifact.status != "applied" {
        artifact.status = strategy_store_artifact::transition(
            strategy_store_artifact::State::from_name(&artifact.status)
                .context("migration_step_failed")?,
            strategy_store_artifact::Event::Complete,
        )
        .context("migration_step_failed")?
        .as_str()
        .to_owned();
        changed = true;
    }
    if changed {
        write_json_atomic(&path, &artifact).context("migration_step_failed")?;
    }
    Ok(recorded)
}

/// Reconcile the recovery record of a store that is already at `format`.
///
/// This is the path every ordinary start takes for the strategy store: the
/// store itself is never rewritten, and the only write that can remain is the
/// recovery record of a conversion that committed its physical layout before
/// recording it.
pub(super) fn reconcile_completed_strategy_store(
    root: &Path,
    format: &'static StrategyStoreFormat,
) -> Result<()> {
    reconcile_strategy_store_artifact(root, format).map(|_| ())
}

/// Drive the store from the layout the file holds to `target`, one edge at a
/// time, checking the physical layout after every edge.
pub(super) fn advance_strategy_store(
    root: &Path,
    target: &'static StrategyStoreFormat,
) -> Result<Vec<&'static str>> {
    let path = root.join(STRATEGY_STORE_DATABASE);
    if !regular_file_present(&path)? {
        return Ok(Vec::new());
    }
    let observed = read_strategy_store_format(&path)?;
    if observed.format_id == target.format_id {
        return reconcile_strategy_store_artifact(root, observed);
    }
    ensure!(
        strategy_format_position(observed.format_id)? < strategy_format_position(target.format_id)?,
        // The published writer is forward-only. A store ahead of this binary is
        // not moved back by admission; that is the migration tool's downgrade
        // path, with its preservation step.
        "state_newer_than_binary"
    );
    let artifact_path = strategy_store_artifact_path(root);
    let mut artifact = begin_strategy_store_artifact(root, observed, target)?;
    let mut applied = Vec::new();
    for edge in strategy_store_path(observed.format_id, target.format_id)? {
        migration_failpoint("before-strategy-store-edge")?;
        delegate_strategy_store_edge(edge, root)?;
        let reached = read_strategy_store_format(&path)?;
        ensure!(
            reached.format_id == edge.to,
            "migration_postcondition_failed"
        );
        // The physical store is at the edge's target now. A stop between here
        // and the record write leaves a pending record over a converted store;
        // the next run reconciles it instead of losing the completion.
        migration_failpoint("after-strategy-store-edge")?;
        if !artifact
            .applied_step_ids
            .iter()
            .any(|id| id.as_str() == edge.step_id)
        {
            artifact.applied_step_ids.push(edge.step_id.to_owned());
        }
        write_json_atomic(&artifact_path, &artifact).context("migration_step_failed")?;
        applied.push(edge.step_id);
    }
    // "Applied" means the store is at the layout this binary calls current. A
    // conversion that stopped at an intermediate layout leaves the record
    // pending, because the rest of the path is still owed.
    if target.format_id == current_strategy_format().format_id && artifact.status != "applied" {
        artifact.status = strategy_store_artifact::transition(
            strategy_store_artifact::State::from_name(&artifact.status)
                .context("migration_step_failed")?,
            strategy_store_artifact::Event::Complete,
        )
        .context("migration_step_failed")?
        .as_str()
        .to_owned();
        write_json_atomic(&artifact_path, &artifact).context("migration_step_failed")?;
    }
    Ok(applied)
}
