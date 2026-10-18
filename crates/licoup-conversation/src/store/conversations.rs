//! Conversation aggregate repository boundary.

use super::{ConversationStore, StoreResult};
use crate::{Conversation, ConversationSummary};

/// Durable aggregate reads. Implementations must reconstruct from the store;
/// callers cannot supply or recover from a process-memory snapshot.
pub trait ConversationRepository {
    fn conversation(&self, conversation_id: &str) -> StoreResult<Conversation>;
    fn conversations(&self, include_archived: bool) -> StoreResult<Vec<ConversationSummary>>;
}

impl ConversationRepository for ConversationStore {
    fn conversation(&self, conversation_id: &str) -> StoreResult<Conversation> {
        self.get(conversation_id)
    }

    fn conversations(&self, include_archived: bool) -> StoreResult<Vec<ConversationSummary>> {
        self.list(include_archived)
    }
}
