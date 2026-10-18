//! Correlation ids carried end to end (contract C07).
//!
//! The ten slots below are the contract's ids: `userInteractionId`, `requestId`,
//! `conversationId`, `runId`, `nodeVisit`, `effectId`, `attemptToken`,
//! `noticeId`, `sourcePosition`, `prepareId`. They ride on
//! [`super::segment::ObservationSegmentRecord`], which is trace/log data.
//!
//! Values are opaque: this module never interprets, trims, or renames them, and
//! an id may legitimately repeat across segments of one causal chain.

use std::fmt::{self, Display, Formatter};

use serde::{Deserialize, Serialize};

use super::privacy::PrivacyBudget;

/// One named correlation slot.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum CorrelationField {
    /// Id of the user action that caused the work.
    UserInteractionId,
    /// Id of one request on a transport boundary.
    RequestId,
    /// Conversation the work belongs to.
    ConversationId,
    /// Run that owns the work.
    RunId,
    /// Visit of a graph node, distinct from the node identity itself.
    NodeVisit,
    /// Effect being executed.
    EffectId,
    /// Attempt token used to reject replayed receipts.
    AttemptToken,
    /// Notice published for a downstream sink.
    NoticeId,
    /// Source position of a declarative or scripted unit.
    SourcePosition,
    /// Prepare stage identity bound to one input preparation.
    PrepareId,
}

impl CorrelationField {
    /// Every slot, in contract order.
    pub const ALL: [CorrelationField; 10] = [
        CorrelationField::UserInteractionId,
        CorrelationField::RequestId,
        CorrelationField::ConversationId,
        CorrelationField::RunId,
        CorrelationField::NodeVisit,
        CorrelationField::EffectId,
        CorrelationField::AttemptToken,
        CorrelationField::NoticeId,
        CorrelationField::SourcePosition,
        CorrelationField::PrepareId,
    ];

    /// The wire name, identical to the contract's field name.
    pub const fn wire(self) -> &'static str {
        match self {
            Self::UserInteractionId => "userInteractionId",
            Self::RequestId => "requestId",
            Self::ConversationId => "conversationId",
            Self::RunId => "runId",
            Self::NodeVisit => "nodeVisit",
            Self::EffectId => "effectId",
            Self::AttemptToken => "attemptToken",
            Self::NoticeId => "noticeId",
            Self::SourcePosition => "sourcePosition",
            Self::PrepareId => "prepareId",
        }
    }
}

/// Correlation ids attached to one observation segment.
///
/// Every slot is optional because a segment may sit between causal hops; absent
/// is honest, a placeholder value is not.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields, default)]
pub struct CorrelationIds {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub user_interaction_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub request_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub conversation_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub run_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub node_visit: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub effect_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub attempt_token: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub notice_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_position: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prepare_id: Option<String>,
}

impl CorrelationIds {
    /// Reads one slot.
    pub fn get(&self, field: CorrelationField) -> Option<&str> {
        match field {
            CorrelationField::UserInteractionId => self.user_interaction_id.as_deref(),
            CorrelationField::RequestId => self.request_id.as_deref(),
            CorrelationField::ConversationId => self.conversation_id.as_deref(),
            CorrelationField::RunId => self.run_id.as_deref(),
            CorrelationField::NodeVisit => self.node_visit.as_deref(),
            CorrelationField::EffectId => self.effect_id.as_deref(),
            CorrelationField::AttemptToken => self.attempt_token.as_deref(),
            CorrelationField::NoticeId => self.notice_id.as_deref(),
            CorrelationField::SourcePosition => self.source_position.as_deref(),
            CorrelationField::PrepareId => self.prepare_id.as_deref(),
        }
    }

    /// Writes one slot, replacing any previous value.
    pub fn set(&mut self, field: CorrelationField, value: impl Into<String>) -> Option<String> {
        let slot = match field {
            CorrelationField::UserInteractionId => &mut self.user_interaction_id,
            CorrelationField::RequestId => &mut self.request_id,
            CorrelationField::ConversationId => &mut self.conversation_id,
            CorrelationField::RunId => &mut self.run_id,
            CorrelationField::NodeVisit => &mut self.node_visit,
            CorrelationField::EffectId => &mut self.effect_id,
            CorrelationField::AttemptToken => &mut self.attempt_token,
            CorrelationField::NoticeId => &mut self.notice_id,
            CorrelationField::SourcePosition => &mut self.source_position,
            CorrelationField::PrepareId => &mut self.prepare_id,
        };
        slot.replace(value.into())
    }

    /// Fluent counterpart of [`Self::set`].
    pub fn with(mut self, field: CorrelationField, value: impl Into<String>) -> Self {
        self.set(field, value);
        self
    }

    /// True when no slot is populated.
    pub fn is_empty(&self) -> bool {
        self.iter().next().is_none()
    }

    /// Iterates populated slots in contract order.
    pub fn iter(&self) -> impl Iterator<Item = (CorrelationField, &str)> {
        CorrelationField::ALL
            .into_iter()
            .filter_map(|field| self.get(field).map(|value| (field, value)))
    }

    /// Returns the first slot [`PrivacyBudget`] refuses.
    pub fn first_violation(
        &self,
        budget: &PrivacyBudget,
    ) -> Option<super::privacy::PrivacyViolation> {
        budget.correlation_violation(self)
    }
}

impl Display for CorrelationIds {
    /// Renders populated ids as `field=value` pairs for a trace/log line.
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        let mut first = true;
        for (field, value) in self.iter() {
            if !first {
                formatter.write_str(" ")?;
            }
            first = false;
            write!(formatter, "{}={}", field.wire(), value)?;
        }
        if first {
            formatter.write_str("-")?;
        }
        Ok(())
    }
}
