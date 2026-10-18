//! Conversation admission for verified peer messages.
//!
//! The ingress is an adapter, not a second owner. It resolves the verified
//! author/device to an existing membership through the host's
//! [`PeerBindings`], checks that membership against the Canonical Conversation
//! the host already owns, and appends the message through that owner's own
//! store door. It creates no conversation, no principal, no membership, and no
//! administrator, and it never executes an effect: a structured command request
//! is carried as a [`PeerEffectIntent`] for the single-owner application entry.
//!
//! The four facts stay separate in every report. Admission into the
//! conversation does not imply endpoint delivery, a local read, or task
//! acceptance; a station hint implies none of them.

use std::collections::{BTreeMap, VecDeque};
use std::sync::Arc;

use licoup_application::ActorClaim;
use licoup_conversation::{
    ConversationStore, EventAppendOutcome, EventKind, EventPartKind, MembershipStatus, NewEventPart,
};

use crate::domain::mobile_relay::endpoint_transport::{
    DedupeOutcome, IntakeLedger, PeerMessage, PeerOrigin, PeerPart, StationHint,
};

use super::bindings::{PeerBinding, PeerBindings};
use super::facts::{
    AcceptanceFact, AdmissionFact, DeliveryFact, PeerFactLedger, PeerFacts, ReadFact,
};

/// Largest number of peer logical ids whose admitted event id is remembered for
/// causal placement.
pub const MAX_CAUSAL_LINKS: usize = 4096;

/// One refused admission. Shape and ACL refusals only; no protocol decision is
/// taken here, and nothing established is destroyed by a refusal.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PeerIngressRefusal {
    /// The verified author/device is not bound to any local membership.
    UnboundPeer,
    /// The bound conversation does not exist locally.
    ConversationMissing,
    /// The bound membership does not exist in that conversation.
    MembershipMissing,
    /// The bound membership is no longer active (left or revoked).
    MembershipInactive,
    /// The conversation owner refused or could not perform the write.
    StoreUnavailable,
}

impl PeerIngressRefusal {
    #[must_use]
    pub const fn code(self) -> &'static str {
        match self {
            Self::UnboundPeer => "peer_unbound",
            Self::ConversationMissing => "peer_conversation_missing",
            Self::MembershipMissing => "peer_membership_missing",
            Self::MembershipInactive => "peer_membership_inactive",
            Self::StoreUnavailable => "peer_store_unavailable",
        }
    }
}

impl core::fmt::Display for PeerIngressRefusal {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter.write_str(self.code())
    }
}

impl std::error::Error for PeerIngressRefusal {}

/// One structured command request, offered to the single-owner entry.
///
/// The claim is always a membership claim of the bound peer; it is never a
/// local administrator and never a synthetic principal. The request is the
/// typed command's own object, decoded by the application contract before any
/// effect runs.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PeerEffectIntent {
    pub claim: ActorClaim,
    pub command_request: serde_json::Value,
    pub message_id: [u8; 16],
}

/// The outcome of one admission attempt.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PeerIngressReport {
    message_id: [u8; 16],
    origin: PeerOrigin,
    station: Option<StationHint>,
    admission: AdmissionFact,
    delivery: DeliveryFact,
    read: ReadFact,
    acceptance: AcceptanceFact,
    effect_intents: Vec<PeerEffectIntent>,
}

impl PeerIngressReport {
    #[must_use]
    pub const fn message_id(&self) -> [u8; 16] {
        self.message_id
    }

    #[must_use]
    pub const fn origin(&self) -> &PeerOrigin {
        &self.origin
    }

    #[must_use]
    pub const fn station(&self) -> Option<StationHint> {
        self.station
    }

    #[must_use]
    pub const fn admission(&self) -> &AdmissionFact {
        &self.admission
    }

    #[must_use]
    pub const fn delivery(&self) -> DeliveryFact {
        self.delivery
    }

    #[must_use]
    pub const fn read(&self) -> ReadFact {
        self.read
    }

    #[must_use]
    pub const fn acceptance(&self) -> AcceptanceFact {
        self.acceptance
    }

    /// The structured commands this message asked for. Empty unless the message
    /// carried explicit command parts; a duplicate carries none, because merging
    /// a message is not a second execution grant.
    #[must_use]
    pub fn effect_intents(&self) -> &[PeerEffectIntent] {
        &self.effect_intents
    }
}

/// The local event one admitted peer message became.
#[derive(Clone, Debug, Eq, PartialEq)]
struct AdmittedLink {
    event_id: String,
    sequence: i64,
}

/// The stateful peer ingress: bindings, dedupe windows, causal links, facts.
///
/// `RecordKey` is the SDK boundary's own replay identity of one committed
/// protected record, carried through from the transport mapping.
pub struct PeerIngress<RecordKey> {
    bindings: Arc<dyn PeerBindings>,
    ledger: IntakeLedger<RecordKey>,
    facts: PeerFactLedger,
    admitted: BTreeMap<[u8; 16], AdmittedLink>,
    order: VecDeque<[u8; 16]>,
}

impl<RecordKey: Clone + Eq> PeerIngress<RecordKey> {
    #[must_use]
    pub fn new(bindings: Arc<dyn PeerBindings>) -> Self {
        Self {
            bindings,
            ledger: IntakeLedger::new(),
            facts: PeerFactLedger::new(),
            admitted: BTreeMap::new(),
            order: VecDeque::new(),
        }
    }

    #[must_use]
    pub fn facts(&self) -> &PeerFactLedger {
        &self.facts
    }

    /// The client-owned fact state, so delivery/read/acceptance can be recorded
    /// on their own without touching the admission or the conversation.
    pub fn facts_mut(&mut self) -> &mut PeerFactLedger {
        &mut self.facts
    }

    /// The conversation event one admitted peer message became, when known.
    #[must_use]
    pub fn admitted_event(&self, message_id: [u8; 16]) -> Option<&str> {
        self.admitted
            .get(&message_id)
            .map(|link| link.event_id.as_str())
    }

    /// Admits one verified peer message into the existing conversation.
    ///
    /// Fail-closed order: bind, then conversation, then active membership, then
    /// dedupe, then the owner's write. A refusal before the write leaves the
    /// store untouched and the dedupe window unrecorded, so a corrected retry
    /// is possible.
    ///
    /// The consumed verification is recorded through the Canonical Conversation
    /// owner itself, under this message's own logical identity, before the row
    /// it authorizes can appear. That record is durable and travels in the same
    /// transaction as the row, so a resend is refused by an ingress that has
    /// never seen the original — after a restart, or in another process — and a
    /// failed write leaves no record behind. The in-memory window is only a
    /// fast path in front of it.
    pub fn admit(
        &mut self,
        store: &ConversationStore,
        message: &PeerMessage<RecordKey>,
    ) -> Result<PeerIngressReport, PeerIngressRefusal> {
        let origin = message.origin().clone();
        let binding = self
            .bindings
            .resolve(&origin.author(), &origin.device())
            .ok_or(PeerIngressRefusal::UnboundPeer)?;
        let conversation = store
            .get(&binding.conversation_id)
            .map_err(|_| PeerIngressRefusal::ConversationMissing)?;
        let membership = conversation
            .memberships
            .iter()
            .find(|membership| membership.id == binding.membership_id)
            .ok_or(PeerIngressRefusal::MembershipMissing)?;
        if membership.status != MembershipStatus::Active {
            return Err(PeerIngressRefusal::MembershipInactive);
        }

        if self
            .ledger
            .classify(message.replay_identity(), message.logical_id())
            == DedupeOutcome::Duplicate
        {
            let link = self.known_link(message.logical_id());
            return Ok(self.duplicate_report(message, origin, link));
        }

        let mut parts = Vec::with_capacity(message.parts().len());
        let mut effect_intents = Vec::new();
        for part in message.parts() {
            match part {
                PeerPart::Text(content) => parts.push(NewEventPart {
                    id: String::new(),
                    kind: EventPartKind::Text,
                    content: content.clone(),
                }),
                PeerPart::Command(request) => {
                    parts.push(NewEventPart {
                        id: String::new(),
                        kind: EventPartKind::Metadata,
                        content: request.to_string(),
                    });
                    effect_intents.push(effect_intent(&binding, message, request));
                }
            }
        }

        let (causation_id, correlation_id) = self.causal_placement(message);
        let outcome = store
            .append_event_once(
                &binding.conversation_id,
                Some(&binding.membership_id),
                EventKind::Message,
                &parts,
                causation_id.as_deref(),
                correlation_id.as_deref(),
                true,
                &peer_admission_key(message.logical_id()),
            )
            .map_err(|_| PeerIngressRefusal::StoreUnavailable)?;
        let event = match outcome {
            EventAppendOutcome::Appended(event) => event,
            // The key was consumed by an admission this ingress may not have
            // performed. The owner names the Event that consumed it, so the
            // merge reports the effect that already exists.
            EventAppendOutcome::AlreadyAdmitted { event_id, sequence } => {
                self.ledger
                    .record(message.replay_identity().clone(), message.logical_id());
                return Ok(self.duplicate_report(message, origin, (event_id, sequence)));
            }
        };

        self.ledger
            .record(message.replay_identity().clone(), message.logical_id());
        self.remember_link(
            message.logical_id(),
            AdmittedLink {
                event_id: event.id.clone(),
                sequence: event.sequence,
            },
        );
        self.facts.on_admitted(message.logical_id());

        let facts = self
            .facts
            .get(message.logical_id())
            .unwrap_or_else(PeerFacts::unreported);
        Ok(PeerIngressReport {
            message_id: message.logical_id(),
            origin,
            station: message.station(),
            admission: AdmissionFact::Admitted {
                event_id: event.id,
                sequence: event.sequence,
            },
            delivery: facts.delivery,
            read: facts.read,
            acceptance: facts.acceptance,
            effect_intents,
        })
    }

    /// The local Event one already-admitted message became, when this ingress
    /// still remembers it.
    fn known_link(&self, message_id: [u8; 16]) -> (Option<String>, Option<i64>) {
        match self.admitted.get(&message_id) {
            Some(link) => (Some(link.event_id.clone()), Some(link.sequence)),
            None => (None, None),
        }
    }

    fn duplicate_report(
        &self,
        message: &PeerMessage<RecordKey>,
        origin: PeerOrigin,
        admitted: (Option<String>, Option<i64>),
    ) -> PeerIngressReport {
        let facts = self
            .facts
            .get(message.logical_id())
            .unwrap_or_else(PeerFacts::unreported);
        PeerIngressReport {
            message_id: message.logical_id(),
            origin,
            station: message.station(),
            admission: AdmissionFact::Duplicate {
                event_id: admitted.0,
                sequence: admitted.1,
            },
            delivery: facts.delivery,
            read: facts.read,
            acceptance: facts.acceptance,
            effect_intents: Vec::new(),
        }
    }

    fn causal_placement(
        &self,
        message: &PeerMessage<RecordKey>,
    ) -> (Option<String>, Option<String>) {
        match message.relates_to() {
            // A reply always shares the thread of the message it answers, even
            // when that message has not arrived yet; the local cause can only be
            // named once the parent's event exists.
            Some(parent) => (
                self.admitted.get(&parent).map(|link| link.event_id.clone()),
                Some(peer_correlation(parent)),
            ),
            None => (None, Some(peer_correlation(message.logical_id()))),
        }
    }

    fn remember_link(&mut self, message_id: [u8; 16], link: AdmittedLink) {
        if self.admitted.len() == MAX_CAUSAL_LINKS
            && let Some(oldest) = self.order.pop_front()
        {
            self.admitted.remove(&oldest);
        }
        self.admitted.insert(message_id, link);
        self.order.push_back(message_id);
    }
}

fn effect_intent<RecordKey>(
    binding: &PeerBinding,
    message: &PeerMessage<RecordKey>,
    request: &serde_json::Value,
) -> PeerEffectIntent {
    PeerEffectIntent {
        claim: ActorClaim::Membership {
            provider_id: binding.provider_id.clone(),
            conversation_id: Some(binding.conversation_id.clone()),
            membership_id: Some(binding.membership_id.clone()),
            parent_dispatch_id: None,
        },
        command_request: request.clone(),
        message_id: message.logical_id(),
    }
}

/// The local correlation id of one peer logical message, so a reply can find
/// its thread even when it arrives before the message it answers.
fn peer_correlation(message_id: [u8; 16]) -> String {
    use core::fmt::Write as _;

    let mut correlation = String::with_capacity(5 + message_id.len() * 2);
    correlation.push_str("peer:");
    for byte in message_id {
        let _ = write!(correlation, "{byte:02x}");
    }
    correlation
}

/// The durable identity of one peer logical message inside a Conversation.
///
/// It is the peer's own logical id held in the peer's own namespace, so it can
/// never collide with a local identity and never depends on arrival order or on
/// which protected record carried the message. The Canonical Conversation owner
/// stores it as the admission key of the Event it authorized, which is what
/// makes duplicate suppression outlive this ingress.
pub(super) fn peer_admission_key(message_id: [u8; 16]) -> String {
    peer_correlation(message_id)
}
