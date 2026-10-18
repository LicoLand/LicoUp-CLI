//! The bounded mobile read model over Canonical Conversation records.
//!
//! Flutter renders from projected facts, never from the aggregate it would
//! have to interpret. This module is that projection for the mobile chat
//! surface: it turns the Canonical Conversation authority's own
//! [`Conversation`], [`ConversationSummary`] and [`EventPage`] values into the
//! small shape the mobile list and thread render, and it pages both by the one
//! bounded [`ClientResourcePolicy`] the client already owns.
//!
//! It resolves nothing. A card carries what the summary carries, a thread
//! carries what the aggregate and the page carry, and an event carries the
//! text part the page already returned. Absent facts stay absent — the
//! projection never infers a title, a delivery state, or a pending turn.

use licoup_client_state::ClientResourcePolicy;
use licoup_conversation::{
    Conversation, ConversationSummary, EventKind, EventPage, EventPartKind, MembershipAccess,
    MembershipStatus, PrincipalKind,
};

/// One membership as the mobile thread header renders it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MemberView {
    pub membership_id: String,
    pub principal_id: String,
    pub display_name: String,
    pub kind: PrincipalKind,
    pub access: MembershipAccess,
    pub status: MembershipStatus,
}

/// One event as the mobile thread renders it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EventView {
    pub id: String,
    pub sequence: i64,
    pub author_membership_id: Option<String>,
    pub kind: EventKind,
    pub created_at_unix_ms: i64,
    pub finalized: bool,
    /// The first non-empty Text part's content, when the event has one.
    pub text: Option<String>,
}

/// One conversation as the mobile chat list renders it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ChatCard {
    pub id: String,
    pub title: String,
    pub archived: bool,
    pub pinned: bool,
    pub is_group: bool,
    pub revision: i64,
    pub updated_at_unix_ms: i64,
    pub membership_count: i64,
    pub event_count: i64,
}

/// The bounded chat list, with the fact that the bound was reached.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ChatList {
    pub conversations: Vec<ChatCard>,
    /// True when the caller offered more summaries than the policy admits.
    pub truncated: bool,
}

/// One conversation's bounded thread.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ThreadView {
    pub conversation_id: String,
    pub title: String,
    pub is_group: bool,
    pub archived: bool,
    pub revision: i64,
    pub members: Vec<MemberView>,
    pub events: Vec<EventView>,
    pub next_cursor: Option<String>,
    pub has_earlier: bool,
    pub total_count: i64,
}

/// Project one conversation summary onto the mobile card shape.
#[must_use]
pub fn project_card(summary: &ConversationSummary) -> ChatCard {
    ChatCard {
        id: summary.id.clone(),
        title: summary.title.clone(),
        archived: summary.archived,
        pinned: summary.pinned,
        is_group: summary.is_group,
        revision: summary.revision,
        updated_at_unix_ms: summary.updated_at_unix_ms,
        membership_count: summary.membership_count,
        event_count: summary.event_count,
    }
}

/// Project the chat list, bounded by the client's own search result limit.
#[must_use]
pub fn project_chat_list(
    policy: ClientResourcePolicy,
    summaries: &[ConversationSummary],
) -> ChatList {
    let limit = policy.bounds().search_result_limit;
    ChatList {
        conversations: summaries.iter().take(limit).map(project_card).collect(),
        truncated: summaries.len() > limit,
    }
}

/// Project one aggregate and one event page onto the mobile thread shape.
///
/// The page is bounded by the client's own history page size, so the mobile
/// thread and the desktop history read the same bound.
#[must_use]
pub fn project_thread(
    policy: ClientResourcePolicy,
    conversation: &Conversation,
    page: &EventPage,
) -> ThreadView {
    let limit = policy.bounds().history_page_size;
    ThreadView {
        conversation_id: conversation.id.clone(),
        title: conversation.title.clone(),
        is_group: conversation.is_group,
        archived: conversation.archived,
        revision: conversation.revision,
        members: conversation
            .memberships
            .iter()
            .map(|membership| MemberView {
                membership_id: membership.id.clone(),
                principal_id: membership.principal.id.clone(),
                display_name: membership.principal.display_name.clone(),
                kind: membership.principal.kind,
                access: membership.access,
                status: membership.status,
            })
            .collect(),
        events: page.events.iter().take(limit).map(project_event).collect(),
        next_cursor: page.next_cursor.clone(),
        has_earlier: page.has_earlier,
        total_count: page.total_count,
    }
}

/// Project one canonical event: identity, ordering and its text part.
#[must_use]
pub fn project_event(event: &licoup_conversation::ConversationEvent) -> EventView {
    EventView {
        id: event.id.clone(),
        sequence: event.sequence,
        author_membership_id: event.author_membership_id.clone(),
        kind: event.kind,
        created_at_unix_ms: event.created_at_unix_ms,
        finalized: event.finalized,
        text: event
            .parts
            .iter()
            .find(|part| part.kind == EventPartKind::Text && !part.content.trim().is_empty())
            .map(|part| part.content.clone()),
    }
}
