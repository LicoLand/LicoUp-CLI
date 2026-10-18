//! Endpoint peer-ingress acceptance at `component-integration` level: the
//! two-endpoint message mapping and the Canonical Conversation peer ingress.
//!
//! What is real here: the fixed LicoArc Candidate artifact is admitted through
//! `licoup_protocol_bindings`, the two endpoints run a real SDK handshake, every
//! message is a real protected record verified and committed by the SDK, and
//! the suite reaches every one of those SDK entries through the boundary's own
//! `EndpointConsumer` / `InboundSession`, so each `TrustFacts` value the ingress
//! consumes is produced by the production owner. The conversation writes go
//! through the real `ConversationStore` owner, and effect requests go through
//! the real `ApplicationFacade` entry.
//!
//! What is synthetic, stated plainly: the caller-owned platform layer (custody
//! material, atomic store, clock, packet carrier) and the host's peer-binding
//! table. No network, no remote station, no user message, and no second
//! conversation authority is involved. The actual two-endpoint product loop,
//! reconnect, and capability boundary remain owned by the durable-delivery and
//! capability-sync nodes.
//!
//! The real-SDK cases need the fixed Candidate as an explicit environment input:
//!
//! ```text
//! LICOARC_AUTHORITY_BUNDLE=<LicoArc>/artifacts/v1/licoarc.bundle.json \
//!   cargo test -p licoup-native --test peer_ingress -- --ignored
//! ```
//!
//! They are `#[ignore]`d so a checkout without the artifact reports them as not
//! run instead of passing silently.
//!
//! The two production modules are mounted by [`domain`] through `#[path]` so
//! this suite compiles and runs the exact source files that the native host
//! wires into `domain::mobile_relay` and `domain::client_conversation`; the
//! crate-level `mod` wiring itself is owned by the integration step.

pub mod domain;
pub mod harness;

use std::sync::Arc;

use licoarc::error::ErrorCode;
use licoarc::reliable::{
    AuthorizedSession, ConfirmationOutcome, ConfirmationStage, EndpointConfirmation, FinalityState,
};
use licoup_protocol_bindings::{EndpointConsumer, ReplayIdentity};
use serde_json::json;

use crate::domain::client_conversation::peer_ingress::{
    AcceptanceFact, AdmissionFact, DeliveryFact, PeerBinding, PeerIngressRefusal, ReadFact,
};
use crate::domain::mobile_relay::endpoint_transport::{
    MAX_PEER_PARTS, MAX_PEER_TEXT_BYTES, MAX_STATION_ID_BYTES, MessageForwarder, PeerAuthor,
    PeerDevice, PeerMessage, PeerMessageBody, PeerPart, PeerUnitRefusal, StationHint, StationRef,
};
use crate::harness::{
    ConversationFixture, FixedBindings, FixtureIngress, HOST_AUTHORITY, PEER_AUTHORITY,
    RecordingActor, RecordingConversation, application_facade, authority_line, conversation_count,
    conversation_fixture, decode_command, establish, messages,
};

/// The SDK boundary's own consumer, so this suite reaches the SDK's trust,
/// handshake, record, and confirmation entries through the production owner
/// instead of a local wrapper.
const CONSUMER: EndpointConsumer = EndpointConsumer::new();

/// One bound peer pair: the endpoints, the host's binding table, and the
/// conversation the peer is bound to.
struct BoundPeer {
    peer: harness::PeerSession,
    host: harness::HostSession,
    bindings: Arc<FixedBindings>,
    fixture: ConversationFixture,
    ingress: FixtureIngress,
    author: [u8; 32],
    device: [u8; 32],
}

fn bind(line: &licoarc::VerifiedProtocolLine) -> BoundPeer {
    let pair = establish(line, 11);
    let fixture = conversation_fixture();
    let bindings = Arc::new(FixedBindings::new());
    let author = PeerAuthor::of_verified(pair.host.facts()).user_authority_state_digest();
    let device = PeerDevice::of_verified(pair.host.facts()).identity_state_digest();
    bindings.bind(
        author,
        device,
        PeerBinding {
            conversation_id: fixture.conversation_id.clone(),
            membership_id: fixture.peer_membership_id.clone(),
            provider_id: "peer-device".to_owned(),
        },
    );
    let ingress = FixtureIngress::new(bindings.clone());
    BoundPeer {
        peer: pair.peer,
        host: pair.host,
        bindings,
        fixture,
        ingress,
        author,
        device,
    }
}

/// Protects one payload at the peer, verifies it at the host, and maps it.
fn verified_message(
    peer: &mut harness::PeerSession,
    host: &mut harness::HostSession,
    body: PeerMessageBody,
) -> PeerMessage<ReplayIdentity> {
    let payload = payload_for(&body);
    let packet = peer.send(&payload);
    let record = host.receive(&packet);
    PeerMessage::of_verified(&record.facts, MessageForwarder::Direct, body)
        .expect("a committed record maps")
}

fn payload_for(body: &PeerMessageBody) -> Vec<u8> {
    // The protected plaintext is the client's own encoding of the decoded
    // message; the SDK treats it as opaque bytes, which is what the real
    // adapter hands to the decoder.
    serde_json::to_vec(&json!({
        "id": hex16(body.logical_id()),
        "parts": body.parts().len(),
    }))
    .expect("the synthetic plaintext encodes")
}

fn body(logical_id: [u8; 16], relates_to: Option<[u8; 16]>, text: &str) -> PeerMessageBody {
    PeerMessageBody::new(
        logical_id,
        relates_to,
        vec![PeerPart::text(text).expect("the text part is bounded")],
    )
    .expect("the body is bounded")
}

fn hex16(id: [u8; 16]) -> String {
    id.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// The Message events the Canonical Conversation store holds for the fixture
/// conversation. Membership changes are not peer messages.
fn stored_message_count(bound: &BoundPeer) -> usize {
    messages(&bound.fixture.store, &bound.fixture.conversation_id).len()
}

// ---------------------------------------------------------------------------
// Provenance, dedupe, causality, station hints
// ---------------------------------------------------------------------------

#[test]
#[ignore = "requires the fixed LicoArc Candidate through LICOARC_AUTHORITY_BUNDLE"]
fn two_verified_messages_admit_into_one_conversation_with_explicit_origin() {
    let line = authority_line();
    let mut bound = bind(&line);
    let first = verified_message(
        &mut bound.peer,
        &mut bound.host,
        body([1; 16], None, "hello from the peer"),
    );
    let second = verified_message(
        &mut bound.peer,
        &mut bound.host,
        body([2; 16], None, "second message"),
    );

    let report = bound
        .ingress
        .admit(&bound.fixture.store, &first)
        .expect("the bound peer is admitted");
    let admission = report.admission().clone();
    assert!(matches!(admission, AdmissionFact::Admitted { .. }));
    assert_eq!(report.delivery(), DeliveryFact::Unreported);
    assert_eq!(report.read(), ReadFact::Unread);
    assert_eq!(report.acceptance(), AcceptanceFact::Unreported);
    assert_eq!(
        report.origin().author().user_authority_state_digest(),
        PEER_AUTHORITY,
        "the author is the verified initiator authority, not a body claim"
    );
    assert_eq!(
        report.origin().device().identity_state_digest(),
        bound.device
    );
    assert_eq!(report.origin().forwarder(), &MessageForwarder::Direct);

    bound
        .ingress
        .admit(&bound.fixture.store, &second)
        .expect("the second bound message is admitted");

    let stored = messages(&bound.fixture.store, &bound.fixture.conversation_id);
    assert_eq!(stored.len(), 2, "two messages, two conversation events");
    for event in &stored {
        assert_eq!(
            event.author_membership_id.as_deref(),
            Some(bound.fixture.peer_membership_id.as_str()),
            "the local author is the bound membership, not a display name"
        );
    }
    assert!(
        stored[0].sequence < stored[1].sequence,
        "local sequence is arrival order"
    );

    // One conversation, unchanged membership set: no second conversation and no
    // temporary administrator was created.
    assert_eq!(conversation_count(&bound.fixture.store), 1);
    let conversation = bound
        .fixture
        .store
        .get(&bound.fixture.conversation_id)
        .expect("the conversation reads");
    assert_eq!(conversation.memberships.len(), 3);
    assert!(
        conversation
            .memberships
            .iter()
            .all(
                |membership| membership.access == licoup_conversation::MembershipAccess::Member
                    || membership.access == licoup_conversation::MembershipAccess::Owner
            ),
        "no membership was promoted or invented"
    );
}

#[test]
#[ignore = "requires the fixed LicoArc Candidate through LICOARC_AUTHORITY_BUNDLE"]
fn a_station_forwarded_message_keeps_author_and_device_and_the_hint_is_not_evidence() {
    let line = authority_line();
    let mut bound = bind(&line);
    let packet = bound.peer.send(b"forwarded");
    let record = bound.host.receive(&packet);
    let station = StationRef::new("station:synthetic").expect("the label is bounded");
    let forwarded = PeerMessage::of_verified(
        &record.facts,
        MessageForwarder::Station(station),
        body([3; 16], None, "carried by a station"),
    )
    .expect("a forwarded record maps")
    .with_station_hint(StationHint::reported(true, true));

    let report = bound
        .ingress
        .admit(&bound.fixture.store, &forwarded)
        .expect("forwarding changes no admission fact");
    assert!(report.origin().is_station_forwarded());
    assert_eq!(
        report.origin().author().user_authority_state_digest(),
        PEER_AUTHORITY
    );
    assert_eq!(
        report.origin().device().identity_state_digest(),
        bound.device
    );
    let hint = report.station().expect("the hint is recorded");
    assert!(hint.station_reported_accepted());
    assert!(hint.station_reported_duplicate());
    assert!(
        !hint.is_endpoint_evidence(),
        "a station acceptance is never endpoint evidence"
    );
    assert_eq!(
        report.delivery(),
        DeliveryFact::Unreported,
        "the station hint cannot become a delivery fact"
    );
    assert_eq!(report.acceptance(), AcceptanceFact::Unreported);
    assert!(report.effect_intents().is_empty());
}

#[test]
#[ignore = "requires the fixed LicoArc Candidate through LICOARC_AUTHORITY_BUNDLE"]
fn out_of_order_arrival_keeps_local_order_and_records_causality() {
    let line = authority_line();
    let mut bound = bind(&line);
    let parent = [0x21; 16];
    let reply = [0x22; 16];

    // The reply arrives before the message it answers.
    let reply_first = verified_message(
        &mut bound.peer,
        &mut bound.host,
        body(reply, Some(parent), "answering early"),
    );
    let report = bound
        .ingress
        .admit(&bound.fixture.store, &reply_first)
        .expect("the reply is admitted at arrival order");
    let first_sequence = match report.admission() {
        AdmissionFact::Admitted { sequence, .. } => *sequence,
        other => panic!("expected an admitted reply, got {other:?}"),
    };

    let parent_message = verified_message(
        &mut bound.peer,
        &mut bound.host,
        body(parent, None, "the original"),
    );
    bound
        .ingress
        .admit(&bound.fixture.store, &parent_message)
        .expect("the parent is admitted later");
    let second_reply = verified_message(
        &mut bound.peer,
        &mut bound.host,
        body([0x23; 16], Some(parent), "answering with the parent known"),
    );
    bound
        .ingress
        .admit(&bound.fixture.store, &second_reply)
        .expect("the later reply is admitted");

    let stored = messages(&bound.fixture.store, &bound.fixture.conversation_id);
    assert_eq!(stored.len(), 3);
    assert_eq!(
        stored[0].sequence, first_sequence,
        "arrival order decides the local sequence, not the peer's author order"
    );
    assert!(
        stored[0].sequence < stored[1].sequence && stored[1].sequence < stored[2].sequence,
        "the local sequence stays arrival order"
    );
    assert_eq!(
        stored[0].correlation_id.as_deref(),
        Some(format!("peer:{}", hex16(parent)).as_str()),
        "an early reply correlates to the peer's logical parent"
    );
    assert!(
        stored[0].causation_id.is_none(),
        "no local event can be named before it exists"
    );
    assert_eq!(
        stored[2].causation_id.as_deref(),
        Some(stored[1].id.as_str()),
        "a reply with a known parent names the parent's local event"
    );
    assert_eq!(
        stored[1].correlation_id.as_deref(),
        Some(format!("peer:{}", hex16(parent)).as_str()),
        "the parent owns its thread"
    );
    assert_eq!(
        stored[2].correlation_id.as_deref(),
        stored[1].correlation_id.as_deref(),
        "a reply shares its parent's thread whether or not the parent was known"
    );
}

#[test]
#[ignore = "requires the fixed LicoArc Candidate through LICOARC_AUTHORITY_BUNDLE"]
fn a_replayed_record_is_refused_by_the_sdk_and_a_duplicate_message_merges() {
    let line = authority_line();
    let mut bound = bind(&line);
    let message = verified_message(
        &mut bound.peer,
        &mut bound.host,
        body([4; 16], None, "once"),
    );
    let first = bound
        .ingress
        .admit(&bound.fixture.store, &message)
        .expect("the first copy is admitted");
    let event_id = first.admission().event_id().unwrap().to_owned();

    // A byte-identical replay of the same protected record is refused by the
    // SDK itself, and nothing is written.
    let replay = bound.peer.send(b"replay");
    let _consumed = bound.host.receive(&replay);
    let refusal = bound
        .host
        .try_receive(&replay)
        .expect_err("the SDK refuses the replayed record");
    assert_eq!(
        refusal
            .cause()
            .expect("the SDK itself refused the replayed record")
            .code,
        ErrorCode::Replay,
        "the SDK boundary carries the SDK's own replay refusal"
    );
    assert_eq!(
        messages(&bound.fixture.store, &bound.fixture.conversation_id).len(),
        1
    );

    // A fresh protected record carrying the same logical message id merges into
    // the first admission instead of creating a second event.
    let duplicate_packet = bound.peer.send(b"re-encoded");
    let duplicate_record = bound.host.receive(&duplicate_packet);
    let duplicate = PeerMessage::of_verified(
        &duplicate_record.facts,
        MessageForwarder::Direct,
        PeerMessageBody::new(
            [4; 16],
            None,
            vec![
                PeerPart::text("once").unwrap(),
                PeerPart::command(json!({
                    "family": "conversation",
                    "command": "get",
                    "conversationId": bound.fixture.conversation_id,
                }))
                .unwrap(),
            ],
        )
        .unwrap(),
    )
    .unwrap();
    let merged = bound
        .ingress
        .admit(&bound.fixture.store, &duplicate)
        .expect("a duplicate is a merge, not an error");
    assert!(merged.admission().is_duplicate());
    assert_eq!(merged.admission().event_id(), Some(event_id.as_str()));
    assert!(
        merged.effect_intents().is_empty(),
        "merging a message is not a second execution grant"
    );
    assert_eq!(
        messages(&bound.fixture.store, &bound.fixture.conversation_id).len(),
        1
    );
}

// ---------------------------------------------------------------------------
// Wrong author, revocation, separate facts, effect authorization
// ---------------------------------------------------------------------------

#[test]
#[ignore = "requires the fixed LicoArc Candidate through LICOARC_AUTHORITY_BUNDLE"]
fn an_unbound_author_is_refused_without_a_member_or_admin() {
    let line = authority_line();
    let mut bound = bind(&line);
    let mut stranger = establish(&line, 61);

    let packet = stranger.peer.send(b"stranger");
    let record = stranger.host.receive(&packet);
    let message = PeerMessage::of_verified(
        &record.facts,
        MessageForwarder::Direct,
        body([5; 16], None, "not bound here"),
    )
    .expect("the stranger's record is real but unbound");

    assert_eq!(
        bound.ingress.admit(&bound.fixture.store, &message),
        Err(PeerIngressRefusal::UnboundPeer)
    );
    assert_eq!(
        messages(&bound.fixture.store, &bound.fixture.conversation_id).len(),
        0,
        "an unbound author writes nothing"
    );
    assert_eq!(conversation_count(&bound.fixture.store), 1);
    assert_eq!(
        bound
            .fixture
            .store
            .get(&bound.fixture.conversation_id)
            .unwrap()
            .memberships
            .len(),
        3,
        "no principal or membership was created for the stranger"
    );

    // The refusal is scoped: the bound peer still admits.
    let accepted = verified_message(
        &mut bound.peer,
        &mut bound.host,
        body([6; 16], None, "still here"),
    );
    assert!(bound.ingress.admit(&bound.fixture.store, &accepted).is_ok());
}

#[test]
#[ignore = "requires the fixed LicoArc Candidate through LICOARC_AUTHORITY_BUNDLE"]
fn revoked_device_and_inactive_membership_stop_new_admission_while_events_stay() {
    let line = authority_line();
    let mut bound = bind(&line);
    let established = verified_message(
        &mut bound.peer,
        &mut bound.host,
        body([7; 16], None, "established before revocation"),
    );
    let report = bound
        .ingress
        .admit(&bound.fixture.store, &established)
        .expect("the established message is admitted");
    let established_event = report.admission().event_id().unwrap().to_owned();

    // Host-side device revocation: the binding is withdrawn.
    assert!(bound.bindings.revoke(bound.author, bound.device));
    let after_revocation = verified_message(
        &mut bound.peer,
        &mut bound.host,
        body([8; 16], None, "after revocation"),
    );
    assert_eq!(
        bound.ingress.admit(&bound.fixture.store, &after_revocation),
        Err(PeerIngressRefusal::UnboundPeer)
    );

    // Re-bind, then revoke through the conversation's own membership record.
    bound.bindings.bind(
        bound.author,
        bound.device,
        PeerBinding {
            conversation_id: bound.fixture.conversation_id.clone(),
            membership_id: bound.fixture.peer_membership_id.clone(),
            provider_id: "peer-device".to_owned(),
        },
    );
    bound
        .fixture
        .store
        .leave_member(
            &bound.fixture.conversation_id,
            &bound.fixture.peer_membership_id,
        )
        .expect("the owner revokes the membership");
    let after_membership_revocation = verified_message(
        &mut bound.peer,
        &mut bound.host,
        body([9; 16], None, "after membership revocation"),
    );
    assert_eq!(
        bound
            .ingress
            .admit(&bound.fixture.store, &after_membership_revocation),
        Err(PeerIngressRefusal::MembershipInactive)
    );

    // The established message stays readable, and the revoked membership is
    // still visible as left rather than deleted.
    let stored = messages(&bound.fixture.store, &bound.fixture.conversation_id);
    assert_eq!(stored.len(), 1);
    assert_eq!(stored[0].id, established_event);
    let conversation = bound
        .fixture
        .store
        .get(&bound.fixture.conversation_id)
        .expect("the conversation still reads");
    let membership = conversation
        .memberships
        .iter()
        .find(|membership| membership.id == bound.fixture.peer_membership_id)
        .expect("the revoked membership stays visible");
    assert_eq!(
        membership.status,
        licoup_conversation::MembershipStatus::Left
    );
}

#[test]
#[ignore = "requires the fixed LicoArc Candidate through LICOARC_AUTHORITY_BUNDLE"]
fn delivery_read_and_acceptance_are_separate_facts_from_admission() {
    let line = authority_line();
    let mut bound = bind(&line);
    let message = verified_message(
        &mut bound.peer,
        &mut bound.host,
        body([10; 16], None, "facts"),
    );
    let logical_id = message.logical_id();
    let report = bound
        .ingress
        .admit(&bound.fixture.store, &message)
        .expect("the message is admitted");
    assert_eq!(report.delivery(), DeliveryFact::Unreported);
    assert_eq!(report.read(), ReadFact::Unread);
    assert_eq!(report.acceptance(), AcceptanceFact::Unreported);

    // The SDK owns delivery and acceptance; the same delivery confirmation can
    // never complete a task, and acceptance needs the expected result digest.
    let session = authorized_session(true, true, [5; 32]);
    let delivered = confirmation(
        ConfirmationStage::EndpointAccepted,
        ConfirmationOutcome::Succeeded,
        vec![[4; 16], [5; 16]],
        None,
    );
    assert_eq!(
        CONSUMER.note_delivery(FinalityState::Pending, [5; 16], &delivered, &session, true),
        Ok(FinalityState::Accepted)
    );
    assert_eq!(
        CONSUMER.note_acceptance(
            FinalityState::Pending,
            [5; 16],
            &delivered,
            &session,
            [9; 32],
            true
        ),
        Ok(FinalityState::Accepted),
        "an endpoint-accepted confirmation is never task acceptance"
    );

    assert!(
        bound
            .ingress
            .facts_mut()
            .record_delivery(logical_id, DeliveryFact::EndpointAccepted)
    );
    let after_delivery = bound.ingress.facts().get(logical_id).unwrap();
    assert_eq!(after_delivery.delivery, DeliveryFact::EndpointAccepted);
    assert_eq!(
        after_delivery.read,
        ReadFact::Unread,
        "delivery does not read the message"
    );
    assert_eq!(after_delivery.acceptance, AcceptanceFact::Unreported);

    let completed = confirmation(
        ConfirmationStage::EffectCompleted,
        ConfirmationOutcome::Succeeded,
        vec![[5; 16]],
        Some([9; 32]),
    );
    assert_eq!(
        CONSUMER.note_acceptance(
            FinalityState::Accepted,
            [5; 16],
            &completed,
            &session,
            [9; 32],
            true
        ),
        Ok(FinalityState::Completed)
    );
    assert!(bound.ingress.facts_mut().record_acceptance(
        logical_id,
        AcceptanceFact::EffectCompleted {
            result_digest: [9; 32]
        }
    ));
    assert!(bound.ingress.facts_mut().mark_read(logical_id));
    let final_facts = bound.ingress.facts().get(logical_id).unwrap();
    assert_eq!(final_facts.read, ReadFact::ReadLocally);
    assert!(final_facts.acceptance.is_completed());
    assert_eq!(
        final_facts.delivery,
        DeliveryFact::EndpointAccepted,
        "recording one fact never rewrites another"
    );

    // Facts cannot be attached to a message the ingress never admitted.
    assert!(
        !bound
            .ingress
            .facts_mut()
            .record_delivery([0xFF; 16], DeliveryFact::EndpointAccepted)
    );
    assert!(bound.ingress.facts().get([0xFF; 16]).is_none());
}

#[test]
#[ignore = "requires the fixed LicoArc Candidate through LICOARC_AUTHORITY_BUNDLE"]
fn a_peer_command_is_never_executed_and_never_becomes_local_admin() {
    let line = authority_line();
    let mut bound = bind(&line);
    // The request is one typed command from the application contract, encoded
    // exactly as a machine interface would receive it.
    let request = licoup_application::ApplicationCommand::Conversation(
        licoup_application::ConversationCommand::Get {
            conversation_id: bound.fixture.conversation_id.clone(),
        },
    )
    .encode()
    .expect("the typed command encodes");
    let message = verified_message(
        &mut bound.peer,
        &mut bound.host,
        PeerMessageBody::new(
            [11; 16],
            None,
            vec![
                PeerPart::text("please look this up").unwrap(),
                PeerPart::command(request.clone()).unwrap(),
            ],
        )
        .unwrap(),
    );
    let report = bound
        .ingress
        .admit(&bound.fixture.store, &message)
        .expect("a message with a command is admitted as a message");
    assert_eq!(report.effect_intents().len(), 1);
    let intent = &report.effect_intents()[0];
    assert!(
        !intent.claim.is_local_admin(),
        "a peer never becomes the local administrator"
    );
    match &intent.claim {
        licoup_application::ActorClaim::Membership {
            provider_id,
            conversation_id,
            membership_id,
            ..
        } => {
            assert_eq!(provider_id, "peer-device");
            assert_eq!(
                conversation_id.as_deref(),
                Some(bound.fixture.conversation_id.as_str())
            );
            assert_eq!(
                membership_id.as_deref(),
                Some(bound.fixture.peer_membership_id.as_str())
            );
        }
        other => panic!("expected a membership claim, got {other:?}"),
    }
    assert_eq!(intent.command_request, request);
    assert_eq!(intent.message_id, [11; 16]);

    // The command part is recorded as structured metadata; the natural text is
    // recorded as text.
    let stored = messages(&bound.fixture.store, &bound.fixture.conversation_id);
    assert_eq!(stored.len(), 1);
    let kinds: Vec<_> = stored[0].parts.iter().map(|part| part.kind).collect();
    assert_eq!(
        kinds,
        vec![
            licoup_conversation::EventPartKind::Text,
            licoup_conversation::EventPartKind::Metadata
        ]
    );

    // Execution runs only through the single-owner application entry, and the
    // entry's own order is what gates it.
    let command = decode_command(&intent.command_request);
    let actor = Arc::new(RecordingActor::allowing());
    let conversation_port = Arc::new(RecordingConversation::new());
    let facade = application_facade(actor.clone(), conversation_port.clone());
    facade
        .execute(&intent.claim, &command)
        .expect("the single entry admits the peer claim");
    assert_eq!(actor.verifies(), 1);
    assert_eq!(conversation_port.calls(), 1);

    let refusing_actor = Arc::new(RecordingActor::refusing());
    let refusing_port = Arc::new(RecordingConversation::new());
    let refusing_facade = application_facade(refusing_actor.clone(), refusing_port.clone());
    let refusal = refusing_facade
        .execute(&intent.claim, &command)
        .expect_err("a refused claim reaches no family port");
    assert_eq!(refusal.code, "actor_refused");
    assert_eq!(refusing_actor.verifies(), 1);
    assert_eq!(refusing_port.calls(), 0);
}

#[test]
#[ignore = "requires the fixed LicoArc Candidate through LICOARC_AUTHORITY_BUNDLE"]
fn natural_agent_text_is_carried_verbatim_without_a_marker() {
    let line = authority_line();
    let mut bound = bind(&line);
    let natural = "just talk to me\n\nsecond paragraph";
    let jsonish = "{\"status\":\"done\"}";
    let message = verified_message(
        &mut bound.peer,
        &mut bound.host,
        PeerMessageBody::new(
            [12; 16],
            None,
            vec![
                PeerPart::text(natural).unwrap(),
                PeerPart::text(jsonish).unwrap(),
            ],
        )
        .unwrap(),
    );
    let report = bound
        .ingress
        .admit(&bound.fixture.store, &message)
        .expect("plain text is admitted as plain text");
    assert!(report.effect_intents().is_empty());

    let stored = messages(&bound.fixture.store, &bound.fixture.conversation_id);
    let contents: Vec<&str> = stored[0]
        .parts
        .iter()
        .map(|part| part.content.as_str())
        .collect();
    assert_eq!(
        contents,
        vec![natural, jsonish],
        "the text reaches the conversation byte-for-byte, with no marker or envelope"
    );
}

#[test]
#[ignore = "requires the fixed LicoArc Candidate through LICOARC_AUTHORITY_BUNDLE"]
fn a_missing_conversation_is_refused_rather_than_created() {
    let line = authority_line();
    let mut pair = establish(&line, 11);
    let bindings = Arc::new(FixedBindings::new());
    let author = PeerAuthor::of_verified(pair.host.facts()).user_authority_state_digest();
    let device = PeerDevice::of_verified(pair.host.facts()).identity_state_digest();
    bindings.bind(
        author,
        device,
        PeerBinding {
            conversation_id: "conversation:missing".to_owned(),
            membership_id: "membership:missing".to_owned(),
            provider_id: "peer-device".to_owned(),
        },
    );
    let mut ingress = FixtureIngress::new(bindings);
    let fixture = conversation_fixture();
    let packet = pair.peer.send(b"into nowhere");
    let record = pair.host.receive(&packet);
    let message = PeerMessage::of_verified(
        &record.facts,
        MessageForwarder::Direct,
        body([13; 16], None, "hello?"),
    )
    .unwrap();

    assert_eq!(
        ingress.admit(&fixture.store, &message),
        Err(PeerIngressRefusal::ConversationMissing)
    );
    assert_eq!(conversation_count(&fixture.store), 1);

    // A bound conversation with a membership that does not exist is refused as
    // well: the ingress never creates the missing membership.
    let mut pair = establish(&line, 11);
    let bindings = Arc::new(FixedBindings::new());
    let author = PeerAuthor::of_verified(pair.host.facts()).user_authority_state_digest();
    let device = PeerDevice::of_verified(pair.host.facts()).identity_state_digest();
    bindings.bind(
        author,
        device,
        PeerBinding {
            conversation_id: fixture.conversation_id.clone(),
            membership_id: "membership:missing".to_owned(),
            provider_id: "peer-device".to_owned(),
        },
    );
    let mut ingress = FixtureIngress::new(bindings);
    let packet = pair.peer.send(b"into no membership");
    let record = pair.host.receive(&packet);
    let message = PeerMessage::of_verified(
        &record.facts,
        MessageForwarder::Direct,
        body([14; 16], None, "hello?"),
    )
    .unwrap();
    assert_eq!(
        ingress.admit(&fixture.store, &message),
        Err(PeerIngressRefusal::MembershipMissing)
    );
    assert_eq!(
        fixture
            .store
            .get(&fixture.conversation_id)
            .unwrap()
            .memberships
            .len(),
        3
    );
}

// ---------------------------------------------------------------------------
// The two endpoint fixtures: recorded verification, the canonical write, and
// what a refusal leaves behind
// ---------------------------------------------------------------------------

/// Fixture one: recorded verification precedes the canonical write.
///
/// The peer endpoint protects one unit; the host endpoint verifies and durably
/// commits that exact unit through the SDK, which advances the caller-owned
/// store revision while the Canonical Conversation store is still untouched;
/// only the message that the recorded facts produce is written, through the
/// real store owner. A resend of the same logical message afterwards is a
/// duplicate and creates no second dispatch.
#[test]
#[ignore = "requires the fixed LicoArc Candidate through LICOARC_AUTHORITY_BUNDLE"]
fn recorded_verification_precedes_the_canonical_write() {
    let line = authority_line();
    let mut bound = bind(&line);
    let logical_id = [0x31; 16];
    let first_body = body(logical_id, None, "recorded before written");

    // 1. Protecting a unit is not verification: nothing is recorded yet and the
    //    conversation store cannot have anything for it.
    let packet = bound.peer.send(&payload_for(&first_body));
    assert_eq!(
        stored_message_count(&bound),
        0,
        "protecting a unit writes no conversation event"
    );

    // 2. The host endpoint verifies and commits the unit. The caller-owned
    //    store revision moves, and the conversation store still holds nothing:
    //    the recorded verification is not a canonical write.
    let revision_before = bound.host.revision();
    let record = bound.host.receive(&packet);
    assert!(
        bound.host.revision() > revision_before,
        "the SDK durably recorded the verified unit in the caller-owned store"
    );
    assert_eq!(
        stored_message_count(&bound),
        0,
        "recorded verification writes no conversation event"
    );

    // 3. A peer message exists only from those recorded facts, and its replay
    //    identity is the SDK's own identity of the record it just committed.
    let message = PeerMessage::of_verified(&record.facts, MessageForwarder::Direct, first_body)
        .expect("the committed record maps");
    assert_eq!(
        message.replay_identity(),
        record.facts.replay_identity(),
        "the unit is the protected record the SDK committed"
    );
    assert!(
        matches!(
            message.replay_identity(),
            ReplayIdentity::ProtectedRecord { pending } if *pending == record.record
        ),
        "the mapped replay identity is the SDK's own identity of the committed record"
    );
    assert_eq!(message.logical_id(), logical_id);

    // 4. The canonical write happens after that, through the real store owner.
    let report = bound
        .ingress
        .admit(&bound.fixture.store, &message)
        .expect("the bound peer is admitted");
    let (event_id, sequence) = match report.admission() {
        AdmissionFact::Admitted { event_id, sequence } => (event_id.clone(), *sequence),
        other => panic!("expected an admitted message, got {other:?}"),
    };
    let stored = messages(&bound.fixture.store, &bound.fixture.conversation_id);
    assert_eq!(stored.len(), 1, "exactly one canonical write follows");
    assert_eq!(stored[0].id, event_id);
    assert_eq!(stored[0].sequence, sequence);
    assert_eq!(
        stored[0].author_membership_id.as_deref(),
        Some(bound.fixture.peer_membership_id.as_str()),
        "the local author is the bound membership"
    );
    assert_eq!(conversation_count(&bound.fixture.store), 1);

    // 5. The recorded unit is consumed: a fresh protected record carrying the
    //    same logical message is a duplicate with no second write.
    let resend_body = body(logical_id, None, "same message again");
    let resend = bound.peer.send(&payload_for(&resend_body));
    let resend_record = bound.host.receive(&resend);
    let resend_message =
        PeerMessage::of_verified(&resend_record.facts, MessageForwarder::Direct, resend_body)
            .expect("the resent record maps");
    let duplicate = bound
        .ingress
        .admit(&bound.fixture.store, &resend_message)
        .expect("a duplicate is a merge, not an error");
    assert!(duplicate.admission().is_duplicate());
    assert_eq!(duplicate.admission().event_id(), Some(event_id.as_str()));
    assert_eq!(stored_message_count(&bound), 1);
}

/// Fixture two: unverified or replayed peer input never reaches the Canonical
/// Conversation store.
///
/// An accepted handshake is not one committed protected record, so it cannot
/// even be named as a peer message; a byte-identical replay of a committed
/// record is refused by the SDK before any write; and a resend that does map is
/// a duplicate of the first admission. The store keeps exactly the events the
/// verified, first-time units produced.
#[test]
#[ignore = "requires the fixed LicoArc Candidate through LICOARC_AUTHORITY_BUNDLE"]
fn unverified_and_replayed_peer_input_never_reaches_the_conversation_store() {
    let line = authority_line();
    let mut bound = bind(&line);

    // The accepted handshake's own facts are sealed but describe no committed
    // record, so the only door into a peer message refuses them.
    assert_eq!(
        PeerMessage::of_verified(
            bound.host.facts(),
            MessageForwarder::Direct,
            body([0x41; 16], None, "unverified"),
        ),
        Err(PeerUnitRefusal::NotARecord),
        "handshake facts are not a verified message unit"
    );
    assert_eq!(stored_message_count(&bound), 0);

    // One verified unit is admitted through the real store owner.
    let logical_id = [0x42; 16];
    let body = body(logical_id, None, "verified once");
    let packet = bound.peer.send(&payload_for(&body));
    let record = bound.host.receive(&packet);
    let message = PeerMessage::of_verified(&record.facts, MessageForwarder::Direct, body)
        .expect("the committed record maps");
    let report = bound
        .ingress
        .admit(&bound.fixture.store, &message)
        .expect("the verified unit is admitted");
    let (event_id, sequence) = match report.admission() {
        AdmissionFact::Admitted { event_id, sequence } => (event_id.clone(), *sequence),
        other => panic!("expected an admitted message, got {other:?}"),
    };
    assert_eq!(stored_message_count(&bound), 1);

    // A byte-identical replay of that consumed record is refused by the SDK
    // itself, and nothing is written for it.
    let refusal = bound
        .host
        .try_receive(&packet)
        .expect_err("the SDK refuses the replayed record");
    assert_eq!(
        refusal
            .cause()
            .expect("the SDK itself refused the replayed record")
            .code,
        ErrorCode::Replay,
        "the SDK boundary carries the SDK's own replay refusal"
    );
    assert_eq!(
        stored_message_count(&bound),
        1,
        "a replayed record writes no conversation event"
    );

    // A fresh protected record carrying the same logical id is a duplicate: it
    // merges into the first admission instead of creating a second dispatch.
    let resend_body = PeerMessageBody::new(
        logical_id,
        None,
        vec![
            PeerPart::text("verified again").unwrap(),
            PeerPart::command(json!({
                "family": "conversation",
                "command": "get",
                "conversationId": bound.fixture.conversation_id,
            }))
            .unwrap(),
        ],
    )
    .unwrap();
    let resend = bound.peer.send(&payload_for(&resend_body));
    let resend_record = bound.host.receive(&resend);
    let resend_message =
        PeerMessage::of_verified(&resend_record.facts, MessageForwarder::Direct, resend_body)
            .expect("the resent record maps");
    let merged = bound
        .ingress
        .admit(&bound.fixture.store, &resend_message)
        .expect("a duplicate is a merge, not an error");
    match merged.admission() {
        AdmissionFact::Duplicate {
            event_id: duplicate_event,
            sequence: duplicate_sequence,
        } => {
            assert_eq!(duplicate_event.as_deref(), Some(event_id.as_str()));
            assert_eq!(*duplicate_sequence, Some(sequence));
        }
        other => panic!("expected a duplicate admission, got {other:?}"),
    }
    assert!(
        merged.effect_intents().is_empty(),
        "merging a message is not a second execution grant"
    );
    assert_eq!(
        stored_message_count(&bound),
        1,
        "a duplicate creates no second dispatch"
    );
    assert_eq!(conversation_count(&bound.fixture.store), 1);
    assert_eq!(
        messages(&bound.fixture.store, &bound.fixture.conversation_id)[0].id,
        event_id
    );
}

#[test]
#[ignore = "requires the fixed LicoArc Candidate through LICOARC_AUTHORITY_BUNDLE"]
fn handshake_facts_are_not_a_message_unit() {
    let line = authority_line();
    let mut pair = establish(&line, 11);
    // The accepted handshake produces sealed facts, but it is not one committed
    // protected record, so it cannot be mapped into a peer message.
    assert_eq!(
        PeerMessage::of_verified(
            pair.host.facts(),
            MessageForwarder::Direct,
            body([15; 16], None, "not a record"),
        ),
        Err(crate::domain::mobile_relay::endpoint_transport::PeerUnitRefusal::NotARecord)
    );
    let _ = pair.peer.send(b"still usable");
}

// ---------------------------------------------------------------------------
// SDK-side confirmation helpers (same shapes the SDK's own entries require)
// ---------------------------------------------------------------------------

fn authorized_session(
    authenticated: bool,
    authorized: bool,
    sender: [u8; 32],
) -> AuthorizedSession {
    AuthorizedSession {
        session_id: [7; 16],
        sender_endpoint: sender,
        expected_sender_endpoint: sender,
        authenticated,
        sender_authorized: authorized,
    }
}

fn confirmation(
    stage: ConfirmationStage,
    outcome: ConfirmationOutcome,
    ids: Vec<[u8; 16]>,
    digest: Option<[u8; 32]>,
) -> EndpointConfirmation {
    EndpointConfirmation {
        confirmation_id: [3; 16],
        confirmed_message_ids: ids,
        stage,
        outcome,
        failure_code: None,
        result_digest: digest,
    }
}

/// The host authority constant is asserted here so a future handshake change
/// cannot silently re-key the fixture.
#[test]
fn the_fixture_authority_digests_are_fixed() {
    assert_eq!(PEER_AUTHORITY, [0xA1; 32]);
    assert_eq!(HOST_AUTHORITY, [0xB2; 32]);
}

/// Shape bounds are pure client-side validation, so they run without the
/// artifact.
#[test]
fn mapping_refuses_unbounded_or_malformed_parts() {
    assert_eq!(PeerPart::text(""), Err(PeerUnitRefusal::EmptyText));
    assert_eq!(
        PeerPart::text("x".repeat(MAX_PEER_TEXT_BYTES + 1)),
        Err(PeerUnitRefusal::TextTooLarge)
    );
    assert_eq!(PeerPart::text("nul\0byte"), Err(PeerUnitRefusal::NulByte));
    assert_eq!(
        PeerPart::command(json!(1)),
        Err(PeerUnitRefusal::CommandNotObject)
    );
    assert_eq!(
        PeerMessageBody::new([0; 16], None, Vec::new()),
        Err(PeerUnitRefusal::EmptyMessage)
    );
    assert_eq!(
        PeerMessageBody::new(
            [0; 16],
            None,
            vec![PeerPart::text("a").unwrap(); MAX_PEER_PARTS + 1]
        ),
        Err(PeerUnitRefusal::TooManyParts)
    );
    assert_eq!(
        StationRef::new(""),
        Err(PeerUnitRefusal::StationIdentityInvalid)
    );
    assert_eq!(
        StationRef::new(&"x".repeat(MAX_STATION_ID_BYTES + 1)),
        Err(PeerUnitRefusal::StationIdentityInvalid)
    );
    assert!(!StationHint::reported(true, true).is_endpoint_evidence());
}
