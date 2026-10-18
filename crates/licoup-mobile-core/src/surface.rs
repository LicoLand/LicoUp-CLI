//! The mobile entry's bounded operation surface.
//!
//! The mobile client is a paired-client entry: it authenticates to its own
//! endpoint, claims a pairing, dispatches and settles secure relay commands,
//! and reaches the Canonical Conversation authority that the desktop host
//! owns. Everything else — Agent execution, workflow execution, gateway
//! hosting, migration — is a desktop obligation and is not part of this
//! surface.
//!
//! Every name here already exists. `mobile.relay.*` is the mobile relay
//! family the generated bridge and the desktop CLI both route, and
//! `conversation.*` is the Canonical Conversation family the desktop host's
//! conversation service routes. The surface declares *which* of those
//! operations the mobile entry exposes; it never renames one, so a command or
//! event keeps exactly one meaning across both clients.

use serde_json::{Value, json};

/// The exact operation closure the mobile entry exposes.
///
/// Ordered by group — pairing and settings, then delivery, then groups — and
/// kept in one place so the generated bridge, the ABI identity and the
/// contract fixture all read the same list.
pub const MOBILE_SURFACE: &[&str] = &[
    // Pairing and settings. The relay configuration is the mobile settings
    // surface; the desktop host keeps invitation *creation*, which the client
    // does not perform.
    "mobile.relay.config.get",
    "mobile.relay.config.set",
    "mobile.relay.pairing.claim",
    "mobile.relay.pairing.status",
    // Durable protocol delivery: E2EE status, secure command dispatch, and the
    // two result doors that settle a dispatched command exactly once.
    "mobile.relay.e2ee.status",
    "mobile.relay.commands.createSecure",
    "mobile.relay.commands.resultSecure",
    "mobile.relay.commands.resultReplayProof",
    // Canonical groups. These are the Canonical Conversation operations the
    // mobile chat surface needs, under their existing canonical names.
    "conversation.create",
    "conversation.list",
    "conversation.get",
    "conversation.events.page",
    "conversation.message.post",
    "conversation.membership.add",
    "conversation.membership.leave",
];

/// Why one operation is on the mobile surface. The group is what the entry
/// routes by and what the closure fixture reasons about.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SurfaceGroup {
    /// Pairing claim/status and the relay configuration that carries the
    /// mobile settings surface.
    Pairing,
    /// Endpoint identity, E2EE status, and the durable secure-command doors.
    Delivery,
    /// Canonical group list/create, membership and message operations.
    Group,
}

impl SurfaceGroup {
    /// Every group, in surface order.
    pub const ALL: &'static [Self] = &[Self::Pairing, Self::Delivery, Self::Group];

    /// The stable wire label, used by the closure fixture's diagnostics.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Pairing => "pairing",
            Self::Delivery => "delivery",
            Self::Group => "group",
        }
    }

    /// The canonical owner that answers this group's operations.
    #[must_use]
    pub const fn owner(self) -> &'static str {
        match self {
            Self::Pairing => "endpoint pairing and relay configuration",
            Self::Delivery => "endpoint identity, custody and durable delivery",
            Self::Group => "Canonical Conversation authority",
        }
    }
}

/// The group one surface operation belongs to, or `None` when the operation is
/// not on the mobile surface.
#[must_use]
pub fn surface_group(operation: &str) -> Option<SurfaceGroup> {
    match operation {
        "mobile.relay.config.get"
        | "mobile.relay.config.set"
        | "mobile.relay.pairing.claim"
        | "mobile.relay.pairing.status" => Some(SurfaceGroup::Pairing),
        "mobile.relay.e2ee.status"
        | "mobile.relay.commands.createSecure"
        | "mobile.relay.commands.resultSecure"
        | "mobile.relay.commands.resultReplayProof" => Some(SurfaceGroup::Delivery),
        "conversation.create"
        | "conversation.list"
        | "conversation.get"
        | "conversation.events.page"
        | "conversation.message.post"
        | "conversation.membership.add"
        | "conversation.membership.leave" => Some(SurfaceGroup::Group),
        _ => None,
    }
}

/// Whether one operation is on the mobile surface.
#[must_use]
pub fn is_mobile_operation(operation: &str) -> bool {
    surface_group(operation).is_some()
}

/// Canonical operations that exist for the desktop executor and are refused by
/// the mobile entry by name.
///
/// This is not a second catalogue: each name is the canonical name its desktop
/// owner already routes, and the mobile entry refuses it for the same reason —
/// the operation needs an execution authority a paired mobile client does not
/// hold. The contract fixture asserts the refusal, so a later widening of the
/// surface has to be deliberate.
pub const DESKTOP_ONLY_OPERATIONS: &[&str] = &[
    // Agent execution runs on the host that owns the Agent process.
    "agent.conversation.send",
    // Dispatch settlement, Subagent claims and profile/strategy authoring are
    // host-side Canonical Conversation obligations.
    "conversation.dispatch.after-post",
    "conversation.subagent.claim",
    "conversation.strategy.set",
    "conversation.export",
];

/// The bounded code the mobile entry answers an off-surface operation with.
pub const UNSUPPORTED_OPERATION_CODE: &str = "mobile_surface_action_unsupported";

/// The bounded code the mobile entry answers a malformed request with.
pub const INVALID_REQUEST_CODE: &str = "mobile_surface_invalid_request";

/// The refusal an off-surface operation receives.
///
/// It has the same three fields the native bridge's own unsupported response
/// carries, so the Dart caller keeps one shape to read.
#[must_use]
pub fn unsupported_operation_response(operation: &str) -> Value {
    json!({
        "ok": false,
        "code": UNSUPPORTED_OPERATION_CODE,
        "action": operation,
    })
}

/// The refusal a request that is not an `{action, params}` object receives.
#[must_use]
pub fn invalid_request_response() -> Value {
    json!({
        "ok": false,
        "code": INVALID_REQUEST_CODE,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_surface_operation_has_exactly_one_group() {
        for operation in MOBILE_SURFACE {
            assert!(
                surface_group(operation).is_some(),
                "{operation} is declared on the surface but has no group"
            );
        }
        for group in SurfaceGroup::ALL {
            assert!(
                MOBILE_SURFACE
                    .iter()
                    .any(|operation| surface_group(operation) == Some(*group)),
                "group {} is empty",
                group.label()
            );
        }
    }

    #[test]
    fn surface_operations_are_unique_and_do_not_restate_desktop_only_operations() {
        let mut seen = std::collections::BTreeSet::new();
        for operation in MOBILE_SURFACE {
            assert!(seen.insert(*operation), "{operation} is declared twice");
            assert!(
                !DESKTOP_ONLY_OPERATIONS.contains(operation),
                "{operation} is both on the surface and refused"
            );
        }
    }
}
