//! The runtime extension host — one capability catalog, on-demand
//! activation, generation hot-swap, host-bound invocation and drain.
//!
//! An extension is a separate program (C09). This module owns the *host* half of
//! that contract: what the client currently serves, which instance of which
//! package generation answers a call, and what happens to work that was already
//! admitted when the catalog moves to a newer generation. It does not own the
//! package bytes or the install transaction ([`crate::platform::extension_packages`]),
//! the wire shapes ([`licoup_extension_contracts`]), or the catalog's admission
//! rules ([`licoup_application::CapabilityDescriptor`]). It composes them.
//!
//! Six rules are enforced by construction rather than described:
//!
//! 1. **One catalog owner, one epoch.** Every surface — the desktop, the CLI, the
//!    MCP process — reads the same immutable [`CatalogSnapshot`] from
//!    [`ExtensionHost::catalog`]. A snapshot is published whole or not at all, so
//!    two readers can never see half of an update: there is no second tool list
//!    and no per-surface cache that could disagree.
//! 2. **Capabilities are namespaced strings, never a vendor enum.** Nothing here
//!    enumerates vendors, providers or agents. A capability is admitted exactly
//!    when an active instance serves its namespaced name, so a new vendor needs
//!    no regeneration of this module.
//! 3. **Package, instance, generation and registry epoch stay four facts.** A
//!    package version can produce several instances with different permission
//!    scopes; one instance runs one generation; the catalog commits those
//!    instances as one epoch. The counters live in
//!    [`crate::platform::extension_packages::state::InstanceMachine`], which is
//!    the single in-flight authority — this module does not keep a second one.
//! 4. **Activation is two-phase and committed by compare-and-swap.** [`ExtensionHost::stage`]
//!    validates declarations, [`ExtensionHost::prepare`] starts and handshakes a
//!    carrier without touching the live catalog, and [`ExtensionHost::activate`]
//!    commits against the epoch the preparation read. An unrelated concurrent
//!    commit is re-based; a newer generation for the same package and permission
//!    scope wins and the loser is drained, so the catalog never mixes versions.
//! 5. **An admitted call is pinned to its generation.** [`ExtensionHost::begin`]
//!    binds the invocation to the instance, generation and epoch that admitted
//!    it; `observe`, `cancel` and the result stay on that binding after the
//!    catalog has moved on. A superseded generation can settle and be cancelled,
//!    but it cannot admit new work.
//! 6. **A hook requests effects; it never dispatches them.** A hook holds a
//!    [`HookTicket`], not a carrier session, and every effect it wants goes back
//!    through [`ExtensionHost::hook_request_effect`] as a fresh admission under
//!    the current epoch. A stale or revoked hook is refused instead of
//!    re-dispatching work from an old generation.
//! 7. **A handle is only meaningful to the host that issued it.** Visible facts
//!    repeat across hosts — the first instance is always `instance-1` and the
//!    first invocation `invocation-1` — so every handle carries a
//!    [`HostIncarnation`], a random token with no public constructor, and every
//!    host checks it before looking anything up. A binding, ticket or prepared
//!    value from another host run is refused by identity, not resolved against a
//!    coincidentally equal counter.
//! 8. **Persistence is a boundary, not a promise.** The record is
//!    [`CatalogJournal`]'s: allocations are written in `prepare`, activations
//!    before the epoch is published, and stops with the epoch they published —
//!    so a restart never reuses a consumed generation or a published epoch.
//!    Active pointers the record still names become unconfirmed predecessors:
//!    they stay visible in every snapshot, route nothing, and block a new
//!    instance of the same package and permission scope until their owner is
//!    confirmed — an unknown owner is never silently replaced. Whether the
//!    identity is durable is the journal's own capability
//!    ([`JournalDurability`]); an in-memory fixture reports `ProcessLocal` and
//!    [`ExtensionHost::identity_is_durable`] says so even though a journal is
//!    present. [`ExtensionHost::without_journal`] is the explicit process-local
//!    mode. One managed root has one *writing* host: the host claims the
//!    journal's writer slot at construction and a second host over the same
//!    journal is refused before it prepares anything; readers are unrestricted
//!    and may share the host and its snapshots. A catalogue `Stopped` is not evidence that the process exited:
//!    [`SessionOwner`] carries that separate fact, an unverified stop is refused
//!    as cleanup evidence, and the session handle is retained until its owner
//!    confirms.
//!
//! **What this is not.** The carriers composed here are the seam the isolation
//! carrier fills with
//! real subprocesses under OS-level confinement. An in-process carrier cannot
//! confine an extension and this module never claims it does: a fault is
//! *isolated* — the offending instance stops being routed, its unsettled work is
//! recorded `unknown`, and every other capability keeps working — but a failure
//! that already reached the operating system is not retracted.
//!
//! The host depends on the extension contracts and the package store, in that
//! direction only. It never imports the workflow runtime or the gateway: a
//! plain local Agent must run with neither installed, and nothing here
//! re-creates a task, budget or effect account.

pub mod carrier;
pub mod catalog;
pub mod host;
pub mod identity;
pub mod invocation;
pub mod isolation;
pub mod journal;
pub mod lifecycle;
pub mod runtime;

pub use carrier::{
    CancelDisposition, CarrierSession, CarrierSpec, DispatchOutcome, ExtensionCarrier, FaultClass,
    InitializeRequest, InitializedProfileSet, Observation, carrier_fault, classify_failure,
};
pub use catalog::{
    CapabilityCatalog, CatalogAdmission, CatalogCapability, CatalogDocument, CatalogEntry,
    CatalogEpoch, CatalogProfile, CatalogProfileStatus, CatalogSnapshot, SessionOwner,
};
pub use host::ExtensionHost;
pub use identity::HostIncarnation;
pub use invocation::{AdmittedInvocation, HookTicket, InvocationBinding, InvocationOutcome};
pub use journal::{
    ActivationRecord, ActivePointer, CatalogJournal, CatalogWatermark, CatalogWriterPermit,
    JournalDurability, MemoryCatalogJournal, RuntimeCatalogJournal, StopReason, StopRecord,
};
pub use lifecycle::{ActivationReceipt, PreparedExtension, StageRequest, StagedExtension};
pub use runtime::{
    AGENT_EXECUTION_CAPABILITY, AgentExecutionCall, ExtensionRuntime, HOST_CONTRACT_RANGE,
};

use licoup_application::{ApplicationFailure, EffectCertainty, RecoveryAction};

/// The component every refusal from this module names, so an interface can tell
/// a runtime-extension refusal from a business one without parsing text.
pub(crate) const COMPONENT: &str = "extension_host";

/// A refusal that is a fact about the catalog, the instance or the request
/// shape, not about the shape of the host.
pub(crate) fn refusal(code: &str, stage: &str) -> ApplicationFailure {
    ApplicationFailure::permanent(code, stage).with_component(COMPONENT)
}

/// A refusal whose next step is a real one: install, enable or select the
/// package that serves this capability.
pub(crate) fn actionable(code: &str, stage: &str, field: &str) -> ApplicationFailure {
    refusal(code, stage)
        .with_field(field)
        .with_recovery(RecoveryAction::InstallOrRetryRuntime)
}

/// A refusal about work whose outcome cannot be known from the answer alone.
pub(crate) fn uncertain(code: &str, stage: &str) -> ApplicationFailure {
    ApplicationFailure::uncertain(code, stage).with_component(COMPONENT)
}

/// Mark a failure as one whose effect may already have happened.
pub(crate) fn as_uncertain(failure: ApplicationFailure) -> ApplicationFailure {
    failure.with_effect(EffectCertainty::Uncertain)
}
