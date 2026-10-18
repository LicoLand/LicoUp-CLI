//! The install journal: what the host was in the middle of doing.
//!
//! A package install crosses three media — a staging directory, the active
//! package directory, and the capability catalogue — and a process can die
//! between any two of them. The plan refuses to pretend otherwise ("不能虚构一个
//! 跨文件和数据库的原子事务"), so the recovery rule is written down instead:
//! the journal records intent before each step, a stage with no `commit` entry
//! is an *abandoned* stage whose bytes are reclaimed, and the previously
//! installed version keeps serving untouched.
//!
//! The journal is append-only and bounded per line. It is a recovery record, not
//! a capability catalogue: nothing here decides what is active.

use crate::platform::extension_packages::{
    append_journal_line, ensure_private_directory, now_unix_ms, read_bounded_text, refusal,
    remove_managed_tree,
};
use licoup_application::ApplicationFailure;
use licoup_extension_contracts::deployment::PackageLifecycle;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

const JOURNAL_STAGE: &str = "extension/package-journal";
const MAX_JOURNAL_BYTES: usize = 8 * 1024 * 1024;
const JOURNAL_FILE: &str = "install.jsonl";

/// What the host was doing when it wrote the entry.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum JournalOperation {
    /// Bytes were copied into a staging directory.
    Stage,
    /// A staged package was published as the installed version.
    Commit,
    /// A staged package was dropped without being published.
    Rollback,
    /// A package was removed.
    Uninstall,
    /// Unreferenced artifacts were reclaimed.
    Gc,
}

/// One recovery record.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct JournalEntry {
    pub at_unix_ms: i64,
    pub operation: JournalOperation,
    pub package_id: String,
    pub version: String,
    pub state: PackageLifecycle,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

/// A staged directory the host found on disk during recovery.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StagedDirectory {
    pub package_id: String,
    pub version: String,
    pub path: PathBuf,
    pub bytes: u64,
}

/// A stage that was never committed, with the bytes it was holding.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AbandonedStage {
    pub package_id: String,
    pub version: String,
    pub path: PathBuf,
    pub reclaimed_bytes: u64,
}

/// What recovery found and what it did about it.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct RecoveryReport {
    pub abandoned: Vec<AbandonedStage>,
    pub reclaimed_bytes: u64,
    /// Installed versions that keep serving: recovery never touches them.
    pub installed_untouched: Vec<String>,
    /// Removals whose intent was recorded and whose bytes recovery finished
    /// reclaiming, as `package@version`.
    pub finished_removals: Vec<String>,
}

impl RecoveryReport {
    pub fn is_clean(&self) -> bool {
        self.abandoned.is_empty()
    }
}

/// The append-only install journal of one managed root.
#[derive(Clone, Debug)]
pub struct InstallJournal {
    root: PathBuf,
    path: PathBuf,
}

impl InstallJournal {
    /// Open (creating when absent) the journal below a managed root.
    pub fn open(root: &Path) -> Result<Self, ApplicationFailure> {
        let directory = root.join("journal");
        ensure_private_directory(&directory)?;
        Ok(Self {
            root: root.to_path_buf(),
            path: directory.join(JOURNAL_FILE),
        })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Record one step before it is taken.
    pub fn append(
        &self,
        operation: JournalOperation,
        package_id: &str,
        version: &str,
        state: PackageLifecycle,
        note: Option<&str>,
    ) -> Result<JournalEntry, ApplicationFailure> {
        let entry = JournalEntry {
            at_unix_ms: now_unix_ms(),
            operation,
            package_id: package_id.to_owned(),
            version: version.to_owned(),
            state,
            note: note.map(str::to_owned),
        };
        let line = serde_json::to_string(&entry).map_err(|_| {
            refusal("package_journal_entry_invalid", JOURNAL_STAGE).with_field("entry")
        })?;
        append_journal_line(&self.path, &line)?;
        Ok(entry)
    }

    /// Every entry, oldest first. A truncated final line is dropped rather than
    /// refusing the whole journal: a process that died mid-write loses the last
    /// intent, not the recovery record.
    pub fn entries(&self) -> Result<Vec<JournalEntry>, ApplicationFailure> {
        let Some(content) = read_bounded_text(&self.path, MAX_JOURNAL_BYTES)? else {
            return Ok(Vec::new());
        };
        let mut entries = Vec::new();
        for line in content.lines() {
            if line.trim().is_empty() {
                continue;
            }
            if let Ok(entry) = serde_json::from_str::<JournalEntry>(line) {
                entries.push(entry);
            }
        }
        Ok(entries)
    }

    /// Whether this package version was ever published to the active directory.
    pub fn committed(&self, package_id: &str, version: &str) -> Result<bool, ApplicationFailure> {
        Ok(self.entries()?.iter().any(|entry| {
            entry.operation == JournalOperation::Commit
                && entry.package_id == package_id
                && entry.version == version
        }))
    }

    /// Reconcile a crash: a stage with no commit entry is abandoned.
    ///
    /// Abandoned stages are reclaimed here, because nothing references them: the
    /// active directory never saw them, and the version they were replacing is
    /// still installed. A stage that was committed is left alone — the rename
    /// already happened, and deleting it would delete the installed package.
    ///
    /// This is the journal-only view: `installed_untouched` stays empty because a
    /// commit line in history cannot say what is installed *now*. The caller that
    /// can check the active directory fills that field in.
    pub fn recover(
        &self,
        staged: &[StagedDirectory],
    ) -> Result<RecoveryReport, ApplicationFailure> {
        let committed: BTreeSet<(String, String)> = self
            .entries()?
            .into_iter()
            .filter(|entry| entry.operation == JournalOperation::Commit)
            .map(|entry| (entry.package_id, entry.version))
            .collect();
        let mut report = RecoveryReport::default();
        for stage in staged {
            if committed.contains(&(stage.package_id.clone(), stage.version.clone())) {
                continue;
            }
            let bytes = if stage.bytes > 0 {
                stage.bytes
            } else {
                crate::platform::extension_packages::directory_bytes(&stage.path)?
            };
            remove_managed_tree(&stage.path)?;
            report.abandoned.push(AbandonedStage {
                package_id: stage.package_id.clone(),
                version: stage.version.clone(),
                path: stage.path.clone(),
                reclaimed_bytes: bytes,
            });
            report.reclaimed_bytes += bytes;
        }
        Ok(report)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::platform::extension_packages::{ensure_private_directory, unique_suffix};

    fn root(tag: &str) -> PathBuf {
        std::env::temp_dir().join(format!("licoup-pkg-journal-{tag}-{}", unique_suffix()))
    }

    #[test]
    fn a_journal_records_intent_and_replays_it_in_order() {
        let root = root("order");
        let journal = InstallJournal::open(&root).expect("journal");
        journal
            .append(
                JournalOperation::Stage,
                "example.specialist.echo",
                "1.0.0",
                PackageLifecycle::Staged,
                None,
            )
            .expect("stage");
        journal
            .append(
                JournalOperation::Commit,
                "example.specialist.echo",
                "1.0.0",
                PackageLifecycle::Installed,
                None,
            )
            .expect("commit");

        let entries = journal.entries().expect("entries");
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].operation, JournalOperation::Stage);
        assert_eq!(entries[1].operation, JournalOperation::Commit);
        assert!(entries[0].at_unix_ms <= entries[1].at_unix_ms);
        assert!(
            journal
                .committed("example.specialist.echo", "1.0.0")
                .expect("query")
        );
        assert!(
            !journal
                .committed("example.specialist.echo", "2.0.0")
                .expect("query")
        );
        remove_managed_tree(&root).expect("cleanup");
    }

    #[test]
    fn a_crash_after_staging_leaves_the_installed_version_alone() {
        let root = root("recover");
        let journal = InstallJournal::open(&root).expect("journal");
        journal
            .append(
                JournalOperation::Commit,
                "example.specialist.echo",
                "1.0.0",
                PackageLifecycle::Installed,
                None,
            )
            .expect("commit");
        journal
            .append(
                JournalOperation::Stage,
                "example.specialist.echo",
                "2.0.0",
                PackageLifecycle::Staged,
                None,
            )
            .expect("stage");

        let abandoned_path = root.join("staging").join("echo-2.0.0");
        ensure_private_directory(&abandoned_path).expect("stage dir");
        std::fs::write(abandoned_path.join("agent.py"), vec![9u8; 64]).expect("write");
        let committed_path = root.join("staging").join("echo-1.0.0");
        ensure_private_directory(&committed_path).expect("stage dir");
        std::fs::write(committed_path.join("agent.py"), vec![1u8; 16]).expect("write");

        let report = journal
            .recover(&[
                StagedDirectory {
                    package_id: "example.specialist.echo".to_owned(),
                    version: "2.0.0".to_owned(),
                    path: abandoned_path.clone(),
                    bytes: 0,
                },
                StagedDirectory {
                    package_id: "example.specialist.echo".to_owned(),
                    version: "1.0.0".to_owned(),
                    path: committed_path.clone(),
                    bytes: 0,
                },
            ])
            .expect("recover");

        assert_eq!(report.reclaimed_bytes, 64);
        assert_eq!(report.abandoned.len(), 1);
        assert_eq!(report.abandoned[0].version, "2.0.0");
        assert!(!abandoned_path.exists());
        assert!(
            committed_path.exists(),
            "a committed stage is the installed package"
        );
        assert!(
            report.installed_untouched.is_empty(),
            "the journal never claims a version is installed from commit history alone"
        );
        remove_managed_tree(&root).expect("cleanup");
    }

    #[test]
    fn a_torn_final_line_costs_the_last_intent_not_the_journal() {
        let root = root("torn");
        let journal = InstallJournal::open(&root).expect("journal");
        journal
            .append(
                JournalOperation::Stage,
                "example.specialist.echo",
                "1.0.0",
                PackageLifecycle::Staged,
                None,
            )
            .expect("stage");
        append_journal_line(journal.path(), "{\"operation\":\"com").expect("torn write");
        assert_eq!(journal.entries().expect("entries").len(), 1);
        remove_managed_tree(&root).expect("cleanup");
    }

    #[test]
    fn an_empty_journal_is_not_an_error() {
        let root = root("empty");
        let journal = InstallJournal::open(&root).expect("journal");
        assert!(journal.entries().expect("entries").is_empty());
        assert!(journal.recover(&[]).expect("recover").is_clean());
        remove_managed_tree(&root).expect("cleanup");
    }
}
