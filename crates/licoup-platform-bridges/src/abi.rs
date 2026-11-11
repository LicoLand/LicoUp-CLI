use serde::{Deserialize, Serialize};

/// In-repo ABI identity. Canonical file: `resources/client_runtime_abi.json`.
pub const CLIENT_RUNTIME_ABI_JSON: &str = include_str!("../resources/client_runtime_abi.json");

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
