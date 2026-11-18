//! The observation catalogue: what this host observes outside its own process,
//! through which injectable port, and which production path consumes it.
//!
//! Why this exists. The observation ports were wired one call site at a time.
//! Each port carried a producer, and each consumer read whichever port it had
//! been handed, but nothing stated the whole set. A port could therefore keep a
//! producer and lose its consumer, or a consumer could read a port no register
//! named, without any check noticing. This module is that register.
//!
//! How a read is admitted. [`admits`] answers whether the catalogue carries a
//! source, and the port owners read it before they publish: a read for a source
//! the catalogue does not carry keeps the fail-closed answer instead of
//! projecting an observation whose consumer nothing registered. The two current
//! readers are
//! [`CodexRuntimeObservation`](crate::domain::conversation::history::CodexRuntimeObservation)
//! and
//! [`CallbackEvolutionEnricher`](crate::domain::workflow_runtime::evolution::CallbackEvolutionEnricher).
//!
//! What is checked, and where. The contract test in `observation_catalogue/tests.rs`
//! holds its own list of observation sources, written from the consumers this
//! crate ships, and drives each consumer with a synthetic observation. The table
//! below must carry exactly that list, both ways: a source dropped from the
//! table leaves a probed consumer with no catalogue entry, and an entry with no
//! consumer that reads its port fails the same test.

/// The injectable port that carries one observation out of its observer.
///
/// A port is a value the composition installs and a consumer reads; it is not
/// the observation itself. The shape follows
/// [`CodexRolloutPort`](crate::domain::conversation::history::CodexRolloutPort):
/// one answer per process, and a program that installs none gets the
/// fail-closed answer rather than a fabricated fact.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ObservationPort {
    /// The Codex rollout port:
    /// `domain::conversation::history::runtime_observation::CodexRolloutPort`.
    CodexRollout,
    /// The workflow evolution observation port:
    /// `domain::workflow_runtime::evolution::EvolutionObservationPort`.
    WorkflowEvolutionNode,
}

impl ObservationPort {
    /// The port's own type name, as the consumer's source declares it.
    ///
    /// The contract test requires this token in the consumer file that answers
    /// for the source, so an entry cannot name a consumer that never mentions
    /// the port it is supposed to read.
    pub const fn port_type(self) -> &'static str {
        match self {
            Self::CodexRollout => "CodexRolloutPort",
            Self::WorkflowEvolutionNode => "EvolutionObservationPort",
        }
    }
}

/// One observation source: the observer, the observation, the port, the consumer.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ObservationSource {
    /// Stable id, unique across the catalogue.
    pub id: &'static str,
    /// The production path that observes, as a Rust path.
    pub observer: &'static str,
    /// What is observed, in one sentence.
    pub observes: &'static str,
    /// The injectable port that carries the observation to its consumer.
    pub port: ObservationPort,
    /// The production path that reads the port and acts on the observation.
    pub consumer: &'static str,
}

/// Codex rollout files a running Codex app-server holds open.
pub const SOURCE_CODEX_ROLLOUTS_OPEN: &str = "codex-rollouts-open";

/// Node lifecycle observations one workflow run recorded.
pub const SOURCE_WORKFLOW_EVOLUTION_NODES: &str = "workflow-evolution-nodes";

/// Every observation source this host consumes.
///
/// The order is the order the port reads are admitted in, and it is not a
/// priority: the sources are independent and no entry depends on another.
pub const SOURCES: &[ObservationSource] = &[
    ObservationSource {
        id: SOURCE_WORKFLOW_EVOLUTION_NODES,
        observer: "domain::workflow_runtime::evolution::DefaultEvolutionObservationPort",
        observes: "the node lifecycle observations one workflow run recorded",
        port: ObservationPort::WorkflowEvolutionNode,
        consumer: "domain::workflow_runtime::evolution::CallbackEvolutionEnricher",
    },
];

/// Every catalogued source, in catalogue order.
pub fn sources() -> &'static [ObservationSource] {
    SOURCES
}

/// Whether the catalogue admits a read of this source's port.
///
/// The port owners read this before publishing. An id the catalogue does not
/// carry has no registered consumer, so the read answers nothing rather than
/// handing a fact to a consumer nobody named.
pub fn admits(id: &str) -> bool {
    sources().iter().any(|source| source.id == id)
}

#[cfg(test)]
mod tests;
