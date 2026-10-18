//! Canonical Conversation ingress for verified peer messages.
//!
//! This is the native host's adapter from the endpoint boundary into the
//! existing conversation owner (M04). It reuses that owner's store, memberships,
//! and revocation semantics; it does not open a second conversation authority,
//! does not create a temporary administrator, and does not execute effects.
//!
//! The pipeline is:
//!
//! 1. a [`PeerMessage`](crate::domain::mobile_relay::endpoint_transport::PeerMessage)
//!    carries SDK-verified provenance and the peer's content;
//! 2. [`PeerIngress::admit`] resolves that provenance to a pre-existing
//!    membership through the host's [`PeerBindings`], checks it against the
//!    active membership of the existing Conversation, and appends the message
//!    through the owner's store door;
//! 3. the report keeps admission, delivery, read, and acceptance separate, and
//!    returns any structured command as a [`PeerEffectIntent`] for the
//!    single-owner application entry — never as an executed effect.
//!
//! [`PeerIngress::admit`]: ingress::PeerIngress::admit

mod bindings;
mod facts;
mod ingress;
#[cfg(test)]
mod tests;

pub use bindings::{PeerBinding, PeerBindings};
pub use facts::{
    AcceptanceFact, AdmissionFact, DeliveryFact, MAX_TRACKED_FACTS, PeerFactLedger, PeerFacts,
    ReadFact,
};
pub use ingress::{
    MAX_CAUSAL_LINKS, PeerEffectIntent, PeerIngress, PeerIngressRefusal, PeerIngressReport,
};
