//! Falsifiable constraints of the peer ingress over the real conversation owner.
//!
//! These cases need no LicoArc authority bundle. The SDK boundary enters only as
//! one [`VerifiedUnit`] value, which the transport module documents as the whole
//! shape this domain reads; a synthetic implementation of it mints no
//! permission, because admission still resolves the author and the device
//! against the host's own binding table and the existing conversation's active
//! membership. The store owner, the ingress, and the admission path are the
//! production ones.
//!
//! They exist because the two-endpoint suite cannot carry these constraints:
//! every case there is `#[ignore]`d behind `LICOARC_AUTHORITY_BUNDLE`, so nothing
//! in the default gate fails when the ingress stops consuming verification
//! before it writes, or stops refusing a replay.

use std::sync::{Arc, Mutex};

use licoup_conversation::{
    Conversation, ConversationStore, EventKind, MembershipAccess, Principal, PrincipalKind,
};

use crate::domain::mobile_relay::endpoint_transport::{
    MessageForwarder, PeerAuthor, PeerDevice, PeerMessage, PeerMessageBody, PeerPart,
    VerifiedDevice, VerifiedUnit,
};

use super::ingress::peer_admission_key;
use super::{AdmissionFact, PeerBinding, PeerBindings, PeerIngress};

const AUTHOR: [u8; 32] = [0x21; 32];
const DEVICE: [u8; 32] = [0x31; 32];

/// One synthetic verified unit.
///
/// The record identity is the caller's own value, so a case can send a
/// re-protected resend of the same logical message — a new record carrying the
/// same logical id — exactly as a peer produces one.
struct FixtureUnit {
    record: Option<String>,
}

impl VerifiedUnit for FixtureUnit {
    type RecordKey = String;

    fn verified_author(&self) -> [u8; 32] {
        AUTHOR
    }

    fn verified_device(&self) -> VerifiedDevice {
        VerifiedDevice::new(DEVICE, [0x11; 32], [0x12; 32])
    }

    fn verified_record(&self) -> Option<Self::RecordKey> {
        self.record.clone()
    }
}

/// The host's own binding table: a verified peer device resolves to an existing
/// membership, and an absent answer is a refusal.
#[derive(Default)]
struct FixedBindings {
    entries: Mutex<Vec<([u8; 32], [u8; 32], PeerBinding)>>,
}

impl FixedBindings {
    fn bind(&self, author: [u8; 32], device: [u8; 32], binding: PeerBinding) {
        self.lock().push((author, device, binding));
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Vec<([u8; 32], [u8; 32], PeerBinding)>> {
        self.entries
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

impl PeerBindings for FixedBindings {
    fn resolve(&self, author: &PeerAuthor, device: &PeerDevice) -> Option<PeerBinding> {
        let author = author.user_authority_state_digest();
        let device = device.identity_state_digest();
        self.lock()
            .iter()
            .find(|(bound_author, bound_device, _)| {
                *bound_author == author && *bound_device == device
            })
            .map(|(_, _, binding)| binding.clone())
    }
}

struct Fixture {
    store: ConversationStore,
    conversation_id: String,
    peer_membership_id: String,
    bindings: Arc<FixedBindings>,
}

fn principal(id: &str, kind: PrincipalKind, display_name: &str) -> Principal {
    Principal {
        id: id.to_owned(),
        kind,
        display_name: display_name.to_owned(),
        agent_id: None,
        created_at_unix_ms: 1,
    }
}

/// One Canonical Conversation with an owner and a peer member, and the binding
/// that resolves the verified peer device to that membership.
fn fixture() -> Fixture {
    let store = ConversationStore::open_in_memory().expect("an in-memory conversation store opens");
    let owner = principal("principal:owner", PrincipalKind::Human, "Owner");
    let peer = principal("principal:peer", PrincipalKind::Human, "Peer Device");
    let conversation: Conversation = store
        .create_direct_conversation_with_members(
            "Peer sync",
            owner,
            &[(peer, MembershipAccess::Member)],
        )
        .expect("the fixture conversation is created");
    let peer_membership_id = conversation
        .memberships
        .iter()
        .find(|membership| membership.principal.id == "principal:peer")
        .map(|membership| membership.id.clone())
        .expect("the fixture membership exists");
    let bindings = Arc::new(FixedBindings::default());
    bindings.bind(
        AUTHOR,
        DEVICE,
        PeerBinding {
            conversation_id: conversation.id.clone(),
            membership_id: peer_membership_id.clone(),
            provider_id: "provider:peer".to_owned(),
        },
    );
    Fixture {
        store,
        conversation_id: conversation.id,
        peer_membership_id,
        bindings,
    }
}

fn ingress(bindings: &Arc<FixedBindings>) -> PeerIngress<String> {
    let bindings: Arc<dyn PeerBindings> = bindings.clone();
    PeerIngress::new(bindings)
}

fn message(logical_id: [u8; 16], record: &str, text: &str) -> PeerMessage<String> {
    let unit = FixtureUnit {
        record: Some(record.to_owned()),
    };
    let body = PeerMessageBody::new(
        logical_id,
        None,
        vec![PeerPart::text(text).expect("a bounded text part")],
    )
    .expect("a bounded body");
    PeerMessage::of_verified(&unit, MessageForwarder::Direct, body)
        .expect("a verified message unit")
}

/// Only the Message events: creating the fixture conversation legitimately
/// records membership changes, which are not peer messages.
fn messages(fixture: &Fixture) -> Vec<licoup_conversation::ConversationEvent> {
    fixture
        .store
        .page_events(&fixture.conversation_id, None, 64)
        .expect("the conversation page reads")
        .events
        .into_iter()
        .filter(|event| event.kind == EventKind::Message)
        .collect()
}

fn admitted(report: &super::PeerIngressReport) -> (String, i64) {
    match report.admission() {
        AdmissionFact::Admitted { event_id, sequence } => (event_id.clone(), *sequence),
        other => panic!("expected an admitted message, got {other:?}"),
    }
}

/// The consumed verification is durable and predicts the row it authorized.
///
/// Asking the owner for the record this message was admitted under writes
/// nothing and answers with exactly the Event that admission produced, so the
/// record and the row are one commit rather than two ordered writes that a
/// crash could separate.
#[test]
fn the_admission_record_names_the_event_it_authorized() {
    let fixture = fixture();
    let mut ingress = ingress(&fixture.bindings);
    let logical_id = [0x41; 16];

    let report = ingress
        .admit(
            &fixture.store,
            &message(logical_id, "record-1", "recorded before written"),
        )
        .expect("the bound peer is admitted");
    let (event_id, sequence) = admitted(&report);

    let stored = messages(&fixture);
    assert_eq!(stored.len(), 1, "exactly one canonical write follows");
    assert_eq!(stored[0].id, event_id);
    assert_eq!(stored[0].sequence, sequence);
    assert_eq!(
        stored[0].author_membership_id.as_deref(),
        Some(fixture.peer_membership_id.as_str()),
        "the local author is the bound membership"
    );

    let probe = fixture
        .store
        .append_event_once(
            &fixture.conversation_id,
            Some(&fixture.peer_membership_id),
            EventKind::Message,
            &[],
            None,
            None,
            true,
            &peer_admission_key(logical_id),
        )
        .expect("the record probe reads");
    assert!(
        !probe.is_appended(),
        "a consumed record authorizes no second row"
    );
    assert_eq!(
        probe.admitted_event(),
        Some((Some(event_id.as_str()), Some(sequence))),
        "the record names the row it authorized"
    );
    assert_eq!(
        messages(&fixture).len(),
        1,
        "probing the record writes nothing"
    );
}

/// A replay is never admitted a second time, and suppression is by identity
/// rather than by "only one message is ever accepted".
#[test]
fn a_replayed_message_is_never_admitted_a_second_time() {
    let fixture = fixture();
    let mut ingress = ingress(&fixture.bindings);
    let logical_id = [0x51; 16];

    let first = ingress
        .admit(
            &fixture.store,
            &message(logical_id, "record-1", "sent once"),
        )
        .expect("the bound peer is admitted");
    let (event_id, _) = admitted(&first);
    assert_eq!(messages(&fixture).len(), 1);

    // The same logical message inside a different protected record: a peer
    // re-protection, which no protocol-level replay check refuses.
    let replay = ingress
        .admit(
            &fixture.store,
            &message(logical_id, "record-2", "sent once"),
        )
        .expect("a duplicate is a merge, not an error");
    assert!(
        replay.admission().is_duplicate(),
        "a replay is never admitted again"
    );
    assert_eq!(
        replay.admission().event_id(),
        Some(event_id.as_str()),
        "the merge names the effect that already exists"
    );
    assert!(
        replay.effect_intents().is_empty(),
        "merging a message is not a second execution grant"
    );
    assert_eq!(messages(&fixture).len(), 1, "a replay writes no second row");

    // A different logical message is still admitted: the refusal is keyed by
    // identity, and it does not wedge the conversation.
    let other = ingress
        .admit(
            &fixture.store,
            &message([0x52; 16], "record-3", "a different message"),
        )
        .expect("a distinct message is admitted");
    assert!(matches!(other.admission(), AdmissionFact::Admitted { .. }));
    assert_eq!(messages(&fixture).len(), 2);
}

/// Suppression survives a process restart, so it cannot live only in a window
/// this ingress happens to hold.
#[test]
fn an_ingress_that_never_saw_the_original_still_refuses_the_replay() {
    let fixture = fixture();
    let logical_id = [0x61; 16];

    {
        let mut first = ingress(&fixture.bindings);
        let report = first
            .admit(
                &fixture.store,
                &message(logical_id, "record-1", "sent once"),
            )
            .expect("the bound peer is admitted");
        let _ = admitted(&report);
    }

    // A fresh ingress holds no window at all. Only the owner's durable record
    // can refuse the resend that follows.
    let mut restarted = ingress(&fixture.bindings);
    let replay = restarted
        .admit(
            &fixture.store,
            &message(logical_id, "record-2", "sent once"),
        )
        .expect("a duplicate is a merge, not an error");
    assert!(
        replay.admission().is_duplicate(),
        "a restart does not re-admit a consumed verification"
    );
    assert!(
        replay.effect_intents().is_empty(),
        "a merge is not a second execution grant"
    );
    assert_eq!(
        messages(&fixture).len(),
        1,
        "a replay after a restart writes no second row"
    );
}
