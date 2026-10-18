//! Host-bound invocations: the binding that pins one call to the generation
//! that admitted it, and the ticket that keeps a hook from dispatching effects
//! on its own.
//!
//! A binding is issued by [`super::host::ExtensionHost::begin`] and is the only
//! thing `observe`, `cancel` and the result accept. It names the instance, the
//! generation and the registry epoch together, so after the catalog has moved
//! to a newer generation the calls still reach the instance that admitted the
//! work — while a *new* call can never be admitted through an old binding.
//!
//! Its fields are private and it carries a [`HostIncarnation`]: a binding is a
//! host-issued handle, not a value a caller composes. The visible facts alone
//! are not identity — a second host's first invocation carries the same
//! `invocation-1`/generation 1/epoch 1 — so the receiver checks the incarnation
//! before it looks anything up.
//!
//! A [`HookTicket`] is what an extension's event handler holds. It is not a
//! carrier session and it carries no dispatch method: every effect a hook wants
//! is requested through
//! [`super::host::ExtensionHost::hook_request_effect`], which re-admits it
//! under the current catalog. A hook whose generation has been superseded or
//! revoked is refused instead of re-dispatching an effect from the old
//! generation, and a ticket another host issued is refused by identity.

use serde_json::Value;

use super::catalog::CatalogEpoch;
use super::identity::HostIncarnation;

/// One call pinned to the instance, generation and epoch that admitted it.
///
/// The fields are private on purpose: a handle is only meaningful to the host
/// that minted it, and the incarnation it carries cannot be forged from
/// outside.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct InvocationBinding {
    /// The host run that issued this binding.
    pub(crate) host: HostIncarnation,
    /// The host-issued invocation identity, unique within that host run.
    pub(crate) invocation_id: String,
    /// The namespaced capability the call was admitted for.
    pub(crate) capability: String,
    /// The profile that serves the capability, when one published profile does.
    pub(crate) profile: Option<String>,
    pub(crate) package_id: String,
    pub(crate) instance_id: String,
    pub(crate) generation: u64,
    /// The catalog epoch the admission read. A later epoch never rewrites this
    /// binding.
    pub(crate) registry_epoch: CatalogEpoch,
}

impl InvocationBinding {
    pub fn invocation_id(&self) -> &str {
        self.invocation_id.as_str()
    }

    pub fn capability(&self) -> &str {
        self.capability.as_str()
    }

    pub fn profile(&self) -> Option<&str> {
        self.profile.as_deref()
    }

    pub fn package_id(&self) -> &str {
        self.package_id.as_str()
    }

    pub fn instance_id(&self) -> &str {
        self.instance_id.as_str()
    }

    pub fn generation(&self) -> u64 {
        self.generation
    }

    /// The catalog epoch the admission read.
    pub fn registry_epoch(&self) -> CatalogEpoch {
        self.registry_epoch
    }

    /// The display label of the issuing host run, for diagnostics.
    pub fn host_display_id(&self) -> u64 {
        self.host.display_id()
    }
}

/// One admitted call: the binding that pins it and what the dispatch produced.
#[derive(Clone, Debug, PartialEq)]
pub struct AdmittedInvocation {
    pub binding: InvocationBinding,
    pub outcome: InvocationOutcome,
}

/// What one admitted call produced.
#[derive(Clone, Debug, PartialEq)]
pub enum InvocationOutcome {
    /// Accepted; completion arrives through `observe`/the result.
    Admitted,
    Finished {
        payload: Value,
    },
    /// The extension's natural reply, carried verbatim.
    Natural(licoup_application::NaturalOutput),
    /// The extension cannot say whether the effect happened.
    Unknown {
        code: String,
    },
}

/// The right to request one effect from one instance generation.
///
/// Holding a ticket proves only that the host registered this hook while the
/// instance was active. It grants no dispatch, it expires with the generation
/// it names, and it is refused by any other host run.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HookTicket {
    /// The host run that issued this ticket. A ticket is a host-issued handle,
    /// not a portable value: an identical-looking ticket from another host is
    /// not this host's decision.
    pub(crate) host: HostIncarnation,
    pub(crate) ticket_id: String,
    pub(crate) package_id: String,
    pub(crate) instance_id: String,
    pub(crate) generation: u64,
    pub(crate) capability: String,
    pub(crate) registered_at: CatalogEpoch,
}

impl HookTicket {
    pub fn ticket_id(&self) -> &str {
        self.ticket_id.as_str()
    }

    pub fn package_id(&self) -> &str {
        self.package_id.as_str()
    }

    pub fn instance_id(&self) -> &str {
        self.instance_id.as_str()
    }

    pub fn generation(&self) -> u64 {
        self.generation
    }

    /// The capability this hook observes.
    pub fn capability(&self) -> &str {
        self.capability.as_str()
    }

    /// The epoch the hook was registered under.
    pub fn registered_at(&self) -> CatalogEpoch {
        self.registered_at
    }

    /// The display label of the issuing host run, for diagnostics.
    pub fn host_display_id(&self) -> u64 {
        self.host.display_id()
    }
}

/// One invocation as the host holds it while it is in flight.
#[derive(Debug)]
pub(crate) struct LiveInvocation {
    pub(crate) binding: InvocationBinding,
    pub(crate) state: LiveState,
}

pub(crate) use crate::state_machines::extension_invocation::State as LiveState;

impl LiveInvocation {
    pub(crate) fn new(binding: InvocationBinding) -> Self {
        Self {
            binding,
            state: crate::state_machines::extension_invocation::INITIAL,
        }
    }

    /// Apply one settlement event through the declarative invocation machine.
    pub(crate) fn settle(&mut self, event: crate::state_machines::extension_invocation::Event) {
        self.state = crate::state_machines::extension_invocation::transition(self.state, event)
            .expect("an outstanding invocation must accept its settlement event");
    }
}
