//! Bounded root-cause classification for terminal runtime failures.
//!
//! A caller that only receives the final surface code (for example
//! `antigravity_hook_receipt_missing` or `codex_usage_limit_exceeded`) cannot
//! tell an authentication failure from an environment mismatch. This module
//! maps the failure code, stage, and spawn/early-exit evidence onto a closed
//! root-cause vocabulary with one machine recovery hint per class. It is a
//! pure, table-driven classifier: drivers keep emitting their exact surface
//! codes and the classification is attached once at the normalization funnel,
//! never inside each driver.

/// Closed root-cause vocabulary for one terminal runtime failure.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RootCause {
    /// 401 / unauthorized / login markers: the provider rejected the
    /// credentials the launched CLI presented.
    Auth,
    /// Vendor timeout, DNS, proxy, or refused/reset connection signatures:
    /// the vendor endpoint was unreachable from the launched environment.
    NetworkUnreachable,
    /// The LicoUp-launched CLI environment differs from the user terminal
    /// (spawn failure, PATH resolution, or a session receipt that never
    /// arrived after the CLI exited early).
    EnvMismatch,
    /// Vendor usage-limit or quota exhaustion.
    Quota,
    /// Evidence does not support a more specific class.
    Unknown,
}

pub const RECOVERY_AUTH: &str = "reauthenticate_provider_and_retry";
pub const RECOVERY_NETWORK: &str = "restore_network_reachability_and_retry";
pub const RECOVERY_ENV_MISMATCH: &str =
    "subagent_env_mismatch: LicoUp-launched CLI environment differs from user terminal";
pub const RECOVERY_QUOTA: &str = "select_available_model_or_wait_for_quota_reset";
pub const RECOVERY_UNKNOWN: &str = "review_terminal_result";

impl RootCause {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Auth => "auth",
            Self::NetworkUnreachable => "network_unreachable",
            Self::EnvMismatch => "env_mismatch",
            Self::Quota => "quota",
            Self::Unknown => "unknown",
        }
    }

    /// The machine recovery hint for this class. Drivers that already carry an
    /// exact recovery keep it; this hint fills the gap.
    pub const fn recovery(self) -> &'static str {
        match self {
            Self::Auth => RECOVERY_AUTH,
            Self::NetworkUnreachable => RECOVERY_NETWORK,
            Self::EnvMismatch => RECOVERY_ENV_MISMATCH,
            Self::Quota => RECOVERY_QUOTA,
            Self::Unknown => RECOVERY_UNKNOWN,
        }
    }
}

/// The evidence available at the normalization funnel for one terminal
/// failure. `message` is the driver's fixed sanitized failure message; child
/// stderr text deliberately never crosses the driver boundary, so
/// `output_tail` stays `None` unless a driver surfaces an explicit sanitized
/// tail signature.
#[derive(Clone, Copy, Debug, Default)]
pub struct FailureEvidence<'a> {
    pub code: &'a str,
    pub stage: &'a str,
    pub message: &'a str,
    pub turn_status: Option<&'a str>,
    /// Process exit status when the CLI child already exited; `Some` on the
    /// hook-receipt path proves the CLI ended before delivering its session.
    pub status_code: Option<i32>,
    pub output_tail: Option<&'a str>,
}

const NETWORK_SIGNATURES: &[&str] = &[
    "etimedout",
    "econnrefused",
    "econnreset",
    "econnaborted",
    "ehostunreach",
    "enetunreach",
    "eai_again",
    "connection refused",
    "connection reset",
    "name resolution",
    "dns",
    "proxy",
];

const SPAWN_SIGNATURES: &[&str] = &[
    "is not available",
    "could not be started",
    "not permitted to run",
    "no such file",
    "not found in",
    "enoent",
];

const AUTH_SIGNATURES: &[&str] = &[
    "401",
    "unauthorized",
    "unauthenticated",
    "not logged in",
    "authentication required",
    "login required",
];

fn any(haystack: &str, needles: &[&str]) -> bool {
    needles.iter().any(|needle| haystack.contains(needle))
}

/// Classify one terminal failure. Rule order is significant: quota and auth
/// are the most specific vendor signals; the hook-receipt anchor requires
/// early-exit or network evidence and outranks the generic network rule;
/// spawn-phase failures are environment mismatches by construction.
pub fn classify_root_cause(evidence: &FailureEvidence<'_>) -> RootCause {
    let code = evidence.code.to_ascii_lowercase();
    let stage = evidence.stage.to_ascii_lowercase();
    let message = evidence.message.to_ascii_lowercase();
    let turn_status = evidence
        .turn_status
        .unwrap_or_default()
        .to_ascii_lowercase();
    let tail = evidence
        .output_tail
        .unwrap_or_default()
        .to_ascii_lowercase();

    if code.contains("usage_limit_exceeded")
        || turn_status.contains("usagelimitexceeded")
        || turn_status.contains("usage_limit_exceeded")
        || any(&message, &["usage limit", "quota exceeded"])
    {
        return RootCause::Quota;
    }
    if any(&code, AUTH_SIGNATURES)
        || any(&turn_status, AUTH_SIGNATURES)
        || any(&message, AUTH_SIGNATURES)
        || any(&tail, AUTH_SIGNATURES)
    {
        return RootCause::Auth;
    }
    if stage.starts_with("process/")
        && (any(&message, SPAWN_SIGNATURES) || any(&tail, SPAWN_SIGNATURES))
    {
        return RootCause::EnvMismatch;
    }
    if evidence.code == "antigravity_hook_receipt_missing"
        && (evidence.status_code.is_some()
            || any(&message, NETWORK_SIGNATURES)
            || any(&tail, NETWORK_SIGNATURES))
    {
        return RootCause::EnvMismatch;
    }
    if any(&message, NETWORK_SIGNATURES) || any(&tail, NETWORK_SIGNATURES) {
        return RootCause::NetworkUnreachable;
    }
    RootCause::Unknown
}

#[cfg(test)]
mod tests {
    use super::*;

    fn classify(
        code: &str,
        stage: &str,
        message: &str,
        turn_status: Option<&str>,
        status_code: Option<i32>,
        output_tail: Option<&str>,
    ) -> RootCause {
        classify_root_cause(&FailureEvidence {
            code,
            stage,
            message,
            turn_status,
            status_code,
            output_tail,
        })
    }

    #[test]
    fn usage_limit_codes_and_statuses_classify_as_quota() {
        for (code, turn_status, message) in [
            (
                "codex_usage_limit_exceeded",
                Some("failed/UsageLimitExceeded"),
                "Codex usage limit exceeded.",
            ),
            (
                "cursor_cli_usage_limit_exceeded",
                Some("usage_limit_exceeded"),
                "",
            ),
            (
                "vendor_turn_failed",
                None,
                "Vendor usage limit was reached.",
            ),
        ] {
            assert_eq!(
                classify(code, "turn/completed", message, turn_status, None, None),
                RootCause::Quota
            );
        }
        assert_eq!(RootCause::Quota.recovery(), RECOVERY_QUOTA);
    }

    #[test]
    fn spawn_not_found_and_path_failures_classify_as_env_mismatch() {
        for (code, message) in [
            (
                "codex_app_server_start_failed",
                "The Codex executable is not available.",
            ),
            (
                "cursor_cli_start_failed",
                "Cursor Agent CLI could not be started.",
            ),
            (
                "claude_code_start_failed",
                "The Claude Code executable is not permitted to run.",
            ),
            (
                "acp_agent_start_failed",
                "The requested ACP agent executable is not available.",
            ),
        ] {
            assert_eq!(
                classify(code, "process/start", message, None, None, None),
                RootCause::EnvMismatch,
                "{code}"
            );
        }
        assert_eq!(RootCause::EnvMismatch.recovery(), RECOVERY_ENV_MISMATCH);
        assert!(
            RootCause::EnvMismatch
                .recovery()
                .contains("subagent_env_mismatch")
        );
    }

    #[test]
    fn hook_receipt_missing_with_early_exit_or_network_signature_is_env_mismatch() {
        // The agy CLI exited before the Stop hook delivered a session receipt.
        assert_eq!(
            classify(
                "antigravity_hook_receipt_missing",
                "session/new",
                "Antigravity hook bridge did not return a native conversation identifier.",
                None,
                Some(1),
                None,
            ),
            RootCause::EnvMismatch
        );
        // A network-signature tail (proxy-less direct connect) is equivalent
        // early-exit evidence even when the exit status was not propagated.
        assert_eq!(
            classify(
                "antigravity_hook_receipt_missing",
                "session/new",
                "Antigravity hook bridge did not return a native conversation identifier.",
                None,
                None,
                Some("dial tcp 127.0.0.1:443: i/o timeout (ETIMEDOUT)"),
            ),
            RootCause::EnvMismatch
        );
        // Without any early-exit or network evidence the classifier stays
        // honest instead of guessing.
        assert_eq!(
            classify(
                "antigravity_hook_receipt_missing",
                "session/new",
                "Antigravity hook bridge did not return a native conversation identifier.",
                None,
                None,
                None,
            ),
            RootCause::Unknown
        );
    }

    #[test]
    fn auth_markers_classify_as_auth() {
        assert_eq!(
            classify(
                "codex_turn_not_completed",
                "turn/completed",
                "Codex rejected the turn as unauthorized.",
                Some("failed/Unauthorized"),
                None,
                None,
            ),
            RootCause::Auth
        );
        assert_eq!(
            classify(
                "vendor_failed",
                "turn/execute",
                "",
                None,
                None,
                Some("HTTP 401")
            ),
            RootCause::Auth
        );
        assert_eq!(RootCause::Auth.recovery(), RECOVERY_AUTH);
    }

    #[test]
    fn vendor_timeout_and_dns_signatures_classify_as_network_unreachable() {
        assert_eq!(
            classify(
                "vendor_request_failed",
                "turn/execute",
                "",
                None,
                None,
                Some("read tcp: connection reset by peer"),
            ),
            RootCause::NetworkUnreachable
        );
        assert_eq!(
            classify(
                "vendor_request_failed",
                "turn/execute",
                "request failed: EAI_AGAIN during name resolution",
                None,
                None,
                None,
            ),
            RootCause::NetworkUnreachable
        );
        assert_eq!(RootCause::NetworkUnreachable.recovery(), RECOVERY_NETWORK);
    }

    #[test]
    fn local_deadline_and_unmatched_failures_stay_unknown() {
        // A bare local CLI timeout is the host's own send-phase deadline, not
        // network evidence.
        assert_eq!(
            classify(
                "antigravity_cli_timeout",
                "turn/execute",
                "Antigravity CLI timed out before completing the turn.",
                None,
                None,
                None,
            ),
            RootCause::Unknown
        );
        assert_eq!(
            classify(
                "codex_final_message_missing",
                "turn/completed",
                "Codex completed the turn without a final agent message.",
                None,
                None,
                None,
            ),
            RootCause::Unknown
        );
        assert_eq!(RootCause::Unknown.recovery(), RECOVERY_UNKNOWN);
    }
}
