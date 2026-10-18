//! One-shot CLI admission for the Adaptive Flywheel strategy surface.
//!
//! Run-driving actions need the persistent runtime, so a one-shot process fails
//! closed before any run state is touched. Cancellation may run here, but a
//! one-shot process composes no actor port, so every in-flight effect lands in
//! the unknown arm rather than being acknowledged. The answer therefore carries
//! an explicit stop disposition so a caller can never read a *request* as an
//! observed exit, and never reads an unknown effect position as a rollback.

use super::{AdmittedCommand, CliExecution};
use anyhow::{Result, anyhow};
use serde_json::{Value, json};

use crate::domain::workflow_runtime::StrategyService;
use licoup_foundation::platform::paths::portable_data_dir;

/// The actions whose answer carries a stop disposition.
const CANCEL_ACTIONS: [&str; 2] = ["strategy.run.cancel", "strategy.assistant.workflow.cancel"];

pub(super) fn handle_strategy_execute(mut command: AdmittedCommand) -> Result<CliExecution> {
    let input = match command.take_option_json("stdin-json") {
        Some(Value::Object(input)) => Value::Object(input),
        Some(_) => return Err(anyhow!("strategy_request_invalid")),
        None => return Err(anyhow!("strategy_request_required")),
    };
    // Run actions drive Agent work on background threads. A one-shot process
    // would orphan the run, so it fails closed with the typed transport
    // rejection before any run state is touched.
    if input
        .get("action")
        .and_then(Value::as_str)
        .is_some_and(strategy_action_requires_persistent_runtime)
    {
        return Err(anyhow!(
            crate::domain::client_conversation::PERSISTENT_TRANSPORT_REQUIRED
        ));
    }
    let action = input
        .get("action")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned();
    let root = portable_data_dir()?;
    let service = StrategyService::open(&root)?;
    let result = service.execute(input)?;
    Ok(CliExecution::Json(annotate_stop_outcome(&action, result)))
}

/// How far one durable workflow stop actually got.
///
/// `Stopped` is the only disposition that claims every in-flight effect reached
/// its own safe boundary. `Requested` means the durable request is recorded
/// while at least one effect is still in flight, and `Unknown` means at least
/// one effect's position could not be established. Neither of the last two
/// asserts that an effect was rolled back or never left the process.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WorkflowStopDisposition {
    /// Every in-flight effect acknowledged the cancellation and settled.
    Stopped,
    /// The request is durable; effects are still in flight.
    Requested,
    /// At least one effect's outcome could not be established.
    Unknown,
}

impl WorkflowStopDisposition {
    /// The stable wire name.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Stopped => "stopped",
            Self::Requested => "requested",
            Self::Unknown => "unknown",
        }
    }

    /// Read the disposition from the run status the cancellation produced.
    ///
    /// A status this mapping does not recognise yields `None`, so an
    /// unexpected run state is reported as unobserved rather than guessed into
    /// a stop.
    pub fn from_run_status(status: Option<&str>) -> Option<Self> {
        match status {
            Some("cancelled") => Some(Self::Stopped),
            Some("cancel-requested") => Some(Self::Requested),
            Some("cancel-in-doubt") => Some(Self::Unknown),
            _ => None,
        }
    }

    /// True when this disposition proves every effect reached a safe boundary.
    pub const fn proves_settled(self) -> bool {
        matches!(self, Self::Stopped)
    }
}

fn strategy_action_requires_persistent_runtime(action: &str) -> bool {
    matches!(
        action,
        "strategy.run.start" | "strategy.run.resume" | "strategy.run.retry"
    )
}

fn is_cancel_action(action: &str) -> bool {
    CANCEL_ACTIONS.contains(&action)
}

/// Add the stop disposition and its diagnostics to a cancellation answer.
///
/// Every other action's answer is returned unchanged, so this annotation never
/// widens what an unrelated action reports.
pub fn annotate_stop_outcome(action: &str, result: Value) -> Value {
    if !is_cancel_action(action) {
        return result;
    }
    let Value::Object(mut object) = result else {
        return result;
    };
    let run_status = object
        .get("status")
        .and_then(Value::as_str)
        .map(str::to_owned);
    let disposition = WorkflowStopDisposition::from_run_status(run_status.as_deref());
    object.insert(
        "stopDisposition".into(),
        json!(disposition.map(WorkflowStopDisposition::as_str)),
    );
    object.insert(
        "stopDiagnostics".into(),
        json!({
            "runStatus": run_status,
            "observed": disposition.is_some(),
            "provesSettled": disposition.is_some_and(WorkflowStopDisposition::proves_settled),
            // A stop request is never evidence that an effect was rolled back,
            // and an unknown position is never evidence of idleness.
            "requiresLiveReadback": !disposition.is_some_and(WorkflowStopDisposition::proves_settled),
        }),
    );
    Value::Object(object)
}

#[cfg(test)]
mod tests {
    use super::{
        WorkflowStopDisposition, annotate_stop_outcome, is_cancel_action,
        strategy_action_requires_persistent_runtime,
    };
    use serde_json::json;

    #[test]
    fn only_run_driving_actions_require_the_persistent_runtime() {
        for action in [
            "strategy.run.start",
            "strategy.run.resume",
            "strategy.run.retry",
        ] {
            assert!(strategy_action_requires_persistent_runtime(action));
        }
        for action in [
            "strategy.run.active",
            "strategy.run.inspect",
            "strategy.run.cancel",
            "strategy.definition.list",
        ] {
            assert!(!strategy_action_requires_persistent_runtime(action));
        }
    }

    #[test]
    fn the_three_stop_states_are_distinguished() {
        assert_eq!(
            WorkflowStopDisposition::from_run_status(Some("cancelled")),
            Some(WorkflowStopDisposition::Stopped)
        );
        assert_eq!(
            WorkflowStopDisposition::from_run_status(Some("cancel-requested")),
            Some(WorkflowStopDisposition::Requested)
        );
        assert_eq!(
            WorkflowStopDisposition::from_run_status(Some("cancel-in-doubt")),
            Some(WorkflowStopDisposition::Unknown)
        );
        // An unrelated or unexpected run status is not guessed into a stop.
        for status in [None, Some("running"), Some("completed"), Some("")] {
            assert_eq!(WorkflowStopDisposition::from_run_status(status), None);
        }
    }

    #[test]
    fn only_a_settled_stop_is_recorded_as_proving_settlement() {
        assert!(WorkflowStopDisposition::Stopped.proves_settled());
        assert!(!WorkflowStopDisposition::Requested.proves_settled());
        assert!(!WorkflowStopDisposition::Unknown.proves_settled());
        assert_eq!(WorkflowStopDisposition::Stopped.as_str(), "stopped");
        assert_eq!(WorkflowStopDisposition::Requested.as_str(), "requested");
        assert_eq!(WorkflowStopDisposition::Unknown.as_str(), "unknown");
    }

    #[test]
    fn a_requested_stop_does_not_prove_a_rollback() {
        let answer = annotate_stop_outcome(
            "strategy.run.cancel",
            json!({ "runId": "run-1", "status": "cancel-requested" }),
        );
        assert_eq!(answer["stopDisposition"], json!("requested"));
        assert_eq!(
            answer["stopDiagnostics"]["runStatus"],
            json!("cancel-requested")
        );
        assert_eq!(answer["stopDiagnostics"]["observed"], json!(true));
        assert_eq!(answer["stopDiagnostics"]["provesSettled"], json!(false));
        assert_eq!(
            answer["stopDiagnostics"]["requiresLiveReadback"],
            json!(true)
        );
    }

    #[test]
    fn an_unknown_stop_keeps_the_effect_position_unresolved() {
        let answer = annotate_stop_outcome(
            "strategy.assistant.workflow.cancel",
            json!({ "runId": "run-1", "status": "cancel-in-doubt" }),
        );
        assert_eq!(answer["stopDisposition"], json!("unknown"));
        assert_eq!(answer["stopDiagnostics"]["provesSettled"], json!(false));
        assert_eq!(
            answer["stopDiagnostics"]["requiresLiveReadback"],
            json!(true)
        );
    }

    #[test]
    fn a_settled_stop_is_the_only_observed_exit() {
        let answer = annotate_stop_outcome(
            "strategy.run.cancel",
            json!({ "runId": "run-1", "status": "cancelled" }),
        );
        assert_eq!(answer["stopDisposition"], json!("stopped"));
        assert_eq!(answer["stopDiagnostics"]["provesSettled"], json!(true));
        assert_eq!(
            answer["stopDiagnostics"]["requiresLiveReadback"],
            json!(false)
        );
    }

    #[test]
    fn an_unexpected_status_is_reported_as_unobserved() {
        let answer = annotate_stop_outcome(
            "strategy.run.cancel",
            json!({ "runId": "run-1", "status": "running" }),
        );
        assert_eq!(answer["stopDisposition"], json!(null));
        assert_eq!(answer["stopDiagnostics"]["observed"], json!(false));
        assert_eq!(
            answer["stopDiagnostics"]["requiresLiveReadback"],
            json!(true)
        );
    }

    #[test]
    fn an_unrelated_action_answer_is_unchanged() {
        let original = json!({ "status": "running", "runId": "run-1" });
        assert_eq!(
            annotate_stop_outcome("strategy.run.inspect", original.clone()),
            original
        );
        assert!(!is_cancel_action("strategy.run.inspect"));
        assert!(is_cancel_action("strategy.run.cancel"));
        assert!(is_cancel_action("strategy.assistant.workflow.cancel"));
    }

    #[test]
    fn a_cancel_answer_that_is_not_an_object_is_returned_unchanged() {
        assert_eq!(
            annotate_stop_outcome("strategy.run.cancel", json!([1, 2, 3])),
            json!([1, 2, 3])
        );
    }
}
