//! Who may control work here, and how that answer is established.
//!
//! Two facts are both required and neither substitutes for the other:
//!
//! * **Verified ingress.** The request arrived through the exact verified unit
//!   path; the peer's own claim in the request body authenticates nothing.
//! * **A current local grant.** This host granted that requester work control,
//!   and the grant covers the intent. A grant is local state: it is given,
//!   withdrawn and revoked here, and no announcement, badge or receipt produces
//!   one.
//!
//! The authority is ordered, and an intent requires at least its own level:
//! [`ControlAuthority::ForceStop`] covers every lower intent,
//! [`ControlAuthority::Stop`] covers inspect and ordinary stop, and
//! [`ControlAuthority::Inspect`] covers inspection only.

use std::collections::BTreeMap;

/// One requester endpoint identity, as the verified ingress resolved it.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct EndpointIdentity(String);

impl EndpointIdentity {
    #[must_use]
    pub fn new(identity: impl Into<String>) -> Self {
        Self(identity.into())
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// The protocol's own replay identity for one verified unit.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct ReplayIdentity(String);

impl ReplayIdentity {
    #[must_use]
    pub fn new(identity: impl Into<String>) -> Self {
        Self(identity.into())
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// How much work control one requester holds here.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum ControlAuthority {
    /// No control at all: a requester without a grant.
    None,
    /// Inspection only.
    Inspect,
    /// Inspection and ordinary stop of owned work.
    Stop,
    /// Every intent, including the force control that terminates an owned
    /// process group.
    ForceStop,
}

impl ControlAuthority {
    /// Whether this authority covers one required level.
    #[must_use]
    pub const fn permits(self, required: Self) -> bool {
        self as u8 >= required as u8
    }

    /// The stable name a caller publishes.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Inspect => "inspect",
            Self::Stop => "stop",
            Self::ForceStop => "forceStop",
        }
    }
}

/// The local grants this host currently holds.
///
/// It is a table of decisions made here, not a copy of anything a peer said.
#[derive(Clone, Debug, Default)]
pub struct ControlGrants {
    grants: BTreeMap<EndpointIdentity, ControlAuthority>,
}

impl ControlGrants {
    #[must_use]
    pub const fn new() -> Self {
        Self {
            grants: BTreeMap::new(),
        }
    }

    /// Grant one requester an authority. A lower grant never lowers a higher
    /// one that is already held.
    pub fn grant(&mut self, requester: &EndpointIdentity, authority: ControlAuthority) {
        let held = self.authority_of(requester);
        let next = if authority.permits(held) {
            authority
        } else {
            held
        };
        self.grants.insert(requester.clone(), next);
    }

    /// Withdraw every grant one requester holds. The requester itself is
    /// untouched: this is a local decision, not a peer's revocation.
    pub fn revoke(&mut self, requester: &EndpointIdentity) -> bool {
        self.grants.remove(requester).is_some()
    }

    /// What one requester currently holds here.
    #[must_use]
    pub fn authority_of(&self, requester: &EndpointIdentity) -> ControlAuthority {
        self.grants
            .get(requester)
            .copied()
            .unwrap_or(ControlAuthority::None)
    }

    /// The requesters that currently hold any control.
    #[must_use]
    pub fn granted(&self) -> Vec<&EndpointIdentity> {
        self.grants
            .iter()
            .filter(|(_, authority)| **authority != ControlAuthority::None)
            .map(|(requester, _)| requester)
            .collect()
    }
}

/// The verified ingress facts of one request.
///
/// Every field is established by the verified unit path before this slice sees
/// the request. A caller that only knows how to read a request body cannot
/// build a verified ingress, and an unverified ingress is refused before any
/// admission, so a refused request leaves no record behind it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct VerifiedIngress {
    requester: EndpointIdentity,
    replay_identity: ReplayIdentity,
    verified: bool,
}

impl VerifiedIngress {
    /// The ingress of a unit the verified path accepted.
    #[must_use]
    pub const fn verified(requester: EndpointIdentity, replay_identity: ReplayIdentity) -> Self {
        Self {
            requester,
            replay_identity,
            verified: true,
        }
    }

    /// The ingress of a unit the verified path did not accept.
    #[must_use]
    pub const fn unverified(requester: EndpointIdentity, replay_identity: ReplayIdentity) -> Self {
        Self {
            requester,
            replay_identity,
            verified: false,
        }
    }

    #[must_use]
    pub const fn is_verified(&self) -> bool {
        self.verified
    }

    #[must_use]
    pub const fn requester(&self) -> &EndpointIdentity {
        &self.requester
    }

    #[must_use]
    pub const fn replay_identity(&self) -> &ReplayIdentity {
        &self.replay_identity
    }
}

#[cfg(test)]
mod tests {
    use super::{
        ControlAuthority, ControlGrants, EndpointIdentity, ReplayIdentity, VerifiedIngress,
    };

    fn requester(name: &str) -> EndpointIdentity {
        EndpointIdentity::new(name)
    }

    #[test]
    fn no_grant_is_no_authority_and_an_intent_needs_at_least_its_own_level() {
        assert!(!ControlAuthority::None.permits(ControlAuthority::Inspect));
        assert!(ControlAuthority::Inspect.permits(ControlAuthority::Inspect));
        assert!(!ControlAuthority::Inspect.permits(ControlAuthority::Stop));
        assert!(ControlAuthority::Stop.permits(ControlAuthority::Inspect));
        assert!(!ControlAuthority::Stop.permits(ControlAuthority::ForceStop));
        assert!(ControlAuthority::ForceStop.permits(ControlAuthority::Stop));
    }

    #[test]
    fn a_grant_is_local_state_and_a_lower_grant_never_lowers_a_higher_one() {
        let mut grants = ControlGrants::new();
        let peer = requester("endpoint-b");
        assert_eq!(grants.authority_of(&peer), ControlAuthority::None);
        assert!(grants.granted().is_empty());

        grants.grant(&peer, ControlAuthority::Stop);
        assert_eq!(grants.authority_of(&peer), ControlAuthority::Stop);
        grants.grant(&peer, ControlAuthority::Inspect);
        assert_eq!(
            grants.authority_of(&peer),
            ControlAuthority::Stop,
            "a later lower grant does not narrow what was already granted"
        );
        grants.grant(&peer, ControlAuthority::ForceStop);
        assert_eq!(grants.authority_of(&peer), ControlAuthority::ForceStop);

        assert_eq!(grants.granted(), vec![&peer]);
        assert!(grants.revoke(&peer));
        assert!(!grants.revoke(&peer));
        assert_eq!(grants.authority_of(&peer), ControlAuthority::None);
    }

    #[test]
    fn one_requester_does_not_inherit_anothers_grant() {
        let mut grants = ControlGrants::new();
        grants.grant(&requester("endpoint-b"), ControlAuthority::ForceStop);
        assert_eq!(
            grants.authority_of(&requester("endpoint-c")),
            ControlAuthority::None
        );
    }

    #[test]
    fn an_unverified_ingress_is_a_different_fact_from_a_verified_one() {
        let verified =
            VerifiedIngress::verified(requester("endpoint-b"), ReplayIdentity::new("replay-1"));
        let unverified =
            VerifiedIngress::unverified(requester("endpoint-b"), ReplayIdentity::new("replay-1"));

        assert!(verified.is_verified());
        assert!(!unverified.is_verified());
        assert_ne!(verified, unverified);
        assert_eq!(verified.requester().as_str(), "endpoint-b");
        assert_eq!(verified.replay_identity().as_str(), "replay-1");
    }
}
