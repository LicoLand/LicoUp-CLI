//! The persisted run snapshot and reducer event vocabulary, reduced by the
//! generic engine's machine.
//!
//! Two kinds of facts stay LicoUp-owned here:
//!
//! * [`RunSnapshot`] is a persisted record (`strategy_runs.snapshot_json`):
//!   it carries the Conversation identity (`conversationId`), the Assistant
//!   Membership a run answers to (`assistantMembershipId`) and the committed
//!   route receipt (`routeReceipt`) — identities the generic engine groups
//!   under its own opaque `contextId`/`runBinding` pair. The engine never
//!   reads either shape, so the reducer carries them through sideband.
//! * [`ReducerEvent`] is a persisted record (`strategy_run_events.event_json`)
//!   plus the durable transition intent (`event` inside it): it keeps the
//!   Assistant-owned failure events and the three-way `advance`/`return`/
//!   `terminate` callback decision spelling. Shared variants translate 1:1
//!   onto the engine's event; the Assistant variants settle locally.

use anyhow::{Result, anyhow, ensure};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};

use acativa::workflow::machine as generic;

use crate::{
    CallbackDecisionKind, CompiledWorkflow, FailureClass, FallbackReceipt, INPUT_ADAPTER_VERSION,
    PendingCallback, StrategyRunStatus,
};

// Every shared machine fact is the engine's own type: command spellings,
// shared-state ledgers, join receipts and the reducer delta are the same
// bytes on either boundary.
pub use generic::{
    CommandKind, CommandStatus, JoinArrival, JoinArrivalReceipt, JoinLedger, JoinSatisfaction,
    ReducerDelta, RunCommand, SharedEntry, SharedResourceState, SharedWriteReceipt,
    StaleJoinArrival, VisitFact, binding_session_key, effect_input_for,
};

/// Whether the run's attempt lineage for one command stays inside the
/// definition's budget. The engine's check reads shared fields only.
pub fn lineage_within_budget(
    snapshot: &RunSnapshot,
    state_id: &str,
    visit: u64,
    slot_id: Option<&str>,
    item_id: Option<&str>,
    max_attempts: u8,
) -> bool {
    generic::lineage_within_budget(
        &to_generic_snapshot(snapshot),
        state_id,
        visit,
        slot_id,
        item_id,
        max_attempts,
    )
}

/// The fallback reason a failed actor or workset command earns, when one is
/// lawful. The engine's rule reads shared fields only.
pub fn fallback_reason(
    workflow: &CompiledWorkflow,
    snapshot: &RunSnapshot,
    command: &RunCommand,
) -> Option<&'static str> {
    generic::fallback_reason(workflow, &to_generic_snapshot(snapshot), command)
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RunSnapshot {
    pub run_id: String,
    pub definition_digest: String,
    pub semantics_digest: String,
    pub status: StrategyRunStatus,
    pub sequence: u64,
    pub input: Value,
    pub active_states: BTreeSet<String>,
    pub completed_states: BTreeSet<String>,
    pub state_visits: BTreeMap<String, u64>,
    /// Per-visit arrivals, so an old visit of a predecessor can never satisfy a
    /// new join round.
    pub join_arrivals: BTreeMap<String, JoinLedger>,
    /// The run's declared causal input plan.
    #[serde(default)]
    pub input_plan: crate::InputPlan,
    /// Versioned shared state. Reading it without naming a revision is not
    /// expressible: the only readers are bindings, and a binding records one.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub shared: BTreeMap<String, SharedResourceState>,
    /// The exact binding every entered node visit received.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub bindings: BTreeMap<String, crate::InputBinding>,
    #[serde(default)]
    pub actor_sessions: BTreeMap<String, String>,
    #[serde(default)]
    pub slot_ordinals: BTreeMap<String, u8>,
    #[serde(default)]
    pub slot_candidate_counts: BTreeMap<String, u8>,
    #[serde(default)]
    pub attempt_lineage: BTreeMap<String, u8>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub merge_sources: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub fallbacks: Vec<FallbackReceipt>,
    /// The Conversation this run answers to, when it is conversation-bound.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub conversation_id: Option<String>,
    /// The Assistant Membership this run answers to.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub assistant_membership_id: Option<String>,
    /// The committed route receipt recorded at admission.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub route_receipt: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    /// Callback-mode edges whose targets wait for the master agent's decision,
    /// in the deterministic order the waits were entered. The decision event
    /// always settles the first entry.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub pending_callbacks: Vec<PendingCallback>,
    pub commands: BTreeMap<String, RunCommand>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub diagnostic_code: Option<String>,
}

impl RunSnapshot {
    pub fn empty(
        run_id: impl Into<String>,
        definition_digest: impl Into<String>,
        semantics_digest: impl Into<String>,
    ) -> Self {
        Self {
            run_id: run_id.into(),
            definition_digest: definition_digest.into(),
            semantics_digest: semantics_digest.into(),
            status: StrategyRunStatus::Pending,
            sequence: 0,
            input: Value::Object(Default::default()),
            active_states: BTreeSet::new(),
            completed_states: BTreeSet::new(),
            state_visits: BTreeMap::new(),
            join_arrivals: BTreeMap::new(),
            input_plan: crate::InputPlan::default(),
            shared: BTreeMap::new(),
            bindings: BTreeMap::new(),
            actor_sessions: BTreeMap::new(),
            slot_ordinals: BTreeMap::new(),
            slot_candidate_counts: BTreeMap::new(),
            attempt_lineage: BTreeMap::new(),
            merge_sources: BTreeMap::new(),
            fallbacks: Vec::new(),
            conversation_id: None,
            assistant_membership_id: None,
            route_receipt: None,
            cwd: None,
            pending_callbacks: Vec::new(),
            commands: BTreeMap::new(),
            diagnostic_code: None,
        }
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum ReducerEvent {
    /// The run's declared causal input plan, before anything runs. A run that
    /// never declares one keeps the safe default; a plan that declares an
    /// adapter version this build does not project is refused, so a stored
    /// version can never be silently read as a different one.
    InputPlanDeclared {
        plan: crate::InputPlan,
    },
    Start {
        input: Value,
    },
    AuthorizationGranted {
        semantics_digest: String,
    },
    AuthorizationDenied,
    AuthorizationRevoked,
    CommandClaimed {
        command_id: String,
        attempt_token: String,
    },
    CommandStarted {
        command_id: String,
        attempt_token: String,
    },
    CommandSucceeded {
        command_id: String,
        attempt_token: String,
        output: Value,
    },
    CommandFailed {
        command_id: String,
        attempt_token: String,
        class: FailureClass,
        code: String,
    },
    /// Assistant-owned Graph effects never enter the generic implicit retry,
    /// fallback, or failure-transition path. The originating Assistant turn
    /// receives this durable terminal outcome and may choose what to do next.
    AssistantEffectFailed {
        command_id: String,
        attempt_token: String,
        class: FailureClass,
        code: String,
    },
    /// A drive-level failure has unknown effect position, so the Assistant
    /// Graph settles in-doubt without duplicating or guessing an effect.
    AssistantDriveFailed {
        code: String,
    },
    /// The master agent's decision for the oldest pending callback wait. The
    /// declared callback edge is only taken (`advance`), the completed state
    /// re-entered (`return`), or the run terminated (`terminate`) after this
    /// run input arrives; the wait itself is durable.
    CallbackDecision {
        state_id: String,
        state_visit: u64,
        decision: CallbackDecisionKind,
    },
    RetryRequested {
        command_id: String,
    },
    CancelRequested,
    CancellationAcknowledged {
        command_id: String,
        attempt_token: String,
    },
    CancellationUnknown {
        command_id: String,
        attempt_token: String,
    },
    FallbackIssued {
        failed_command_id: String,
        next_ordinal: u8,
        locator: Value,
        from_value_id: String,
        to_value_id: String,
        reason: String,
        attempts: u8,
    },
}

#[derive(Clone, Debug, PartialEq)]
pub struct ReducerOutput {
    pub snapshot: RunSnapshot,
    pub emitted_commands: Vec<RunCommand>,
    pub applied: bool,
    /// What this step changed, for a consumer that persists incrementally.
    pub delta: ReducerDelta,
}

/// Reduce one event against one snapshot. Assistant-owned failure events
/// settle under this crate's own rules; every shared event runs through the
/// engine's reducer over the shared field projection, and the LicoUp-owned
/// bindings ride the same step untouched.
pub fn reduce(
    workflow: &CompiledWorkflow,
    previous: &RunSnapshot,
    event: ReducerEvent,
) -> Result<ReducerOutput> {
    match event {
        event @ (ReducerEvent::AssistantEffectFailed { .. }
        | ReducerEvent::AssistantDriveFailed { .. }) => reduce_assistant(previous, event),
        event => {
            let generic = generic::reduce(
                workflow,
                &to_generic_snapshot(previous),
                translate_event(event, previous),
            )?;
            Ok(ReducerOutput {
                snapshot: from_generic_snapshot(generic.snapshot, previous),
                emitted_commands: generic.emitted_commands,
                applied: generic.applied,
                delta: generic.delta,
            })
        }
    }
}

/// The engine's snapshot view of one LicoUp run: every shared field moves
/// across; the LicoUp-only bindings have no engine slot and stay sideband —
/// the engine never reads its own opaque pair either.
fn to_generic_snapshot(previous: &RunSnapshot) -> generic::RunSnapshot {
    generic::RunSnapshot {
        run_id: previous.run_id.clone(),
        definition_digest: previous.definition_digest.clone(),
        semantics_digest: previous.semantics_digest.clone(),
        status: previous.status,
        sequence: previous.sequence,
        input: previous.input.clone(),
        active_states: previous.active_states.clone(),
        completed_states: previous.completed_states.clone(),
        state_visits: previous.state_visits.clone(),
        join_arrivals: previous.join_arrivals.clone(),
        input_plan: previous.input_plan.clone(),
        shared: previous.shared.clone(),
        bindings: previous.bindings.clone(),
        actor_sessions: previous.actor_sessions.clone(),
        slot_ordinals: previous.slot_ordinals.clone(),
        slot_candidate_counts: previous.slot_candidate_counts.clone(),
        attempt_lineage: previous.attempt_lineage.clone(),
        merge_sources: previous.merge_sources.clone(),
        fallbacks: previous.fallbacks.clone(),
        context_id: previous.conversation_id.clone(),
        run_binding: previous.route_receipt.clone(),
        cwd: previous.cwd.clone(),
        pending_callbacks: previous.pending_callbacks.clone(),
        commands: previous.commands.clone(),
        diagnostic_code: previous.diagnostic_code.clone(),
    }
}

/// Rebuild the persisted LicoUp snapshot from the engine's result. The engine
/// never writes the opaque binding pair, so the LicoUp bindings return
/// exactly as they arrived; every engine-touched field comes back with it.
fn from_generic_snapshot(generic: generic::RunSnapshot, previous: &RunSnapshot) -> RunSnapshot {
    RunSnapshot {
        run_id: generic.run_id,
        definition_digest: generic.definition_digest,
        semantics_digest: generic.semantics_digest,
        status: generic.status,
        sequence: generic.sequence,
        input: generic.input,
        active_states: generic.active_states,
        completed_states: generic.completed_states,
        state_visits: generic.state_visits,
        join_arrivals: generic.join_arrivals,
        input_plan: generic.input_plan,
        shared: generic.shared,
        bindings: generic.bindings,
        actor_sessions: generic.actor_sessions,
        slot_ordinals: generic.slot_ordinals,
        slot_candidate_counts: generic.slot_candidate_counts,
        attempt_lineage: generic.attempt_lineage,
        merge_sources: generic.merge_sources,
        fallbacks: generic.fallbacks,
        conversation_id: previous.conversation_id.clone(),
        assistant_membership_id: previous.assistant_membership_id.clone(),
        route_receipt: previous.route_receipt.clone(),
        cwd: generic.cwd,
        pending_callbacks: generic.pending_callbacks,
        commands: generic.commands,
        diagnostic_code: generic.diagnostic_code,
    }
}

/// Map one shared event onto the engine's event. The only translated variant
/// is the callback decision: the persisted `advance`/`return`/`terminate`
/// spelling becomes the engine's positional settlement — `advance` names the
/// edge's declared target, `return` names the completed state the edge left.
/// A decision for a wait that is not the oldest pending one is stale; the
/// engine refuses it on the pending match before reading the decision, so the
/// positional value it never inspects can be any lawful one.
fn translate_event(event: ReducerEvent, previous: &RunSnapshot) -> generic::ReducerEvent {
    match event {
        ReducerEvent::InputPlanDeclared { plan } => {
            generic::ReducerEvent::InputPlanDeclared { plan }
        }
        ReducerEvent::Start { input } => generic::ReducerEvent::Start { input },
        ReducerEvent::AuthorizationGranted { semantics_digest } => {
            generic::ReducerEvent::AuthorizationGranted { semantics_digest }
        }
        ReducerEvent::AuthorizationDenied => generic::ReducerEvent::AuthorizationDenied,
        ReducerEvent::AuthorizationRevoked => generic::ReducerEvent::AuthorizationRevoked,
        ReducerEvent::CommandClaimed {
            command_id,
            attempt_token,
        } => generic::ReducerEvent::CommandClaimed {
            command_id,
            attempt_token,
        },
        ReducerEvent::CommandStarted {
            command_id,
            attempt_token,
        } => generic::ReducerEvent::CommandStarted {
            command_id,
            attempt_token,
        },
        ReducerEvent::CommandSucceeded {
            command_id,
            attempt_token,
            output,
        } => generic::ReducerEvent::CommandSucceeded {
            command_id,
            attempt_token,
            output,
        },
        ReducerEvent::CommandFailed {
            command_id,
            attempt_token,
            class,
            code,
        } => generic::ReducerEvent::CommandFailed {
            command_id,
            attempt_token,
            class,
            code,
        },
        ReducerEvent::CallbackDecision {
            state_id,
            state_visit,
            decision,
        } => {
            let pending = previous.pending_callbacks.first().filter(|pending| {
                pending.state_id == state_id && pending.state_visit == state_visit
            });
            let decision = match decision {
                CallbackDecisionKind::Advance => pending
                    .map(|pending| acativa::CallbackDecision::Enter(pending.target.clone()))
                    .unwrap_or(acativa::CallbackDecision::Terminate),
                CallbackDecisionKind::Return => pending
                    .map(|pending| acativa::CallbackDecision::Enter(pending.state_id.clone()))
                    .unwrap_or(acativa::CallbackDecision::Terminate),
                CallbackDecisionKind::Terminate => acativa::CallbackDecision::Terminate,
            };
            generic::ReducerEvent::CallbackDecision {
                state_id,
                state_visit,
                decision,
            }
        }
        ReducerEvent::RetryRequested { command_id } => {
            generic::ReducerEvent::RetryRequested { command_id }
        }
        ReducerEvent::CancelRequested => generic::ReducerEvent::CancelRequested,
        ReducerEvent::CancellationAcknowledged {
            command_id,
            attempt_token,
        } => generic::ReducerEvent::CancellationAcknowledged {
            command_id,
            attempt_token,
        },
        ReducerEvent::CancellationUnknown {
            command_id,
            attempt_token,
        } => generic::ReducerEvent::CancellationUnknown {
            command_id,
            attempt_token,
        },
        ReducerEvent::FallbackIssued {
            failed_command_id,
            next_ordinal,
            locator,
            from_value_id,
            to_value_id,
            reason,
            attempts,
        } => generic::ReducerEvent::FallbackIssued {
            failed_command_id,
            next_ordinal,
            locator,
            from_value_id,
            to_value_id,
            reason,
            attempts,
        },
        ReducerEvent::AssistantEffectFailed { .. } | ReducerEvent::AssistantDriveFailed { .. } => {
            unreachable!("assistant events settle under this crate's own rules")
        }
    }
}

/// The reducer step for the Assistant-owned failure events. The bookkeeping —
/// sequence, adapter gate, delta shape — mirrors the engine's `reduce`
/// exactly; the settle rules below are this crate's, and neither arm can
/// enqueue an automatic transition, so no drain step follows them.
fn reduce_assistant(previous: &RunSnapshot, event: ReducerEvent) -> Result<ReducerOutput> {
    ensure!(
        previous.input_plan.input_adapter_version == INPUT_ADAPTER_VERSION,
        "strategy_input_adapter_mismatch"
    );
    let mut snapshot = previous.clone();
    let mut delta = ReducerDelta {
        run_id: previous.run_id.clone(),
        sequence: previous.sequence,
        status_before: previous.status,
        status_after: previous.status,
        ..ReducerDelta::default()
    };
    snapshot.sequence = snapshot
        .sequence
        .checked_add(1)
        .ok_or_else(|| anyhow!("strategy_event_sequence_overflow"))?;
    delta.sequence = snapshot.sequence;
    let mut applied = true;
    match event {
        ReducerEvent::AssistantEffectFailed {
            command_id,
            attempt_token,
            class,
            code,
        } => settle_assistant_effect_failure(
            &mut snapshot,
            &mut delta,
            &mut applied,
            &command_id,
            &attempt_token,
            class,
            &code,
        )?,
        ReducerEvent::AssistantDriveFailed { code } => {
            settle_assistant_drive_failure(&mut snapshot, &mut delta, &mut applied, &code)?
        }
        _ => unreachable!("only assistant failure events arrive here"),
    }
    if !applied {
        snapshot.sequence = previous.sequence;
    }
    delta.applied = applied;
    delta.status_after = snapshot.status;
    if !applied {
        // A replayed event changes nothing, so its delta claims nothing.
        delta.entered.clear();
        delta.bindings.clear();
        delta.results.clear();
        delta.joins_satisfied.clear();
        delta.shared_writes.clear();
        delta.settled_commands.clear();
        delta.emitted_commands.clear();
    }
    Ok(ReducerOutput {
        snapshot,
        emitted_commands: Vec::new(),
        applied,
        delta,
    })
}

fn set_command_status(
    snapshot: &mut RunSnapshot,
    delta: &mut ReducerDelta,
    command_id: &str,
    next: CommandStatus,
) {
    let Some(command) = snapshot.commands.get_mut(command_id) else {
        return;
    };
    if command.status == next {
        return;
    }
    command.status = next;
    if !delta
        .settled_commands
        .iter()
        .any(|settled| settled == command_id)
    {
        delta.settled_commands.push(command_id.to_owned());
    }
}

fn validate_failure_code(code: &str) -> Result<()> {
    ensure!(
        !code.is_empty()
            && code.len() <= 96
            && code.chars().all(|character| {
                character.is_ascii_lowercase()
                    || character.is_ascii_digit()
                    || matches!(character, '-' | '_')
            }),
        "strategy_failure_code_invalid"
    );
    Ok(())
}

fn settle_assistant_effect_failure(
    snapshot: &mut RunSnapshot,
    delta: &mut ReducerDelta,
    applied: &mut bool,
    command_id: &str,
    attempt_token: &str,
    class: FailureClass,
    code: &str,
) -> Result<()> {
    validate_failure_code(code)?;
    let current = snapshot
        .commands
        .get(command_id)
        .cloned()
        .ok_or_else(|| anyhow!("strategy_callback_stale"))?;
    ensure!(
        current.attempt_token == attempt_token,
        "strategy_callback_stale"
    );
    if matches!(
        current.status,
        CommandStatus::Failed | CommandStatus::InDoubt
    ) && current.failure_class == Some(class)
        && current.failure_code.as_deref() == Some(code)
    {
        *applied = false;
        return Ok(());
    }
    ensure!(
        matches!(
            current.status,
            CommandStatus::Pending | CommandStatus::Claimed | CommandStatus::Running
        ),
        "strategy_callback_conflict"
    );
    let command = snapshot
        .commands
        .get_mut(command_id)
        .ok_or_else(|| anyhow!("strategy_callback_stale"))?;
    command.failure_class = Some(class);
    command.failure_code = Some(code.to_owned());
    set_command_status(
        snapshot,
        delta,
        command_id,
        if class == FailureClass::InDoubt {
            CommandStatus::InDoubt
        } else {
            CommandStatus::Failed
        },
    );
    snapshot.pending_callbacks.clear();
    cancel_unstarted_assistant_commands(snapshot, delta);
    snapshot.status = if class == FailureClass::InDoubt {
        StrategyRunStatus::CancelInDoubt
    } else {
        StrategyRunStatus::Failed
    };
    snapshot.diagnostic_code = Some(code.to_owned());
    Ok(())
}

fn settle_assistant_drive_failure(
    snapshot: &mut RunSnapshot,
    delta: &mut ReducerDelta,
    applied: &mut bool,
    code: &str,
) -> Result<()> {
    validate_failure_code(code)?;
    if matches!(
        snapshot.status,
        StrategyRunStatus::Completed
            | StrategyRunStatus::Failed
            | StrategyRunStatus::Cancelled
            | StrategyRunStatus::Blocked
            | StrategyRunStatus::CancelInDoubt
            // A run parked on a callback is resting, not in flight. The
            // drive that parked it has nothing left to prove, so a failure
            // reported after the park cannot unmake it — and clearing the
            // pending callbacks here would strand the master's decision.
            | StrategyRunStatus::Waiting
    ) {
        *applied = false;
        return Ok(());
    }
    cancel_unstarted_assistant_commands(snapshot, delta);
    snapshot.pending_callbacks.clear();
    let started = snapshot
        .commands
        .iter()
        .filter(|(_, command)| {
            matches!(
                command.status,
                CommandStatus::Claimed | CommandStatus::Running
            )
        })
        .map(|(id, _)| id.clone())
        .collect::<Vec<_>>();
    for command_id in started {
        if let Some(command) = snapshot.commands.get_mut(&command_id) {
            command.failure_class = Some(FailureClass::InDoubt);
            command.failure_code = Some(code.to_owned());
        }
        set_command_status(snapshot, delta, &command_id, CommandStatus::InDoubt);
    }
    snapshot.status = StrategyRunStatus::CancelInDoubt;
    snapshot.diagnostic_code = Some(code.to_owned());
    Ok(())
}

fn cancel_unstarted_assistant_commands(snapshot: &mut RunSnapshot, delta: &mut ReducerDelta) {
    let unstarted = snapshot
        .commands
        .iter()
        .filter(|(_, command)| {
            matches!(
                command.status,
                CommandStatus::Pending | CommandStatus::Retryable | CommandStatus::CancelRequested
            )
        })
        .map(|(id, _)| id.clone())
        .collect::<Vec<_>>();
    for command_id in unstarted {
        set_command_status(snapshot, delta, &command_id, CommandStatus::Cancelled);
    }
}
