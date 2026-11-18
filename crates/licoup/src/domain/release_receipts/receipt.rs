//! Immutable local receipt: source/tree/version/target/template/artifact
//! binding plus per-check closure status. Remote and signing actions stay
//! `authorization_required`.

use std::collections::BTreeMap;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ClosureStatus {
    Verified,
    NotRun,
    AuthorizationRequired,
}

impl ClosureStatus {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Verified => "verified",
            Self::NotRun => "not_run",
            Self::AuthorizationRequired => "authorization_required",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum ReceiptCheck {
    Build,
    Install,
    Update,
    Launch,
    Identity,
    Ruleset,
    Branch,
    Train,
    Sign,
    Publish,
    RemoteMutation,
}

impl ReceiptCheck {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Build => "build",
            Self::Install => "install",
            Self::Update => "update",
            Self::Launch => "launch",
            Self::Identity => "identity",
            Self::Ruleset => "ruleset",
            Self::Branch => "branch",
            Self::Train => "train",
            Self::Sign => "sign",
            Self::Publish => "publish",
            Self::RemoteMutation => "remote_mutation",
        }
    }

    pub const fn requires_external_authority(self) -> bool {
        matches!(self, Self::Sign | Self::Publish | Self::RemoteMutation)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ReleaseTrainEdge {
    CandidateToNightly,
    NightlyToStable,
    StableToRelease,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReceiptBinding {
    pub source_revision: String,
    pub source_tree: String,
    pub version: String,
    pub target: String,
    pub template_digest: String,
    pub artifact_digest: String,
}

impl ReceiptBinding {
    pub fn validate(&self) -> Result<(), ReceiptError> {
        if !is_object_id(&self.source_revision) || !is_object_id(&self.source_tree) {
            return Err(ReceiptError::InvalidBinding);
        }
        if self.version.is_empty() || !is_target(&self.target) {
            return Err(ReceiptError::InvalidBinding);
        }
        if !is_digest(&self.template_digest) || !is_digest(&self.artifact_digest) {
            return Err(ReceiptError::InvalidBinding);
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ReceiptError {
    InvalidBinding,
    MultipleTargets,
    CheckNotRunnable,
    ExternalAuthorityRequired,
    ImmutableField,
}

impl ReceiptError {
    pub const fn code(self) -> &'static str {
        match self {
            Self::InvalidBinding => "receipt_invalid_binding",
            Self::MultipleTargets => "receipt_multiple_targets",
            Self::CheckNotRunnable => "receipt_check_not_runnable",
            Self::ExternalAuthorityRequired => "authorization_required",
            Self::ImmutableField => "receipt_immutable",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LocalReleaseReceipt {
    binding: ReceiptBinding,
    checks: BTreeMap<ReceiptCheck, ClosureStatus>,
    train_edge: Option<ReleaseTrainEdge>,
}

impl LocalReleaseReceipt {
    pub fn open(binding: ReceiptBinding) -> Result<Self, ReceiptError> {
        if binding.target.contains('+') || binding.target.contains(',') {
            return Err(ReceiptError::MultipleTargets);
        }
        binding.validate()?;
        let mut checks = BTreeMap::new();
        for check in [
            ReceiptCheck::Build,
            ReceiptCheck::Install,
            ReceiptCheck::Update,
            ReceiptCheck::Launch,
            ReceiptCheck::Identity,
            ReceiptCheck::Ruleset,
            ReceiptCheck::Branch,
            ReceiptCheck::Train,
            ReceiptCheck::Sign,
            ReceiptCheck::Publish,
            ReceiptCheck::RemoteMutation,
        ] {
            let status = if check.requires_external_authority() {
                ClosureStatus::AuthorizationRequired
            } else {
                ClosureStatus::NotRun
            };
            checks.insert(check, status);
        }
        Ok(Self {
            binding,
            checks,
            train_edge: None,
        })
    }

    pub fn binding(&self) -> &ReceiptBinding {
        &self.binding
    }

    pub fn status(&self, check: ReceiptCheck) -> ClosureStatus {
        self.checks
            .get(&check)
            .copied()
            .unwrap_or(ClosureStatus::NotRun)
    }

    pub fn record_local(
        &mut self,
        check: ReceiptCheck,
        status: ClosureStatus,
    ) -> Result<(), ReceiptError> {
        if check.requires_external_authority() {
            return Err(ReceiptError::ExternalAuthorityRequired);
        }
        if status == ClosureStatus::AuthorizationRequired {
            return Err(ReceiptError::CheckNotRunnable);
        }
        self.checks.insert(check, status);
        Ok(())
    }

    pub fn external_action(&self, check: ReceiptCheck) -> Result<ClosureStatus, ReceiptError> {
        if !check.requires_external_authority() {
            return Err(ReceiptError::CheckNotRunnable);
        }
        Ok(ClosureStatus::AuthorizationRequired)
    }

    pub fn bind_train_edge(&mut self, edge: ReleaseTrainEdge) -> Result<(), ReceiptError> {
        if self.train_edge.is_some() {
            return Err(ReceiptError::ImmutableField);
        }
        self.train_edge = Some(edge);
        self.checks
            .insert(ReceiptCheck::Train, ClosureStatus::NotRun);
        Ok(())
    }

    pub fn train_edge(&self) -> Option<ReleaseTrainEdge> {
        self.train_edge
    }

    pub fn target_count(&self) -> usize {
        1
    }
}

fn is_object_id(value: &str) -> bool {
    let len = value.len();
    (len == 40 || len == 64)
        && value.bytes().all(|byte| byte.is_ascii_hexdigit())
        && !value.bytes().all(|byte| byte == b'0')
}

fn is_digest(value: &str) -> bool {
    let Some(hex) = value.strip_prefix("sha256:") else {
        return false;
    };
    hex.len() == 64 && hex.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn is_target(value: &str) -> bool {
    let mut chars = value.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    first.is_ascii_lowercase()
        && value
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
        && !value.contains("++")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn binding() -> ReceiptBinding {
        ReceiptBinding {
            source_revision: "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".to_owned(),
            source_tree: "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb".to_owned(),
            version: "0.1.0".to_owned(),
            target: "macos-universal".to_owned(),
            template_digest: format!("sha256:{}", "11".repeat(32)),
            artifact_digest: format!("sha256:{}", "22".repeat(32)),
        }
    }

    #[test]
    fn one_target_receipt_starts_local_checks_not_run() {
        let receipt = LocalReleaseReceipt::open(binding()).expect("open");
        assert_eq!(receipt.target_count(), 1);
        assert_eq!(receipt.status(ReceiptCheck::Build), ClosureStatus::NotRun);
        assert_eq!(
            receipt.status(ReceiptCheck::Sign),
            ClosureStatus::AuthorizationRequired
        );
        assert_eq!(
            receipt.status(ReceiptCheck::Publish),
            ClosureStatus::AuthorizationRequired
        );
    }

    #[test]
    fn combined_targets_are_rejected() {
        let mut value = binding();
        value.target = "macos-universal+linux-x64".to_owned();
        assert_eq!(
            LocalReleaseReceipt::open(value).expect_err("multi").code(),
            "receipt_multiple_targets"
        );
    }

    #[test]
    fn signing_and_publish_never_become_verified() {
        let mut receipt = LocalReleaseReceipt::open(binding()).expect("open");
        receipt
            .record_local(ReceiptCheck::Build, ClosureStatus::Verified)
            .expect("build");
        assert_eq!(
            receipt
                .record_local(ReceiptCheck::Sign, ClosureStatus::Verified)
                .expect_err("sign")
                .code(),
            "authorization_required"
        );
        assert_eq!(
            receipt
                .external_action(ReceiptCheck::Publish)
                .expect("status"),
            ClosureStatus::AuthorizationRequired
        );
        assert_eq!(receipt.status(ReceiptCheck::Build), ClosureStatus::Verified);
    }

    #[test]
    fn train_has_exactly_three_merge_edges() {
        let mut receipt = LocalReleaseReceipt::open(binding()).expect("open");
        receipt
            .bind_train_edge(ReleaseTrainEdge::CandidateToNightly)
            .expect("edge");
        assert!(
            receipt
                .bind_train_edge(ReleaseTrainEdge::NightlyToStable)
                .is_err()
        );
        assert_eq!(
            [
                ReleaseTrainEdge::CandidateToNightly,
                ReleaseTrainEdge::NightlyToStable,
                ReleaseTrainEdge::StableToRelease
            ]
            .len(),
            3
        );
    }
}
