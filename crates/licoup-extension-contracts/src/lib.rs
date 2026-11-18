//! The extension SDK contract: five narrow profiles, one wire protocol, no SDK
//! runtime.
//!
//! An extension is a separate program. It is not a Dart widget, not a Rust trait
//! object, and not a plugin loaded into the client's address space. The client
//! speaks one line-delimited JSON-RPC 2.0 protocol to it and this crate owns that
//! contract's *shape* — the profile ids and their required and optional methods,
//! the frame bound, the event envelope, and the package facts that decide whether
//! a capability can be served at all. It owns no catalog, no scheduler, no
//! process supervision, no storage and no authority: everything runtime is the
//! host's, and everything identity is [`licoup_application::extension`]'s.
//!
//! Three rules shape the whole platform, and each is enforced here rather than
//! described:
//!
//! - **The five profiles are narrow, not one universal `call`.** C09 lifecycle
//!   and Agent execution, C10 model provider, C11 usage and metrics, C12 package
//!   and deployment closure, C13 declarative UI. A specialist Agent that only
//!   streams text implements three methods and is complete; it is not a degraded
//!   provider. A profile's methods that nobody calls are never started, so a
//!   missing optional profile refuses nothing outside its own operations.
//! - **Local import is first-class.** Installing a package a user built on this
//!   machine, or pointing at an adapter process that already exists, requires no
//!   registry, no directory service and no account. Nothing in this contract
//!   reads, resolves or downloads over the network, and no rule here can be
//!   satisfied only by a hosted catalog.
//! - **Absence is a catalog fact, not an exception.** A runtime that is not
//!   installed is reported as an unavailable capability with an actionable
//!   recovery. It is never an invalid request, never a parse error, and never a
//!   reason the rest of the client fails to start.
//!
//! The modules below are the five profiles in that same order, plus the transport
//! they all ride on: [`profile`] and [`transport`] for C09, [`agent`] for the
//! execution half of C09, [`provider`] for C10, [`usage`] for C11, [`deployment`]
//! for C12 and [`ui`] for C13.
//!
//! Nothing in this crate parses a natural-language reply. An Agent's ordinary
//! output is [`licoup_application::NaturalOutput`]: carried verbatim, never
//! required to be JSON, and never made invalid because it looks like a broken
//! envelope.

pub mod agent;
pub mod deployment;
pub mod manifest;
pub mod profile;
pub mod provider;
pub mod transport;
pub mod ui;
pub mod usage;

/// The wire identifiers this contract publishes. They are the `const` values of
/// the schemas under `schemas/extensions/`, and the agreement is asserted by
/// `tests/schema_agreement.rs` so a hand-edited schema cannot drift from the
/// crate.
pub mod wire {
    /// The extension package manifest.
    pub const MANIFEST: &str = "licoup.extension-package.v1";
    /// One model provider configuration.
    pub const PROVIDER: &str = "licoup.model-provider.v1";
    /// One usage observation.
    pub const USAGE: &str = "licoup.usage-observation.v1";
    /// One declarative UI contribution.
    pub const UI: &str = "licoup.ui-contribution.v1";
    /// The deployment facts a host publishes for a selected package set.
    pub const DEPLOYMENT: &str = "licoup.deployment.v1";
}

/// The failure vocabulary this contract raises, built once so every module names
/// the same component and none of them invents a second error account.
pub(crate) mod refusal {
    use licoup_application::{ApplicationFailure, RecoveryAction};

    /// The component every failure from this contract names, so a client can tell
    /// an extension-platform refusal from a business one without parsing text.
    pub(crate) const COMPONENT: &str = "extension_sdk";

    /// A refusal that is a fact about the package or the profile, not about the
    /// shape of the request.
    pub(crate) fn new(code: &str, stage: &str) -> ApplicationFailure {
        ApplicationFailure::permanent(code, stage).with_component(COMPONENT)
    }

    /// A refusal whose next step is a real one for the user: install, enable or
    /// select the package that serves this capability.
    pub(crate) fn actionable(code: &str, stage: &str, field: &str) -> ApplicationFailure {
        new(code, stage)
            .with_field(field)
            .with_recovery(RecoveryAction::InstallOrRetryRuntime)
    }
}

pub use licoup_application::{
    ActivationMode, AdoptedAttributes, ApplicationFailure, CapabilityDescriptor,
    ContractCompatibility, ContractRange, DeclaredAttribute, DiscoveredCapabilities,
    EffectCertainty, LifecycleSupport, NaturalOutput, OperationState, QuotaShape, ReceiptKind,
    RecoveryAction, Requirement, is_authority_field, is_namespaced, is_semver,
};
