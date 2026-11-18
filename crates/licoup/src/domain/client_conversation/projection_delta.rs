//! Typed projection deltas for the conversation send stream.
//!
//! Mirrors the settlement delta discipline: every projection the native host
//! emits is one typed value with a stable wire payload, and nothing is emitted
//! that the consumer would have to infer. The submitted user message and the
//! explicit interaction capability flags are projected from canonical
//! `licoup-conversation` rules ([`licoup_conversation::projection`]); the host
//! only supplies its own capability facts.

use licoup_conversation::projection::{
    InteractionCapability, USER_MESSAGE_EVENT_KIND, user_message_event_payload,
};

/// One typed projection delta emitted by the native send stream.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum ProjectionDelta {
    /// The user's submitted message, projected for immediate readback.
    UserMessageCreated {
        text: String,
        turn_state: String,
        capability: InteractionCapability,
    },
}

impl ProjectionDelta {
    /// Stream-level event kind this delta travels under.
    pub(crate) const fn event_kind(&self) -> &'static str {
        match self {
            Self::UserMessageCreated { .. } => USER_MESSAGE_EVENT_KIND,
        }
    }

    /// Wire payload for the emitted stream event.
    pub(crate) fn to_event_payload(&self) -> serde_json::Value {
        match self {
            Self::UserMessageCreated {
                text,
                turn_state,
                capability,
            } => user_message_event_payload(text, turn_state, *capability)
                .unwrap_or_else(|| serde_json::json!({})),
        }
    }
}

/// Project one submitted user message from host facts.
///
/// Returns `None` when the text is empty or the turn state is unknown to the
/// canonical vocabulary — the host never emits a delta it cannot ground.
pub(crate) fn project_submitted_user_message(
    text: &str,
    turn_state: &str,
    cancel_supported: bool,
) -> Option<ProjectionDelta> {
    const CANONICAL_TURN_STATES: [&str; 8] = [
        "pending",
        "claimed",
        "running",
        "waiting-for-human",
        "succeeded",
        "failed",
        "interrupted",
        "cancelled",
    ];
    if text.trim().is_empty() || !CANONICAL_TURN_STATES.contains(&turn_state) {
        return None;
    }
    let turn_active = licoup_conversation::projection::turn_state_active(turn_state);
    Some(ProjectionDelta::UserMessageCreated {
        text: text.trim().to_owned(),
        turn_state: turn_state.to_owned(),
        capability: InteractionCapability::of(cancel_supported, turn_active),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn user_message_delta_projects_text_role_and_explicit_flags() {
        let delta = project_submitted_user_message("Running turn", "succeeded", true)
            .expect("grounded projection");
        assert_eq!(delta.event_kind(), "conversation.user.message");
        let payload = delta.to_event_payload();
        assert_eq!(payload["text"], "Running turn");
        assert_eq!(payload["role"], "user");
        assert_eq!(payload["lifecyclePrefix"][0], "submitted");
        assert_eq!(payload["turnState"]["state"], "succeeded");
        assert_eq!(payload["turnState"]["inputEnabled"], true);
        assert_eq!(payload["turnState"]["cancelEnabled"], false);
    }

    #[test]
    fn user_message_delta_projects_cancel_capability_while_turn_is_active() {
        let delta = project_submitted_user_message("Compute", "running", true)
            .expect("grounded projection");
        let payload = delta.to_event_payload();
        assert_eq!(payload["turnState"]["inputEnabled"], false);
        assert_eq!(payload["turnState"]["cancelEnabled"], true);
    }

    #[test]
    fn user_message_delta_rejects_unbounded_inputs() {
        assert!(project_submitted_user_message("   ", "succeeded", true).is_none());
        assert!(project_submitted_user_message("Compute", "unknown-state", true).is_none());
        assert!(project_submitted_user_message("Compute", "", true).is_none());
    }

    #[test]
    fn user_message_delta_payload_grounds_with_canonical_rules() {
        let delta = project_submitted_user_message("Create in this project", "succeeded", false)
            .expect("grounded projection");
        assert_eq!(
            delta.to_event_payload(),
            json!({
                "text": "Create in this project",
                "role": "user",
                "lifecyclePrefix": ["submitted"],
                "turnState": {
                    "state": "succeeded",
                    "inputEnabled": true,
                    "cancelEnabled": false,
                }
            })
        );
    }
}
