//! Stable directional domain labels for endpoint-owned mailboxes.

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum SecureMeshMailboxDirection {
    PairwiseInitiatorToResponder,
    PairwiseResponderToInitiator,
    MlsGroupToMembers,
}

impl SecureMeshMailboxDirection {
    pub(super) fn stable_label(self) -> &'static [u8] {
        match self {
            Self::PairwiseInitiatorToResponder => b"pairwise.initiator-to-responder.v1",
            Self::PairwiseResponderToInitiator => b"pairwise.responder-to-initiator.v1",
            Self::MlsGroupToMembers => b"mls.group-to-members.v1",
        }
    }
}
