//! Canonical, host-neutral projection plan for one dispatched native send.
//!
//! The native delta stream is the sole authority for what Flutter renders.
//! This module resolves the two facts Flutter must never infer — the
//! submitted user message and the explicit interaction capability state — into
//! the exact payload vocabulary carried by the send stream. The native host
//! only composes its own capability facts and canonical turn state into an
//! [`InteractionCapability`]; everything below is vocabulary-stable so the host
//! and its protocol doubles stay aligned.

use serde_json::{Value, json};

/// Stream-level event kind for the submitted-user-message projection.
///
/// This is a live observer delta. It is never a Canonical Conversation Event
/// Part: human speech is already one Message Event, and agent turns must not
/// accumulate that projection onto the agent-authored Event.
pub const USER_MESSAGE_EVENT_KIND: &str = "conversation.user.message";
/// Payload key for the submitted user text.
pub const PAYLOAD_USER_TEXT: &str = "text";
/// Payload key for the participant role of the submitted user message.
pub const PAYLOAD_USER_ROLE: &str = "role";
/// Participant role value for the submitted user message.
pub const PAYLOAD_USER_ROLE_VALUE: &str = "user";
/// Ordered lifecycle prefix the native turn belongs to.
pub const PAYLOAD_LIFECYCLE_PREFIX: &str = "lifecyclePrefix";
/// The only lifecycle stage a submitted user message has at emission: it is
/// submitted before the native transport claims the turn.
pub const SUBMITTED_STAGE: &str = "submitted";
/// Payload key carrying the projected turn-state envelope.
pub const PAYLOAD_TURN_STATE: &str = "turnState";
/// Turn-state envelope key: the canonical turn state wire value.
pub const TURN_STATE_KEY: &str = "state";
/// Turn-state envelope key: explicit input availability.
pub const INPUT_ENABLED_KEY: &str = "inputEnabled";
/// Turn-state envelope key: explicit cancel availability.
pub const CANCEL_ENABLED_KEY: &str = "cancelEnabled";

/// Explicit interaction capability projected from Rust capability state.
///
/// Both flags are `bool`, never nullable: an absent field must never be
/// mistaken for "unknown" by the consumer, because the native host decides
/// both from what the pinned protocol actually supports.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct InteractionCapability {
    pub input_enabled: bool,
    pub cancel_enabled: bool,
}

impl InteractionCapability {
    /// Resolve the flags from capability facts. Input is enabled exactly when
    /// no turn is in flight; cancel is offered exactly when the pinned runtime
    /// exposes a native cancel channel AND a turn is in flight.
    pub const fn of(cancel_supported: bool, turn_active: bool) -> Self {
        Self {
            input_enabled: !turn_active,
            cancel_enabled: cancel_supported && turn_active,
        }
    }
}

/// Whether a canonical turn-state wire value represents an in-flight turn.
pub fn turn_state_active(state: &str) -> bool {
    matches!(
        state,
        "pending" | "claimed" | "running" | "waiting-for-human"
    )
}

/// The projected turn-state envelope for one delta. Flutter reads the flags
/// only from this explicit envelope (or the flat payload fallbacks) and never
/// infers them from lifecycle stages.
pub fn turn_state_payload(state: &str, capability: InteractionCapability) -> Value {
    json!({
        TURN_STATE_KEY: state,
        INPUT_ENABLED_KEY: capability.input_enabled,
        CANCEL_ENABLED_KEY: capability.cancel_enabled,
    })
}

/// Project one submitted user message delta payload.
///
/// Returns `None` for empty text: the native host never emits a delta whose
/// sole content would be fabricated whitespace.
pub fn user_message_event_payload(
    text: &str,
    state: &str,
    capability: InteractionCapability,
) -> Option<Value> {
    let text = text.trim();
    if text.is_empty() {
        return None;
    }
    Some(json!({
        PAYLOAD_USER_TEXT: text,
        PAYLOAD_USER_ROLE: PAYLOAD_USER_ROLE_VALUE,
        PAYLOAD_LIFECYCLE_PREFIX: [SUBMITTED_STAGE],
        PAYLOAD_TURN_STATE: turn_state_payload(state, capability),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn projection_input_enabled_only_when_no_turn_is_active() {
        let runtime_capability = InteractionCapability::of(false, true);
        assert!(!runtime_capability.input_enabled);
        assert!(!runtime_capability.cancel_enabled);

        let idle_capability = InteractionCapability::of(false, false);
        assert!(idle_capability.input_enabled);
        assert!(!idle_capability.cancel_enabled);
    }

    #[test]
    fn projection_cancel_enabled_only_with_native_channel_and_active_turn() {
        let no_channel = InteractionCapability::of(false, true);
        let with_channel = InteractionCapability::of(true, true);
        let idle_channel = InteractionCapability::of(true, false);
        assert!(!no_channel.cancel_enabled);
        assert!(with_channel.cancel_enabled);
        assert!(!idle_channel.cancel_enabled);
        assert!(with_channel.input_enabled == false);
        assert!(idle_channel.input_enabled);
    }

    #[test]
    fn projection_turn_state_active_wires_only_in_flight() {
        for state in ["pending", "claimed", "running", "waiting-for-human"] {
            assert!(turn_state_active(state), "{state}");
        }
        for state in ["succeeded", "failed", "interrupted", "cancelled"] {
            assert!(!turn_state_active(state), "{state}");
        }
    }

    #[test]
    fn projection_user_message_payload_contains_text_role_and_explicit_flags() {
        let capability = InteractionCapability::of(true, false);
        let payload = user_message_event_payload("Create in this project", "succeeded", capability);
        let payload = payload.expect("non-empty text projects");
        assert_eq!(payload[PAYLOAD_USER_TEXT], "Create in this project");
        assert_eq!(payload[PAYLOAD_USER_ROLE], "user");
        assert_eq!(payload[PAYLOAD_LIFECYCLE_PREFIX][0], "submitted");
        assert_eq!(payload[PAYLOAD_TURN_STATE][TURN_STATE_KEY], "succeeded");
        assert_eq!(payload[PAYLOAD_TURN_STATE][INPUT_ENABLED_KEY], true);
        assert_eq!(payload[PAYLOAD_TURN_STATE][CANCEL_ENABLED_KEY], false);
    }

    #[test]
    fn projection_user_message_payload_rejects_empty_text() {
        for text in ["", "   ", "\n\t"] {
            assert!(
                user_message_event_payload(text, "running", InteractionCapability::of(false, true))
                    .is_none(),
                "{text:?}"
            );
        }
    }
}
