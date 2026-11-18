//! Provider-neutral production Subagent Mesh adapter registry.
//!
//! Concrete provider code remains in the composing host. This crate owns the
//! sole lookup surface for caller and target ports, eliminating provider
//! conditionals and duplicate inventories from the MCP application.

use licoup_agent_runtime::{McpCallerIntegration, ProviderId, SubagentRuntimeAdapter};
use std::collections::BTreeMap;
use std::sync::Arc;

/// Opaque adapter identity. Values are static catalog names, never paths.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct AdapterId {
    name: &'static str,
}

impl AdapterId {
    pub const fn new(name: &'static str) -> Self {
        Self { name }
    }

    pub const fn as_str(self) -> &'static str {
        self.name
    }
}

/// One immutable-by-clone registry. Registration rejects duplicates and
/// requires each provider to expose both directions before it can be used by
/// the common application.
#[derive(Clone, Default)]
pub struct AdapterRegistry {
    callers: BTreeMap<ProviderId, Arc<dyn McpCallerIntegration>>,
    runtimes: BTreeMap<ProviderId, Arc<dyn SubagentRuntimeAdapter>>,
}

impl AdapterRegistry {
    pub fn empty() -> Self {
        Self {
            callers: BTreeMap::new(),
            runtimes: BTreeMap::new(),
        }
    }

    /// Compatibility lookup for the semantic authority registry. Production
    /// dispatch uses `runtime` and `caller` below.
    pub fn get(&self, id: AdapterId) -> Option<AdapterId> {
        self.runtimes
            .keys()
            .any(|entry| entry.as_str() == id.as_str())
            .then_some(id)
    }

    pub fn len(&self) -> usize {
        self.runtimes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.runtimes.is_empty()
    }

    pub fn register_pair(
        &mut self,
        caller: Arc<dyn McpCallerIntegration>,
        runtime: Arc<dyn SubagentRuntimeAdapter>,
    ) -> Result<(), &'static str> {
        let provider = caller.provider_id().clone();
        if runtime.provider_id() != &provider {
            return Err("adapter_provider_identity_mismatch");
        }
        if self.callers.contains_key(&provider) || self.runtimes.contains_key(&provider) {
            return Err("adapter_provider_duplicate");
        }
        self.callers.insert(provider.clone(), caller);
        self.runtimes.insert(provider, runtime);
        Ok(())
    }

    pub fn caller(&self, provider: &ProviderId) -> Option<Arc<dyn McpCallerIntegration>> {
        self.callers.get(provider).cloned()
    }

    pub fn runtime(&self, provider: &ProviderId) -> Option<Arc<dyn SubagentRuntimeAdapter>> {
        self.runtimes.get(provider).cloned()
    }

    pub fn providers(&self) -> impl ExactSizeIterator<Item = &ProviderId> {
        self.runtimes.keys()
    }

    /// Providers admitted as mesh callers. `register_pair` admits both
    /// directions together, so this is also the mesh member set: the authority
    /// for which Agents receive a discovery token and may occupy a Membership
    /// seat. Ordered by provider id for deterministic publication.
    pub fn caller_providers(&self) -> impl ExactSizeIterator<Item = &ProviderId> {
        self.callers.keys()
    }
}

#[cfg(test)]
mod tests {
    use super::{AdapterId, AdapterRegistry};

    #[test]
    fn empty_registry_has_no_adapters() {
        let registry = AdapterRegistry::empty();
        assert!(registry.is_empty());
        assert_eq!(registry.len(), 0);
        assert_eq!(registry.get(AdapterId::new("codex")), None);
        assert_eq!(registry.caller_providers().len(), 0);
    }
}
