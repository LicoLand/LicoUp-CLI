//! Results and stable operation references.
//!
//! A caller needs to be able to ask "what happened to the thing I started?"
//! without knowing which interface started it. [`OperationReference`] is that
//! answer: one identity per operation, carrying the state the product already
//! publishes on receipts, so a CLI call and an MCP call for the same operation
//! return the same reference rather than two lookalikes.

use crate::command::Operation;
use crate::failure::ApplicationFailure;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Where an operation is in its lifecycle.
///
/// The wire strings are the ones the product already publishes on subagent
/// receipts, so a reference can be compared with an existing receipt without a
/// translation table.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum OperationState {
    Accepted,
    Processing,
    Responding,
    CancelRequested,
    Cancelled,
    Completed,
    Failed,
    /// The outcome is not knowable from the failure alone; read the durable
    /// record before acting.
    ReconciliationRequired,
}

impl OperationState {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Accepted => "accepted",
            Self::Processing => "processing",
            Self::Responding => "responding",
            Self::CancelRequested => "cancel-requested",
            Self::Cancelled => "cancelled",
            Self::Completed => "completed",
            Self::Failed => "failed",
            Self::ReconciliationRequired => "reconciliation-required",
        }
    }

    /// Whether the operation still holds a live slot.
    pub const fn is_live(self) -> bool {
        matches!(
            self,
            Self::Accepted | Self::Processing | Self::Responding | Self::CancelRequested
        )
    }

    /// Whether the operation has stopped for good.
    pub const fn is_terminal(self) -> bool {
        matches!(
            self,
            Self::Cancelled | Self::Completed | Self::Failed | Self::ReconciliationRequired
        )
    }
}

/// The identity of one operation, in the terms both interfaces already use.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OperationReference {
    /// Which operation this refers to.
    pub operation: String,
    /// The durable identifier: dispatch id, run id, or export id.
    pub id: String,
    pub state: OperationState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub conversation_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub membership_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub depth: Option<u8>,
    /// Present when the caller supplied one. A replay carrying the same key
    /// must return this same reference instead of starting a second operation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub idempotency_key: Option<String>,
}

impl OperationReference {
    pub fn new(operation: Operation, id: impl Into<String>, state: OperationState) -> Self {
        Self {
            operation: operation.as_str().to_owned(),
            id: id.into(),
            state,
            conversation_id: None,
            membership_id: None,
            depth: None,
            idempotency_key: None,
        }
    }

    pub fn with_conversation(mut self, conversation_id: impl Into<String>) -> Self {
        self.conversation_id = Some(conversation_id.into());
        self
    }

    pub fn with_membership(mut self, membership_id: impl Into<String>) -> Self {
        self.membership_id = Some(membership_id.into());
        self
    }

    pub fn with_depth(mut self, depth: u8) -> Self {
        self.depth = Some(depth);
        self
    }

    pub fn with_idempotency_key(mut self, key: impl Into<String>) -> Self {
        self.idempotency_key = Some(key.into());
        self
    }

    /// Whether this reference identifies the same operation as `other`.
    ///
    /// Two interfaces serving the same operation must agree on all of this; a
    /// difference means one of them invented its own identity.
    pub fn same_operation(&self, other: &Self) -> bool {
        self.operation == other.operation
            && self.id == other.id
            && self.conversation_id == other.conversation_id
            && self.membership_id == other.membership_id
    }
}

/// One successful command result: the reference, plus the operation's own body.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CommandOutcome {
    pub reference: OperationReference,
    #[serde(default, skip_serializing_if = "Value::is_null")]
    pub payload: Value,
}

impl CommandOutcome {
    pub fn new(reference: OperationReference) -> Self {
        Self {
            reference,
            payload: Value::Null,
        }
    }

    pub fn with_payload(mut self, payload: Value) -> Self {
        self.payload = payload;
        self
    }

    /// A result that carries no reference — a read, such as a listing or a
    /// search. Reads never produce an effect, so they have no live identity to
    /// hand back; the operation still names itself, because a caller that reads
    /// projects must not be told it read conversations.
    pub fn read(operation: Operation, payload: Value) -> Self {
        Self {
            reference: OperationReference::new(operation, "", OperationState::Completed),
            payload,
        }
    }

    /// Whether this outcome carries a live operation a caller may follow.
    pub fn is_live(&self) -> bool {
        self.reference.state.is_live()
    }
}

/// A command either produced an outcome or failed. The two are exclusive, so a
/// caller cannot see a result and a failure for the same attempt.
#[derive(Clone, Debug, PartialEq)]
pub enum CommandResolution {
    Resolved(CommandOutcome),
    Failed(ApplicationFailure),
}

impl CommandResolution {
    pub fn outcome(&self) -> Option<&CommandOutcome> {
        match self {
            Self::Resolved(outcome) => Some(outcome),
            Self::Failed(_) => None,
        }
    }

    pub fn failure(&self) -> Option<&ApplicationFailure> {
        match self {
            Self::Resolved(_) => None,
            Self::Failed(failure) => Some(failure),
        }
    }

    pub fn is_ok(&self) -> bool {
        matches!(self, Self::Resolved(_))
    }
}
