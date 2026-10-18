//! Secure Mesh remote-approval request/response envelopes and pending-operation CAS.
//!
//! Approval detail stays encrypted on the wire. Local projections expose only
//! display-safe summaries. Relay stores must never receive plaintext operation
//! detail, prompts, file paths, or tool arguments.

mod capability;
mod input;
mod ledger;
mod model;
mod projection;
mod request;
mod response;

pub const SECURE_MESH_APPROVAL_REQUEST_PROTOCOL: &str = "secure_mesh.approval_request.v1";
pub const SECURE_MESH_APPROVAL_RESPONSE_PROTOCOL: &str = "secure_mesh.approval_response.v1";
pub const SECURE_MESH_APPROVAL_CONTENT_TYPE: &str =
    "application/licomesh.secure-mesh.approval.v1+json";
pub const SECURE_MESH_APPROVAL_STATUS: &str =
    "approval_request_response_cas_fanout_available_plaintext_relay_blocked";

const MAX_TEXT_BYTES: usize = 512;
const MAX_SUMMARY_BYTES: usize = 1_024;
const MAX_TOOL_NAME_BYTES: usize = 128;
const MAX_TOOL_NAMES: usize = 32;
const MAX_PENDING: usize = 64;

pub use capability::evaluate_approval_adapter_capability_json;
pub use request::{evaluate_approval_fanout_json, evaluate_approval_request_json};
pub use response::{list_approval_inbox_json, resolve_approval_response_json};

#[cfg(test)]
mod tests;
