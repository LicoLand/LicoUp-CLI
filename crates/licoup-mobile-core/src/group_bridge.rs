//! The mobile client's canonical group application path.
//!
//! The mobile chat surface does not keep a group lifecycle of its own. Its
//! ordinary chat controls dispatch the Canonical Conversation operations —
//! create, list, read, page, post a message, add and retire a membership —
//! through the same authenticated endpoint application path every other mobile
//! operation travels, and the host that owns the Conversation authority answers
//! them. There is one group authority and this module is only the client half of
//! reaching it.
//!
//! What this module adds over [`MobileEntry`] is the *delivery* half of that
//! path. A dispatched group operation is a protected envelope with an identity,
//! sent to a host that may be unreachable when its result is produced:
//!
//! * the envelope and its request are recorded before the authority is asked,
//!   so the same identity can never become a second group operation;
//! * the envelope is settled from the authority's own answer, never from a local
//!   guess about what the authority did;
//! * a reconnect re-drives exactly the envelopes still pending and resumes
//!   reading after the highest contiguously settled sequence, so it can repeat
//!   nothing already settled and skip nothing that is not.
//!
//! The re-driven envelope reaches the same authority again under the same
//! identity; the authority is the one that decides whether it is looking at a
//! new operation or at one it has already applied. This module never invents a
//! completion, and it never appends an Event itself — the Canonical
//! Conversation authority is the only writer.

use std::collections::BTreeMap;

use anyhow::{Result, anyhow, ensure};
use serde_json::Value;

use crate::delivery::{DeliveryLedger, DeliveryRecord, DeliveryRefusal};
use crate::entry::MobileEntry;
use crate::host::MobileOperationHost;
use crate::surface::{SurfaceGroup, surface_group};

/// The canonical group operations this path dispatches.
///
/// They are the group group of [`crate::surface::MOBILE_SURFACE`], in the same
/// order, and they are the Canonical Conversation operations the desktop host
/// already routes. The fixture asserts the two lists stay one set, so a group
/// operation cannot exist here without being on the declared mobile surface.
pub const MOBILE_GROUP_OPERATIONS: &[&str] = &[
    "conversation.create",
    "conversation.list",
    "conversation.get",
    "conversation.events.page",
    "conversation.message.post",
    "conversation.membership.add",
    "conversation.membership.leave",
];

/// The parameter that names the conversation a group operation acts on.
///
/// `conversation.create` and `conversation.list` address no existing
/// conversation, so they carry no value here and the ledger records an empty
/// one: a delivery that belongs to no conversation still has an identity that
/// must not be duplicated.
pub const GROUP_CONVERSATION_PARAM: &str = "conversationId";

/// The group operations that only read the authority.
///
/// A read changes no position in the conversation, so it settles at the cursor
/// the client already had; a change must report the sequence it produced.
pub const READ_ONLY_GROUP_OPERATIONS: &[&str] = &[
    "conversation.list",
    "conversation.get",
    "conversation.events.page",
];

/// Whether one group operation only reads the authority.
#[must_use]
pub fn is_read_only_group_operation(operation: &str) -> bool {
    READ_ONLY_GROUP_OPERATIONS.contains(&operation)
}

/// Whether one operation is on the canonical group application path.
#[must_use]
pub fn is_group_operation(operation: &str) -> bool {
    surface_group(operation) == Some(SurfaceGroup::Group)
}

/// One envelope still awaiting the authority's answer, with the request that
/// must be re-driven.
#[derive(Clone, Debug, PartialEq)]
pub struct PendingDelivery {
    /// The delivery record the ledger keeps for this envelope.
    pub record: DeliveryRecord,
    /// The request this client sent for it. A re-drive sends this request again
    /// rather than composing a new one, so the operation the authority sees is
    /// the operation it was already asked for.
    pub request: Value,
}

/// What a reconnect must re-drive for one conversation, and where it may resume
/// reading.
#[derive(Clone, Debug, PartialEq)]
pub struct ReconnectPlan {
    /// The highest sequence whose delivery is settled with no older delivery
    /// still pending, or `None` when the conversation must be read from its
    /// first Event.
    pub resume_after: Option<i64>,
    /// The envelopes still awaiting the authority's answer, oldest first.
    pub pending: Vec<PendingDelivery>,
}

/// The mobile client's canonical group application path over one answering host.
pub struct MobileGroupApplication<H> {
    entry: MobileEntry<H>,
    ledger: DeliveryLedger,
    in_flight: BTreeMap<String, Value>,
}

impl<H: MobileOperationHost> MobileGroupApplication<H> {
    /// The path over the answering host, keeping at most `capacity` delivery
    /// records.
    #[must_use]
    pub fn new(host: H, capacity: usize) -> Self {
        Self {
            entry: MobileEntry::new(host),
            ledger: DeliveryLedger::live(capacity),
            in_flight: BTreeMap::new(),
        }
    }

    #[must_use]
    pub fn entry(&self) -> &MobileEntry<H> {
        &self.entry
    }

    #[must_use]
    pub fn ledger(&self) -> &DeliveryLedger {
        &self.ledger
    }

    /// Dispatch one canonical group operation as the protected envelope
    /// `envelope_id`, and settle it from the authority's answer.
    ///
    /// The envelope and its request are recorded before the authority is asked.
    /// A duplicate identity is an error rather than a second operation: the
    /// caller re-drives the existing delivery through [`Self::reconnect`], which
    /// is what makes a resend safe. An answer that refuses the operation is
    /// returned as it stands and leaves the envelope pending, because only the
    /// authority knows whether its refusal is final. An answer that admits it
    /// settles the envelope with the sequence the authority itself reported, so
    /// an answer that carries no sequence is an error instead of an invented
    /// completion.
    pub fn dispatch(&mut self, envelope_id: &str, request: &Value) -> Result<Value> {
        let action = request
            .get("action")
            .and_then(Value::as_str)
            .ok_or_else(|| anyhow!("mobile_group_invalid_request"))?;
        ensure!(
            is_group_operation(action),
            "mobile_group_operation_required"
        );
        let conversation_id = request
            .get("params")
            .and_then(|params| params.get(GROUP_CONVERSATION_PARAM))
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned();
        self.ledger
            .record_sent(envelope_id, &conversation_id, None)
            .map_err(refusal)?;
        self.in_flight
            .insert(envelope_id.to_owned(), request.clone());
        let response = self.entry.dispatch(request)?;
        self.settle_from_answer(envelope_id, action, &conversation_id, &response)?;
        Ok(response)
    }

    /// Settle a dispatched envelope from its result door.
    ///
    /// A result delivered later — the `resultSecure` or `resultReplayProof`
    /// answer of the same envelope — settles the delivery it belongs to. An
    /// envelope this client never sent is refused: this path settles its own
    /// deliveries and does not adopt someone else's.
    pub fn settle(&mut self, envelope_id: &str, sequence: i64) -> Result<()> {
        self.ledger
            .confirm(envelope_id, sequence)
            .map_err(refusal)?;
        self.in_flight.remove(envelope_id);
        Ok(())
    }

    /// What a reconnect must do for one conversation.
    #[must_use]
    pub fn reconnect(&self, conversation_id: &str) -> ReconnectPlan {
        let pending = self
            .ledger
            .pending()
            .into_iter()
            .filter(|record| record.conversation_id == conversation_id)
            .filter_map(|record| {
                self.in_flight
                    .get(&record.envelope_id)
                    .cloned()
                    .map(|request| PendingDelivery { record, request })
            })
            .collect();
        ReconnectPlan {
            resume_after: self.ledger.resume_after(conversation_id),
            pending,
        }
    }

    /// Settle one envelope from the answer the authority gave it.
    ///
    /// A read settles at the cursor the client already had, because it moved no
    /// position; a change must report the sequence it produced, so an admitting
    /// answer that carries none is an error rather than an invented completion.
    fn settle_from_answer(
        &mut self,
        envelope_id: &str,
        action: &str,
        conversation_id: &str,
        response: &Value,
    ) -> Result<()> {
        if response.get("ok").and_then(Value::as_bool) != Some(true) {
            return Ok(());
        }
        let sequence = match admitted_sequence(response) {
            Some(sequence) => sequence,
            None if is_read_only_group_operation(action) => self
                .ledger
                .resume_after(conversation_id)
                .unwrap_or_default(),
            None => return Err(anyhow!("mobile_group_answer_without_sequence")),
        };
        self.settle(envelope_id, sequence)
    }
}

/// The sequence an admitted answer reported.
///
/// The Canonical Conversation authority answers a message with the Event it
/// appended and a group change with the record it changed; both name their
/// sequence in the event or receipt they return. Nothing here derives a
/// sequence from a local counter.
fn admitted_sequence(response: &Value) -> Option<i64> {
    for pointer in [
        "/sequence",
        "/event/sequence",
        "/receipt/sequence",
        "/conversation/sequence",
    ] {
        if let Some(sequence) = response.pointer(pointer).and_then(Value::as_i64) {
            return Some(sequence);
        }
    }
    None
}

fn refusal(refusal: DeliveryRefusal) -> anyhow::Error {
    anyhow!("{}", refusal.code())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::surface::MOBILE_SURFACE;
    use serde_json::json;

    /// One host that answers every group operation the way an admitting
    /// authority does, and records what it was asked.
    #[derive(Default)]
    struct AdmittingHost {
        seen: std::cell::RefCell<Vec<String>>,
    }

    impl MobileOperationHost for AdmittingHost {
        fn config_get(&self, _params: &Value) -> Result<Value> {
            unimplemented!("the group path does not read the relay configuration")
        }
        fn config_set(&self, _params: &Value) -> Result<Value> {
            unimplemented!("the group path does not write the relay configuration")
        }
        fn pairing_claim(&self, _params: &Value) -> Result<Value> {
            unimplemented!("the group path does not claim a pairing")
        }
        fn pairing_status(&self, _params: &Value) -> Result<Value> {
            unimplemented!("the group path does not read the pairing status")
        }
        fn e2ee_status(&self, _params: &Value) -> Result<Value> {
            unimplemented!("the group path does not read the E2EE status")
        }
        fn command_create_secure(&self, _params: &Value) -> Result<Value> {
            unimplemented!("the group path dispatches the operation itself")
        }
        fn command_result_secure(&self, _params: &Value) -> Result<Value> {
            unimplemented!("the group path settles from the operation answer")
        }
        fn command_result_replay_proof(&self, _params: &Value) -> Result<Value> {
            unimplemented!("the group path settles from the operation answer")
        }
        fn conversation_create(&self, params: &Value) -> Result<Value> {
            self.record("conversation.create", params)
        }
        fn conversation_list(&self, params: &Value) -> Result<Value> {
            self.record("conversation.list", params)
        }
        fn conversation_get(&self, params: &Value) -> Result<Value> {
            self.record("conversation.get", params)
        }
        fn conversation_events_page(&self, params: &Value) -> Result<Value> {
            self.record("conversation.events.page", params)
        }
        fn conversation_message_post(&self, params: &Value) -> Result<Value> {
            self.record("conversation.message.post", params)
        }
        fn conversation_membership_add(&self, params: &Value) -> Result<Value> {
            self.record("conversation.membership.add", params)
        }
        fn conversation_membership_leave(&self, params: &Value) -> Result<Value> {
            self.record("conversation.membership.leave", params)
        }
    }

    impl AdmittingHost {
        fn record(&self, operation: &str, _params: &Value) -> Result<Value> {
            self.seen.borrow_mut().push(operation.to_owned());
            Ok(json!({ "ok": true, "action": operation, "sequence": 1 }))
        }
    }

    #[test]
    fn the_group_path_declares_exactly_the_surfaces_group_operations() {
        let declared: Vec<&str> = MOBILE_SURFACE
            .iter()
            .copied()
            .filter(|operation| is_group_operation(operation))
            .collect();
        assert_eq!(
            declared, MOBILE_GROUP_OPERATIONS,
            "the group application path and the declared mobile surface must be one set"
        );
    }

    #[test]
    fn one_envelope_identity_never_becomes_two_group_operations() {
        let mut application = MobileGroupApplication::new(AdmittingHost::default(), 8);
        let request = json!({
            "action": "conversation.message.post",
            "params": { "conversationId": "conversation:1", "text": "hello" },
        });
        application
            .dispatch("envelope:1", &request)
            .expect("the first dispatch is admitted");
        let duplicate = application
            .dispatch("envelope:1", &request)
            .expect_err("a resend of one identity is refused");
        assert_eq!(duplicate.to_string(), "mobile_delivery_duplicate_envelope");
        assert_eq!(
            application.entry().host().seen.borrow().len(),
            1,
            "the refused resend reached the authority"
        );
    }

    #[test]
    fn an_operation_outside_the_group_path_is_refused_before_it_is_recorded() {
        let mut application = MobileGroupApplication::new(AdmittingHost::default(), 8);
        let refusal = application
            .dispatch(
                "envelope:1",
                &json!({ "action": "mobile.relay.pairing.claim", "params": {} }),
            )
            .expect_err("a pairing claim is not a group operation");
        assert_eq!(refusal.to_string(), "mobile_group_operation_required");
        assert!(application.ledger().is_empty());
        assert!(application.entry().host().seen.borrow().is_empty());
    }

    #[test]
    fn an_answer_without_a_sequence_does_not_settle_a_delivery() {
        struct UnnumberedHost;
        impl MobileOperationHost for UnnumberedHost {
            fn config_get(&self, _params: &Value) -> Result<Value> {
                unimplemented!()
            }
            fn config_set(&self, _params: &Value) -> Result<Value> {
                unimplemented!()
            }
            fn pairing_claim(&self, _params: &Value) -> Result<Value> {
                unimplemented!()
            }
            fn pairing_status(&self, _params: &Value) -> Result<Value> {
                unimplemented!()
            }
            fn e2ee_status(&self, _params: &Value) -> Result<Value> {
                unimplemented!()
            }
            fn command_create_secure(&self, _params: &Value) -> Result<Value> {
                unimplemented!()
            }
            fn command_result_secure(&self, _params: &Value) -> Result<Value> {
                unimplemented!()
            }
            fn command_result_replay_proof(&self, _params: &Value) -> Result<Value> {
                unimplemented!()
            }
            fn conversation_create(&self, _params: &Value) -> Result<Value> {
                Ok(json!({ "ok": true }))
            }
            fn conversation_list(&self, params: &Value) -> Result<Value> {
                Ok(json!({ "ok": true, "params": params }))
            }
            fn conversation_get(&self, _params: &Value) -> Result<Value> {
                unimplemented!()
            }
            fn conversation_events_page(&self, _params: &Value) -> Result<Value> {
                unimplemented!()
            }
            fn conversation_message_post(&self, _params: &Value) -> Result<Value> {
                unimplemented!()
            }
            fn conversation_membership_add(&self, _params: &Value) -> Result<Value> {
                unimplemented!()
            }
            fn conversation_membership_leave(&self, _params: &Value) -> Result<Value> {
                unimplemented!()
            }
        }

        let mut application = MobileGroupApplication::new(UnnumberedHost, 8);
        let refusal = application
            .dispatch("envelope:1", &json!({ "action": "conversation.create" }))
            .expect_err("a change with no sequence cannot settle a delivery");
        assert_eq!(refusal.to_string(), "mobile_group_answer_without_sequence");
        assert_eq!(
            application.reconnect("").pending.len(),
            1,
            "the unanswered delivery stays pending for a reconnect"
        );
    }

    #[test]
    fn a_reconnect_reports_the_pending_envelope_and_its_own_request() {
        let mut application = MobileGroupApplication::new(AdmittingHost::default(), 8);
        let request = json!({
            "action": "conversation.create",
            "params": { "title": "Paired group" },
        });
        application
            .dispatch("envelope:1", &request)
            .expect("the create is admitted");
        let plan = application.reconnect("");
        assert_eq!(plan.resume_after, Some(1));
        assert!(
            plan.pending.is_empty(),
            "an admitted dispatch is settled, not pending"
        );

        // A dispatch whose answer never arrived stays pending with its request,
        // so the reconnect re-sends the operation it was already asked for.
        let mut application = MobileGroupApplication::new(RefusingHost, 8);
        application
            .dispatch("envelope:2", &request)
            .expect("a value refusal is an answer, not an error");
        let plan = application.reconnect("");
        assert_eq!(plan.pending.len(), 1);
        assert_eq!(plan.pending[0].record.envelope_id, "envelope:2");
        assert_eq!(plan.pending[0].request, request);
        assert_eq!(
            plan.resume_after, None,
            "a pending delivery with no admitted sequence names no resume point"
        );
    }

    /// One host whose authority refused the operation as a value.
    struct RefusingHost;

    impl MobileOperationHost for RefusingHost {
        fn config_get(&self, _params: &Value) -> Result<Value> {
            unimplemented!()
        }
        fn config_set(&self, _params: &Value) -> Result<Value> {
            unimplemented!()
        }
        fn pairing_claim(&self, _params: &Value) -> Result<Value> {
            unimplemented!()
        }
        fn pairing_status(&self, _params: &Value) -> Result<Value> {
            unimplemented!()
        }
        fn e2ee_status(&self, _params: &Value) -> Result<Value> {
            unimplemented!()
        }
        fn command_create_secure(&self, _params: &Value) -> Result<Value> {
            unimplemented!()
        }
        fn command_result_secure(&self, _params: &Value) -> Result<Value> {
            unimplemented!()
        }
        fn command_result_replay_proof(&self, _params: &Value) -> Result<Value> {
            unimplemented!()
        }
        fn conversation_create(&self, _params: &Value) -> Result<Value> {
            Ok(json!({ "ok": false, "code": "conversation_title_required" }))
        }
        fn conversation_list(&self, _params: &Value) -> Result<Value> {
            unimplemented!()
        }
        fn conversation_get(&self, _params: &Value) -> Result<Value> {
            unimplemented!()
        }
        fn conversation_events_page(&self, _params: &Value) -> Result<Value> {
            unimplemented!()
        }
        fn conversation_message_post(&self, _params: &Value) -> Result<Value> {
            unimplemented!()
        }
        fn conversation_membership_add(&self, _params: &Value) -> Result<Value> {
            unimplemented!()
        }
        fn conversation_membership_leave(&self, _params: &Value) -> Result<Value> {
            unimplemented!()
        }
    }

    #[test]
    fn a_delivery_this_client_never_sent_is_not_settled_by_a_stranger_result() {
        let mut application = MobileGroupApplication::new(AdmittingHost::default(), 8);
        let refusal = application
            .settle("envelope:absent", 3)
            .expect_err("an unknown envelope is refused");
        assert_eq!(refusal.to_string(), "mobile_delivery_unknown_envelope");
    }
}
