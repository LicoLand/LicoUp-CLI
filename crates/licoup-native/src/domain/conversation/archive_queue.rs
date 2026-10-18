use anyhow::{Result, anyhow};
use serde_json::Value;

pub(crate) const DEFAULT_MAX_ATTEMPTS: u64 = 2;

#[derive(Clone, Debug)]
pub(crate) struct ArchiveJob {
    pub(crate) job_id: String,
    pub(crate) request: Value,
    pub(crate) target_scan: Value,
    pub(crate) status: String,
    pub(crate) phase: String,
    pub(crate) attempt: u64,
    pub(crate) max_attempts: u64,
    pub(crate) archive_result: Value,
    pub(crate) validation_result: Value,
    pub(crate) created_at: String,
    pub(crate) updated_at: String,
    pub(crate) retry_after: String,
    pub(crate) last_error: String,
    pub(crate) completed_at: String,
    pub(crate) failed_at: String,
    pub(crate) cancelled_at: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ArchiveJobStatus {
    Queued,
    Scanning,
    Archiving,
    Verifying,
    RetryScheduled,
    Completed,
    Failed,
    Cancelled,
}

impl ArchiveJobStatus {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::Scanning => "scanning",
            Self::Archiving => "archiving",
            Self::Verifying => "verifying",
            Self::RetryScheduled => "retry_scheduled",
            Self::Completed => "completed",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
        }
    }

    pub(crate) fn from_str(value: &str) -> Result<Self> {
        match value {
            "queued" => Ok(Self::Queued),
            "scanning" => Ok(Self::Scanning),
            "archiving" => Ok(Self::Archiving),
            "verifying" => Ok(Self::Verifying),
            "retry_scheduled" => Ok(Self::RetryScheduled),
            "completed" => Ok(Self::Completed),
            "failed" => Ok(Self::Failed),
            "cancelled" => Ok(Self::Cancelled),
            other => Err(anyhow!("unknown archive job status: {}", other)),
        }
    }

    pub(crate) fn terminal(self) -> bool {
        matches!(self, Self::Completed | Self::Failed | Self::Cancelled)
    }
}

pub(crate) struct RetryPolicy {
    pub(crate) max_attempts: u64,
    base_backoff_seconds: u64,
}

impl RetryPolicy {
    pub(crate) fn new(max_attempts: u64, base_backoff_seconds: u64) -> Self {
        Self {
            max_attempts: max_attempts.clamp(1, 10),
            base_backoff_seconds,
        }
    }

    pub(crate) fn should_retry(&self, attempt: u64, error_kind: &str) -> bool {
        attempt < self.max_attempts
            && matches!(
                error_kind,
                "archive_failed" | "archive_error" | "verification_failed" | "verification_error"
            )
    }

    pub(crate) fn retry_delay_seconds(&self, attempt: u64) -> u64 {
        if self.base_backoff_seconds == 0 {
            return 0;
        }
        let shift = attempt.saturating_sub(1).min(10) as u32;
        let multiplier = 1_u64.checked_shl(shift).unwrap_or(1 << 10);
        self.base_backoff_seconds.saturating_mul(multiplier)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_codec_and_terminal_classification_are_consistent() {
        for status in [
            ArchiveJobStatus::Queued,
            ArchiveJobStatus::Scanning,
            ArchiveJobStatus::Archiving,
            ArchiveJobStatus::Verifying,
            ArchiveJobStatus::RetryScheduled,
            ArchiveJobStatus::Completed,
            ArchiveJobStatus::Failed,
            ArchiveJobStatus::Cancelled,
        ] {
            assert_eq!(ArchiveJobStatus::from_str(status.as_str()).unwrap(), status);
        }
        assert!(ArchiveJobStatus::Completed.terminal());
        assert!(!ArchiveJobStatus::RetryScheduled.terminal());
    }

    #[test]
    fn retry_policy_bounds_attempts_and_exponentially_backs_off() {
        let policy = RetryPolicy::new(20, 5);
        assert_eq!(policy.max_attempts, 10);
        assert_eq!(policy.retry_delay_seconds(1), 5);
        assert_eq!(policy.retry_delay_seconds(3), 20);
        assert!(policy.should_retry(3, "verification_failed"));
        assert!(!policy.should_retry(3, "permission_denied"));
    }
}
