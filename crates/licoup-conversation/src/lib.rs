//! Durable, host-lifecycle-neutral Canonical Conversation authority.
//!
//! SQLite is the sole authority. Process memory is limited to bounded
//! connection/cache state and every host start uses the same cold-recovery
//! operation before it serves conversation work.

pub mod client_conversation;
pub mod continuity;
pub mod projection;
pub mod state_machine;
pub mod store;

pub use client_conversation::*;
pub use state_machine::{
    ALL_SEND_EVENTS, ALL_SEND_STATES, ALL_TURN_EVENTS, ALL_TURN_STATES, SEND_TRANSITIONS,
    SendEvent, SendState, SendTransition, TURN_TRANSITIONS, TransitionError, TurnEvent,
    TurnTransition,
};
pub use store::{
    ColdRecoverableConversationStore, ColdRecoveryReport, ConversationRepository,
    ConversationRuntimeScope, ConversationStore, DEFAULT_CONVERSATION_POOL_SIZE,
    DEFAULT_EVENT_PAGE_SIZE, DirectTurnExecutionContext, DispatchRepository, EventAppendOutcome,
    EventPagePosition, EventRepository, ImageAttachmentReference, MAX_EVENT_PAGE_SIZE,
    MAX_SUBAGENT_INVOCATION_DEPTH, NewEventPart, StoreError, StoreResult,
};
