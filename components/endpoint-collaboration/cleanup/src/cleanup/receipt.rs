//! The restricted receipt path and the file-stage receipt.
//!
//! A cleanup reports progress to the replacement endpoint over one restricted,
//! authenticated control path that the endpoint already admits its own
//! operations on. This module owns the *shape* of that report; the transport
//! that carries it belongs to the endpoint owner.
//!
//! Two facts are structural rather than conventional:
//!
//! * A file-stage receipt reports [`CleanupReceiptKind::FileStage`] and can
//!   never report completion. There is no conversion from it into a final
//!   receipt, and no caller flag that makes one.
//! * A receipt never restores ordinary admission. Revocation is absorbing on
//!   the endpoint side, and nothing here carries an admission decision, so a
//!   delivered file-stage receipt cannot re-admit a revoked device.

use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::journal::CleanupStage;
use super::target::CleanupTarget;

/// Largest receipt this owner will hand to a control path.
pub const MAX_CLEANUP_RECEIPT_BYTES: usize = 64 * 1024;

/// Which report a restricted envelope carries.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum CleanupReceiptKind {
    /// The file stage settled. The cleanup is partial.
    FileStage,
    /// Every required stage was observed settled.
    FinalCleanup,
}

impl CleanupReceiptKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::FileStage => "file-stage",
            Self::FinalCleanup => "final-cleanup",
        }
    }
}

/// What one delivery attempt observed.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ReceiptDelivery {
    /// The restricted path accepted the envelope.
    Delivered,
    /// The path could not carry it. The caller stays unconfirmed.
    Unavailable { code: String },
}

impl ReceiptDelivery {
    pub fn is_delivered(&self) -> bool {
        matches!(self, Self::Delivered)
    }

    pub fn unavailable(code: impl Into<String>) -> Self {
        Self::Unavailable { code: code.into() }
    }
}

/// The restricted authenticated control path a cleanup reports through.
pub trait CleanupReceiptPath: Send + Sync {
    fn backend(&self) -> &'static str;

    /// Deliver one envelope. A refusal is reported, never converted into a
    /// successful delivery.
    fn deliver(&self, envelope: &RestrictedReceiptEnvelope) -> Result<ReceiptDelivery>;
}

/// One bounded report on the restricted control path.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct RestrictedReceiptEnvelope {
    kind: CleanupReceiptKind,
    subject: String,
    device: String,
    operation: String,
    inventory_digest: String,
    /// Whether this envelope reports the whole cleanup finished.
    complete: bool,
    /// Always false. Present so a reader can assert the fact instead of
    /// inferring it from the absence of a field.
    admission_restored: bool,
    body: serde_json::Value,
    digest: String,
}

impl RestrictedReceiptEnvelope {
    pub(crate) fn new(
        kind: CleanupReceiptKind,
        target: &CleanupTarget,
        inventory_digest: &str,
        complete: bool,
        body: serde_json::Value,
    ) -> Result<Self> {
        let mut envelope = Self {
            kind,
            subject: target.subject().as_str().to_string(),
            device: target.device().as_str().to_string(),
            operation: target.operation().as_str().to_string(),
            inventory_digest: inventory_digest.to_string(),
            complete,
            admission_restored: false,
            body,
            digest: String::new(),
        };
        envelope.digest = envelope.canonical_digest()?;
        Ok(envelope)
    }

    pub fn kind(&self) -> CleanupReceiptKind {
        self.kind
    }

    pub fn subject(&self) -> &str {
        &self.subject
    }

    pub fn device(&self) -> &str {
        &self.device
    }

    pub fn operation(&self) -> &str {
        &self.operation
    }

    pub fn inventory_digest(&self) -> &str {
        &self.inventory_digest
    }

    pub fn complete(&self) -> bool {
        self.complete
    }

    pub fn admission_restored(&self) -> bool {
        self.admission_restored
    }

    pub fn body(&self) -> &serde_json::Value {
        &self.body
    }

    pub fn digest(&self) -> &str {
        &self.digest
    }

    /// Bind every authority field and the body into one digest, so a rewritten
    /// envelope is detectable at both ends.
    fn canonical_digest(&self) -> Result<String> {
        let body = serde_json::to_string(&self.body)
            .map_err(|_| anyhow::anyhow!("cleanup_receipt_unencodable"))?;
        let mut hasher = Sha256::new();
        for field in [
            self.kind.as_str(),
            self.subject.as_str(),
            self.device.as_str(),
            self.operation.as_str(),
            self.inventory_digest.as_str(),
        ] {
            hasher.update(field.as_bytes());
            hasher.update([0]);
        }
        hasher.update([u8::from(self.complete)]);
        hasher.update([u8::from(self.admission_restored)]);
        hasher.update(body.as_bytes());
        let digest = format!("{:x}", hasher.finalize());
        ensure!(
            serde_json::to_string(self)
                .map(|encoded| encoded.len() <= MAX_CLEANUP_RECEIPT_BYTES)
                .unwrap_or(false),
            "cleanup_receipt_exceeds_bound"
        );
        Ok(digest)
    }
}

/// The report the file stage emits once every frozen file entry is settled.
///
/// It is deliberately not a cleanup result. [`Self::complete`] is `false`,
/// [`Self::outstanding_stages`] names credential deletion, terminal settlement
/// and completion while they are still pending, and the type has no conversion
/// into a final receipt.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct FileStageReceipt {
    subject: String,
    device: String,
    operation: String,
    inventory_digest: String,
    removed_count: usize,
    already_absent_count: usize,
    removed_file_bytes: u64,
    journal_revision: u64,
    outstanding_stages: Vec<CleanupStage>,
}

impl FileStageReceipt {
    pub(crate) fn new(
        target: &CleanupTarget,
        inventory_digest: &str,
        removed_count: usize,
        already_absent_count: usize,
        removed_file_bytes: u64,
        journal_revision: u64,
        outstanding_stages: Vec<CleanupStage>,
    ) -> Self {
        Self {
            subject: target.subject().as_str().to_string(),
            device: target.device().as_str().to_string(),
            operation: target.operation().as_str().to_string(),
            inventory_digest: inventory_digest.to_string(),
            removed_count,
            already_absent_count,
            removed_file_bytes,
            journal_revision,
            outstanding_stages,
        }
    }

    pub fn subject(&self) -> &str {
        &self.subject
    }

    pub fn device(&self) -> &str {
        &self.device
    }

    pub fn operation(&self) -> &str {
        &self.operation
    }

    pub fn inventory_digest(&self) -> &str {
        &self.inventory_digest
    }

    pub fn removed_count(&self) -> usize {
        self.removed_count
    }

    pub fn already_absent_count(&self) -> usize {
        self.already_absent_count
    }

    pub fn removed_file_bytes(&self) -> u64 {
        self.removed_file_bytes
    }

    pub fn journal_revision(&self) -> u64 {
        self.journal_revision
    }

    /// Stages that are still pending when this receipt is truthful.
    pub fn outstanding_stages(&self) -> &[CleanupStage] {
        &self.outstanding_stages
    }

    /// Always false. A file stage cannot report a finished cleanup.
    pub const fn complete(&self) -> bool {
        false
    }

    /// Always false. No receipt re-admits a revoked device.
    pub const fn admission_restored(&self) -> bool {
        false
    }

    pub fn kind(&self) -> CleanupReceiptKind {
        CleanupReceiptKind::FileStage
    }

    /// The restricted envelope this receipt is delivered as.
    pub fn envelope(&self) -> Result<RestrictedReceiptEnvelope> {
        let target = CleanupTarget::new(
            super::target::CleanupSubject::new(self.subject.clone())?,
            super::target::DeviceId::new(self.device.clone())?,
            super::target::OperationId::new(self.operation.clone())?,
        );
        RestrictedReceiptEnvelope::new(
            CleanupReceiptKind::FileStage,
            &target,
            &self.inventory_digest,
            false,
            serde_json::json!({
                "kind": CleanupReceiptKind::FileStage.as_str(),
                "complete": false,
                "admissionRestored": false,
                "removedCount": self.removed_count,
                "alreadyAbsentCount": self.already_absent_count,
                "removedFileBytes": self.removed_file_bytes,
                "journalRevision": self.journal_revision,
                "outstandingStages": self
                    .outstanding_stages
                    .iter()
                    .map(|stage| stage.as_str())
                    .collect::<Vec<_>>(),
            }),
        )
    }

    /// Deliver this receipt over the restricted control path.
    ///
    /// A rejected or unavailable path stays rejected; the caller must not read
    /// it as a delivered progress report.
    pub fn deliver(&self, path: &dyn CleanupReceiptPath) -> Result<ReceiptDelivery> {
        path.deliver(&self.envelope()?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cleanup::target::{CleanupSubject, DeviceId, OperationId};

    fn target() -> CleanupTarget {
        CleanupTarget::new(
            CleanupSubject::new("subject-a").unwrap(),
            DeviceId::new("device-a").unwrap(),
            OperationId::generate(),
        )
    }

    #[test]
    fn a_file_stage_receipt_never_reports_completion_or_admission() {
        let receipt = FileStageReceipt::new(
            &target(),
            "digest",
            2,
            1,
            30,
            3,
            CleanupStage::FilesSettled.outstanding(),
        );
        assert!(!receipt.complete());
        assert!(!receipt.admission_restored());
        let envelope = receipt.envelope().unwrap();
        assert_eq!(envelope.kind(), CleanupReceiptKind::FileStage);
        assert!(!envelope.complete());
        assert!(!envelope.admission_restored());
        assert_eq!(envelope.digest().len(), 64);
        assert_eq!(
            envelope.body()["outstandingStages"],
            serde_json::json!(["credentials-settled", "terminal-settlement", "complete"])
        );
    }

    #[test]
    fn the_envelope_digest_binds_every_authority_field() {
        let base = RestrictedReceiptEnvelope::new(
            CleanupReceiptKind::FileStage,
            &target(),
            "digest-a",
            false,
            serde_json::json!({"removedCount": 1}),
        )
        .unwrap();
        let other_digest = RestrictedReceiptEnvelope::new(
            CleanupReceiptKind::FileStage,
            &target(),
            "digest-b",
            false,
            serde_json::json!({"removedCount": 1}),
        )
        .unwrap();
        let completed = RestrictedReceiptEnvelope::new(
            CleanupReceiptKind::FinalCleanup,
            &target(),
            "digest-a",
            true,
            serde_json::json!({"removedCount": 1}),
        )
        .unwrap();
        assert_ne!(base.digest(), other_digest.digest());
        assert_ne!(base.digest(), completed.digest());
        assert!(completed.complete());
        assert!(!completed.admission_restored());
    }
}
