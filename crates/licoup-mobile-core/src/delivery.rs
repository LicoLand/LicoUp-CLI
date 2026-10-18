//! Durable protocol delivery state for the mobile entry.
//!
//! A paired mobile client sends protected envelopes and settles them later,
//! across a disconnect. Two facts therefore have to survive the process: which
//! envelopes are already delivered, and where a reconnect may resume reading.
//! This module owns exactly those two facts as a bounded record set.
//!
//! It is deliberately not a queue and not a store. The conversation rows stay
//! in the Canonical Conversation authority and the envelope bytes stay in the
//! relay; what the ledger holds is the *identity* of an envelope the caller
//! sent (`LicoArcRelayEnvelope::envelope_id`) and its settlement state. That
//! identity is what makes a resend a no-op instead of a second delivery.
//!
//! A restored ledger is checked against the same acceptance a live one is:
//! the caller rebuilds the protocol version it persisted and the frozen
//! acceptance decides whether the record set may be used at all.

use std::collections::{BTreeMap, VecDeque};

use licoup_protocol_bindings::{AcceptedVersion, ProtocolVersion, VersionRefusal};

/// The settlement state of one delivered envelope.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DeliveryState {
    /// Sent, not yet settled. A reconnect must re-drive it.
    Pending,
    /// Settled by its result or replay proof.
    Confirmed,
}

/// One envelope's delivery identity and settlement state.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DeliveryRecord {
    /// `LicoArcRelayEnvelope::envelope_id` of the delivered envelope.
    pub envelope_id: String,
    /// The Canonical Conversation the delivery belongs to.
    pub conversation_id: String,
    /// The canonical Event sequence the delivery produced, when the send has
    /// already been admitted.
    pub sequence: Option<i64>,
    pub state: DeliveryState,
}

impl DeliveryRecord {
    #[must_use]
    pub fn is_pending(&self) -> bool {
        self.state == DeliveryState::Pending
    }
}

/// Why one delivery operation was refused.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DeliveryRefusal {
    /// This envelope identity is already recorded. A resend merges; it never
    /// becomes a second delivery.
    DuplicateEnvelope,
    /// No record carries this envelope identity.
    UnknownEnvelope,
    /// The bound was reached and only pending records remain, so nothing may
    /// be evicted without losing delivery state.
    CapacityExceeded,
    /// The restored protocol version is not the accepted one.
    VersionRefused(VersionRefusal),
    /// A confirmed delivery cannot be re-confirmed with a different sequence.
    SequenceConflict,
}

impl DeliveryRefusal {
    /// The stable wire label, so a caller reads one code per refusal.
    #[must_use]
    pub const fn code(self) -> &'static str {
        match self {
            Self::DuplicateEnvelope => "mobile_delivery_duplicate_envelope",
            Self::UnknownEnvelope => "mobile_delivery_unknown_envelope",
            Self::CapacityExceeded => "mobile_delivery_capacity_exceeded",
            Self::VersionRefused(_) => "mobile_delivery_protocol_version_refused",
            Self::SequenceConflict => "mobile_delivery_sequence_conflict",
        }
    }
}

/// The bounded delivery ledger one mobile client keeps.
///
/// Records are keyed by envelope identity, and eviction only ever drops the
/// oldest *confirmed* record: a pending delivery is never forgotten, so a
/// reconnect cannot silently skip work.
#[derive(Debug)]
pub struct DeliveryLedger {
    capacity: usize,
    order: VecDeque<String>,
    records: BTreeMap<String, DeliveryRecord>,
}

impl DeliveryLedger {
    /// A ledger for one live session, over an already-accepted version.
    #[must_use]
    pub fn live(capacity: usize) -> Self {
        Self {
            capacity: capacity.max(1),
            order: VecDeque::new(),
            records: BTreeMap::new(),
        }
    }

    /// A ledger restored from the records a previous process persisted.
    ///
    /// The caller rebuilds the protocol version record it wrote and this
    /// checks it against the frozen acceptance before any restored record is
    /// used, so a record set produced under another Line is refused whole
    /// rather than partially trusted.
    pub fn restored(
        capacity: usize,
        wire_id: &str,
        generation: u64,
        protocol_line_id: [u8; 32],
        protection_profile_id: [u8; 32],
    ) -> Result<Self, DeliveryRefusal> {
        let version =
            ProtocolVersion::restored(wire_id, generation, protocol_line_id, protection_profile_id);
        AcceptedVersion::fixed()
            .check(&version)
            .map_err(DeliveryRefusal::VersionRefused)?;
        Ok(Self::live(capacity))
    }

    /// Record one sent envelope. A duplicate identity is refused so the caller
    /// re-drives the existing delivery instead of creating a second one.
    pub fn record_sent(
        &mut self,
        envelope_id: &str,
        conversation_id: &str,
        sequence: Option<i64>,
    ) -> Result<(), DeliveryRefusal> {
        if self.records.contains_key(envelope_id) {
            return Err(DeliveryRefusal::DuplicateEnvelope);
        }
        if self.records.len() >= self.capacity {
            self.evict_oldest_confirmed()?;
        }
        self.order.push_back(envelope_id.to_owned());
        self.records.insert(
            envelope_id.to_owned(),
            DeliveryRecord {
                envelope_id: envelope_id.to_owned(),
                conversation_id: conversation_id.to_owned(),
                sequence,
                state: DeliveryState::Pending,
            },
        );
        Ok(())
    }

    /// Settle one recorded envelope.
    ///
    /// Re-confirming an already-confirmed delivery with the same sequence is
    /// idempotent; naming a different sequence is a conflict, because two
    /// sequences for one envelope is the duplicated delivery this ledger
    /// exists to prevent.
    pub fn confirm(&mut self, envelope_id: &str, sequence: i64) -> Result<(), DeliveryRefusal> {
        let record = self
            .records
            .get_mut(envelope_id)
            .ok_or(DeliveryRefusal::UnknownEnvelope)?;
        if record.state == DeliveryState::Confirmed && record.sequence != Some(sequence) {
            return Err(DeliveryRefusal::SequenceConflict);
        }
        record.sequence = Some(sequence);
        record.state = DeliveryState::Confirmed;
        Ok(())
    }

    /// The sequence a reconnect may resume reading after, for one conversation.
    ///
    /// It is the highest sequence whose delivery is confirmed with no older
    /// delivery still pending. Resuming there can repeat nothing that is not
    /// already settled and can skip nothing that is not settled, which is what
    /// makes a reconnect safe in both directions.
    #[must_use]
    pub fn resume_after(&self, conversation_id: &str) -> Option<i64> {
        let mut oldest_pending: Option<i64> = None;
        for record in self.records.values() {
            if record.conversation_id != conversation_id || !record.is_pending() {
                continue;
            }
            let Some(sequence) = record.sequence else {
                // A pending delivery with no admitted sequence names no place
                // to resume from, so the whole conversation waits for it.
                return None;
            };
            oldest_pending = Some(match oldest_pending {
                Some(current) => current.min(sequence),
                None => sequence,
            });
        }
        let mut highest_confirmed: Option<i64> = None;
        for record in self.records.values() {
            if record.conversation_id != conversation_id || record.state != DeliveryState::Confirmed
            {
                continue;
            }
            let Some(sequence) = record.sequence else {
                continue;
            };
            if oldest_pending.is_some_and(|pending| sequence >= pending) {
                continue;
            }
            highest_confirmed = Some(match highest_confirmed {
                Some(current) => current.max(sequence),
                None => sequence,
            });
        }
        highest_confirmed
    }

    /// Every record, in insertion order. This is what a caller persists.
    #[must_use]
    pub fn records(&self) -> Vec<DeliveryRecord> {
        self.order
            .iter()
            .filter_map(|envelope_id| self.records.get(envelope_id).cloned())
            .collect()
    }

    /// The records still awaiting settlement.
    #[must_use]
    pub fn pending(&self) -> Vec<DeliveryRecord> {
        self.records()
            .into_iter()
            .filter(DeliveryRecord::is_pending)
            .collect()
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.records.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.records.is_empty()
    }

    fn evict_oldest_confirmed(&mut self) -> Result<(), DeliveryRefusal> {
        let evictable = self.order.iter().position(|envelope_id| {
            self.records
                .get(envelope_id)
                .is_some_and(|record| !record.is_pending())
        });
        let Some(position) = evictable else {
            return Err(DeliveryRefusal::CapacityExceeded);
        };
        if let Some(envelope_id) = self.order.remove(position) {
            self.records.remove(&envelope_id);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use licoup_protocol_bindings::{
        ACCEPTED_GENERATION, ACCEPTED_PROTECTION_PROFILE_ID, ACCEPTED_PROTOCOL_LINE_ID,
        ACCEPTED_WIRE_ID,
    };

    #[test]
    fn a_resend_is_refused_instead_of_becoming_a_second_delivery() {
        let mut ledger = DeliveryLedger::live(8);
        ledger
            .record_sent("envelope:1", "conversation:1", Some(1))
            .expect("the first send is recorded");
        assert_eq!(
            ledger.record_sent("envelope:1", "conversation:1", Some(1)),
            Err(DeliveryRefusal::DuplicateEnvelope)
        );
        assert_eq!(ledger.len(), 1);
    }

    #[test]
    fn a_reconnect_resumes_after_the_last_contiguously_confirmed_sequence() {
        let mut ledger = DeliveryLedger::live(8);
        for sequence in 1..=3 {
            ledger
                .record_sent(
                    &format!("envelope:{sequence}"),
                    "conversation:1",
                    Some(sequence),
                )
                .expect("the send is recorded");
        }
        assert_eq!(ledger.resume_after("conversation:1"), None);

        ledger.confirm("envelope:1", 1).expect("settled");
        ledger.confirm("envelope:3", 3).expect("settled");
        assert_eq!(
            ledger.resume_after("conversation:1"),
            Some(1),
            "the pending delivery at sequence 2 still blocks the cursor"
        );

        ledger.confirm("envelope:2", 2).expect("settled");
        assert_eq!(ledger.resume_after("conversation:1"), Some(3));
        assert!(ledger.pending().is_empty());
    }

    #[test]
    fn re_confirming_the_same_sequence_is_idempotent_and_a_different_one_conflicts() {
        let mut ledger = DeliveryLedger::live(4);
        ledger
            .record_sent("envelope:1", "conversation:1", None)
            .expect("recorded");
        ledger.confirm("envelope:1", 4).expect("settled");
        ledger.confirm("envelope:1", 4).expect("re-settled");
        assert_eq!(
            ledger.confirm("envelope:1", 5),
            Err(DeliveryRefusal::SequenceConflict)
        );
        assert_eq!(
            ledger.confirm("envelope:absent", 5),
            Err(DeliveryRefusal::UnknownEnvelope)
        );
    }

    #[test]
    fn eviction_drops_only_confirmed_records_and_never_a_pending_delivery() {
        let mut ledger = DeliveryLedger::live(2);
        ledger
            .record_sent("envelope:1", "conversation:1", Some(1))
            .expect("recorded");
        ledger
            .record_sent("envelope:2", "conversation:1", Some(2))
            .expect("recorded");
        assert_eq!(
            ledger.record_sent("envelope:3", "conversation:1", Some(3)),
            Err(DeliveryRefusal::CapacityExceeded)
        );

        ledger.confirm("envelope:1", 1).expect("settled");
        ledger
            .record_sent("envelope:3", "conversation:1", Some(3))
            .expect("the confirmed record is evicted first");
        assert_eq!(ledger.len(), 2);
        assert_eq!(
            ledger.pending().len(),
            2,
            "both surviving records are still pending"
        );
    }

    #[test]
    fn a_restored_ledger_is_accepted_only_for_the_accepted_protocol_line() {
        assert!(
            DeliveryLedger::restored(
                4,
                ACCEPTED_WIRE_ID,
                ACCEPTED_GENERATION,
                ACCEPTED_PROTOCOL_LINE_ID,
                ACCEPTED_PROTECTION_PROFILE_ID,
            )
            .is_ok()
        );
        assert!(matches!(
            DeliveryLedger::restored(
                4,
                ACCEPTED_WIRE_ID,
                ACCEPTED_GENERATION + 1,
                ACCEPTED_PROTOCOL_LINE_ID,
                ACCEPTED_PROTECTION_PROFILE_ID,
            ),
            Err(DeliveryRefusal::VersionRefused(
                VersionRefusal::Generation { .. }
            ))
        ));
        assert!(matches!(
            DeliveryLedger::restored(
                4,
                ACCEPTED_WIRE_ID,
                ACCEPTED_GENERATION,
                [0; 32],
                ACCEPTED_PROTECTION_PROFILE_ID,
            ),
            Err(DeliveryRefusal::VersionRefused(
                VersionRefusal::ProtocolLine
            ))
        ));
    }
}
