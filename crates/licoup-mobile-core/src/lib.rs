//! The focused mobile core: the paired client's small endpoint-application
//! entry, separated from the desktop client's complete executor.
//!
//! A paired mobile client authenticates to its own endpoint, claims a pairing,
//! dispatches and settles protected commands, and reaches the Canonical
//! Conversation authority the desktop host owns. It does not run Agents, does
//! not host the gateway, does not execute workflows, and does not convert a
//! data root. Those are desktop obligations, and this crate is built so that
//! they cannot become mobile dependencies by accident.
//!
//! # What this crate owns
//!
//! * [`surface`] — the exact operation closure the mobile entry admits, under
//!   the canonical names the desktop owners already route.
//! * [`host`] — the application port that answers those operations. The port
//!   carries the canonical parameter and result objects; it renames nothing.
//! * [`entry`] — admission and routing for one request, plus the bounded read
//!   projections the mobile list and thread render.
//! * [`identity`] — the endpoint identity/custody admission rule.
//! * [`delivery`] — durable protocol delivery state: which envelopes are
//!   already delivered, and where a reconnect resumes.
//! * [`group_bridge`] — the canonical group application path: the group
//!   operations a mobile chat control dispatches, and the delivery state that
//!   makes a resend and a reconnect safe.
//! * [`read_model`] — the bounded mobile read model over canonical
//!   Conversation records.
//! * [`settings`] — the resource policy and client state root the entry reads.
//!
//! # What it deliberately does not own
//!
//! The protocol, the Canonical Conversation authority, custody of private
//! material, the relay, and the pairing records all stay with their existing
//! owners. This crate declares which of their operations a mobile client
//! exposes and how a request is admitted; it answers none of them itself.
//! `licoup-native` keeps its own executor for the desktop client, so the two
//! clients share one meaning per command without sharing one binary.
//!
//! # Dependency closure
//!
//! The crate depends on the endpoint core's port types, the protocol bindings'
//! accepted version, the Canonical Conversation authority, the bounded client
//! state, the portable foundation, and the ABI identity — and on nothing else
//! from this workspace. `tests/dependency_closure.rs` walks the workspace
//! manifests and fails if that closure grows to include a desktop-only crate.

pub mod delivery;
pub mod entry;
pub mod group_bridge;
pub mod host;
pub mod identity;
pub mod read_model;
pub mod settings;
pub mod surface;

pub use delivery::{DeliveryLedger, DeliveryRecord, DeliveryRefusal, DeliveryState};
pub use entry::{MAX_MOBILE_ACTION_BYTES, MAX_MOBILE_REQUEST_BYTES, MobileEntry};
pub use group_bridge::{
    GROUP_CONVERSATION_PARAM, MOBILE_GROUP_OPERATIONS, MobileGroupApplication, PendingDelivery,
    READ_ONLY_GROUP_OPERATIONS, ReconnectPlan, is_group_operation, is_read_only_group_operation,
};
pub use host::MobileOperationHost;
pub use identity::{IdentityRefusal, MOBILE_REQUIRED_CUSTODY, admit_custody};
pub use read_model::{ChatCard, ChatList, EventView, MemberView, ThreadView};
pub use settings::MobileSettings;
pub use surface::{
    DESKTOP_ONLY_OPERATIONS, MOBILE_SURFACE, SurfaceGroup, UNSUPPORTED_OPERATION_CODE,
    is_mobile_operation, surface_group, unsupported_operation_response,
};
