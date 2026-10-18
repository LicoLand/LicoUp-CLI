//! Host-owned binding from a verified peer device to an existing membership.
//!
//! This port is deliberately small and one-directional. The ingress asks the
//! host which pre-existing Conversation membership a verified author/device pair
//! belongs to; the host answers from its durable trust state, and an absent
//! answer is a refusal. Nothing in the ingress creates a principal, a
//! membership, or an administrator, and a message can never name its own
//! binding.

use crate::domain::mobile_relay::endpoint_transport::{PeerAuthor, PeerDevice};

/// One pre-existing local binding of a verified peer device.
///
/// `provider_id` is the local mesh caller identity the host already admits for
/// this device; it is what the single-owner application entry verifies, so the
/// ingress never invents one.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PeerBinding {
    pub conversation_id: String,
    pub membership_id: String,
    pub provider_id: String,
}

/// Resolves verified peer identities to existing local bindings.
///
/// Implementations own the durable trust state (pairing/roster/membership
/// records). They must fail closed: `None` means the peer is not bound to a
/// membership, and revocation is expressed by answering `None` again.
pub trait PeerBindings: Send + Sync {
    fn resolve(&self, author: &PeerAuthor, device: &PeerDevice) -> Option<PeerBinding>;
}
