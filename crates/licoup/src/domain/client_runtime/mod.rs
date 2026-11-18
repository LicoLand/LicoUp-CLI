//! Typed in-process client runtime: generation-safe handles, wake-only
//! callbacks, monotonic stream cursors, and closed Agent private IPC.
//!
//! Hosts later bind this surface through `licoup-platform-bridges`. Until that
//! crate exists, these types stay in the native domain and remain the ABI
//! contract. GUI callers never supply origin, risk, confirmation, or
//! authentication fields.

mod abi;
mod agent_ipc;
mod arena;
mod runtime;
mod spool;
mod stream;

pub use abi::{
    RuntimeCommand, RuntimeError, RuntimeEvent, RuntimeEventClass, SharedBufferId,
    StreamReplayClass,
};
pub use agent_ipc::{AgentIpcError, AgentIpcMessage, AgentPrivateIpc};
pub use arena::{Handle, HandleArena, HandleKind};
pub use runtime::{ClientRuntime, FutureState, SubscriptionState, WakeCallback};
pub use spool::{OutputSpool, SpoolError};
pub use stream::{LatestStateMerge, StreamCursor, StreamItem, StreamQueue};
