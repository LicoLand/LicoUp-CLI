//! Closed in-process ABI. Only create/destroy, future poll/complete/cancel/free,
//! subscription drain/cancel/free, and shared-buffer free are exposed.

use super::arena::{Handle, HandleKind};
use super::stream::StreamCursor;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RuntimeCommand {
    Create,
    Destroy,
    FuturePoll,
    FutureComplete,
    FutureCancel,
    FutureFree,
    SubscriptionDrain,
    SubscriptionCancel,
    SubscriptionFree,
    SharedBufferFree,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StreamReplayClass {
    /// Intermediate revisions may be coalesced. Cursor still advances.
    LatestState,
    /// Full ordered delivery. No silent truncation.
    Ordered,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RuntimeEventClass {
    Wake,
    FutureReady,
    SubscriptionReady,
    CursorInvalidated,
    Capacity,
    Teardown,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RuntimeEvent {
    Wake {
        handle: Handle,
    },
    FutureReady {
        handle: Handle,
    },
    SubscriptionReady {
        handle: Handle,
        cursor: StreamCursor,
    },
    CursorInvalidated {
        handle: Handle,
        earliest: StreamCursor,
    },
    Capacity {
        kind: HandleKind,
    },
    Teardown {
        handle: Handle,
    },
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct SharedBufferId(pub u64);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RuntimeError {
    StaleHandle {
        kind: HandleKind,
    },
    HandleKindMismatch {
        expected: HandleKind,
        actual: HandleKind,
    },
    CapacityExceeded {
        kind: HandleKind,
        capacity: u32,
    },
    InvalidState,
    Cancelled,
    AlreadyCompleted,
    CursorInvalid {
        earliest: u64,
    },
    InternalInvariant,
    AgentIpcClosed,
}

impl RuntimeError {
    pub const fn code(self) -> &'static str {
        match self {
            Self::StaleHandle { .. } => "stale_handle",
            Self::HandleKindMismatch { .. } => "handle_kind_mismatch",
            Self::CapacityExceeded { .. } => "capacity_exceeded",
            Self::InvalidState => "invalid_state",
            Self::Cancelled => "cancelled",
            Self::AlreadyCompleted => "already_completed",
            Self::CursorInvalid { .. } => "cursor_invalid",
            Self::InternalInvariant => "internal_invariant",
            Self::AgentIpcClosed => "agent_ipc_closed",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use licoup_platform_bridges::{AbiIdentity, CLIENT_RUNTIME_OPERATIONS};

    #[test]
    fn abi_surface_matches_the_versioned_identity() {
        let identity = AbiIdentity::load();
        assert_eq!(identity.abi_version, 1);
        assert_eq!(identity.operations.len(), CLIENT_RUNTIME_OPERATIONS.len());
        for operation in CLIENT_RUNTIME_OPERATIONS {
            assert!(identity.operations.iter().any(|item| item == operation));
        }
        assert_eq!(RuntimeError::Cancelled.code(), "cancelled");
        assert_eq!(RuntimeError::AgentIpcClosed.code(), "agent_ipc_closed");
    }
}
