//! Typed remote work control for the optional endpoint collaboration package.
//!
//! This package is the control slice of the endpoint collaboration package
//! (`org.licoland.feature.collaboration`). It owns the *decision* half of a
//! remote stop: which verified requester may ask, which already-admitted work a
//! request selects, which request identity has already been answered, and what
//! this host is allowed to claim afterwards. It owns no effect: the work itself
//! belongs to the kernel's own owners — the persistent conversation turn, the
//! durable workflow run, the Subagent MCP dispatch claim and the supervised lane
//! session — and this slice reaches them only through the consumer-owned port
//! [`LocalWorkOwner`] that the kernel implements.
//!
//! Three rules are the whole reason this slice exists, and each is falsifiable
//! from its public surface:
//!
//! * **Admission precedes effect.** A control request whose durable identity has
//!   already been answered is *never* asked of an owner a second time. The
//!   second delivery of one request is answered from the recorded admission, so
//!   a replayed or duplicated control cannot repeat a stop, and a request that
//!   reuses an identity with different content is refused instead of being
//!   treated as new work.
//! * **A request is never proof of exit.** [`OwnerDisposition`] records what an
//!   owner *said*; only an observed end is a confirmed one. The unconfirmed
//!   states stay visible as unconfirmed.
//! * **Authority is local and current.** Verified ingress is necessary and not
//!   sufficient: the requester must also hold a current local grant that covers
//!   the intent, and force control additionally requires a target scope this
//!   host verifies as its own plus the locally produced redacted diagnostics.
//!
//! Nothing here authenticates a peer, reads a protected key, terminates a
//! process or writes a durable store. The kernel performs the effect through
//! [`LocalWorkOwner`] once this slice has admitted the request, and the caller's
//! own persistence keeps [`RemoteControlLedger::durable_record`].

#![forbid(unsafe_code)]

pub mod control;

/// The namespaced identity of this slice's owning package.
pub const PACKAGE_ID: &str = "org.licoland.feature.collaboration";

/// The capability this slice contributes. It is the identity a remote control is
/// checked against, not a label the request itself may grant.
pub const CAPABILITY_ID: &str = "endpoint.work-control.v1";

/// The schema of the ledger's durable record.
pub const REMOTE_CONTROL_SCHEMA: &str = "licoup.endpoint-remote-control.v1";
