//! Semantic registry for client authority destinations and owning crates.

use licoup_agent_adapters::AdapterRegistry;
use licoup_client_state::ClientResourcePolicy;
use licoup_endpoint_core::ClientSession;
use licoup_platform_bridges::{AbiIdentity, HandleArena};

/// Crate that owns a client authority destination.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClientAuthorityOwner {
    ProtocolBindings,
    ClientState,
    EndpointCore,
    PlatformBridges,
    AgentAdapters,
    NativeComposition,
}

impl ClientAuthorityOwner {
    pub fn crate_name(self) -> &'static str {
        match self {
            Self::ProtocolBindings => "licoup-protocol-bindings",
            Self::ClientState => "licoup-client-state",
            Self::EndpointCore => "licoup-endpoint-core",
            Self::PlatformBridges => "licoup-platform-bridges",
            Self::AgentAdapters => "licoup-agent-adapters",
            Self::NativeComposition => "licoup-native",
        }
    }
}

/// Client authority destination. Each destination has exactly one owner.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClientAuthorityDestination {
    ProtocolInputAdmission,
    RuntimeAbi,
    RuntimeSession,
    InteractionPolicy,
    ResourceCatalog,
    UsageRollup,
    AgentHubInstall,
    ProtectedCommunication,
    ProviderHistorySearch,
    Quarantine,
    ReleaseEvidence,
    AgentRuntimeAdapters,
    OsPrimitivePorts,
}

impl ClientAuthorityDestination {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ProtocolInputAdmission => "protocol_input_admission",
            Self::RuntimeAbi => "runtime_abi",
            Self::RuntimeSession => "runtime_session",
            Self::InteractionPolicy => "interaction_policy",
            Self::ResourceCatalog => "resource_catalog",
            Self::UsageRollup => "usage_rollup",
            Self::AgentHubInstall => "agent_hub_install",
            Self::ProtectedCommunication => "protected_communication",
            Self::ProviderHistorySearch => "provider_history_search",
            Self::Quarantine => "quarantine",
            Self::ReleaseEvidence => "release_evidence",
            Self::AgentRuntimeAdapters => "agent_runtime_adapters",
            Self::OsPrimitivePorts => "os_primitive_ports",
        }
    }
}

/// One destination-to-owner record.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ClientAuthorityRecord {
    pub destination: ClientAuthorityDestination,
    pub owner: ClientAuthorityOwner,
}

/// Complete client authority ownership table.
pub const CLIENT_AUTHORITY_REGISTRY: &[ClientAuthorityRecord] = &[
    ClientAuthorityRecord {
        destination: ClientAuthorityDestination::ProtocolInputAdmission,
        owner: ClientAuthorityOwner::ProtocolBindings,
    },
    ClientAuthorityRecord {
        destination: ClientAuthorityDestination::RuntimeAbi,
        owner: ClientAuthorityOwner::PlatformBridges,
    },
    ClientAuthorityRecord {
        destination: ClientAuthorityDestination::OsPrimitivePorts,
        owner: ClientAuthorityOwner::PlatformBridges,
    },
    ClientAuthorityRecord {
        destination: ClientAuthorityDestination::RuntimeSession,
        owner: ClientAuthorityOwner::EndpointCore,
    },
    ClientAuthorityRecord {
        destination: ClientAuthorityDestination::InteractionPolicy,
        owner: ClientAuthorityOwner::EndpointCore,
    },
    ClientAuthorityRecord {
        destination: ClientAuthorityDestination::UsageRollup,
        owner: ClientAuthorityOwner::EndpointCore,
    },
    ClientAuthorityRecord {
        destination: ClientAuthorityDestination::ProtectedCommunication,
        owner: ClientAuthorityOwner::EndpointCore,
    },
    ClientAuthorityRecord {
        destination: ClientAuthorityDestination::Quarantine,
        owner: ClientAuthorityOwner::EndpointCore,
    },
    ClientAuthorityRecord {
        destination: ClientAuthorityDestination::ResourceCatalog,
        owner: ClientAuthorityOwner::ClientState,
    },
    ClientAuthorityRecord {
        destination: ClientAuthorityDestination::ProviderHistorySearch,
        owner: ClientAuthorityOwner::ClientState,
    },
    ClientAuthorityRecord {
        destination: ClientAuthorityDestination::AgentRuntimeAdapters,
        owner: ClientAuthorityOwner::AgentAdapters,
    },
    ClientAuthorityRecord {
        destination: ClientAuthorityDestination::AgentHubInstall,
        owner: ClientAuthorityOwner::NativeComposition,
    },
    ClientAuthorityRecord {
        destination: ClientAuthorityDestination::ReleaseEvidence,
        owner: ClientAuthorityOwner::NativeComposition,
    },
];

pub fn owner_for(destination: ClientAuthorityDestination) -> ClientAuthorityOwner {
    CLIENT_AUTHORITY_REGISTRY
        .iter()
        .find(|record| record.destination == destination)
        .map(|record| record.owner)
        .expect("client_authority_destination_unregistered")
}

/// Shared type surface for later nodes. These constructors do not admit a Protocol Line.
pub fn standard_resource_policy() -> ClientResourcePolicy {
    ClientResourcePolicy::standard()
}

pub fn ordinary_session() -> ClientSession {
    ClientSession::device_unlocked()
}

pub fn empty_adapter_registry() -> AdapterRegistry {
    AdapterRegistry::empty()
}

pub fn runtime_abi_identity() -> AbiIdentity {
    AbiIdentity::load()
}

pub fn bounded_handle_arena<T>(capacity: usize) -> HandleArena<T> {
    HandleArena::bounded(capacity)
}

#[cfg(test)]
mod tests {
    use super::{
        CLIENT_AUTHORITY_REGISTRY, ClientAuthorityDestination, ClientAuthorityOwner,
        bounded_handle_arena, empty_adapter_registry, ordinary_session, owner_for,
        runtime_abi_identity, standard_resource_policy,
    };
    use licoup_endpoint_core::ClientSessionState;
    use licoup_platform_bridges::CLIENT_RUNTIME_ABI_VERSION;

    #[test]
    fn every_destination_has_exactly_one_owner() {
        let mut seen = Vec::new();
        for record in CLIENT_AUTHORITY_REGISTRY {
            assert!(!seen.contains(&record.destination), "duplicate destination");
            seen.push(record.destination);
            assert_eq!(owner_for(record.destination), record.owner);
            assert!(!record.owner.crate_name().is_empty());
            assert!(!record.destination.as_str().is_empty());
        }
        assert_eq!(seen.len(), CLIENT_AUTHORITY_REGISTRY.len());
        assert_eq!(
            owner_for(ClientAuthorityDestination::ProtocolInputAdmission),
            ClientAuthorityOwner::ProtocolBindings
        );
        assert_eq!(
            owner_for(ClientAuthorityDestination::RuntimeAbi),
            ClientAuthorityOwner::PlatformBridges
        );
    }

    #[test]
    fn composition_root_exposes_stub_types_without_a_protocol_line() {
        assert!(!standard_resource_policy().allows_unbounded_collections());
        assert_eq!(
            ordinary_session().state(),
            ClientSessionState::DeviceUnlocked
        );
        assert!(empty_adapter_registry().is_empty());
        assert_eq!(
            runtime_abi_identity().abi_version,
            CLIENT_RUNTIME_ABI_VERSION
        );
        let mut arena = bounded_handle_arena::<u8>(2);
        let handle = arena.insert(1).expect("insert");
        assert_eq!(arena.get(handle).copied(), Some(1));
    }
}
