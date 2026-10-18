//! Credential vault port.
//!
//! Custody — reading the system keyring and asking for owner authorization —
//! belongs to the host. The runtime only rebuilds a live lease from the
//! handoff the host already authorized, through this port.

use crate::api_key::{GatewayCredentialHandoff, GatewayCredentialLease};
use anyhow::{Result, anyhow};
use std::sync::{Arc, RwLock};

pub trait GatewayVaultPort: Send + Sync {
    /// Rebuild a gateway lease from a handoff produced by the custody owner.
    fn lease_from_handoff(
        &self,
        handoff: GatewayCredentialHandoff,
    ) -> Result<GatewayCredentialLease>;
}

static VAULT: RwLock<Option<Arc<dyn GatewayVaultPort>>> = RwLock::new(None);

/// Install the custody owner's lease answer. Reinstalling the same owner is
/// accepted so repeated startup is harmless.
pub fn install(port: Arc<dyn GatewayVaultPort>) -> Result<(), &'static str> {
    let mut guard = VAULT
        .write()
        .map_err(|_| "gateway_vault_port_lock_failed")?;
    match guard.as_ref() {
        Some(installed) if Arc::ptr_eq(installed, &port) => return Ok(()),
        Some(_) => return Err("gateway_vault_port_already_installed"),
        None => {}
    }
    *guard = Some(port);
    Ok(())
}

pub fn installed() -> bool {
    VAULT.read().map(|guard| guard.is_some()).unwrap_or(false)
}

#[cfg(test)]
pub fn clear() {
    if let Ok(mut guard) = VAULT.write() {
        *guard = None;
    }
}

/// Consume the installed port; fails closed when nothing is installed.
pub fn require() -> Result<Arc<dyn GatewayVaultPort>> {
    VAULT
        .read()
        .ok()
        .and_then(|guard| guard.clone())
        .ok_or_else(|| anyhow!("gateway_vault_port_unavailable"))
}
