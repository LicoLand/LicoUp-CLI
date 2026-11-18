//! The constructors one Codex app-server failure and one failed run are built
//! from.
//!
//! They live beside the vocabulary they construct rather than beside the driver
//! that reports them: a failure's shape, its resolution hint and the transitions
//! a failed execution produces are all this Agent's own answers, and the client
//! only decides *when* to build one.
//!
//! A failed run's transitions come from this package's own parser, so the
//! failure a driver reports and the failure a replayed transcript reports are
//! the same transition, produced by the same code.

use super::model::{EffectiveSettings, ProtocolFailure, ProtocolFailurePayload, RunResult};

impl ProtocolFailure {
    /// One failure with no resolution hint and no bound identity.
    pub fn new(code: &'static str, message: &'static str, stage: &'static str) -> Self {
        Self::from_payload(ProtocolFailurePayload {
            code,
            message,
            stage,
            component: None,
            retryable: None,
            recovery: None,
            user_interaction_required: false,
            request_method: None,
            session_id: None,
            thread_id: None,
            turn_id: None,
            turn_status: None,
        })
    }

    /// The same failure, with the resolution a caller can act on.
    pub fn with_resolution(
        mut self,
        component: &'static str,
        retryable: bool,
        recovery: &'static str,
    ) -> Self {
        self.component = Some(component);
        self.retryable = Some(retryable);
        self.recovery = Some(recovery);
        self
    }
}

impl RunResult {
    /// One failed run, in the shape every Codex execution reports.
    pub fn failed(
        failure: ProtocolFailure,
        started_at: String,
        status_code: Option<i32>,
        stdout_truncated: bool,
        stderr_truncated: bool,
    ) -> Self {
        let transitions =
            crate::parser::failure_transitions(failure.code, failure.stage, failure.message);
        Self {
            ok: false,
            output: String::new(),
            transitions,
            session_id: failure.session_id.clone().unwrap_or_default(),
            thread_id: failure.thread_id.clone().unwrap_or_default(),
            turn_id: failure.turn_id.clone().unwrap_or_default(),
            turn_status: failure.turn_status.clone().unwrap_or_default(),
            effective: EffectiveSettings::default(),
            error: Some(failure),
            status_code,
            stdout_truncated,
            stderr_truncated,
            started_at,
        }
    }
}
