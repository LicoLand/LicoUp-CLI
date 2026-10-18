//! The four peer facts, reported separately and never collapsed.
//!
//! One admitted peer message produces exactly one conversation admission, and
//! that admission says nothing about the other three facts:
//!
//! * **delivery** — the peer endpoint accepted the logical message; the pinned
//!   SDK's `note_delivery` entry owns this decision;
//! * **read** — client-owned local read state; the pinned SDK carries no read
//!   receipt, so only the local user can move this fact;
//! * **work acceptance** — the peer completed the task; the pinned SDK's
//!   `note_acceptance` entry owns this decision and only completes with the
//!   expected result digest;
//! * **admission** — the message was accepted into the Canonical Conversation
//!   by the existing owner.
//!
//! A station hint is not one of these facts and cannot produce any of them.

use std::collections::{BTreeMap, VecDeque};

/// Largest number of messages whose facts are tracked at once.
pub const MAX_TRACKED_FACTS: usize = 4096;

/// Whether the peer endpoint accepted the logical message.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DeliveryFact {
    Unreported,
    EndpointAccepted,
}

/// Whether the local user read the message.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ReadFact {
    Unread,
    ReadLocally,
}

/// Whether the peer completed the task.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AcceptanceFact {
    Unreported,
    EffectCompleted { result_digest: [u8; 32] },
}

impl AcceptanceFact {
    #[must_use]
    pub const fn is_completed(self) -> bool {
        matches!(self, Self::EffectCompleted { .. })
    }
}

/// What happened to the message at conversation admission.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum AdmissionFact {
    Admitted {
        event_id: String,
        sequence: i64,
    },
    Duplicate {
        event_id: Option<String>,
        sequence: Option<i64>,
    },
}

impl AdmissionFact {
    #[must_use]
    pub fn event_id(&self) -> Option<&str> {
        match self {
            Self::Admitted { event_id, .. } => Some(event_id),
            Self::Duplicate { event_id, .. } => event_id.as_deref(),
        }
    }

    #[must_use]
    pub const fn is_duplicate(&self) -> bool {
        matches!(self, Self::Duplicate { .. })
    }
}

/// The three independent facts of one message, in one record.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PeerFacts {
    pub delivery: DeliveryFact,
    pub read: ReadFact,
    pub acceptance: AcceptanceFact,
}

impl PeerFacts {
    #[must_use]
    pub const fn unreported() -> Self {
        Self {
            delivery: DeliveryFact::Unreported,
            read: ReadFact::Unread,
            acceptance: AcceptanceFact::Unreported,
        }
    }
}

/// Bounded per-message fact state for one ingress.
///
/// Recording a fact for a message the ingress never admitted is refused, so a
/// caller cannot attach a delivery or acceptance claim to an unknown identity.
#[derive(Debug, Default)]
pub struct PeerFactLedger {
    entries: BTreeMap<[u8; 16], PeerFacts>,
    order: VecDeque<[u8; 16]>,
}

impl PeerFactLedger {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Records one admitted message with every fact still unreported.
    pub fn on_admitted(&mut self, message_id: [u8; 16]) {
        if self.entries.contains_key(&message_id) {
            return;
        }
        if self.entries.len() == MAX_TRACKED_FACTS
            && let Some(oldest) = self.order.pop_front()
        {
            self.entries.remove(&oldest);
        }
        self.entries.insert(message_id, PeerFacts::unreported());
        self.order.push_back(message_id);
    }

    #[must_use]
    pub fn get(&self, message_id: [u8; 16]) -> Option<PeerFacts> {
        self.entries.get(&message_id).copied()
    }

    /// Records endpoint delivery. Unknown messages are refused.
    pub fn record_delivery(&mut self, message_id: [u8; 16], fact: DeliveryFact) -> bool {
        let Some(entry) = self.entries.get_mut(&message_id) else {
            return false;
        };
        if entry.delivery == DeliveryFact::EndpointAccepted && fact == DeliveryFact::Unreported {
            return false;
        }
        entry.delivery = fact;
        true
    }

    /// Records the local user's read state. Unknown messages are refused.
    pub fn mark_read(&mut self, message_id: [u8; 16]) -> bool {
        let Some(entry) = self.entries.get_mut(&message_id) else {
            return false;
        };
        entry.read = ReadFact::ReadLocally;
        true
    }

    /// Records task acceptance. Unknown messages are refused, and an acceptance
    /// without a completed result digest does not exist as a value.
    pub fn record_acceptance(&mut self, message_id: [u8; 16], fact: AcceptanceFact) -> bool {
        let Some(entry) = self.entries.get_mut(&message_id) else {
            return false;
        };
        if entry.acceptance.is_completed() && !fact.is_completed() {
            return false;
        }
        entry.acceptance = fact;
        true
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}
