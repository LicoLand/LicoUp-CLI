//! Fail-closed entry for installed optional-collaboration MCP registrations.
//!
//! Installation remains useful as an exact, authority-bound local operation,
//! but an installed registration is not an outbound capability. Until LicoUp
//! owns a production broker that presents the exact request to the user and
//! returns an authenticated, one-shot capability, stale or hand-written bridge
//! invocations must not expose tools or perform network I/O.

use anyhow::{Result, anyhow};

use crate::platform::client_state::ClientStateStore;

pub(crate) fn serve_mcp_bridge(
    store: &ClientStateStore,
    agent_id: &str,
    registration_id: &str,
) -> Result<()> {
    // Verify the package, payload, registration file, destination, and
    // protected AuthorityRegistration first. This keeps a stale invocation
    // from bypassing the install binding while still ending in a deterministic
    // fail-closed state before stdin is read or a transport is created.
    super::registration::load_bridge_registration(store, agent_id, registration_id)?;
    Err(anyhow!(
        "collaboration_mcp_authenticated_authorization_broker_unavailable"
    ))
}
