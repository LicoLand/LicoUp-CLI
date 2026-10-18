//! What a remote control may ask for, and which durable owner a target names.
//!
//! The vocabulary is deliberately the kernel's own: [`WorkOwner`] carries the
//! stop-owner names the local work owners already publish, and a target is one
//! owner plus that owner's own durable scope id. A request that names an owner
//! this build does not have selects nothing and is refused by the port rather
//! than translated into some nearest local thing.

/// The durable owner of one piece of admitted work.
///
/// It is an identity, not a label a request may invent: [`Self::from_name`]
/// resolves the kernel's own names, and anything else names no owner here.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum WorkOwner {
    /// The persistent conversation turn of one membership.
    ConversationTurn,
    /// The durable workflow run behind `strategy.run.*`.
    WorkflowRun,
    /// The durable Subagent MCP dispatch claim.
    SubagentClaim,
    /// The supervised lane session of a local Agent service or adapter turn.
    LaneSession,
}

impl WorkOwner {
    /// The stable name this owner publishes.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ConversationTurn => "conversationTurn",
            Self::WorkflowRun => "workflowRun",
            Self::SubagentClaim => "subagentClaim",
            Self::LaneSession => "laneSession",
        }
    }

    /// The owner one published name resolves to, when this build has it.
    #[must_use]
    pub fn from_name(name: &str) -> Option<Self> {
        match name {
            "conversationTurn" => Some(Self::ConversationTurn),
            "workflowRun" => Some(Self::WorkflowRun),
            "subagentClaim" => Some(Self::SubagentClaim),
            "laneSession" => Some(Self::LaneSession),
            _ => None,
        }
    }
}

/// One piece of work, named by its durable owner and that owner's scope id.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct WorkTarget {
    owner: WorkOwner,
    scope: String,
}

impl WorkTarget {
    #[must_use]
    pub fn new(owner: WorkOwner, scope: impl Into<String>) -> Self {
        Self {
            owner,
            scope: scope.into(),
        }
    }

    #[must_use]
    pub const fn owner(&self) -> WorkOwner {
        self.owner
    }

    #[must_use]
    pub fn scope(&self) -> &str {
        &self.scope
    }
}

/// How much of a target's owned work a stop selects.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum StopScope {
    /// The target and the descendants it durably owns.
    Subtree,
    /// Exactly one owned child of the named parent.
    OwnedChild(WorkTarget),
}

/// One typed remote work control intent.
///
/// The intent is what the verified requester asked for; whether this host may
/// perform it is a separate answer, and the intent never carries its own
/// authority.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RemoteWorkIntent {
    /// Read the current state of one target. It changes no work.
    Inspect { target: WorkTarget },
    /// Stop one target and the work it durably owns.
    Stop { target: WorkTarget },
    /// Stop exactly one owned child of one parent.
    StopOwnedChild {
        parent: WorkTarget,
        child: WorkTarget,
    },
    /// Force-stop one target through the kernel's own owned-process control.
    ForceStop { target: WorkTarget },
}

impl RemoteWorkIntent {
    /// The stable kind a caller publishes.
    #[must_use]
    pub const fn kind(&self) -> &'static str {
        match self {
            Self::Inspect { .. } => "work.inspect",
            Self::Stop { .. } => "work.stop",
            Self::StopOwnedChild { .. } => "work.stopOwnedChild",
            Self::ForceStop { .. } => "work.forceStop",
        }
    }

    /// The target this intent names.
    #[must_use]
    pub const fn target(&self) -> &WorkTarget {
        match self {
            Self::Inspect { target } | Self::Stop { target } | Self::ForceStop { target } => target,
            Self::StopOwnedChild { child, .. } => child,
        }
    }

    /// Whether performing this intent changes any work.
    ///
    /// An inspect is admitted and answered like any other request — a replayed
    /// inspect asks the owner nothing a second time — but it stops nothing.
    #[must_use]
    pub const fn changes_work(&self) -> bool {
        !matches!(self, Self::Inspect { .. })
    }
}

/// The longest correlation id one force control's diagnostics may carry.
pub const MAX_CORRELATION_ID_BYTES: usize = 64;

/// The longest stable reason code one force control's diagnostics may carry.
pub const MAX_DIAGNOSTIC_REASON_BYTES: usize = 64;

/// The locally produced, redacted diagnostics a force control must carry.
///
/// It is redacted by construction: a correlation id the local activity log
/// already minted and a stable reason code. It carries no payload, no path, no
/// user content and no peer-supplied text, so a remote requester cannot write
/// into this host's diagnostics through the request it sent.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RedactedDiagnostics {
    correlation_id: String,
    reason_code: String,
}

impl RedactedDiagnostics {
    /// Build the diagnostics for one force control.
    pub fn new(
        correlation_id: impl Into<String>,
        reason_code: impl Into<String>,
    ) -> Result<Self, DiagnosticsRefusal> {
        let correlation_id = correlation_id.into();
        let reason_code = reason_code.into();
        validate_diagnostic_field(&correlation_id, MAX_CORRELATION_ID_BYTES, "correlationId")?;
        validate_diagnostic_field(&reason_code, MAX_DIAGNOSTIC_REASON_BYTES, "reasonCode")?;
        Ok(Self {
            correlation_id,
            reason_code,
        })
    }

    #[must_use]
    pub fn correlation_id(&self) -> &str {
        &self.correlation_id
    }

    #[must_use]
    pub fn reason_code(&self) -> &str {
        &self.reason_code
    }
}

fn validate_diagnostic_field(
    value: &str,
    bound: usize,
    field: &'static str,
) -> Result<(), DiagnosticsRefusal> {
    if value.trim().is_empty() {
        return Err(DiagnosticsRefusal::EmptyField { field });
    }
    if value.len() > bound {
        return Err(DiagnosticsRefusal::TooLong { field, bound });
    }
    Ok(())
}

/// Why diagnostics could not be built.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DiagnosticsRefusal {
    /// The field was empty, so the diagnostic would say nothing.
    EmptyField { field: &'static str },
    /// The field exceeded its bound.
    TooLong { field: &'static str, bound: usize },
}

impl DiagnosticsRefusal {
    /// The stable reason a caller publishes.
    #[must_use]
    pub const fn reason(self) -> &'static str {
        "endpoint_remote_control_diagnostics_invalid"
    }
}

#[cfg(test)]
mod tests {
    use super::{
        DiagnosticsRefusal, MAX_CORRELATION_ID_BYTES, RedactedDiagnostics, RemoteWorkIntent,
        StopScope, WorkOwner, WorkTarget,
    };

    #[test]
    fn every_owner_name_round_trips_and_nothing_else_resolves() {
        for owner in [
            WorkOwner::ConversationTurn,
            WorkOwner::WorkflowRun,
            WorkOwner::SubagentClaim,
            WorkOwner::LaneSession,
        ] {
            assert_eq!(WorkOwner::from_name(owner.as_str()), Some(owner));
        }
        assert_eq!(WorkOwner::from_name("taskRunner"), None);
    }

    #[test]
    fn a_target_is_its_owner_and_that_owners_own_scope() {
        let first = WorkTarget::new(WorkOwner::WorkflowRun, "run-1");
        let second = WorkTarget::new(WorkOwner::WorkflowRun, "run-2");
        let other_owner = WorkTarget::new(WorkOwner::LaneSession, "run-1");

        assert_ne!(first, second);
        assert_ne!(
            first, other_owner,
            "the same scope id in another owner is another target"
        );
        assert_eq!(first.owner(), WorkOwner::WorkflowRun);
        assert_eq!(first.scope(), "run-1");
    }

    #[test]
    fn inspect_changes_no_work_and_every_other_intent_does() {
        let target = WorkTarget::new(WorkOwner::ConversationTurn, "turn-1");
        assert!(
            !RemoteWorkIntent::Inspect {
                target: target.clone()
            }
            .changes_work()
        );
        assert!(
            RemoteWorkIntent::Stop {
                target: target.clone()
            }
            .changes_work()
        );
        assert!(
            RemoteWorkIntent::StopOwnedChild {
                parent: target.clone(),
                child: WorkTarget::new(WorkOwner::SubagentClaim, "claim-1"),
            }
            .changes_work()
        );
        assert!(RemoteWorkIntent::ForceStop { target }.changes_work());
        assert_eq!(
            StopScope::OwnedChild(WorkTarget::new(WorkOwner::SubagentClaim, "claim-1")),
            StopScope::OwnedChild(WorkTarget::new(WorkOwner::SubagentClaim, "claim-1"))
        );
    }

    #[test]
    fn diagnostics_are_bounded_redacted_fields_and_never_empty() {
        let diagnostics = RedactedDiagnostics::new("correlation-1", "remoteForceStopConfirmed")
            .expect("bounded fields");
        assert_eq!(diagnostics.correlation_id(), "correlation-1");
        assert_eq!(diagnostics.reason_code(), "remoteForceStopConfirmed");

        assert_eq!(
            RedactedDiagnostics::new("  ", "reason").unwrap_err(),
            DiagnosticsRefusal::EmptyField {
                field: "correlationId"
            }
        );
        assert_eq!(
            RedactedDiagnostics::new("id", "").unwrap_err(),
            DiagnosticsRefusal::EmptyField {
                field: "reasonCode"
            }
        );
        assert_eq!(
            RedactedDiagnostics::new("x".repeat(MAX_CORRELATION_ID_BYTES + 1), "reason")
                .unwrap_err(),
            DiagnosticsRefusal::TooLong {
                field: "correlationId",
                bound: MAX_CORRELATION_ID_BYTES
            }
        );
        assert_eq!(
            DiagnosticsRefusal::TooLong {
                field: "correlationId",
                bound: MAX_CORRELATION_ID_BYTES
            }
            .reason(),
            "endpoint_remote_control_diagnostics_invalid"
        );
    }
}
