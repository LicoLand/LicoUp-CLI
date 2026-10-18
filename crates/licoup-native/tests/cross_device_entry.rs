//! Product-level falsification of the cross-device production entry.
//!
//! This suite drives `licoup_native::domain::cross_device_entry` — the same
//! module the native route calls and the composition root installs — over the
//! real `ConversationStore` owner and the real peer ingress. Nothing here is a
//! reimplementation of the entry: the assertions read the entry's own receipts
//! and ledger, and the conversation rows it wrote through the host's store.
//!
//! Two constraints are falsified:
//!
//! 1. **The verification record precedes the conversation row.** The test reads
//!    the ledger ordinal the receipt carries before it reads the row, and it
//!    shows the second door into the conversation write refusing a unit that has
//!    no recorded verification. Inverting the order inside `ingest` fails this
//!    suite.
//! 2. **A replay is never admitted again.** An exact replay is refused and
//!    records nothing; a re-protected resend merges instead of becoming a second
//!    row. Removing the entry's identity check, or the ingress's own intake
//!    dedupe, fails this suite.
//!
//! What is substituted, stated plainly: the SDK-verified unit. The pinned SDK's
//! `ReplayIdentity` and `TrustFacts` can only be produced by a committed
//! protected record, so this suite supplies the same facts through
//! [`VerifiedUnit`] — the exact port the SDK boundary implements — with a key it
//! can name. The entry's own code path is unchanged; the SDK edge is the
//! `CrossDeviceEdge` a device layer attaches, and it is absent here, which is
//! why this suite never claims a verified packet.

use std::sync::Arc;

use licoup_conversation::{ConversationStore, MembershipAccess, Principal, PrincipalKind};
use licoup_native::domain::client_conversation::peer_ingress::{AdmissionFact, PeerBinding};
use licoup_native::domain::cross_device_entry::{
    CrossDeviceEntry, CrossDeviceRefusal, HostPeerBindings,
};
use licoup_native::domain::mobile_relay::endpoint_transport::{
    MessageForwarder, PeerMessageBody, PeerPart, VerifiedDevice, VerifiedUnit,
};

/// The fixture's own replay identity of one committed protected record.
type TestKey = u64;

const AUTHOR: [u8; 32] = [0xA1; 32];
const DEVICE: [u8; 32] = [0xB2; 32];

/// One verified unit's facts, in the shape the SDK boundary hands them over.
struct VerifiedFacts {
    author: [u8; 32],
    device: VerifiedDevice,
    record: TestKey,
}

impl VerifiedUnit for VerifiedFacts {
    type RecordKey = TestKey;

    fn verified_author(&self) -> [u8; 32] {
        self.author
    }

    fn verified_device(&self) -> VerifiedDevice {
        self.device
    }

    fn verified_record(&self) -> Option<TestKey> {
        Some(self.record)
    }
}

/// The ingress under test, over one real Canonical Conversation.
struct Fixture {
    store: ConversationStore,
    conversation_id: String,
    membership_id: String,
    entry: CrossDeviceEntry<TestKey>,
}

impl Fixture {
    fn facts(&self, record: TestKey) -> VerifiedFacts {
        VerifiedFacts {
            author: AUTHOR,
            device: VerifiedDevice::new(DEVICE, [0xC3; 32], [0xD4; 32]),
            record,
        }
    }

    fn messages(&self) -> Vec<licoup_conversation::ConversationEvent> {
        self.store
            .page_events(&self.conversation_id, None, 64)
            .expect("the conversation page reads")
            .events
            .into_iter()
            .filter(|event| event.kind == licoup_conversation::EventKind::Message)
            .collect()
    }
}

fn fixture() -> Fixture {
    let store = ConversationStore::open_in_memory().expect("an in-memory conversation store opens");
    let owner = principal("principal:owner", PrincipalKind::Human, "Owner");
    let peer = principal("principal:peer", PrincipalKind::Human, "Peer Device");
    let assistant = principal("principal:assistant", PrincipalKind::Agent, "Assistant");
    let conversation = store
        .create_conversation_with_members(
            "Peer sync",
            owner,
            &[
                (peer, MembershipAccess::Member),
                (assistant, MembershipAccess::Member),
            ],
        )
        .expect("the fixture conversation is created");
    let membership_id = conversation
        .memberships
        .iter()
        .find(|membership| membership.principal.id == "principal:peer")
        .map(|membership| membership.id.clone())
        .expect("the peer membership exists");

    let bindings = Arc::new(HostPeerBindings::new());
    bindings.bind(
        AUTHOR,
        DEVICE,
        PeerBinding {
            conversation_id: conversation.id.clone(),
            membership_id: membership_id.clone(),
            provider_id: "peer-device".to_owned(),
        },
    );
    Fixture {
        store,
        conversation_id: conversation.id,
        membership_id,
        entry: CrossDeviceEntry::new(bindings),
    }
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

fn body(logical_id: [u8; 16], text: &str) -> PeerMessageBody {
    PeerMessageBody::new(logical_id, None, vec![PeerPart::text(text).unwrap()])
        .expect("the fixture body is bounded")
}

/// One bound peer pair: verified facts, a binding, and a Canonical Conversation
/// the host already owns.
fn admitted() -> Fixture {
    let fixture = fixture();
    // Nothing writes a conversation row before a verified unit arrives.
    assert_eq!(fixture.messages().len(), 0);
    fixture
}

// ---------------------------------------------------------------------------
// Constraint one: recorded verification precedes the conversation row
// ---------------------------------------------------------------------------

#[test]
fn the_verification_record_precedes_the_conversation_row() {
    let mut fixture = admitted();
    let facts = fixture.facts(1);
    let logical_id = [0x31; 16];

    let receipt = fixture
        .entry
        .ingest(
            &fixture.store,
            &facts,
            MessageForwarder::Direct,
            body(logical_id, "recorded before written"),
            None,
        )
        .expect("the verified unit is admitted");

    // The record exists and carries the ledger's first ordinal; the row exists
    // only because that record was written first.
    assert_eq!(
        receipt.verification().ordinal(),
        1,
        "the first admission records the first verification"
    );
    assert_eq!(receipt.verification().identity(), &1);
    assert!(fixture.entry.ledger().contains(&1));
    assert_eq!(fixture.entry.ledger().next_ordinal(), 2);

    let (event_id, sequence) = match receipt.admission() {
        AdmissionFact::Admitted { event_id, sequence } => (event_id.clone(), *sequence),
        other => panic!("expected an admitted message, got {other:?}"),
    };
    assert_eq!(receipt.logical_id(), logical_id);
    let messages = fixture.messages();
    assert_eq!(messages.len(), 1, "exactly one canonical write follows");
    assert_eq!(messages[0].id, event_id);
    assert_eq!(messages[0].sequence, sequence);
    assert_eq!(
        messages[0].author_membership_id.as_deref(),
        Some(fixture.membership_id.as_str()),
        "the local author is the bound membership"
    );

    // The gate is load-bearing, not decorative: the second door into the
    // conversation write refuses a unit no record stands behind, and writes
    // nothing for it.
    let unrecorded = fixture.facts(99);
    let refusal = fixture
        .entry
        .redrive(
            &fixture.store,
            &unrecorded,
            MessageForwarder::Direct,
            body([0x32; 16], "never recorded"),
            None,
        )
        .expect_err("a unit with no recorded verification cannot be written");
    assert_eq!(refusal, CrossDeviceRefusal::VerificationNotRecorded);
    assert_eq!(
        refusal.code(),
        "cross_device_verification_not_recorded",
        "the refusal names the ordering constraint"
    );
    assert_eq!(fixture.messages().len(), 1, "the refusal wrote no row");
    assert_eq!(fixture.entry.ledger().len(), 1);
}

#[test]
fn a_recorded_unit_is_re_drivable_without_a_second_record_or_row() {
    let mut fixture = admitted();
    let facts = fixture.facts(4);
    let logical_id = [0x33; 16];

    let first = fixture
        .entry
        .ingest(
            &fixture.store,
            &facts,
            MessageForwarder::Direct,
            body(logical_id, "written once"),
            None,
        )
        .expect("the verified unit is admitted");
    let event_id = first.admission().event_id().unwrap().to_owned();

    // Re-driving an already recorded unit passes the gate, appends no second
    // record, and merges into the row that is already there.
    let redriven = fixture
        .entry
        .redrive(
            &fixture.store,
            &facts,
            MessageForwarder::Direct,
            body(logical_id, "written once"),
            None,
        )
        .expect("a recorded unit is re-drivable");
    assert_eq!(redriven.verification().ordinal(), 1);
    assert!(redriven.admission().is_duplicate());
    assert_eq!(redriven.admission().event_id(), Some(event_id.as_str()));
    assert_eq!(fixture.entry.ledger().len(), 1, "no second record");
    assert_eq!(fixture.messages().len(), 1, "no second row");
}

// ---------------------------------------------------------------------------
// Constraint two: a replay is never admitted again
// ---------------------------------------------------------------------------

#[test]
fn an_exact_replay_is_refused_and_records_nothing() {
    let mut fixture = admitted();
    let facts = fixture.facts(7);
    let logical_id = [0x42; 16];

    let first = fixture
        .entry
        .ingest(
            &fixture.store,
            &facts,
            MessageForwarder::Direct,
            body(logical_id, "verified once"),
            None,
        )
        .expect("the verified unit is admitted");
    let event_id = first.admission().event_id().unwrap().to_owned();
    assert_eq!(fixture.messages().len(), 1);
    assert_eq!(fixture.entry.ledger().len(), 1);

    // The same committed record, offered again: the same replay identity, so the
    // entry refuses it before any conversation write and records nothing.
    let replay = fixture.entry.ingest(
        &fixture.store,
        &facts,
        MessageForwarder::Direct,
        body(logical_id, "verified once"),
        None,
    );
    assert_eq!(replay, Err(CrossDeviceRefusal::ReplayRefused));
    assert_eq!(fixture.entry.ledger().len(), 1, "a replay records nothing");
    assert_eq!(fixture.messages().len(), 1, "a replay writes no row");
    assert_eq!(fixture.messages()[0].id, event_id);
}

#[test]
fn a_re_protected_resend_merges_instead_of_becoming_a_second_row() {
    let mut fixture = admitted();
    let logical_id = [0x51; 16];
    let first_facts = fixture.facts(11);
    let first = fixture
        .entry
        .ingest(
            &fixture.store,
            &first_facts,
            MessageForwarder::Direct,
            body(logical_id, "the message"),
            None,
        )
        .expect("the verified unit is admitted");
    let (event_id, sequence) = match first.admission() {
        AdmissionFact::Admitted { event_id, sequence } => (event_id.clone(), *sequence),
        other => panic!("expected an admitted message, got {other:?}"),
    };

    // A re-protected resend is a different committed record carrying the same
    // logical message. It reaches the ingress, which merges it: no second row,
    // no second execution grant.
    let resend_body = PeerMessageBody::new(
        logical_id,
        None,
        vec![
            PeerPart::text("the message").unwrap(),
            PeerPart::command(serde_json::json!({
                "family": "conversation",
                "command": "get",
                "conversationId": fixture.conversation_id,
            }))
            .unwrap(),
        ],
    )
    .unwrap();
    let resend_facts = fixture.facts(12);
    let resend = fixture
        .entry
        .ingest(
            &fixture.store,
            &resend_facts,
            MessageForwarder::Direct,
            resend_body,
            None,
        )
        .expect("a resend is a merge, not an error");
    match resend.admission() {
        AdmissionFact::Duplicate {
            event_id: duplicate,
            sequence: duplicate_sequence,
        } => {
            assert_eq!(duplicate.as_deref(), Some(event_id.as_str()));
            assert_eq!(*duplicate_sequence, Some(sequence));
        }
        other => panic!("expected a duplicate admission, got {other:?}"),
    }
    assert_eq!(
        resend.effect_intents(),
        0,
        "merging a message is not a second execution grant"
    );
    assert_eq!(
        fixture.entry.ledger().len(),
        2,
        "the resend is its own record"
    );
    assert_eq!(
        fixture.messages().len(),
        1,
        "a resend creates no second row"
    );
}

// ---------------------------------------------------------------------------
// The request decoder the native route shares
// ---------------------------------------------------------------------------

#[test]
fn the_request_decoder_refuses_a_body_it_cannot_carry() {
    use licoup_native::domain::cross_device_entry::{
        decode_body, decode_forwarder, decode_station_hint,
    };
    use serde_json::json;

    let decoded = decode_body(&json!({
        "logicalId": "00112233445566778899aabbccddeeff",
        "parts": [{ "kind": "text", "content": "hello" }],
    }))
    .expect("a bounded body decodes");
    assert_eq!(
        decoded.logical_id(),
        [
            0x00, 0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88, 0x99, 0xaa, 0xbb, 0xcc, 0xdd,
            0xee, 0xff
        ]
    );
    assert_eq!(decoded.parts().len(), 1);

    assert_eq!(
        decode_body(&json!({ "logicalId": "short", "parts": [] })),
        Err(CrossDeviceRefusal::MalformedRequest(
            "peer_logical_id_invalid"
        ))
    );
    assert_eq!(
        decode_body(&json!({
            "logicalId": "00112233445566778899aabbccddeeff",
            "parts": [],
        })),
        Err(CrossDeviceRefusal::Unit(
            licoup_native::domain::mobile_relay::endpoint_transport::PeerUnitRefusal::EmptyMessage
        ))
    );

    assert_eq!(
        decode_forwarder(&json!({ "forwarder": "direct" })),
        Ok(MessageForwarder::Direct)
    );
    assert!(matches!(
        decode_forwarder(&json!({ "forwarder": { "station": "relay-a" } })),
        Ok(MessageForwarder::Station(label)) if label.as_str() == "relay-a"
    ));
    assert_eq!(
        decode_forwarder(&json!({ "forwarder": { "station": "" } })),
        Err(CrossDeviceRefusal::Unit(
            licoup_native::domain::mobile_relay::endpoint_transport::PeerUnitRefusal::StationIdentityInvalid
        ))
    );

    let hint = decode_station_hint(&json!({ "stationAccepted": true, "stationDuplicate": false }))
        .expect("a complete report decodes")
        .expect("a report is present");
    assert!(hint.station_reported_accepted());
    assert!(!hint.station_reported_duplicate());
    assert!(!hint.is_endpoint_evidence(), "a hint is never evidence");
    assert_eq!(
        decode_station_hint(&json!({ "stationAccepted": true })),
        Err(CrossDeviceRefusal::MalformedRequest(
            "peer_station_hint_invalid"
        )),
        "a partial report is refused rather than completed"
    );
}
