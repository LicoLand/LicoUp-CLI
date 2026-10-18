//! Run the client's own conversion and report what it concluded.
//!
//! This module holds no conversion logic. The client's migration owner is the only
//! implementation of every domain move, and it already carries the typed semantics this
//! tool does not have: the canonical conversation import runs through the Conversation
//! owner, and the strategy store is canonicalised by the workflow compiler. A second
//! implementation here would be exactly the duplicated authority the delivery forbids,
//! and it would drift from the client on the first schema change.
//!
//! What this module owns is the *report*: for each domain it was asked about, the tool
//! says whether the client converted it, found it already current, still needs platform
//! authority for it, refused to read it, or did not account for it at all. Completion is
//! never inferred. A refused authority stops the run before the owner is asked to move
//! anything, so the report carries the owner's own code instead of a failed conversion.

use crate::error::{
    DATA_ROOT_MISSING, DATA_ROOT_NOT_DIRECTORY, FRONTIER_UNAVAILABLE, ToolError, ToolResult,
    WRITERS_RUNNING,
};
use crate::inspect::{RefusedDomain, inspect};
use serde::Serialize;
use std::path::Path;

/// One domain's outcome, in the tool's own vocabulary.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum DomainOutcome {
    /// The client's owner performed the move.
    Converted,
    /// The client's owner reported the domain already at its target.
    AlreadyCurrent,
    /// The client's owner still needs platform credential authority.
    PendingAuthorization,
    /// The client's owner did not account for this domain; it stays owed.
    StillOwed,
}

/// One domain's reported outcome.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DomainConversion {
    pub domain_id: String,
    pub outcome: DomainOutcome,
}

/// What one conversion run concluded.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConvertReport {
    /// `converted`, `pendingAuthorization`, or `partial`.
    ///
    /// The tool's own lock excludes other runs of this tool; it cannot stop a client or
    /// another writer that never heard of the lock. So a run that leaves a domain owed
    /// reports that fact in its status instead of presenting the move as finished.
    pub status: &'static str,
    /// The product version the client reported as running.
    pub running_product_version: String,
    pub frontier_id: String,
    pub domains: Vec<DomainConversion>,
    /// Domains that still owe work after this run.
    pub still_owed: Vec<String>,
    /// Domains whose stored authority the client's owner refused to read. A refused
    /// domain keeps the run unfinished and is never reported as converted.
    pub refused: Vec<RefusedDomain>,
}

impl ConvertReport {
    /// Whether every domain this run was asked about is converted or already current and
    /// no authority was refused.
    pub fn is_complete(&self) -> bool {
        self.still_owed.is_empty() && self.refused.is_empty()
    }
}

/// Ask the client's own owner to convert this root, then classify its result.
///
/// `owed` names the domains the caller believes need work; the classification only uses
/// the client's result, so an empty result leaves every owed domain in `still_owed`
/// rather than reporting a success the owner did not claim. `writers_stopped` is the
/// operator's statement that no writer is running against the root: the run refuses
/// without it, before it opens or moves anything, because the client's owner is the only
/// authority that may move a domain and nothing in this tool can stop a foreign writer.
pub fn convert(
    data_root: &Path,
    owed: &[String],
    writers_stopped: bool,
) -> ToolResult<ConvertReport> {
    if !data_root.exists() {
        return Err(DATA_ROOT_MISSING);
    }
    if !data_root.is_dir() {
        return Err(DATA_ROOT_NOT_DIRECTORY);
    }
    if !writers_stopped {
        return Err(WRITERS_RUNNING);
    }
    let absolute = data_root.canonicalize().map_err(|_| DATA_ROOT_MISSING)?;

    // The owner is asked to move a domain only after every authority it would probe
    // reads cleanly. A refused authority is the client's own verdict about this root,
    // so the run reports it and stops before opening anything for writing.
    let observed = inspect(&absolute)?;
    if !observed.refused.is_empty() {
        return Ok(ConvertReport {
            status: "partial",
            running_product_version:
                licoup_native::domain::client_state_migration::running_product_version()
                    .map_err(|_| FRONTIER_UNAVAILABLE)?
                    .to_string(),
            frontier_id: observed.frontier_id,
            domains: owed
                .iter()
                .map(|domain_id| DomainConversion {
                    domain_id: domain_id.clone(),
                    outcome: DomainOutcome::StillOwed,
                })
                .collect(),
            still_owed: owed.to_vec(),
            refused: observed.refused,
        });
    }

    let admission = licoup_native::domain::client_state_migration::admit(&absolute)
        .map_err(|_| ToolError::new("migration_owner_refused"))?;

    let applied: std::collections::BTreeSet<&str> = admission
        .applied_domain_ids
        .iter()
        .map(String::as_str)
        .collect();
    let unchanged: std::collections::BTreeSet<&str> = admission
        .skipped_domain_ids
        .iter()
        .map(String::as_str)
        .collect();
    let awaiting: std::collections::BTreeSet<&str> = admission
        .pending_authorization_domain_ids
        .iter()
        .map(String::as_str)
        .collect();

    let mut domains = Vec::with_capacity(owed.len());
    let mut still_owed = Vec::new();
    for domain_id in owed {
        let outcome = if applied.contains(domain_id.as_str()) {
            DomainOutcome::Converted
        } else if unchanged.contains(domain_id.as_str()) {
            DomainOutcome::AlreadyCurrent
        } else if awaiting.contains(domain_id.as_str()) {
            DomainOutcome::PendingAuthorization
        } else {
            DomainOutcome::StillOwed
        };
        if matches!(
            outcome,
            DomainOutcome::StillOwed | DomainOutcome::PendingAuthorization
        ) {
            still_owed.push(domain_id.clone());
        }
        domains.push(DomainConversion {
            domain_id: domain_id.clone(),
            outcome,
        });
    }

    // The status is a statement about what this run left behind, so it is derived from
    // the owner's own verdict and never asserted: a root with an owed domain reports an
    // owed domain. `pendingAuthorization` names the one remaining case the client's owner
    // itself cannot finish without platform authority, and any other owed domain is
    // reported as the partial result it is.
    let status = if still_owed.is_empty() {
        "converted"
    } else if still_owed.iter().all(|domain_id| {
        domains.iter().any(|domain| {
            domain.domain_id == *domain_id && domain.outcome == DomainOutcome::PendingAuthorization
        })
    }) {
        "pendingAuthorization"
    } else {
        "partial"
    };

    Ok(ConvertReport {
        status,
        running_product_version: admission.running_product_version,
        frontier_id: admission.frontier_id,
        domains,
        still_owed,
        refused: Vec::new(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use licoup_native::platform::llm_api_key_vault::{
        LegacyCredentialMigrationDisposition, PlatformLlmApiKeyVault,
    };

    fn scratch(name: &str) -> std::path::PathBuf {
        let base = std::env::temp_dir().canonicalize().expect("temp dir");
        let root = base.join(format!(
            "licoup-migrate-convert-{name}-{}",
            std::process::id()
        ));
        if root.exists() {
            std::fs::remove_dir_all(&root).expect("clear");
        }
        std::fs::create_dir_all(&root).expect("create");
        root
    }

    #[test]
    fn a_conversion_run_reports_every_domain_it_was_asked_about() {
        let root = scratch("report");
        let owed = vec![
            "canonical-conversation".to_string(),
            "adaptive-flywheel".to_string(),
        ];
        let report = convert(&root, &owed, true).expect("the owner runs on a disposable root");
        assert_eq!(report.domains.len(), 2);
        for domain in &report.domains {
            assert!(
                owed.contains(&domain.domain_id),
                "only asked-about domains are reported"
            );
        }
        // The owner performs both of these itself, so neither stays owed.
        assert!(
            report.still_owed.is_empty(),
            "the owner accounted for both domains: {:?}",
            report.still_owed
        );
        assert!(report.is_complete());
        assert_eq!(report.status, "converted");
    }

    #[test]
    fn a_fresh_root_reports_the_platform_credential_owners_verdict() {
        let root = scratch("owed");
        // Every domain the client's frontier declares, including the credential domain
        // whose owner decides whether this platform needs separate authorization.
        let owed: Vec<String> =
            licoup_native::domain::client_state_migration::frontier_projection_struct()
                .expect("frontier")
                .domains
                .into_iter()
                .map(|domain| domain.domain_id)
                .collect();
        let report = convert(&root, &owed, true).expect("the owner runs");
        let requires_authorization =
            PlatformLlmApiKeyVault::legacy_credential_migration_disposition()
                .expect("platform credential disposition")
                == LegacyCredentialMigrationDisposition::RequiresAuthorization;
        assert_eq!(!report.still_owed.is_empty(), requires_authorization);
        assert_eq!(!report.is_complete(), requires_authorization);
        assert_eq!(
            report.status,
            if requires_authorization {
                "pendingAuthorization"
            } else {
                "converted"
            },
            "the report follows the credential owner's platform verdict: {report:?}"
        );
    }

    #[test]
    fn a_missing_root_is_refused_before_the_owner_runs() {
        let root = scratch("missing");
        let absent = root.join("absent");
        assert_eq!(
            convert(&absent, &[], true).expect_err("missing root"),
            DATA_ROOT_MISSING
        );
    }

    #[test]
    fn a_run_without_the_stopped_writer_statement_mutates_nothing() {
        let root = scratch("unconfirmed");
        std::fs::write(root.join("untouched.txt"), b"source").expect("seed");
        let before: Vec<String> = std::fs::read_dir(&root)
            .expect("read root")
            .filter_map(Result::ok)
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .collect();

        let error = convert(&root, &["gap".to_string()], false).expect_err("refused");
        assert_eq!(error, WRITERS_RUNNING);
        let after: Vec<String> = std::fs::read_dir(&root)
            .expect("read root")
            .filter_map(Result::ok)
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(
            before, after,
            "a refused conversion must not open or move anything"
        );
    }
}
