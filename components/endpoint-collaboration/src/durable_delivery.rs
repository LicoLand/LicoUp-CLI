//! The package's durable delivery store over the kernel's consumer-owned ports.
//!
//! The pinned SDK owns the protocol: it commits one delivery unit together with
//! the protocol snapshot and hands that same unit back when it is re-driven or
//! settled. `licoup_endpoint_core` declares the caller-owned port that carries
//! those units ([`AtomicState`]), and this module is the endpoint collaboration
//! package's implementation of it.
//!
//! Three rules it exists to hold:
//!
//! * **Old-or-new.** One commit applies its snapshot, its custody mutations and
//!   its delivery units together and advances exactly one revision, or it applies
//!   nothing. A commit that names a revision the store is not at is refused as a
//!   conflict without touching anything.
//! * **A failed delivery is never a settlement.** [`may_settle`] is the one rule
//!   the transport outcome is read through: only `Accepted` permits removing an
//!   obligation. A rejected, transient or ambiguous attempt leaves the unit
//!   durable, which is what makes the re-drive the SDK performs the *same*
//!   delivery rather than a fresh one.
//! * **Nothing is discarded by age or by count.** The store has no expiry and no
//!   retry ceiling: a pending unit leaves it only through [`AtomicState::settle`],
//!   and that removes exactly the unit it was given.
//!
//! The store keeps the protocol layer's own values: [`AtomicState::Snapshot`] and
//! [`AtomicState::Pending`] stay generic, so no second dialect of protocol state is
//! created here and nothing has to be rebuilt on the way back into the protocol.

use licoup_endpoint_core::{
    AtomicState, CustodyLifecycle, CustodyPurpose, KeyMutation, PortFailure, Revision, StateCommit,
    TransportOutcome, Versioned,
};

/// Whether one transport outcome permits settling a committed delivery unit.
///
/// Only acceptance does. In particular [`TransportOutcome::Ambiguous`] means the
/// attempt may or may not have been accepted, so the unit stays durable and is
/// re-driven unchanged.
#[must_use]
pub const fn may_settle(outcome: TransportOutcome) -> bool {
    matches!(outcome, TransportOutcome::Accepted)
}

/// The purposes a commit may adopt a staged handle for.
///
/// The protocol layer adopts a tentative private key so the next commit can use
/// it, and it deletes only handles it already adopted. A commit naming anything
/// else is refused rather than stored, because a store that keeps an impossible
/// mutation is a store that will apply it.
const ADOPTABLE_PURPOSES: &[CustodyPurpose] = &[
    CustodyPurpose::X25519Private,
    CustodyPurpose::MlKem768Private,
];

/// The purposes a commit may delete an adopted handle for.
///
/// The deleted handle is the private material a session or an encapsulation used;
/// a signing identity is not deletable through the protocol state machine, because
/// a snapshot that ends with no signing handle is not a state this protocol
/// emits.
const DELETABLE_PURPOSES: &[CustodyPurpose] = &[
    CustodyPurpose::X25519Private,
    CustodyPurpose::MlKem768Private,
    CustodyPurpose::MlKemEncapsulationEntropy,
];

/// One caller-owned durable protocol store over generic protocol state.
///
/// `Snapshot` and `Pending` are the protocol layer's own types. The store holds
/// them unchanged: it never inspects a payload, never re-identifies a unit and
/// never decides a protocol outcome.
#[derive(Clone, Debug)]
pub struct DurableDeliveryStore<Snapshot, Pending> {
    revision: Revision,
    snapshot: Snapshot,
    pending: Vec<Pending>,
}

impl<Snapshot, Pending> DurableDeliveryStore<Snapshot, Pending> {
    /// An empty store at the initial revision.
    pub const fn new(snapshot: Snapshot) -> Self {
        Self {
            revision: Revision::initial(),
            snapshot,
            pending: Vec::new(),
        }
    }

    /// The store a previous run committed.
    ///
    /// It is the port's own restored value handed back unchanged, so recovery
    /// cannot invent a revision or drop a unit on the way in.
    #[must_use]
    pub fn restored(revision: Revision, snapshot: Snapshot, pending: Vec<Pending>) -> Self {
        Self {
            revision,
            snapshot,
            pending,
        }
    }

    #[must_use]
    pub const fn revision(&self) -> Revision {
        self.revision
    }

    /// The committed units this store still owes a boundary for.
    #[must_use]
    pub fn pending(&self) -> &[Pending] {
        &self.pending
    }
}

impl<Snapshot: Clone, Pending: Clone + Eq> AtomicState for DurableDeliveryStore<Snapshot, Pending> {
    type Snapshot = Snapshot;
    type Pending = Pending;

    fn load(&self) -> Result<Versioned<Self::Snapshot, Self::Pending>, PortFailure> {
        Ok(Versioned::restored(
            self.revision,
            self.snapshot.clone(),
            self.pending.clone(),
        ))
    }

    fn compare_and_swap(
        &mut self,
        expected: Revision,
        commit: StateCommit<Self::Snapshot, Self::Pending>,
    ) -> Result<Revision, PortFailure> {
        if expected != self.revision {
            return Err(PortFailure::Conflict);
        }
        validate_key_mutations(commit.key_mutations())?;
        let next = commit.into_versioned(expected)?;
        // The commit is applied whole: the snapshot, the delivery units it carries
        // and the revision move together, which is what the port promises a caller
        // whose protocol state and custody are coupled.
        self.snapshot = next.state().clone();
        self.pending = next.pending().to_vec();
        self.revision = next.revision();
        Ok(self.revision)
    }

    fn settle(
        &mut self,
        revision: Revision,
        pending: &Self::Pending,
    ) -> Result<Revision, PortFailure> {
        if revision != self.revision {
            return Err(PortFailure::Conflict);
        }
        let Some(index) = self.pending.iter().position(|item| item == pending) else {
            // The unit is not in this store: settling it would remove a different
            // obligation or silently claim a delivery nobody committed.
            return Err(PortFailure::Unavailable);
        };
        let next = revision.successor()?;
        self.pending.remove(index);
        self.revision = next;
        Ok(self.revision)
    }
}

/// Refuse a custody mutation the protocol layer never emits.
///
/// Adoption is tentative (`Staged`) and deletion is adopted: a commit that says
/// otherwise is an impossible mutation, and a store that keeps it would apply it
/// at the next recovery.
fn validate_key_mutations(mutations: &[KeyMutation]) -> Result<(), PortFailure> {
    for mutation in mutations {
        let accepted = match mutation {
            KeyMutation::Adopt(handle) => {
                handle.lifecycle() == CustodyLifecycle::Staged
                    && ADOPTABLE_PURPOSES.contains(&handle.purpose())
            }
            KeyMutation::Delete(handle) => {
                handle.lifecycle() == CustodyLifecycle::Adopted
                    && DELETABLE_PURPOSES.contains(&handle.purpose())
            }
        };
        if !accepted {
            return Err(PortFailure::Unavailable);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{DurableDeliveryStore, may_settle};
    use licoup_endpoint_core::{
        AtomicState, CustodyHandle, CustodyLifecycle, CustodyPurpose, KeyMutation, PortFailure,
        Revision, StateCommit, TransportOutcome,
    };

    /// One committed delivery unit, in this test's own vocabulary: the store stays
    /// generic over what the protocol layer commits.
    #[derive(Clone, Debug, Eq, PartialEq)]
    struct Unit(u128);

    fn store() -> DurableDeliveryStore<&'static str, Unit> {
        DurableDeliveryStore::new("snapshot")
    }

    fn commit(units: Vec<Unit>) -> StateCommit<&'static str, Unit> {
        StateCommit::new("next", Vec::new(), units)
    }

    #[test]
    fn one_commit_applies_snapshot_units_and_revision_together() {
        let mut store = store();

        let revision = store
            .compare_and_swap(Revision::initial(), commit(vec![Unit(1), Unit(2)]))
            .expect("the initial revision is current");

        assert_eq!(revision, Revision::from_value(1));
        assert_eq!(store.pending().to_vec(), vec![Unit(1), Unit(2)]);
        assert_eq!(store.load().unwrap().state(), &"next");
    }

    #[test]
    fn a_commit_at_a_stale_revision_applies_nothing() {
        let mut store = store();
        store
            .compare_and_swap(Revision::initial(), commit(vec![Unit(1)]))
            .expect("committed");

        assert_eq!(
            store.compare_and_swap(Revision::initial(), commit(vec![Unit(9)])),
            Err(PortFailure::Conflict)
        );
        assert_eq!(
            store.pending().to_vec(),
            vec![Unit(1)],
            "the stale commit changed nothing"
        );
        assert_eq!(store.load().unwrap().state(), &"next");
        assert_eq!(store.revision(), Revision::from_value(1));
    }

    #[test]
    fn only_acceptance_settles_a_committed_delivery() {
        assert!(may_settle(TransportOutcome::Accepted));
        for outcome in [
            TransportOutcome::Rejected,
            TransportOutcome::Transient,
            TransportOutcome::Ambiguous,
        ] {
            assert!(
                !may_settle(outcome),
                "{outcome:?} must leave the committed unit durable"
            );
        }

        let mut store = store();
        store
            .compare_and_swap(Revision::initial(), commit(vec![Unit(4)]))
            .expect("committed");
        // A failed attempt is not a settlement: the unit is still owed.
        assert_eq!(store.pending().to_vec(), vec![Unit(4)]);
    }

    #[test]
    fn settling_removes_exactly_the_unit_it_was_given() {
        let mut store = store();
        store
            .compare_and_swap(Revision::initial(), commit(vec![Unit(1), Unit(2), Unit(3)]))
            .expect("committed");

        let revision = store
            .settle(Revision::from_value(1), &Unit(2))
            .expect("the unit is committed here");

        assert_eq!(revision, Revision::from_value(2));
        assert_eq!(store.pending().to_vec(), vec![Unit(1), Unit(3)]);
    }

    #[test]
    fn settling_a_unit_this_store_does_not_hold_is_refused() {
        let mut store = store();
        store
            .compare_and_swap(Revision::initial(), commit(vec![Unit(1)]))
            .expect("committed");

        assert_eq!(
            store.settle(Revision::from_value(1), &Unit(7)),
            Err(PortFailure::Unavailable)
        );
        assert_eq!(store.pending().to_vec(), vec![Unit(1)]);
        assert_eq!(store.revision(), Revision::from_value(1));
    }

    #[test]
    fn a_settlement_at_a_stale_revision_applies_nothing() {
        let mut store = store();
        store
            .compare_and_swap(Revision::initial(), commit(vec![Unit(1)]))
            .expect("committed");

        assert_eq!(
            store.settle(Revision::initial(), &Unit(1)),
            Err(PortFailure::Conflict)
        );
        assert_eq!(store.pending().to_vec(), vec![Unit(1)]);
    }

    #[test]
    fn a_restored_store_reports_the_revision_and_units_it_was_given() {
        let store = DurableDeliveryStore::restored(
            Revision::from_value(12),
            "restored",
            vec![Unit(5), Unit(6)],
        );

        let loaded = store.load().expect("a durable store loads");
        assert_eq!(loaded.revision(), Revision::from_value(12));
        assert_eq!(loaded.state(), &"restored");
        assert_eq!(loaded.pending().to_vec(), vec![Unit(5), Unit(6)]);
    }

    #[test]
    fn an_impossible_custody_mutation_is_refused_without_applying_the_commit() {
        let mut store = store();
        let impossible = StateCommit::new(
            "next",
            vec![KeyMutation::Adopt(CustodyHandle::adopted(
                1,
                CustodyPurpose::Ed25519Signing,
            ))],
            vec![Unit(1)],
        );

        assert_eq!(
            store.compare_and_swap(Revision::initial(), impossible),
            Err(PortFailure::Unavailable)
        );
        assert_eq!(store.revision(), Revision::initial());
        assert!(store.pending().is_empty());
        assert_eq!(store.load().unwrap().state(), &"snapshot");
    }

    #[test]
    fn a_tentative_adoption_and_an_adopted_deletion_are_accepted() {
        let mut store = store();
        let legitimate = StateCommit::new(
            "next",
            vec![
                KeyMutation::Adopt(CustodyHandle::staged(2, CustodyPurpose::X25519Private)),
                KeyMutation::Delete(CustodyHandle::adopted(3, CustodyPurpose::MlKem768Private)),
            ],
            vec![Unit(1)],
        );

        assert_eq!(
            store.compare_and_swap(Revision::initial(), legitimate),
            Ok(Revision::from_value(1))
        );
        assert_eq!(store.pending().to_vec(), vec![Unit(1)]);
    }

    #[test]
    fn deleting_a_staged_handle_is_refused() {
        let mut store = store();
        let impossible = StateCommit::new(
            "next",
            vec![KeyMutation::Delete(CustodyHandle::staged(
                4,
                CustodyPurpose::X25519Private,
            ))],
            Vec::new(),
        );

        assert_eq!(
            store.compare_and_swap(Revision::initial(), impossible),
            Err(PortFailure::Unavailable)
        );
        assert_eq!(store.revision(), Revision::initial());
    }

    #[test]
    fn the_revision_bound_is_refused_before_anything_is_applied() {
        let mut store = DurableDeliveryStore::restored(
            Revision::from_value(licoup_endpoint_core::MAX_SAFE_INTEGER),
            "snapshot",
            vec![Unit(1)],
        );

        assert_eq!(
            store.compare_and_swap(
                Revision::from_value(licoup_endpoint_core::MAX_SAFE_INTEGER),
                commit(vec![Unit(2)])
            ),
            Err(PortFailure::BoundExceeded)
        );
        assert_eq!(store.pending().to_vec(), vec![Unit(1)]);
    }

    #[test]
    fn a_delete_of_a_signing_handle_is_refused() {
        assert_eq!(
            super::validate_key_mutations(&[KeyMutation::Delete(CustodyHandle::adopted(
                9,
                CustodyPurpose::MlDsa65Signing,
            ))]),
            Err(PortFailure::Unavailable)
        );
        assert_eq!(
            super::validate_key_mutations(&[KeyMutation::Delete(CustodyHandle::adopted(
                9,
                CustodyPurpose::Ed25519Signing,
            ))]),
            Err(PortFailure::Unavailable)
        );
        assert_eq!(CustodyLifecycle::Adopted, CustodyLifecycle::Adopted);
    }
}
