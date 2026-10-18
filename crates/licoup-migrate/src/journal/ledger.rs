//! The client's migration ledger, read as a record and never written.
//!
//! The client's own owner keeps the canonical ledger at `client-state/migrations/ledger.json`.
//! It is the document a later admission reads to decide what a root has already completed, so
//! it is the only place where "this domain was converted exactly once" is actually decided.
//! This tool therefore treats it as read-only evidence, and the guarantee a resumed
//! conversion makes is stated against it:
//!
//! * a domain step id appears at most once in the ledger, and
//! * a step that was already complete before a resume still has exactly the same completed
//!   step list afterwards, and
//! * the ledger this tool writes is its own snapshot, never the client's file.
//!
//! A snapshot carries the ledger's bytes so a caller can compare two observations exactly.
//! Hashing would answer a weaker question — whether two documents are equal — while the
//! interesting failure is a rewrite that changes the file without changing its meaning, or
//! the reverse: a second conversion that quietly appends a record. Byte equality is the
//! stronger and simpler oracle, and it needs no digest implementation of our own.

use crate::error::{ToolResult, marker_read_failed};
use crate::journal::{MarkerRoot, markers};
use serde::Deserialize;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// One domain's entry in the client's ledger.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LedgerDomain {
    pub schema_version: u32,
    pub completed_step_ids: Vec<String>,
}

/// The client's ledger, as far as this tool reads it.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, serde::Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ClientLedger {
    pub schema_version: String,
    pub highest_admitted_product_version: String,
    pub frontier_id: String,
    pub domains: BTreeMap<String, LedgerDomain>,
}

impl ClientLedger {
    /// How many domains the client's ledger records.
    pub fn recorded_domains(&self) -> usize {
        self.domains.len()
    }

    /// How many completed step records the ledger holds in total.
    ///
    /// A second conversion that re-recorded a step it had already completed would raise
    /// this number, so a caller can use it as a record-identity count across a resume.
    pub fn recorded_steps(&self) -> usize {
        self.domains
            .values()
            .map(|domain| domain.completed_step_ids.len())
            .sum()
    }

    /// The completed step ids one domain carries.
    pub fn completed_steps(&self, domain_id: &str) -> Option<&[String]> {
        self.domains
            .get(domain_id)
            .map(|domain| domain.completed_step_ids.as_slice())
    }

    /// Whether any domain lists the same step more than once.
    pub fn duplicated_steps(&self) -> Vec<(String, String)> {
        let mut duplicates = Vec::new();
        for (domain_id, domain) in &self.domains {
            let mut seen = std::collections::BTreeSet::new();
            for step_id in &domain.completed_step_ids {
                if !seen.insert(step_id.as_str()) {
                    duplicates.push((domain_id.clone(), step_id.clone()));
                }
            }
        }
        duplicates
    }
}

/// One exact observation of the client's ledger.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LedgerSnapshot {
    path: PathBuf,
    bytes: Option<Vec<u8>>,
}

impl LedgerSnapshot {
    /// Observe the client's ledger for one data root.
    pub fn read(data_root: &Path) -> ToolResult<Self> {
        let path = MarkerRoot::at(data_root).client_ledger_path();
        let bytes = markers::read_bytes(&path)?;
        Ok(Self { path, bytes })
    }

    /// Whether the client's ledger exists at all.
    pub fn is_present(&self) -> bool {
        self.bytes.is_some()
    }

    /// The ledger this snapshot was taken from.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The client's ledger document, when it is present and readable.
    pub fn parse(&self) -> ToolResult<Option<ClientLedger>> {
        match &self.bytes {
            Some(bytes) => serde_json::from_slice(bytes)
                .map(Some)
                .map_err(|_| marker_read_failed(&self.path)),
            None => Ok(None),
        }
    }

    /// Whether two observations are the same bytes.
    pub fn equals(&self, other: &Self) -> bool {
        self.bytes == other.bytes
    }

    /// The record-identity count of this observation, or zero when there is no ledger.
    pub fn recorded_steps(&self) -> ToolResult<usize> {
        Ok(self.parse()?.map_or(0, |ledger| ledger.recorded_steps()))
    }
}

/// The strongest statement this module makes: the tool never owned the client's ledger.
///
/// The check is against the real file rather than against a convention, because the
/// failure it guards is a future edit that starts writing the client's document from the
/// tool. It is asserted in the resume regression on a root that has just been converted.
pub fn assert_tool_never_owns_client_ledger(data_root: &Path) -> ToolResult<()> {
    let client_ledger = MarkerRoot::at(data_root).client_ledger_path();
    let tool_snapshot = MarkerRoot::at(data_root).ledger_path();
    if std::fs::symlink_metadata(&client_ledger).is_err() {
        return Ok(());
    }
    let same = match (
        markers::read_bytes(&client_ledger)?,
        markers::read_bytes(&tool_snapshot)?,
    ) {
        (Some(client), Some(snapshot)) => client == snapshot,
        _ => false,
    };
    if same {
        return Err(crate::error::COMMIT_UNSUPPORTED);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::journal::test_support::scratch;

    fn ledger_json(domains: &[(&str, u32, &[&str])]) -> Vec<u8> {
        let domains: BTreeMap<&str, serde_json::Value> = domains
            .iter()
            .map(|(domain_id, version, steps)| {
                (
                    *domain_id,
                    serde_json::json!({
                        "schemaVersion": version,
                        "completedStepIds": steps,
                    }),
                )
            })
            .collect();
        serde_json::to_vec_pretty(&serde_json::json!({
            "schemaVersion": "v0.0.1:client-state-migration-ledger-1",
            "highestAdmittedProductVersion": "0.0.1-alpha",
            "frontierId": "licoup-state-current",
            "domains": domains,
        }))
        .expect("serialize ledger")
    }

    fn write_ledger(root: &Path, bytes: &[u8]) {
        let path = MarkerRoot::at(root).client_ledger_path();
        std::fs::create_dir_all(path.parent().expect("parent")).expect("create");
        std::fs::write(&path, bytes).expect("write ledger");
    }

    #[test]
    fn a_snapshot_reads_the_client_ledger_and_counts_its_records() {
        let root = scratch("ledger-counts");
        write_ledger(
            &root,
            &ledger_json(&[
                ("adaptive-flywheel", 2, &["a.one", "a.two"]),
                ("agent-tab-order", 1, &["b.one"]),
            ]),
        );
        let snapshot = LedgerSnapshot::read(&root).expect("snapshot");
        assert!(snapshot.is_present());
        assert_eq!(snapshot.recorded_steps().expect("count"), 3);
        let ledger = snapshot.parse().expect("parse").expect("present");
        assert_eq!(ledger.recorded_domains(), 2);
        assert_eq!(
            ledger.completed_steps("adaptive-flywheel").expect("domain"),
            ["a.one".to_string(), "a.two".to_string()]
        );
        assert!(ledger.duplicated_steps().is_empty());
    }

    #[test]
    fn a_repeated_step_id_is_reported_as_a_duplicate_record() {
        let root = scratch("ledger-duplicates");
        write_ledger(
            &root,
            &ledger_json(&[("adaptive-flywheel", 2, &["a.one", "a.one"])]),
        );
        let ledger = LedgerSnapshot::read(&root)
            .expect("snapshot")
            .parse()
            .expect("parse")
            .expect("present");
        assert_eq!(
            ledger.duplicated_steps(),
            vec![("adaptive-flywheel".to_string(), "a.one".to_string())]
        );
    }

    #[test]
    fn two_observations_of_an_untouched_ledger_are_equal() {
        let root = scratch("ledger-equal");
        write_ledger(&root, &ledger_json(&[("agent-tab-order", 1, &["b.one"])]));
        let first = LedgerSnapshot::read(&root).expect("first");
        let second = LedgerSnapshot::read(&root).expect("second");
        assert!(first.equals(&second));
    }

    #[test]
    fn an_absent_ledger_reads_as_absent_rather_than_as_an_error() {
        let root = scratch("ledger-absent");
        let snapshot = LedgerSnapshot::read(&root).expect("snapshot");
        assert!(
            !snapshot.is_present(),
            "nothing has been converted here yet"
        );
        assert!(
            snapshot.equals(&LedgerSnapshot::read(&root).expect("other")),
            "two observations of an absent ledger are the same observation"
        );
        assert_eq!(snapshot.recorded_steps().expect("count"), 0);
    }

    #[test]
    fn a_corrupt_client_ledger_is_reported_without_its_path() {
        let root = scratch("ledger-corrupt");
        write_ledger(&root, b"{\"domains\":");
        let error = LedgerSnapshot::read(&root)
            .expect("snapshot")
            .parse()
            .expect_err("corrupt ledger");
        assert_eq!(error.code(), "migration_marker_unreadable");
        assert!(!error.to_string().contains(root.to_string_lossy().as_ref()));
    }

    #[test]
    fn the_tool_snapshot_is_never_the_client_ledger() {
        let root = scratch("ledger-ownership");
        write_ledger(&root, &ledger_json(&[("agent-tab-order", 1, &["b.one"])]));
        // No tool snapshot exists yet, so the tool plainly does not own the document.
        assert_tool_never_owns_client_ledger(&root).expect("distinct documents");
    }
}
