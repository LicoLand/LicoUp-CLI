use super::commit::{
    commit_proposal, consume_logical_wake, enqueue_follow_up, read_basis, read_child_relations,
    read_matters_page, read_parent_grants, read_relation,
};
use super::generated::{ContinuityFailure, ContinuityInterpretationProposal, ContinuityWake};
use super::ports::{
    ContinuityCommitPort, ContinuityCommitReceipt, ContinuityReadPort, FollowUpPort,
};
use crate::store::ConversationStore;

impl ContinuityCommitPort for ConversationStore {
    fn commit(
        &self,
        proposal: &ContinuityInterpretationProposal,
    ) -> Result<ContinuityCommitReceipt, ContinuityFailure> {
        commit_proposal(self, proposal)
    }
}

impl ContinuityReadPort for ConversationStore {
    fn commit_basis(
        &self,
        conversation_id: &str,
    ) -> Result<super::ContinuityCommitBasis, ContinuityFailure> {
        read_basis(self, conversation_id)
    }

    fn list_matters(
        &self,
        conversation_id: &str,
        after: Option<&str>,
        limit: usize,
    ) -> Result<Vec<super::ContinuityMatter>, ContinuityFailure> {
        read_matters_page(self, conversation_id, after, limit)
    }

    fn relation_for_goal(
        &self,
        goal_id: &str,
    ) -> Result<super::ContinuityTaskConversationRelation, ContinuityFailure> {
        read_relation(self, goal_id)
    }

    fn list_child_relations(
        &self,
        parent_conversation_id: &str,
        after: Option<&str>,
        limit: usize,
    ) -> Result<Vec<super::ContinuityTaskConversationRelation>, ContinuityFailure> {
        read_child_relations(self, parent_conversation_id, after, limit)
    }

    fn list_parent_grants(
        &self,
        recipient_conversation_id: &str,
        recipient_membership_id: &str,
        after: Option<&str>,
        limit: usize,
    ) -> Result<Vec<super::ContinuityParentContextGrant>, ContinuityFailure> {
        read_parent_grants(
            self,
            recipient_conversation_id,
            recipient_membership_id,
            after,
            limit,
        )
    }
}

impl FollowUpPort for ConversationStore {
    fn enqueue_wake(
        &self,
        wake: &ContinuityWake,
    ) -> Result<ContinuityCommitReceipt, ContinuityFailure> {
        enqueue_follow_up(self, wake)
    }
}

pub fn frozen_unavailable_commit() -> super::UnavailableContinuityCommit {
    super::UnavailableContinuityCommit
}

pub fn consume_follow_up(
    store: &ConversationStore,
    logical_wake_id: &str,
) -> Result<bool, ContinuityFailure> {
    consume_logical_wake(store, logical_wake_id)
}
