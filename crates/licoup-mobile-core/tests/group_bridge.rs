//! The mobile-to-desktop group fixture.
//!
//! The claim this fixture exists for is a statement about *authority*: a paired
//! mobile client creates and lists a group, resolves its memberships, posts a
//! message and reconnects — and every one of those operations is answered by the
//! Canonical Conversation authority, with one group lifecycle and no duplicated
//! delivery.
//!
//! So the desktop side here is a real `ConversationStore` — the same authority
//! the desktop client reads — and the mobile side is the production
//! [`MobileGroupApplication`] over it. What stands in for production is the
//! *transport*: the relay would carry each request as a protected envelope with
//! an identity, and the desktop host would answer an envelope it has already
//! applied from its secure-command replay ledger. The fixture carries the
//! identity on the authority and keeps the replay record itself, and it asserts
//! the consequence the real path has to have: a re-driven envelope appends no
//! second Event.
//!
//! The position a delivery settles at is the conversation's own Event sequence,
//! because that is the one ordering the mobile client resumes reading by. A
//! conversation that has no Event yet answers position `0`, which is the same
//! cursor as reading it from the beginning.

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::rc::Rc;

use anyhow::{Result, anyhow};
use licoup_conversation::{
    ConversationStore, EventKind, MembershipAccess, MembershipStatus, NewEventPart, Principal,
    PrincipalKind,
};
use licoup_mobile_core::{
    GROUP_CONVERSATION_PARAM, MOBILE_GROUP_OPERATIONS, MobileGroupApplication, MobileOperationHost,
    is_read_only_group_operation,
};
use serde_json::{Value, json};

/// The desktop side of the fixture: the real Conversation authority, the relay
/// envelope currently being delivered, the answers already applied, and the one
/// envelope whose answer the fixture drops to model a lost connection.
struct DesktopAuthority {
    store: ConversationStore,
    owner: Principal,
    /// The envelope the relay is delivering. Production carries this on the
    /// protected envelope beside the request; the fixture's transport is a
    /// direct call, so it carries it here.
    envelope: RefCell<String>,
    /// The answer each already-applied envelope received, keyed by that
    /// envelope. This is the fixture's stand-in for the desktop host's
    /// secure-command replay ledger.
    applied: RefCell<BTreeMap<String, Value>>,
    /// The envelope whose answer never reaches the client, after the authority
    /// has already applied it.
    drops_answer: RefCell<Option<String>>,
}

impl DesktopAuthority {
    fn open(owner: Principal) -> Rc<Self> {
        Rc::new(Self {
            store: ConversationStore::open_in_memory().expect("the authority opens"),
            owner,
            envelope: RefCell::new(String::new()),
            applied: RefCell::new(BTreeMap::new()),
            drops_answer: RefCell::new(None),
        })
    }

    fn recorded(&self, envelope: &str) -> Option<Value> {
        self.applied.borrow().get(envelope).cloned()
    }

    /// The conversation's last Event sequence, or `0` when it has no Event yet.
    fn position(&self, conversation_id: &str) -> Result<i64> {
        Ok(self
            .store
            .page_events(conversation_id, None, 100)?
            .events
            .last()
            .map(|event| event.sequence)
            .unwrap_or_default())
    }

    /// Every Message Event the authority holds, so a duplicated delivery has
    /// somewhere to show up.
    fn message_count(&self, conversation_id: &str) -> Result<usize> {
        Ok(self
            .store
            .page_events(conversation_id, None, 100)?
            .events
            .into_iter()
            .filter(|event| event.kind == EventKind::Message)
            .count())
    }

    /// Deliver one request under one envelope identity, as the transport would.
    fn deliver(&self, host: &Host, envelope: &str, request: &Value) -> Result<Value> {
        *self.envelope.borrow_mut() = envelope.to_owned();
        let action = request
            .get("action")
            .and_then(Value::as_str)
            .ok_or_else(|| anyhow!("invalid_request"))?;
        let params = request.get("params").cloned().unwrap_or_else(|| json!({}));
        host.route(action, &params)
    }
}

/// The mobile operation host the desktop authority answers through.
struct Host {
    authority: Rc<DesktopAuthority>,
}

impl Host {
    /// Answer one operation.
    ///
    /// An envelope this authority has already applied is answered from its
    /// record, which is what makes a re-drive safe on the authority's side
    /// rather than only on the client's.
    fn route(&self, action: &str, params: &Value) -> Result<Value> {
        let envelope = self.authority.envelope.borrow().clone();
        if let Some(recorded) = self.authority.recorded(&envelope) {
            return Ok(recorded);
        }
        let answer = match action {
            "conversation.create" => self.create(params)?,
            "conversation.list" => self.list(params)?,
            "conversation.get" => self.get(params)?,
            "conversation.events.page" => self.events_page(params)?,
            "conversation.message.post" => self.message_post(params)?,
            "conversation.membership.add" => self.membership_add(params)?,
            "conversation.membership.leave" => self.membership_leave(params)?,
            _ => return Err(anyhow!("unsupported_operation")),
        };
        if answer.get("ok").and_then(Value::as_bool) == Some(true) {
            self.authority
                .applied
                .borrow_mut()
                .insert(envelope.clone(), answer.clone());
        }
        if self.authority.drops_answer.borrow().as_deref() == Some(envelope.as_str()) {
            *self.authority.drops_answer.borrow_mut() = None;
            return Err(anyhow!("relay_transport_unavailable"));
        }
        Ok(answer)
    }

    fn create(&self, params: &Value) -> Result<Value> {
        let title = text(params, "title").ok_or_else(|| anyhow!("conversation_title_required"))?;
        let members = params
            .get("members")
            .and_then(Value::as_array)
            .map(|members| {
                members
                    .iter()
                    .map(|member| {
                        Ok((
                            principal(
                                member
                                    .get("id")
                                    .and_then(Value::as_str)
                                    .ok_or_else(|| anyhow!("invalid_request"))?,
                                PrincipalKind::Agent,
                                member
                                    .get("displayName")
                                    .and_then(Value::as_str)
                                    .unwrap_or("Assistant"),
                            ),
                            MembershipAccess::Member,
                        ))
                    })
                    .collect::<Result<Vec<_>>>()
            })
            .transpose()?
            .unwrap_or_default();
        let conversation = self.authority.store.create_conversation_with_members(
            title,
            self.authority.owner.clone(),
            &members,
        )?;
        Ok(json!({
            "ok": true,
            "action": "conversation.create",
            "conversation": conversation,
            "sequence": self.authority.position(&conversation.id)?,
        }))
    }

    fn list(&self, params: &Value) -> Result<Value> {
        let include_archived = params
            .get("includeArchived")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        Ok(json!({
            "ok": true,
            "action": "conversation.list",
            "conversations": self.authority.store.list(include_archived)?,
        }))
    }

    fn get(&self, params: &Value) -> Result<Value> {
        let conversation_id = conversation_id(params).ok_or_else(|| anyhow!("invalid_request"))?;
        let conversation = self.authority.store.get(&conversation_id)?;
        Ok(json!({
            "ok": true,
            "action": "conversation.get",
            "conversation": conversation,
            "sequence": self.authority.position(&conversation_id)?,
        }))
    }

    fn events_page(&self, params: &Value) -> Result<Value> {
        let conversation_id = conversation_id(params).ok_or_else(|| anyhow!("invalid_request"))?;
        let limit = params.get("limit").and_then(Value::as_u64).unwrap_or(50) as usize;
        let page = self
            .authority
            .store
            .page_events(&conversation_id, None, limit)?;
        Ok(json!({
            "ok": true,
            "action": "conversation.events.page",
            "page": page,
            "sequence": self.authority.position(&conversation_id)?,
        }))
    }

    fn message_post(&self, params: &Value) -> Result<Value> {
        let conversation_id = conversation_id(params).ok_or_else(|| anyhow!("invalid_request"))?;
        let text = text(params, "text").ok_or_else(|| anyhow!("message_text_required"))?;
        let author = self.author_membership(&conversation_id);
        let event = self.authority.store.append_event(
            &conversation_id,
            author.as_deref(),
            EventKind::Message,
            &[NewEventPart {
                id: String::new(),
                kind: licoup_conversation::EventPartKind::Text,
                content: text.to_owned(),
            }],
            None,
            None,
            true,
        )?;
        Ok(json!({
            "ok": true,
            "action": "conversation.message.post",
            "event": event,
            "sequence": self.authority.position(&conversation_id)?,
        }))
    }

    fn membership_add(&self, params: &Value) -> Result<Value> {
        let conversation_id = conversation_id(params).ok_or_else(|| anyhow!("invalid_request"))?;
        let principal = principal(
            text(params, "principalId").ok_or_else(|| anyhow!("invalid_request"))?,
            PrincipalKind::Human,
            text(params, "displayName").unwrap_or("Paired device"),
        );
        let access = match text(params, "access") {
            Some("owner") => MembershipAccess::Owner,
            _ => MembershipAccess::Member,
        };
        let membership = self
            .authority
            .store
            .add_member(&conversation_id, principal, access)?;
        Ok(json!({
            "ok": true,
            "action": "conversation.membership.add",
            "membership": membership,
            "sequence": self.authority.position(&conversation_id)?,
        }))
    }

    fn membership_leave(&self, params: &Value) -> Result<Value> {
        let conversation_id = conversation_id(params).ok_or_else(|| anyhow!("invalid_request"))?;
        let membership_id =
            text(params, "membershipId").ok_or_else(|| anyhow!("invalid_request"))?;
        self.authority
            .store
            .leave_member(&conversation_id, membership_id)?;
        Ok(json!({
            "ok": true,
            "action": "conversation.membership.leave",
            "sequence": self.authority.position(&conversation_id)?,
        }))
    }

    /// The membership an unaddressed post is authored by: the conversation's
    /// owner when it has one, otherwise its first membership. The authority
    /// still decides whether the post is admitted.
    fn author_membership(&self, conversation_id: &str) -> Option<String> {
        let conversation = self.authority.store.get(conversation_id).ok()?;
        conversation
            .memberships
            .iter()
            .find(|membership| membership.access == MembershipAccess::Owner)
            .or_else(|| conversation.memberships.first())
            .map(|membership| membership.id.clone())
    }
}

impl MobileOperationHost for Host {
    fn config_get(&self, params: &Value) -> Result<Value> {
        self.route("mobile.relay.config.get", params)
    }
    fn config_set(&self, params: &Value) -> Result<Value> {
        self.route("mobile.relay.config.set", params)
    }
    fn pairing_claim(&self, params: &Value) -> Result<Value> {
        self.route("mobile.relay.pairing.claim", params)
    }
    fn pairing_status(&self, params: &Value) -> Result<Value> {
        self.route("mobile.relay.pairing.status", params)
    }
    fn e2ee_status(&self, params: &Value) -> Result<Value> {
        self.route("mobile.relay.e2ee.status", params)
    }
    fn command_create_secure(&self, params: &Value) -> Result<Value> {
        self.route("mobile.relay.commands.createSecure", params)
    }
    fn command_result_secure(&self, params: &Value) -> Result<Value> {
        self.route("mobile.relay.commands.resultSecure", params)
    }
    fn command_result_replay_proof(&self, params: &Value) -> Result<Value> {
        self.route("mobile.relay.commands.resultReplayProof", params)
    }
    fn conversation_create(&self, params: &Value) -> Result<Value> {
        self.route("conversation.create", params)
    }
    fn conversation_list(&self, params: &Value) -> Result<Value> {
        self.route("conversation.list", params)
    }
    fn conversation_get(&self, params: &Value) -> Result<Value> {
        self.route("conversation.get", params)
    }
    fn conversation_events_page(&self, params: &Value) -> Result<Value> {
        self.route("conversation.events.page", params)
    }
    fn conversation_message_post(&self, params: &Value) -> Result<Value> {
        self.route("conversation.message.post", params)
    }
    fn conversation_membership_add(&self, params: &Value) -> Result<Value> {
        self.route("conversation.membership.add", params)
    }
    fn conversation_membership_leave(&self, params: &Value) -> Result<Value> {
        self.route("conversation.membership.leave", params)
    }
}

fn conversation_id(params: &Value) -> Option<String> {
    params
        .get(GROUP_CONVERSATION_PARAM)
        .and_then(Value::as_str)
        .map(str::to_owned)
}

fn text<'a>(params: &'a Value, key: &str) -> Option<&'a str> {
    params.get(key).and_then(Value::as_str)
}

fn principal(id: &str, kind: PrincipalKind, display_name: &str) -> Principal {
    Principal {
        id: id.to_owned(),
        kind,
        display_name: display_name.to_owned(),
        agent_id: (kind == PrincipalKind::Agent).then(|| id.to_owned()),
        created_at_unix_ms: 1_700_000_000_000,
    }
}

/// The paired mobile client over the desktop authority it is paired with.
struct MobileClient {
    application: MobileGroupApplication<Host>,
    authority: Rc<DesktopAuthority>,
}

impl MobileClient {
    fn paired(authority: &Rc<DesktopAuthority>) -> Self {
        Self {
            application: MobileGroupApplication::new(
                Host {
                    authority: Rc::clone(authority),
                },
                16,
            ),
            authority: Rc::clone(authority),
        }
    }

    /// Dispatch one request as the protected envelope `envelope`.
    fn dispatch(&mut self, envelope: &str, request: &Value) -> Result<Value> {
        *self.authority.envelope.borrow_mut() = envelope.to_owned();
        self.application.dispatch(envelope, request)
    }
}

#[test]
fn a_paired_client_reaches_the_canonical_group_authority_over_its_envelopes() -> Result<()> {
    let authority = DesktopAuthority::open(principal("human:owner", PrincipalKind::Human, "Owner"));
    let mut client = MobileClient::paired(&authority);

    let created = client.dispatch(
        "envelope:create",
        &json!({
            "action": "conversation.create",
            "params": {
                "title": "Paired group",
                "members": [{ "id": "agent:assistant", "displayName": "Assistant" }],
            },
        }),
    )?;
    let conversation_id = created["conversation"]["id"]
        .as_str()
        .ok_or_else(|| anyhow!("the authority did not name the created conversation"))?
        .to_owned();

    let listed = client.dispatch(
        "envelope:list",
        &json!({ "action": "conversation.list", "params": { "includeArchived": false } }),
    )?;
    assert_eq!(
        listed["conversations"]
            .as_array()
            .map(Vec::len)
            .unwrap_or_default(),
        1,
        "the mobile list must read the group the authority created"
    );
    assert_eq!(listed["conversations"][0]["id"], json!(conversation_id));

    let joined = client.dispatch(
        "envelope:join",
        &json!({
            "action": "conversation.membership.add",
            "params": {
                "conversationId": conversation_id,
                "principalId": "human:peer",
                "displayName": "Paired device",
            },
        }),
    )?;
    let peer_membership = joined["membership"]["id"]
        .as_str()
        .ok_or_else(|| anyhow!("the authority did not name the added membership"))?
        .to_owned();
    let aggregate = authority.store.get(&conversation_id)?;
    assert_eq!(
        aggregate.memberships.len(),
        3,
        "the membership the mobile client added must be the authority's own record"
    );

    client.dispatch(
        "envelope:post",
        &json!({
            "action": "conversation.message.post",
            "params": { "conversationId": conversation_id, "text": "hello from the paired device" },
        }),
    )?;
    assert_eq!(
        authority.message_count(&conversation_id)?,
        1,
        "one posted message is one Event in the authority"
    );
    let page = client.dispatch(
        "envelope:page",
        &json!({
            "action": "conversation.events.page",
            "params": { "conversationId": conversation_id, "limit": 50 },
        }),
    )?;
    let events = page["page"]["events"]
        .as_array()
        .cloned()
        .ok_or_else(|| anyhow!("the authority answered no Event page"))?;
    assert!(
        events
            .iter()
            .any(|event| event["parts"][0]["content"] == json!("hello from the paired device")),
        "the mobile page reads the authority's own Event"
    );

    client.dispatch(
        "envelope:leave",
        &json!({
            "action": "conversation.membership.leave",
            "params": { "conversationId": conversation_id, "membershipId": peer_membership },
        }),
    )?;
    let aggregate = authority.store.get(&conversation_id)?;
    let retired = aggregate
        .memberships
        .iter()
        .find(|membership| membership.id == peer_membership)
        .ok_or_else(|| anyhow!("the retired membership is the authority's own record"))?;
    assert_eq!(
        retired.status,
        MembershipStatus::Left,
        "the retired membership is the authority's own record"
    );

    // Every dispatched envelope settled, so a reconnect has nothing to re-drive
    // and resumes after the last position the authority reported.
    let plan = client.application.reconnect(&conversation_id);
    assert!(plan.pending.is_empty());
    assert_eq!(
        plan.resume_after,
        Some(authority.position(&conversation_id)?)
    );
    Ok(())
}

#[test]
fn a_reconnect_re_drives_the_unanswered_envelope_and_the_authority_appends_once() -> Result<()> {
    let authority = DesktopAuthority::open(principal("human:owner", PrincipalKind::Human, "Owner"));
    let mut client = MobileClient::paired(&authority);

    let created = client.dispatch(
        "envelope:create",
        &json!({
            "action": "conversation.create",
            "params": {
                "title": "Reconnect",
                "members": [{ "id": "agent:assistant", "displayName": "Assistant" }],
            },
        }),
    )?;
    let conversation_id = created["conversation"]["id"]
        .as_str()
        .ok_or_else(|| anyhow!("the authority did not name the created conversation"))?
        .to_owned();

    // The post reaches the authority, but its answer never reaches the client:
    // the delivery stays pending, which is what a dropped connection looks like.
    *authority.drops_answer.borrow_mut() = Some("envelope:post".to_owned());
    let post = json!({
        "action": "conversation.message.post",
        "params": { "conversationId": conversation_id, "text": "sent once" },
    });
    let failure = client
        .dispatch("envelope:post", &post)
        .expect_err("the answer never arrived");
    assert_eq!(failure.to_string(), "relay_transport_unavailable");
    assert_eq!(
        authority.message_count(&conversation_id)?,
        1,
        "the authority applied the post the client never got an answer for"
    );

    // The reconnect names exactly that envelope and the request it carried.
    let plan = client.application.reconnect(&conversation_id);
    assert_eq!(
        plan.pending.len(),
        1,
        "a reconnect re-drives the one unanswered delivery"
    );
    assert_eq!(plan.pending[0].record.envelope_id, "envelope:post");
    assert_eq!(plan.pending[0].request, post);

    // While the delivery is pending the same identity cannot be recorded again,
    // so the client re-drives the envelope it has rather than composing a second
    // one under the same identity.
    let duplicate = client
        .dispatch("envelope:post", &post)
        .expect_err("a resend of one identity is refused");
    assert_eq!(duplicate.to_string(), "mobile_delivery_duplicate_envelope");

    // Re-driving under the same identity reaches the authority, which answers
    // what it already applied instead of appending a second Event.
    let redriven = authority.deliver(
        &Host {
            authority: Rc::clone(&authority),
        },
        &plan.pending[0].record.envelope_id,
        &plan.pending[0].request,
    )?;
    assert_eq!(redriven["ok"], json!(true));
    assert_eq!(
        authority.message_count(&conversation_id)?,
        1,
        "a re-driven envelope must not become a second delivery"
    );
    client
        .application
        .settle(
            "envelope:post",
            redriven["sequence"].as_i64().unwrap_or_default(),
        )
        .expect("the authority's own answer settles the delivery");
    assert!(
        client
            .application
            .reconnect(&conversation_id)
            .pending
            .is_empty()
    );
    Ok(())
}

#[test]
fn the_group_path_declares_only_surface_operations_and_orders_reads_and_changes() {
    for operation in MOBILE_GROUP_OPERATIONS {
        assert!(
            licoup_mobile_core::is_mobile_operation(operation),
            "{operation} is dispatched by the group path but is not on the mobile surface"
        );
        assert_eq!(
            is_read_only_group_operation(operation),
            matches!(
                *operation,
                "conversation.list" | "conversation.get" | "conversation.events.page"
            ),
            "{operation} is classified wrongly for settlement"
        );
    }
}
