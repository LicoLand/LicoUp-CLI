use serde::{Deserialize, Serialize};

/// In-repo ABI identity. Canonical file: `schemas/client_bridge/client_runtime_abi.json`.
pub const CLIENT_RUNTIME_ABI_JSON: &str =
    include_str!("../../../schemas/client_bridge/client_runtime_abi.json");

pub const CLIENT_RUNTIME_ABI_VERSION: u32 = 1;

pub const CLIENT_RUNTIME_OPERATIONS: &[&str] = &[
    "runtime.create",
    "runtime.destroy",
    "future.poll",
    "future.complete",
    "future.cancel",
    "future.free",
    "subscription.drain",
    "subscription.cancel",
    "subscription.free",
    "shared_buffer.free",
];

/// Same-process ABI version, layout identity, and operation surface.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AbiIdentity {
    pub abi_version: u32,
    pub layout_identity: String,
    pub operations: Vec<String>,
}

impl AbiIdentity {
    pub fn load() -> Self {
        serde_json::from_str(CLIENT_RUNTIME_ABI_JSON).expect("client_runtime_abi_identity_invalid")
    }

    pub fn layout_identity(&self) -> &str {
        &self.layout_identity
    }
}

/// The mobile entry's ABI version.
///
/// The desktop runtime identity above is a handle-and-future surface: a caller
/// creates a runtime, polls futures and drains subscriptions. The mobile entry
/// is a different shape — one JSON request per call, addressed by operation
/// name — so it carries its own version even though both are same-process ABI
/// identities of one product.
pub const MOBILE_RUNTIME_ABI_VERSION: u32 = 1;

/// The mobile entry's layout identity.
pub const MOBILE_RUNTIME_LAYOUT_IDENTITY: &str = "licoup.mobile-entry.abi.v1.endpoint-application";

/// The operations one mobile entry admits.
///
/// Canonical names only: `mobile.relay.*` is the mobile relay family the
/// generated bridge and the desktop CLI both route, and `conversation.*` is
/// the Canonical Conversation family the desktop host routes. Pairing,
/// settings, durable delivery and groups are the mobile client's obligations;
/// Agent execution, workflow execution, gateway hosting and migration are the
/// desktop executor's, so they are absent here by construction.
pub const MOBILE_RUNTIME_OPERATIONS: &[&str] = &[
    "mobile.relay.config.get",
    "mobile.relay.config.set",
    "mobile.relay.pairing.claim",
    "mobile.relay.pairing.status",
    "mobile.relay.e2ee.status",
    "mobile.relay.commands.createSecure",
    "mobile.relay.commands.resultSecure",
    "mobile.relay.commands.resultReplayProof",
    "conversation.create",
    "conversation.list",
    "conversation.get",
    "conversation.events.page",
    "conversation.message.post",
    "conversation.membership.add",
    "conversation.membership.leave",
];

/// The mobile entry's identity, in the same value the desktop identity uses.
///
/// There is one identity type because there is one ABI question — which
/// operations does this boundary admit — and two answers, one per entry.
#[must_use]
pub fn mobile_abi_identity() -> AbiIdentity {
    AbiIdentity {
        abi_version: MOBILE_RUNTIME_ABI_VERSION,
        layout_identity: MOBILE_RUNTIME_LAYOUT_IDENTITY.to_owned(),
        operations: MOBILE_RUNTIME_OPERATIONS
            .iter()
            .map(|operation| (*operation).to_owned())
            .collect(),
    }
}
