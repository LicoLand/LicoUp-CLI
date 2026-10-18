mod mls;
mod pairwise;
mod policy;
mod projection;
mod schema;

use anyhow::{Result, anyhow};
use serde_json::Value;

pub use mls::{open_lifecycle_service_action_mls, seal_lifecycle_service_action_mls};
pub use pairwise::{
    open_lifecycle_service_action_pairwise, seal_lifecycle_service_action_pairwise,
};
pub use schema::{SECURE_MESH_LIFECYCLE_CONTENT_TYPE, SECURE_MESH_LIFECYCLE_STATUS};

/// Evaluate an untrusted lifecycle request into the only redacted policy projection that may be
/// placed inside a protected pairwise or MLS envelope.
pub fn evaluate_service_action_json(params: &Value) -> Result<Value> {
    projection::project_policy_decision(policy::evaluate(params)?)
}

pub fn reject_plaintext_lifecycle_service_action_transport(params: &Value) -> Result<()> {
    let _ = evaluate_service_action_json(params)?;
    Err(anyhow!(
        "secure mesh lifecycle service action plaintext transport is forbidden; pairwise or MLS envelope required"
    ))
}

#[cfg(test)]
mod tests;
