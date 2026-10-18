//! Machine receipts, and what is deliberately not one.
//!
//! A receipt is written by the tool that ran an invocation. It reports one of
//! three observed facts — the call was admitted, it completed, or its effect is
//! unknown — and it is the only place a caller learns them, because a receipt
//! carries the `requestId` it answers.
//!
//! An agent's natural reply is not a receipt and needs no envelope. It is
//! carried verbatim in [`NaturalOutput`], is never parsed, and a body that
//! merely looks like JSON is still text. A malformed structured envelope is a
//! fault of the machine interface that produced it, and never makes a natural
//! reply invalid.

use crate::command::Operation;
use crate::failure::{ApplicationFailure, EffectCertainty};
use crate::result::{OperationReference, OperationState};
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// What a tool observed about one invocation.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum ReceiptKind {
    /// The tool started the work. This is not a completion.
    Admitted,
    /// The tool observed the end of the work.
    Completed,
    /// The effect may or may not have happened. The durable record decides.
    Unknown,
}

impl ReceiptKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Admitted => "admitted",
            Self::Completed => "completed",
            Self::Unknown => "unknown",
        }
    }

    /// The state this receipt reports.
    ///
    /// An unknown effect is not a completed one: it reports
    /// [`OperationState::ReconciliationRequired`], so no caller reads "the tool
    /// answered" as "the work happened".
    pub const fn state(self) -> OperationState {
        match self {
            Self::Admitted => OperationState::Accepted,
            Self::Completed => OperationState::Completed,
            Self::Unknown => OperationState::ReconciliationRequired,
        }
    }
}

/// The machine receipt one invocation produces.
///
/// Every constructor normalizes the reference's state to the kind it reports, so
/// constructors report one state. Decoding rejects contradictory machine data;
/// neither construction nor decoding authenticates the tool's observation.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolReceipt {
    /// The invocation this answers.
    pub request_id: String,
    pub kind: ReceiptKind,
    /// The neutral operation name, identical to the reference's.
    pub operation: String,
    /// The identity the caller may follow, for a completed receipt as well as a
    /// live one.
    pub reference: OperationReference,
    /// The operation's own body. Present when the receipt carries one.
    #[serde(default, skip_serializing_if = "Value::is_null")]
    pub payload: Value,
    /// Present only for an unknown effect, where it always requires
    /// reconciliation. See [`ToolReceipt::unknown`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failure: Option<ApplicationFailure>,
}

impl<'de> Deserialize<'de> for ToolReceipt {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct Wire {
            request_id: String,
            kind: ReceiptKind,
            operation: String,
            reference: OperationReference,
            #[serde(default)]
            payload: Value,
            #[serde(default)]
            failure: Option<ApplicationFailure>,
        }
        let wire = Wire::deserialize(deserializer)?;
        if wire.reference.state != wire.kind.state()
            || wire.operation != wire.reference.operation
            || match wire.kind {
                ReceiptKind::Unknown => !wire.failure.as_ref().is_some_and(|failure| {
                    failure.effect == EffectCertainty::Uncertain
                        && failure.requires_reconciliation()
                }),
                ReceiptKind::Admitted | ReceiptKind::Completed => wire.failure.is_some(),
            }
        {
            return Err(serde::de::Error::custom("inconsistent tool receipt"));
        }
        Ok(Self {
            request_id: wire.request_id,
            kind: wire.kind,
            operation: wire.operation,
            reference: wire.reference,
            payload: wire.payload,
            failure: wire.failure,
        })
    }
}

impl ToolReceipt {
    /// An invocation the tool admitted but has not finished.
    pub fn admitted(request_id: impl Into<String>, reference: OperationReference) -> Self {
        Self::from_reference(
            request_id,
            ReceiptKind::Admitted,
            reference,
            Value::Null,
            None,
        )
    }

    /// An invocation whose effect the tool observed.
    pub fn completed(
        request_id: impl Into<String>,
        reference: OperationReference,
        payload: Value,
    ) -> Self {
        Self::from_reference(request_id, ReceiptKind::Completed, reference, payload, None)
    }

    /// A completed read, which has no live identity to follow.
    pub fn read(request_id: impl Into<String>, operation: Operation, payload: Value) -> Self {
        Self::completed(
            request_id,
            OperationReference::new(operation, "", OperationState::Completed),
            payload,
        )
    }

    /// An effect that may or may not have happened.
    ///
    /// The failure is built here, as an uncertain one, so the receipt cannot
    /// report a blind retry as the next step and cannot report the work as
    /// completed: the caller reconciles against the durable record, which is the
    /// only thing that knows whether the effect happened.
    pub fn unknown(
        request_id: impl Into<String>,
        reference: OperationReference,
        code: &str,
        stage: &str,
    ) -> Self {
        Self::from_reference(
            request_id,
            ReceiptKind::Unknown,
            reference,
            Value::Null,
            Some(ApplicationFailure::uncertain(code, stage)),
        )
    }

    /// The state this receipt reports, as the reference carries it.
    pub fn state(&self) -> OperationState {
        self.reference.state
    }

    pub fn failure(&self) -> Option<&ApplicationFailure> {
        self.failure.as_ref()
    }

    fn from_reference(
        request_id: impl Into<String>,
        kind: ReceiptKind,
        mut reference: OperationReference,
        payload: Value,
        failure: Option<ApplicationFailure>,
    ) -> Self {
        reference.state = kind.state();
        Self {
            request_id: request_id.into(),
            kind,
            operation: reference.operation.clone(),
            reference,
            payload,
            failure,
        }
    }
}

/// Natural output: what an agent, a runtime or the user actually said.
///
/// It is carried verbatim. There is no required schema — an ordinary reply needs
/// no envelope — so a body that looks like JSON or like a broken envelope is
/// still just text here, and nothing in this crate parses it into a receipt.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NaturalOutput {
    pub text: String,
}

impl NaturalOutput {
    pub fn new(text: impl Into<String>) -> Self {
        Self { text: text.into() }
    }

    pub fn text(&self) -> &str {
        self.text.as_str()
    }

    pub fn is_empty(&self) -> bool {
        self.text.is_empty()
    }
}
