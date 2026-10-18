//! Canonical Event repository boundary.

use super::{ConversationStore, StoreResult};
use crate::{ConversationEvent, EventPage};

/// Exclusive sequence anchors for forward and backward reads. Latest starts
/// from the newest persisted event, independently of the number of retained rows.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EventPagePosition {
    After(i64),
    Before(i64),
    Latest,
}

/// Durable Event reads shared by initial load and post-reconnect projection.
pub trait EventRepository {
    fn event_page(
        &self,
        conversation_id: &str,
        after_sequence: Option<i64>,
        limit: usize,
    ) -> StoreResult<EventPage>;
    fn search_events(&self, query: &str, limit: usize) -> StoreResult<Vec<ConversationEvent>>;
}

impl EventRepository for ConversationStore {
    fn event_page(
        &self,
        conversation_id: &str,
        after_sequence: Option<i64>,
        limit: usize,
    ) -> StoreResult<EventPage> {
        self.page_events(conversation_id, after_sequence, limit)
    }

    fn search_events(&self, query: &str, limit: usize) -> StoreResult<Vec<ConversationEvent>> {
        self.search(query, limit)
    }
}
