//! Manual stop and owned-process force-stop control for every current work owner.
//!
//! One manual-stop entry point resolves the durable owner of a piece of
//! admitted work and routes the request to that owner's existing dispatcher:
//! the persistent conversation turn, the durable workflow run, the Subagent
//! MCP dispatch claim, or the supervised lane session. Force stop is separate
//! and narrower: it can only terminate a LicoUp-owned process group whose
//! durable ownership record is re-verified at execution time, and it never
//! widens to a shared or external process.
//!
//! A request is never proof of exit. Every outcome — the request, the user's
//! dialog choice, the acknowledged cancellation, and the observed or
//! unconfirmed end — is recorded through the existing private `ActivityLog`
//! owner as a bounded, redacted event keyed by an opaque correlation id.
//! Normal cancellation is recorded as a control fact, never as a fault.

use serde_json::{Map, Value, json};
use std::path::Path;
use std::sync::OnceLock;
use std::time::Duration;

use super::client_state::ActivityLog;
use super::local_service;
use super::process_supervisor::{OwnedGroupExit, OwnedProcessGroup};

/// The persistent conversation turn owner: an admitted turn of one Membership.
pub const STOP_OWNER_CONVERSATION_TURN: &str = "conversationTurn";
/// The durable workflow run owner behind `strategy.run.*` and the Assistant
/// workflow.
pub const STOP_OWNER_WORKFLOW_RUN: &str = "workflowRun";
/// The durable Subagent MCP dispatch claim owner.
pub const STOP_OWNER_SUBAGENT_CLAIM: &str = "subagentClaim";
/// The supervised lane session owner (local Agent services and adapter turns).
pub const STOP_OWNER_LANE_SESSION: &str = "laneSession";

/// Bounded observation bound for one confirmed force stop. The bound is a
/// diagnosis limit, not a deadline that converts a running process into a
/// stopped one: an unobserved exit is reported as unconfirmed.
const FORCE_STOP_OBSERVATION_BOUND: Duration = Duration::from_secs(5);
const MAX_REASON_BYTES: usize = 64;
const MAX_SCOPE_ID_BYTES: usize = 64;
const MAX_AFFECTED_TASKS: usize = 16;

/// Answers one Subagent MCP dispatch-claim stop for this process.
///
/// The claim owner is a domain owner, so the stop control asks this port
/// instead of reaching into the domain layer: the composition above installs
/// the answer, and the port reports the claim owner's own result — the
/// disposition on success, or its stable error code.
pub type StopSubagentClaim = fn(
    conversation_id: &str,
    caller_membership_id: &str,
    target_membership_id: &str,
) -> Result<Value, &'static str>;

/// The fail-closed answer: a process that installed no claim dispatcher
/// refuses the stop instead of reporting it as requested or acknowledged.
fn no_subagent_claim_stop(
    _conversation_id: &str,
    _caller_membership_id: &str,
    _target_membership_id: &str,
) -> Result<Value, &'static str> {
    Err("subagent_claim_stop_unavailable")
}

/// One process's installed claim-stop dispatcher.
///
/// The production program has exactly one; tests build their own so the
/// fail-closed default and an installed answer are both observable without
/// depending on test order.
pub struct SubagentClaimStopPort {
    dispatcher: OnceLock<StopSubagentClaim>,
}

impl SubagentClaimStopPort {
    pub const fn new() -> Self {
        Self {
            dispatcher: OnceLock::new(),
        }
    }

    /// Install the composition's dispatcher. One dispatcher per port: a second
    /// installation is refused rather than silently replacing the first.
    pub fn install(&self, dispatcher: StopSubagentClaim) -> Result<(), &'static str> {
        self.dispatcher
            .set(dispatcher)
            .map_err(|_| "subagent claim stop is already installed")
    }

    /// The installed dispatcher's answer, or the fail-closed one.
    pub fn stop(
        &self,
        conversation_id: &str,
        caller_membership_id: &str,
        target_membership_id: &str,
    ) -> Result<Value, &'static str> {
        self.dispatcher
            .get()
            .copied()
            .unwrap_or(no_subagent_claim_stop)(
            conversation_id,
            caller_membership_id,
            target_membership_id,
        )
    }
}

impl Default for SubagentClaimStopPort {
    fn default() -> Self {
        Self::new()
    }
}

static SUBAGENT_CLAIM_STOP: SubagentClaimStopPort = SubagentClaimStopPort::new();

/// The process-wide port the stop control reads.
pub fn subagent_claim_stop_port() -> &'static SubagentClaimStopPort {
    &SUBAGENT_CLAIM_STOP
}

/// Install the composition's claim-stop dispatcher for this process.
pub fn install_subagent_claim_stop(dispatcher: StopSubagentClaim) -> Result<(), &'static str> {
    SUBAGENT_CLAIM_STOP.install(dispatcher)
}

/// One current work owner that can be stopped manually.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StopOwnerKind {
    ConversationTurn,
    WorkflowRun,
    SubagentClaim,
    LaneSession,
}

impl StopOwnerKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ConversationTurn => STOP_OWNER_CONVERSATION_TURN,
            Self::WorkflowRun => STOP_OWNER_WORKFLOW_RUN,
            Self::SubagentClaim => STOP_OWNER_SUBAGENT_CLAIM,
            Self::LaneSession => STOP_OWNER_LANE_SESSION,
        }
    }
}

/// One resolved stop request. The owner identity is exactly one of the durable
/// identities below; an ambiguous request is refused instead of guessed.
#[derive(Clone, Debug, PartialEq)]
pub struct StopTarget {
    pub kind: StopOwnerKind,
    pub correlation_id: String,
    pub reason: String,
    pub turn_handle: Option<String>,
    pub run_id: Option<String>,
    pub conversation_id: Option<String>,
    pub caller_membership_id: Option<String>,
    pub membership_id: Option<String>,
    pub agent_id: Option<String>,
    pub session_id: Option<String>,
}

/// The bounded result one owner dispatcher reports. Raw owner errors, command
/// lines and payloads never cross this seam.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OwnerStopDisposition {
    /// The owner accepted the cancellation for the exact in-flight work.
    Acknowledged,
    /// The owner accepted the request; the effect position stays unknown until
    /// it settles.
    Requested,
    /// The owner reports no in-flight work for the exact identity.
    NotActive,
    /// No owner path is reachable; nothing was cancelled.
    Unavailable,
}

/// The host-composed seams for owners whose control path is process-local. A
/// one-shot process that hosts neither the persistent turn runtime nor the
/// durable run drive passes `None` and receives an explicit unavailable
/// outcome instead of a fabricated cancellation.
#[derive(Clone, Copy, Default)]
pub struct WorkStopPorts<'a> {
    pub turn: Option<&'a (dyn Fn(&StopTarget) -> OwnerStopDisposition + Sync)>,
    pub run: Option<&'a (dyn Fn(&StopTarget) -> OwnerStopDisposition + Sync)>,
}

/// Resolve the durable owner of one manual stop request. Exactly one owner
/// identity must be present.
pub fn resolve_stop_target(params: &Value) -> Result<StopTarget, &'static str> {
    let object = params.as_object().ok_or("invalid_request")?;
    let turn_handle = bounded_text(object, "turnHandle", 160);
    let run_id = bounded_text(object, "runId", 160);
    let claim_id = bounded_text(object, "claimId", 160);
    let session_id = bounded_text(object, "sessionId", 256);
    let agent_id =
        bounded_text(object, "agent", 96).or_else(|| bounded_text(object, "agentId", 96));
    let identified = [
        turn_handle.is_some(),
        run_id.is_some(),
        claim_id.is_some(),
        session_id.is_some(),
    ]
    .into_iter()
    .filter(|present| *present)
    .count();
    if identified > 1 {
        return Err("stop_target_ambiguous");
    }
    let conversation_id = bounded_text(object, "conversationId", 160);
    let caller_membership_id = bounded_text(object, "callerMembershipId", 160);
    let membership_id = bounded_text(object, "membershipId", 160);
    let kind = if turn_handle.is_some() {
        StopOwnerKind::ConversationTurn
    } else if run_id.is_some() {
        StopOwnerKind::WorkflowRun
    } else if claim_id.is_some() || (membership_id.is_some() && session_id.is_none()) {
        StopOwnerKind::SubagentClaim
    } else if session_id.is_some() {
        StopOwnerKind::LaneSession
    } else {
        return Err("stop_target_unknown");
    };
    Ok(StopTarget {
        kind,
        correlation_id: correlation_id(),
        reason: bounded_reason(object),
        turn_handle,
        run_id,
        conversation_id,
        caller_membership_id,
        membership_id,
        agent_id,
        session_id,
    })
}

/// Stop one piece of admitted work through its actual owner. The response
/// always carries the resolved owner kind, the opaque correlation id, and
/// whether the outcome was recorded durably.
pub fn stop_work(params: &Value, ports: &WorkStopPorts<'_>) -> Value {
    let target = match resolve_stop_target(params) {
        Ok(target) => target,
        Err(code) => return stop_refusal(code),
    };
    let disposition = match target.kind {
        StopOwnerKind::ConversationTurn => match ports.turn {
            Some(turn) if target.turn_handle.is_some() => turn(&target),
            Some(_) => OwnerStopDisposition::NotActive,
            None => OwnerStopDisposition::Unavailable,
        },
        StopOwnerKind::WorkflowRun => match ports.run {
            Some(run) if target.run_id.is_some() => run(&target),
            Some(_) => OwnerStopDisposition::NotActive,
            None => OwnerStopDisposition::Unavailable,
        },
        StopOwnerKind::SubagentClaim => stop_subagent_claim(&target),
        StopOwnerKind::LaneSession => match target.session_id.as_deref() {
            Some(session_id) => stop_lane_session(session_id, target.agent_id.as_deref()),
            None => OwnerStopDisposition::NotActive,
        },
    };
    let (status, anomaly) = match disposition {
        OwnerStopDisposition::Acknowledged => ("stop-requested", None),
        OwnerStopDisposition::Requested => ("stop-requested", None),
        OwnerStopDisposition::NotActive => ("not-active", None),
        OwnerStopDisposition::Unavailable => ("owner-unavailable", Some("owner_unavailable")),
    };
    let recorded = record_stop_events(&target, disposition, anomaly);
    json!({
        "ok": status != "owner-unavailable",
        "status": status,
        "ownerKind": target.kind.as_str(),
        "correlationId": target.correlation_id,
        "disposition": match disposition {
            OwnerStopDisposition::Acknowledged => "acknowledged",
            OwnerStopDisposition::Requested => "requested",
            OwnerStopDisposition::NotActive => "not-active",
            OwnerStopDisposition::Unavailable => "unavailable",
        },
        "diagnostics": recorded,
    })
}

/// Preview one owned-process force stop. This reads durable ownership records
/// only: it sends no signal and terminates nothing.
pub fn force_stop_preview(params: &Value) -> Value {
    let correlation_id = correlation_id();
    let requested_scope = params
        .as_object()
        .and_then(|object| bounded_text(object, "scopeId", MAX_SCOPE_ID_BYTES));
    let scopes = owned_process_scopes();
    let Some(scope_id) = requested_scope else {
        let candidates = scopes
            .iter()
            .map(|scope| {
                json!({
                    "scopeId": scope.scope_id,
                    "kind": scope.kind,
                    "ownerRef": scope.owner_ref,
                })
            })
            .collect::<Vec<_>>();
        return json!({
            "ok": true,
            "status": "scope-required",
            "correlationId": correlation_id,
            "candidates": candidates,
        });
    };
    let Some(scope) = scopes.into_iter().find(|scope| scope.scope_id == scope_id) else {
        return json!({
            "ok": false,
            "status": "scope-unavailable",
            "correlationId": correlation_id,
            "error": {
                "code": "force_stop_scope_unavailable",
                "stage": "process/authorize",
            },
        });
    };
    let mut affected_tasks = scope.affected_tasks.clone();
    affected_tasks.truncate(MAX_AFFECTED_TASKS);
    json!({
        "ok": true,
        "status": "preview",
        "correlationId": correlation_id,
        "scope": {
            "scopeId": scope.scope_id,
            "kind": scope.kind,
            "ownerRef": scope.owner_ref,
            "pid": scope.pid,
            "processGroupVerified": true,
            "affectedTaskCount": scope.affected_tasks.len(),
        },
        "affectedTasks": affected_tasks,
        "riskCodes": scope.risk_codes,
        "riskSummary": scope.risk_summary,
        "confirmationToken": confirmation_token(&scope.scope_id, scope.revision),
    })
}

/// Confirm one force stop. The confirmed scope is re-resolved from its durable
/// ownership record, re-verified against the presented token, and only then
/// terminated. A missing or false confirmation, or a changed target, sends no
/// signal. A request without an observed exit stays unconfirmed.
pub fn force_stop_confirm(params: &Value) -> Value {
    let object = params.as_object();
    let correlation_id = correlation_id();
    let scope_id = object.and_then(|object| bounded_text(object, "scopeId", MAX_SCOPE_ID_BYTES));
    let token = object.and_then(|object| bounded_text(object, "confirmationToken", 160));
    let confirmed = object
        .and_then(|object| object.get("confirmed"))
        .and_then(Value::as_bool)
        == Some(true);
    if !confirmed {
        let target = RecordingTarget {
            correlation_id: correlation_id.clone(),
            scope_id: scope_id.clone().unwrap_or_default(),
            outcome: "declined",
            reason_code: "user_declined",
        };
        let recorded = record_force_stop(&target);
        return json!({
            "ok": true,
            "status": "declined",
            "correlationId": correlation_id,
            "signalled": false,
            "diagnostics": recorded,
        });
    }
    let Some(scope_id) = scope_id else {
        return force_stop_refusal("force_stop_scope_required", &correlation_id);
    };
    let Some(token) = token else {
        return force_stop_refusal("force_stop_confirmation_required", &correlation_id);
    };
    // Re-verify the confirmed target at execution time. A target replaced
    // since the dialog opened is refused; the new process is never signalled.
    let scope = owned_process_scopes()
        .into_iter()
        .find(|scope| scope.scope_id == scope_id)
        .filter(|scope| confirmation_token(&scope_id, scope.revision) == token);
    let Some(scope) = scope else {
        let target = RecordingTarget {
            correlation_id: correlation_id.clone(),
            scope_id,
            outcome: "unconfirmed",
            reason_code: "confirmed_target_changed",
        };
        let recorded = record_force_stop(&target);
        return json!({
            "ok": false,
            "status": "unconfirmed",
            "correlationId": correlation_id,
            "signalled": false,
            "error": {
                "code": "force_stop_confirmed_target_changed",
                "stage": "process/authorize",
            },
            "diagnostics": recorded,
        });
    };
    let request = RecordingTarget {
        correlation_id: correlation_id.clone(),
        scope_id: scope.scope_id.clone(),
        outcome: "requested",
        reason_code: "confirmed",
    };
    let requested = record_force_stop(&request);
    let Some(group) = OwnedProcessGroup::verify(scope.pid) else {
        let unconfirmed = record_force_stop(&RecordingTarget {
            correlation_id: correlation_id.clone(),
            scope_id: scope.scope_id.clone(),
            outcome: "unconfirmed",
            reason_code: "ownership_unverified",
        });
        return json!({
            "ok": false,
            "status": "unconfirmed",
            "correlationId": correlation_id,
            "signalled": false,
            "termination": {
                "requested": true,
                "observedExit": false,
                "forced": false,
                "reasonCode": "ownership_unverified",
            },
            "diagnostics": merge_recordings(requested, unconfirmed),
        });
    };
    let exit = group.terminate_and_observe(FORCE_STOP_OBSERVATION_BOUND);
    let (status, observed, forced, reason_code) = match exit {
        OwnedGroupExit::ObservedExit { forced, .. } => (
            "observed-exit",
            true,
            forced,
            if forced { "killed" } else { "terminated" },
        ),
        OwnedGroupExit::Unconfirmed => ("unconfirmed", false, true, "exit_not_observed"),
    };
    let outcome = record_force_stop(&RecordingTarget {
        correlation_id: correlation_id.clone(),
        scope_id: scope.scope_id.clone(),
        outcome: if observed {
            "observed-exit"
        } else {
            "unconfirmed"
        },
        reason_code,
    });
    json!({
        "ok": observed,
        "status": status,
        "correlationId": correlation_id,
        "scopeId": scope.scope_id,
        "signalled": true,
        "termination": {
            "requested": true,
            "observedExit": observed,
            "forced": forced,
            "reasonCode": reason_code,
        },
        "convergedRevision": scope.revision,
        "diagnostics": merge_recordings(requested, outcome),
    })
}

fn force_stop_refusal(code: &'static str, correlation_id: &str) -> Value {
    json!({
        "ok": false,
        "status": "invalid",
        "correlationId": correlation_id,
        "signalled": false,
        "error": {"code": code, "stage": "process/authorize"},
    })
}

fn stop_refusal(code: &'static str) -> Value {
    json!({
        "ok": false,
        "status": "invalid",
        "error": {"code": code, "stage": "work/stop"},
    })
}

/// Cancel one active Subagent MCP dispatch claim. The request is attributed to
/// the claim's own durable caller membership; the MCP edge is the existing
/// claim owner, so no second dispatch registry is created.
fn stop_subagent_claim(target: &StopTarget) -> OwnerStopDisposition {
    let Some(conversation_id) = target.conversation_id.as_deref() else {
        return OwnerStopDisposition::Unavailable;
    };
    let Some(caller_membership_id) = target.caller_membership_id.as_deref() else {
        return OwnerStopDisposition::Unavailable;
    };
    let Some(target_membership_id) = target.membership_id.as_deref() else {
        return OwnerStopDisposition::Unavailable;
    };
    match subagent_claim_stop_port().stop(
        conversation_id,
        caller_membership_id,
        target_membership_id,
    ) {
        Ok(value) => match value.get("state").and_then(Value::as_str) {
            Some("cancelled") => OwnerStopDisposition::Acknowledged,
            _ => OwnerStopDisposition::Requested,
        },
        Err(code) => match code {
            "subagent_cancel_unavailable" => OwnerStopDisposition::NotActive,
            _ => OwnerStopDisposition::Unavailable,
        },
    }
}

/// Cancel one supervised lane session through the existing lane dispatcher.
/// The lane owner keeps its own per-adapter control implementation; this
/// control plane never grows a second adapter branch.
fn stop_lane_session(session_id: &str, agent_id: Option<&str>) -> OwnerStopDisposition {
    let Some(agent_id) = agent_id else {
        return OwnerStopDisposition::Unavailable;
    };
    let params = json!({
        "agent": agent_id,
        "agentId": agent_id,
        "sessionId": session_id,
    });
    match crate::agent_port::cancel(&params) {
        Ok(response) => match response.get("ok").and_then(Value::as_bool) {
            Some(true) => OwnerStopDisposition::Acknowledged,
            _ => match response.get("status").and_then(Value::as_str) {
                Some("not_active") => OwnerStopDisposition::NotActive,
                _ => OwnerStopDisposition::Unavailable,
            },
        },
        Err(_) => OwnerStopDisposition::Unavailable,
    }
}

/// One LicoUp-owned execution process group presented to the confirmation
/// dialog, with the durable owner record and the affected tasks it names.
#[derive(Clone, Debug)]
struct OwnedProcessScope {
    scope_id: String,
    revision: u64,
    kind: &'static str,
    owner_ref: String,
    pid: u32,
    affected_tasks: Vec<Value>,
    risk_codes: Vec<&'static str>,
    risk_summary: String,
}

/// Every local Agent service whose durable state and pid record this data root
/// owns. Force stop reads the same records the serve owner writes through
/// `local_service`, so no second service registry exists; the list itself
/// stays with the force-stop owner, which is the only caller, so the
/// target-neutral local service foundation never names a target policy.
fn owned_serve_specs() -> Vec<local_service::ServeSpec> {
    vec![
        super::opencode_serve::CONTROL_SPEC,
        super::kilo_code_host::CONTROL_SPEC,
    ]
}

/// The owned process groups this data root verifiably owns: a local Agent
/// service whose durable state record marks it owned, whose pid record names a
/// live process, and whose process group is the process itself. A listener
/// without that record, or a process inside a shared group, is never a force
/// stop target.
fn owned_process_scopes() -> Vec<OwnedProcessScope> {
    #[cfg(test)]
    if let Some(scopes) = test_scopes::current_override() {
        return scopes;
    }
    owned_serve_specs()
        .into_iter()
        .filter_map(|spec| owned_local_service_scope(&spec))
        .collect()
}

fn owned_local_service_scope(spec: &local_service::ServeSpec) -> Option<OwnedProcessScope> {
    let paths = local_service::service_paths(spec.state_dir).ok()?;
    let state = local_service::read_service_state(&paths).ok()?;
    if state.get("owned").and_then(Value::as_bool) != Some(true) {
        return None;
    }
    let pid = local_service::service_pid(&paths).ok()??;
    if !local_service::process_alive(pid) {
        return None;
    }
    // Ownership means this exact process leads its own group; LicoUp spawned
    // it detached. Anything else is a shared or external process.
    OwnedProcessGroup::verify(pid)?;
    let attach_url = state
        .get("attachUrl")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let active_turns = local_service::active_endpoint_turns(attach_url);
    let mut affected_tasks = active_turns
        .iter()
        .map(|(driver_id, session_id)| {
            json!({
                "taskKind": "agent-turn",
                "taskRef": format!("{driver_id}:{session_id}"),
            })
        })
        .collect::<Vec<_>>();
    if affected_tasks.is_empty() {
        affected_tasks.push(json!({
            "taskKind": "agent-local-service",
            "taskRef": spec.identity,
        }));
    }
    let mut risk_codes = vec!["owned-service-terminated", "service-restart-required"];
    if !active_turns.is_empty() {
        risk_codes.insert(0, "unsaved-agent-progress");
    }
    Some(OwnedProcessScope {
        scope_id: format!("local-service.{identity}", identity = spec.identity),
        revision: pid as u64,
        kind: "local-service",
        owner_ref: spec.identity.to_string(),
        pid,
        affected_tasks,
        risk_codes,
        risk_summary: format!(
            "Terminating the {} process group stops the local Agent service LicoUp started.{}",
            spec.identity,
            if active_turns.is_empty() {
                ""
            } else {
                " Turns it is running now lose unsaved progress."
            }
        ),
    })
}

fn confirmation_token(scope_id: &str, revision: u64) -> String {
    format!("{scope_id}#{revision}")
}

struct RecordingTarget {
    correlation_id: String,
    scope_id: String,
    outcome: &'static str,
    reason_code: &'static str,
}

/// Persist the request and its outcome. A failed diagnostic write is reported
/// as not recorded; it never claims that an error record was retained.
fn record_stop_events(
    target: &StopTarget,
    disposition: OwnerStopDisposition,
    anomaly: Option<&'static str>,
) -> Value {
    let base = json!({
        "target": target.correlation_id,
        "correlationId": target.correlation_id,
        "ownerKind": target.kind.as_str(),
        "reason": target.reason,
        "outcome": match disposition {
            OwnerStopDisposition::Acknowledged => "acknowledged",
            OwnerStopDisposition::Requested => "requested",
            OwnerStopDisposition::NotActive => "not-active",
            OwnerStopDisposition::Unavailable => "unavailable",
        },
    });
    let mut event_types = Vec::new();
    let mut recorded = append_event("work.stop.requested", base.clone());
    if recorded {
        event_types.push("work.stop.requested");
    }
    let (event_type, payload) = match anomaly {
        Some(reason_code) => (
            "work.stop.anomaly",
            json!({
                "target": target.correlation_id,
                "correlationId": target.correlation_id,
                "ownerKind": target.kind.as_str(),
                "reasonCode": reason_code,
            }),
        ),
        None => (
            "work.stop.accepted",
            json!({
                "target": target.correlation_id,
                "correlationId": target.correlation_id,
                "ownerKind": target.kind.as_str(),
                "outcome": base["outcome"].clone(),
            }),
        ),
    };
    if append_event(event_type, payload) {
        event_types.push(event_type);
    } else {
        recorded = false;
    }
    json!({"recorded": recorded, "eventTypes": event_types})
}

fn record_force_stop(target: &RecordingTarget) -> Value {
    let event_type = match target.outcome {
        "observed-exit" => "work.force-stop.observed-exit",
        "unconfirmed" => "work.force-stop.unconfirmed",
        "declined" => "work.force-stop.declined",
        _ => "work.force-stop.requested",
    };
    let recorded = append_event(
        event_type,
        json!({
            "target": target.correlation_id,
            "correlationId": target.correlation_id,
            "scopeId": target.scope_id,
            "outcome": target.outcome,
            "reasonCode": target.reason_code,
        }),
    );
    json!({
        "recorded": recorded,
        "eventTypes": if recorded { json!([event_type]) } else { json!([]) },
    })
}

fn merge_recordings(first: Value, second: Value) -> Value {
    let mut merged = Vec::new();
    for recording in [&first, &second] {
        if let Some(event_types) = recording.get("eventTypes").and_then(Value::as_array) {
            merged.extend(event_types.iter().cloned());
        }
    }
    json!({
        "recorded": first.get("recorded").and_then(Value::as_bool) == Some(true)
            && second.get("recorded").and_then(Value::as_bool) == Some(true),
        "eventTypes": merged,
    })
}

/// Append one bounded, redacted diagnostic event. Errors stay bounded and are
/// surfaced as a boolean fact, never as a raw error or payload.
fn append_event(event_type: &str, payload: Value) -> bool {
    ActivityLog::portable()
        .and_then(|log| log.append(event_type, payload))
        .is_ok()
}

fn append_event_at(data_root: &Path, event_type: &str, payload: Value) -> bool {
    ActivityLog::in_data_root(data_root)
        .and_then(|log| log.append(event_type, payload))
        .is_ok()
}

fn correlation_id() -> String {
    format!("stop-{}", uuid::Uuid::new_v4().simple())
}

/// A fresh opaque correlation id for one stop request.
pub fn new_correlation_id() -> String {
    correlation_id()
}

/// Record the durable run-cancel request and its owner acknowledgement through
/// the diagnostic owner at the run store's own data root. `owner_acknowledged`
/// reports whether every in-flight command's owner accepted the cancellation;
/// an unacknowledged request keeps its effect unknown and is recorded as the
/// anomaly it is.
pub fn record_run_stop(data_root: &Path, correlation_id: &str, owner_acknowledged: bool) -> Value {
    let requested = append_event_at(
        data_root,
        "work.stop.requested",
        json!({
            "target": correlation_id,
            "correlationId": correlation_id,
            "ownerKind": STOP_OWNER_WORKFLOW_RUN,
        }),
    );
    let (event_type, payload) = if owner_acknowledged {
        (
            "work.stop.accepted",
            json!({
                "target": correlation_id,
                "correlationId": correlation_id,
                "ownerKind": STOP_OWNER_WORKFLOW_RUN,
                "outcome": "acknowledged",
            }),
        )
    } else {
        (
            "work.stop.unconfirmed",
            json!({
                "target": correlation_id,
                "correlationId": correlation_id,
                "ownerKind": STOP_OWNER_WORKFLOW_RUN,
                "reasonCode": "owner_did_not_acknowledge",
            }),
        )
    };
    let outcome = append_event_at(data_root, event_type, payload);
    json!({
        "recorded": requested && outcome,
        "eventTypes": if requested && outcome {
            json!(["work.stop.requested", event_type])
        } else {
            json!([])
        },
    })
}

fn bounded_reason(object: &Map<String, Value>) -> String {
    bounded_text(object, "reason", MAX_REASON_BYTES)
        .filter(|reason| {
            reason.bytes().all(|byte| {
                byte.is_ascii_alphanumeric() || byte == b'.' || byte == b'-' || byte == b'_'
            })
        })
        .unwrap_or_else(|| "user-stop".to_string())
}

fn bounded_text(object: &Map<String, Value>, key: &str, max_bytes: usize) -> Option<String> {
    object
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty() && value.len() <= max_bytes)
        .map(str::to_owned)
}

#[cfg(test)]
pub(super) mod test_scopes {
    use super::OwnedProcessScope;
    use std::cell::RefCell;

    thread_local! {
        static SCOPES: RefCell<Option<Vec<OwnedProcessScope>>> = const { RefCell::new(None) };
    }

    /// Install one synthetic owned-process scope list for the current test
    /// thread. Production resolution is never consulted while an override is
    /// set.
    pub(super) fn set(scopes: Option<Vec<OwnedProcessScope>>) {
        SCOPES.with(|slot| *slot.borrow_mut() = scopes);
    }

    pub(super) fn current_override() -> Option<Vec<OwnedProcessScope>> {
        SCOPES.with(|slot| slot.borrow().clone())
    }

    pub(super) fn scope(
        scope_id: &str,
        pid: u32,
        affected_tasks: Vec<serde_json::Value>,
        risk_codes: Vec<&'static str>,
    ) -> OwnedProcessScope {
        OwnedProcessScope {
            scope_id: scope_id.to_string(),
            revision: pid as u64,
            kind: "local-service",
            owner_ref: scope_id.to_string(),
            pid,
            affected_tasks,
            risk_codes,
            risk_summary: "Terminating this owned process group stops local work.".to_string(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::path::PathBuf;
    use std::process::{Child, Command, Stdio};

    struct PortableRoot {
        root: PathBuf,
        previous: Option<PathBuf>,
    }

    impl PortableRoot {
        fn new() -> Self {
            let root =
                std::env::temp_dir().join(format!("licoup-stop-control-{}", uuid::Uuid::new_v4()));
            std::fs::create_dir_all(&root).unwrap();
            let previous = licoup_foundation::platform::paths::set_portable_data_dir_override(
                Some(root.clone()),
            );
            Self { root, previous }
        }
    }

    impl Drop for PortableRoot {
        fn drop(&mut self) {
            licoup_foundation::platform::paths::set_portable_data_dir_override(
                self.previous.take(),
            );
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }

    /// A real LicoUp-style owned execution group: detached into its own
    /// process group, exactly as the serve owner spawns one.
    #[cfg(unix)]
    fn spawn_owned_group(command: &str) -> Child {
        use std::os::unix::process::CommandExt;
        let mut child = Command::new("sh");
        child
            .arg("-c")
            .arg(command)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .process_group(0);
        child.spawn().unwrap()
    }

    fn read_events(root: &std::path::Path) -> Vec<Value> {
        let log = ActivityLog::in_data_root(root).unwrap();
        log.list(&json!({"limit": 1000})).unwrap()["events"]
            .as_array()
            .cloned()
            .unwrap_or_default()
    }

    fn cleanup_group(pid: u32) {
        use std::time::Duration;
        if let Some(group) = OwnedProcessGroup::verify(pid) {
            let _ = group.terminate_and_observe(Duration::from_secs(2));
        }
    }

    #[test]
    fn preview_names_the_owned_scope_and_sends_no_signal() {
        #[cfg(unix)]
        {
            let root = PortableRoot::new();
            let child = spawn_owned_group("sleep 30");
            let pid = child.id();
            test_scopes::set(Some(vec![test_scopes::scope(
                "local-service.opencode_serve",
                pid,
                vec![json!({"taskKind": "agent-turn", "taskRef": "opencode:session-1"})],
                vec!["unsaved-agent-progress", "owned-service-terminated"],
            )]));
            let preview = force_stop_preview(&json!({"scopeId": "local-service.opencode_serve"}));
            assert_eq!(preview["ok"], true);
            assert_eq!(preview["status"], "preview");
            assert_eq!(preview["scope"]["pid"], pid);
            assert_eq!(preview["scope"]["processGroupVerified"], true);
            assert_eq!(preview["affectedTasks"][0]["taskRef"], "opencode:session-1");
            assert!(
                preview["riskCodes"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|code| code == "unsaved-agent-progress")
            );
            assert!(preview["confirmationToken"].as_str().is_some());
            // The preview is a read: the owned process is still running.
            let mut child = child;
            assert!(child.try_wait().unwrap().is_none());
            test_scopes::set(None);
            cleanup_group(pid);
            assert!(
                read_events(&root.root).is_empty(),
                "a preview records no stop event"
            );
        }
    }

    #[test]
    fn declining_a_confirmed_force_stop_sends_no_signal() {
        #[cfg(unix)]
        {
            let root = PortableRoot::new();
            let mut child = spawn_owned_group("sleep 30");
            let pid = child.id();
            test_scopes::set(Some(vec![test_scopes::scope(
                "local-service.kilo_code_serve",
                pid,
                Vec::new(),
                vec!["owned-service-terminated"],
            )]));
            let declined = force_stop_confirm(&json!({
                "scopeId": "local-service.kilo_code_serve",
                "confirmed": false,
            }));
            assert_eq!(declined["status"], "declined");
            assert_eq!(declined["signalled"], false);
            assert!(
                child.try_wait().unwrap().is_none(),
                "decline sent no signal"
            );
            test_scopes::set(None);
            cleanup_group(pid);
            let events = read_events(&root.root);
            assert!(
                events
                    .iter()
                    .any(|event| event["type"] == "work.force-stop.declined"),
                "the user's choice is recorded: {events:?}"
            );
        }
    }

    #[test]
    fn confirmed_force_stop_terminates_only_the_verified_group_and_observes_exit() {
        #[cfg(unix)]
        {
            let root = PortableRoot::new();
            let mut child = spawn_owned_group("sleep 30 & wait");
            let pid = child.id();
            let mut unrelated = spawn_owned_group("sleep 30");
            let unrelated_pid = unrelated.id();
            test_scopes::set(Some(vec![
                test_scopes::scope(
                    "local-service.opencode_serve",
                    pid,
                    Vec::new(),
                    vec!["owned-service-terminated"],
                ),
                test_scopes::scope(
                    "local-service.unrelated",
                    unrelated_pid,
                    Vec::new(),
                    vec!["owned-service-terminated"],
                ),
            ]));
            let preview = force_stop_preview(&json!({"scopeId": "local-service.opencode_serve"}));
            let token = preview["confirmationToken"].as_str().unwrap().to_owned();
            let confirmed = force_stop_confirm(&json!({
                "scopeId": "local-service.opencode_serve",
                "confirmationToken": token,
                "confirmed": true,
            }));
            assert_eq!(confirmed["ok"], true, "{confirmed}");
            assert_eq!(confirmed["status"], "observed-exit");
            assert_eq!(confirmed["termination"]["observedExit"], true);
            assert!(child.try_wait().unwrap().is_some(), "the root exited");
            // The whole group is gone, so a descendant cannot hold the turn.
            let group_gone = unsafe { libc::kill(-(pid as libc::pid_t), 0) } != 0;
            assert!(group_gone, "the owned process group was terminated");
            // The unrelated owned scope is untouched.
            assert!(
                unrelated.try_wait().unwrap().is_none(),
                "an unselected scope keeps running"
            );
            test_scopes::set(None);
            cleanup_group(unrelated_pid);
            let events = read_events(&root.root);
            assert!(
                events
                    .iter()
                    .any(|event| event["type"] == "work.force-stop.requested"),
                "the confirmed request is recorded"
            );
            assert!(
                events
                    .iter()
                    .any(|event| event["type"] == "work.force-stop.observed-exit"),
                "the observed exit is recorded: {events:?}"
            );
        }
    }

    #[test]
    fn a_target_whose_ownership_is_unverified_stays_unconfirmed() {
        #[cfg(unix)]
        {
            let root = PortableRoot::new();
            // A child that shares this process's group is not owned: force stop
            // refuses it instead of widening the signal to a shared group.
            let mut shared = Command::new("sh")
                .args(["-c", "sleep 30"])
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .unwrap();
            let pid = shared.id();
            assert!(OwnedProcessGroup::verify(pid).is_none());
            test_scopes::set(Some(vec![test_scopes::scope(
                "local-service.opencode_serve",
                pid,
                Vec::new(),
                vec!["owned-service-terminated"],
            )]));
            let preview = force_stop_preview(&json!({"scopeId": "local-service.opencode_serve"}));
            let token = preview["confirmationToken"].as_str().unwrap().to_owned();
            let confirmed = force_stop_confirm(&json!({
                "scopeId": "local-service.opencode_serve",
                "confirmationToken": token,
                "confirmed": true,
            }));
            assert_eq!(confirmed["status"], "unconfirmed");
            assert_eq!(confirmed["signalled"], false);
            assert_eq!(
                confirmed["termination"]["reasonCode"],
                "ownership_unverified"
            );
            assert!(
                shared.try_wait().unwrap().is_none(),
                "a shared group is never signalled"
            );
            test_scopes::set(None);
            let _ = shared.kill();
            let _ = shared.wait();
            let events = read_events(&root.root);
            assert!(
                events
                    .iter()
                    .any(|event| event["type"] == "work.force-stop.unconfirmed"),
                "an unobserved exit stays unconfirmed: {events:?}"
            );
        }
    }

    #[test]
    fn a_stale_confirmation_token_never_signals_a_replaced_target() {
        #[cfg(unix)]
        {
            let root = PortableRoot::new();
            let mut child = spawn_owned_group("sleep 30");
            let pid = child.id();
            test_scopes::set(Some(vec![test_scopes::scope(
                "local-service.opencode_serve",
                pid,
                Vec::new(),
                vec!["owned-service-terminated"],
            )]));
            let stale = force_stop_confirm(&json!({
                "scopeId": "local-service.opencode_serve",
                "confirmationToken": "local-service.opencode_serve#1",
                "confirmed": true,
            }));
            assert_eq!(stale["status"], "unconfirmed");
            assert_eq!(stale["signalled"], false);
            assert!(child.try_wait().unwrap().is_none());
            test_scopes::set(None);
            cleanup_group(pid);
            let events = read_events(&root.root);
            assert!(
                events
                    .iter()
                    .any(|event| event["type"] == "work.force-stop.unconfirmed"),
                "a changed target is recorded as unconfirmed"
            );
        }
    }

    #[test]
    fn durable_local_service_records_are_the_only_force_stop_scopes() {
        #[cfg(unix)]
        {
            let root = PortableRoot::new();
            let mut child = spawn_owned_group("sleep 30");
            let pid = child.id();
            let paths = local_service::service_paths("opencode-serve").unwrap();
            local_service::state::write_json(
                &paths.state_path,
                &json!({
                    "schemaVersion": "v0.0.1:opencode-serve-2",
                    "status": "running",
                    "running": true,
                    "owned": true,
                    "port": 24173,
                    "attachUrl": "http://127.0.0.1:24173",
                }),
            )
            .unwrap();
            local_service::state::write_pid(&paths.pid_path, pid).unwrap();

            // The production resolver reads the durable owner record; a
            // candidate list is offered before a scope is named.
            let candidates = force_stop_preview(&json!({}));
            assert_eq!(candidates["status"], "scope-required");
            assert!(
                candidates["candidates"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|scope| scope["scopeId"] == "local-service.opencode_serve"),
                "{candidates}"
            );
            let preview = force_stop_preview(&json!({"scopeId": "local-service.opencode_serve"}));
            assert_eq!(preview["ok"], true, "{preview}");
            assert_eq!(preview["scope"]["pid"], pid);
            assert_eq!(preview["scope"]["kind"], "local-service");
            assert_eq!(preview["scope"]["processGroupVerified"], true);
            let token = preview["confirmationToken"].as_str().unwrap().to_owned();
            let confirmed = force_stop_confirm(&json!({
                "scopeId": "local-service.opencode_serve",
                "confirmationToken": token,
                "confirmed": true,
            }));
            assert_eq!(confirmed["status"], "observed-exit", "{confirmed}");
            assert!(child.try_wait().unwrap().is_some());
            let events = read_events(&root.root);
            assert!(
                events
                    .iter()
                    .any(|event| event["type"] == "work.force-stop.observed-exit")
            );

            // A listener without the durable owned record is never a target.
            local_service::state::write_json(
                &paths.state_path,
                &json!({"status": "running", "running": true, "owned": false}),
            )
            .unwrap();
            let refused = force_stop_preview(&json!({"scopeId": "local-service.opencode_serve"}));
            assert_eq!(refused["status"], "scope-unavailable");
            assert_eq!(refused["ok"], false);
        }
    }

    #[test]
    fn stop_routes_each_owner_through_its_existing_dispatcher() {
        let root = PortableRoot::new();
        let turn_calls = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let run_calls = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let turn_record = std::sync::Arc::clone(&turn_calls);
        let turn = move |target: &StopTarget| {
            turn_record.lock().unwrap().push(target.turn_handle.clone());
            OwnerStopDisposition::Acknowledged
        };
        let run_record = std::sync::Arc::clone(&run_calls);
        let run = move |target: &StopTarget| {
            run_record.lock().unwrap().push(target.run_id.clone());
            OwnerStopDisposition::Requested
        };
        let ports = WorkStopPorts {
            turn: Some(&turn),
            run: Some(&run),
        };
        let turn_stop = stop_work(
            &json!({
                "turnHandle": "dispatch:one",
                "conversationId": "conversation:one",
            }),
            &ports,
        );
        assert_eq!(turn_stop["status"], "stop-requested");
        assert_eq!(turn_stop["ownerKind"], STOP_OWNER_CONVERSATION_TURN);
        assert_eq!(turn_stop["disposition"], "acknowledged");
        let run_stop = stop_work(&json!({"runId": "run-one"}), &ports);
        assert_eq!(run_stop["ownerKind"], STOP_OWNER_WORKFLOW_RUN);
        assert_eq!(run_stop["disposition"], "requested");
        // An ambiguous or unknown target is refused instead of guessed.
        let ambiguous = stop_work(
            &json!({"turnHandle": "dispatch:one", "runId": "run-one"}),
            &ports,
        );
        assert_eq!(ambiguous["error"]["code"], "stop_target_ambiguous");
        let unknown = stop_work(&json!({}), &ports);
        assert_eq!(unknown["error"]["code"], "stop_target_unknown");
        // With no host port, the owner path is explicitly unavailable.
        let unavailable = stop_work(
            &json!({"turnHandle": "dispatch:one", "conversationId": "conversation:one"}),
            &WorkStopPorts::default(),
        );
        assert_eq!(unavailable["status"], "owner-unavailable");
        assert_eq!(unavailable["ok"], false);
        assert_eq!(turn_calls.lock().unwrap().len(), 1);
        assert_eq!(run_calls.lock().unwrap().len(), 1);

        // The request and the anomaly are durable, and survive a fresh reader.
        let events = read_events(&root.root);
        assert!(
            events
                .iter()
                .any(|event| event["type"] == "work.stop.requested")
        );
        assert!(
            events
                .iter()
                .any(|event| event["type"] == "work.stop.anomaly"),
            "an unreachable owner is a recorded anomaly: {events:?}"
        );
    }

    #[test]
    fn stop_diagnostics_never_contain_raw_payloads_or_paths() {
        let root = PortableRoot::new();
        let turn = |_: &StopTarget| OwnerStopDisposition::Acknowledged;
        let ports = WorkStopPorts {
            turn: Some(&turn),
            run: None,
        };
        let response = stop_work(
            &json!({
                "turnHandle": "dispatch:two",
                "conversationId": "conversation:two",
                "reason": "/fixture/private/secret-token",
                "prompt": "confidential work content",
                "apiKey": "redacted-synthetic-canary",
            }),
            &ports,
        );
        assert_eq!(response["status"], "stop-requested");
        let events = read_events(&root.root);
        let encoded = serde_json::to_string(&events).unwrap();
        assert!(!encoded.contains("/fixture/private"), "{encoded}");
        assert!(!encoded.contains("confidential work content"), "{encoded}");
        assert!(!encoded.contains("redacted-synthetic-canary"), "{encoded}");
        assert!(
            events.iter().all(|event| event["payload"]["correlationId"]
                .as_str()
                .is_some_and(|id| id.starts_with("stop-"))),
            "every recorded stop event carries the opaque correlation id"
        );
    }

    #[test]
    fn an_unanswered_claim_stop_port_refuses_instead_of_reporting_a_stop() {
        let port = SubagentClaimStopPort::new();
        assert_eq!(
            port.stop("conversation:one", "caller:one", "member:one"),
            Err("subagent_claim_stop_unavailable"),
        );
    }

    #[test]
    fn an_installed_claim_dispatcher_is_the_only_answer_the_port_gives() {
        fn cancelled(_: &str, _: &str, _: &str) -> Result<Value, &'static str> {
            Ok(json!({"state": "cancelled"}))
        }
        fn unavailable(_: &str, _: &str, _: &str) -> Result<Value, &'static str> {
            Err("subagent_cancel_unavailable")
        }
        let port = SubagentClaimStopPort::new();
        port.install(cancelled).unwrap();
        assert_eq!(
            port.stop("conversation:one", "caller:one", "member:one"),
            Ok(json!({"state": "cancelled"})),
        );
        assert!(port.install(unavailable).is_err());
        assert_eq!(
            port.stop("conversation:one", "caller:one", "member:one"),
            Ok(json!({"state": "cancelled"})),
        );
    }
}
