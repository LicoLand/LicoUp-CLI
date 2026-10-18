//! The mobile entry's contract fixture.
//!
//! Three claims are falsified here, each against the production owner rather
//! than a restatement of it:
//!
//! 1. **The mobile surface is one declared set, registered once.** The crate's
//!    surface and the ABI identity the bridge publishes are compared as sets,
//!    so a surface that grows in one place and not the other fails.
//! 2. **Admission is bounded.** Every surface operation reaches its owner;
//!    every desktop-only operation and every unknown operation is answered
//!    with the bounded refusal and reaches no owner at all.
//! 3. **The read model reads the Canonical Conversation authority.** The
//!    projections are checked against a real in-memory `ConversationStore`
//!    with a real group, real memberships and a real event, so a projection
//!    that drifted from the authority's own fields fails.

use std::cell::RefCell;

use anyhow::{Result, anyhow};
use licoup_conversation::{
    ConversationStore, EventKind, MembershipAccess, NewEventPart, Principal, PrincipalKind,
};
use licoup_mobile_core::{
    DESKTOP_ONLY_OPERATIONS, MOBILE_SURFACE, MobileEntry, MobileOperationHost, SurfaceGroup,
    UNSUPPORTED_OPERATION_CODE, is_mobile_operation, surface_group,
};
use licoup_platform_bridges::MOBILE_RUNTIME_OPERATIONS;
use serde_json::{Value, json};

/// One answering host that records what it was asked and answers the canonical
/// shape every owner answers.
#[derive(Default)]
struct RecordingHost {
    seen: RefCell<Vec<String>>,
}

impl RecordingHost {
    fn record(&self, operation: &str, params: &Value) -> Result<Value> {
        self.seen.borrow_mut().push(operation.to_owned());
        Ok(json!({ "ok": true, "action": operation, "params": params }))
    }

    fn seen(&self) -> Vec<String> {
        self.seen.borrow().clone()
    }
}

macro_rules! host_method {
    ($name:ident, $operation:literal) => {
        fn $name(&self, params: &Value) -> Result<Value> {
            self.record($operation, params)
        }
    };
}

impl MobileOperationHost for RecordingHost {
    host_method!(config_get, "mobile.relay.config.get");
    host_method!(config_set, "mobile.relay.config.set");
    host_method!(pairing_claim, "mobile.relay.pairing.claim");
    host_method!(pairing_status, "mobile.relay.pairing.status");
    host_method!(e2ee_status, "mobile.relay.e2ee.status");
    host_method!(command_create_secure, "mobile.relay.commands.createSecure");
    host_method!(command_result_secure, "mobile.relay.commands.resultSecure");
    host_method!(
        command_result_replay_proof,
        "mobile.relay.commands.resultReplayProof"
    );
    host_method!(conversation_create, "conversation.create");
    host_method!(conversation_list, "conversation.list");
    host_method!(conversation_get, "conversation.get");
    host_method!(conversation_events_page, "conversation.events.page");
    host_method!(conversation_message_post, "conversation.message.post");
    host_method!(conversation_membership_add, "conversation.membership.add");
    host_method!(
        conversation_membership_leave,
        "conversation.membership.leave"
    );
}

fn entry() -> MobileEntry<RecordingHost> {
    MobileEntry::new(RecordingHost::default())
}

#[test]
fn the_mobile_surface_and_the_registered_abi_identity_are_one_set() {
    let declared: Vec<&str> = MOBILE_SURFACE.to_vec();
    assert_eq!(
        declared, MOBILE_RUNTIME_OPERATIONS,
        "the entry's surface and the registered mobile ABI identity must be the same ordered set"
    );
    let mut sorted = declared.clone();
    sorted.sort_unstable();
    sorted.dedup();
    assert_eq!(
        sorted.len(),
        declared.len(),
        "an operation is declared twice on the mobile surface"
    );
}

#[test]
fn every_surface_operation_has_a_group_and_a_named_owner() {
    for operation in MOBILE_SURFACE {
        let group = surface_group(operation)
            .unwrap_or_else(|| panic!("{operation} is on the surface without a group"));
        assert!(
            !group.owner().trim().is_empty(),
            "{} has no answering owner",
            group.label()
        );
    }
    for group in SurfaceGroup::ALL {
        assert!(
            MOBILE_SURFACE
                .iter()
                .any(|operation| surface_group(operation) == Some(*group)),
            "group {} answers no operation on the surface",
            group.label()
        );
    }
    assert_eq!(
        surface_group("mobile.relay.pairing.claim"),
        Some(SurfaceGroup::Pairing)
    );
    assert_eq!(
        surface_group("mobile.relay.commands.createSecure"),
        Some(SurfaceGroup::Delivery)
    );
    assert_eq!(
        surface_group("conversation.list"),
        Some(SurfaceGroup::Group)
    );
    assert!(!is_mobile_operation("agent.conversation.send"));
}

#[test]
fn every_surface_operation_reaches_its_owner_and_no_other() -> Result<()> {
    let entry = entry();
    for operation in MOBILE_SURFACE {
        let response = entry.dispatch(&json!({
            "action": operation,
            "params": { "probe": operation },
        }))?;
        assert_eq!(
            response.get("ok").and_then(Value::as_bool),
            Some(true),
            "{operation} did not reach its owner"
        );
        assert_eq!(
            response.get("action").and_then(Value::as_str),
            Some(*operation)
        );
    }
    assert_eq!(
        entry.host().seen(),
        MOBILE_SURFACE.to_vec(),
        "each operation must reach exactly the one owner named for it"
    );
    Ok(())
}

#[test]
fn desktop_only_and_unknown_operations_are_refused_without_reaching_an_owner() -> Result<()> {
    let entry = entry();
    for operation in DESKTOP_ONLY_OPERATIONS
        .iter()
        .copied()
        .chain(["agent.conversation.send", "secure_mesh.mls.group.create"])
    {
        let response = entry.dispatch(&json!({ "action": operation }))?;
        assert_eq!(
            response.get("ok").and_then(Value::as_bool),
            Some(false),
            "{operation} was admitted"
        );
        assert_eq!(
            response.get("code").and_then(Value::as_str),
            Some(UNSUPPORTED_OPERATION_CODE)
        );
        assert_eq!(
            response.get("action").and_then(Value::as_str),
            Some(operation)
        );
    }
    assert!(
        entry.host().seen().is_empty(),
        "a refused operation reached an owner: {:?}",
        entry.host().seen()
    );
    Ok(())
}

#[test]
fn malformed_requests_are_refused_as_values() -> Result<()> {
    let entry = entry();
    for request in [
        json!("not an object"),
        json!({}),
        json!({ "action": 7 }),
        json!({ "action": "" }),
        json!({ "action": "conversation.list", "params": "not an object" }),
        json!({ "action": "x".repeat(129) }),
    ] {
        let response = entry.dispatch(&request)?;
        assert_eq!(
            response.get("ok").and_then(Value::as_bool),
            Some(false),
            "{request} was admitted"
        );
        assert_eq!(
            response.get("code").and_then(Value::as_str),
            Some("mobile_surface_invalid_request")
        );
    }
    assert!(entry.host().seen().is_empty());
    assert!(
        entry.dispatch_json("{").is_err(),
        "invalid JSON is an error"
    );
    assert!(
        entry
            .dispatch_json(&"x".repeat(2 * 1024 * 1024 + 1))
            .is_err(),
        "an oversized request is refused before it is parsed"
    );
    Ok(())
}

#[test]
fn an_answering_owners_failure_propagates_instead_of_becoming_a_success() {
    struct FailingHost;
    impl MobileOperationHost for FailingHost {
        fn config_get(&self, _params: &Value) -> Result<Value> {
            Err(anyhow!("owner_refused"))
        }
        fn config_set(&self, _params: &Value) -> Result<Value> {
            Err(anyhow!("owner_refused"))
        }
        fn pairing_claim(&self, _params: &Value) -> Result<Value> {
            Err(anyhow!("owner_refused"))
        }
        fn pairing_status(&self, _params: &Value) -> Result<Value> {
            Err(anyhow!("owner_refused"))
        }
        fn e2ee_status(&self, _params: &Value) -> Result<Value> {
            Err(anyhow!("owner_refused"))
        }
        fn command_create_secure(&self, _params: &Value) -> Result<Value> {
            Err(anyhow!("owner_refused"))
        }
        fn command_result_secure(&self, _params: &Value) -> Result<Value> {
            Err(anyhow!("owner_refused"))
        }
        fn command_result_replay_proof(&self, _params: &Value) -> Result<Value> {
            Err(anyhow!("owner_refused"))
        }
        fn conversation_create(&self, _params: &Value) -> Result<Value> {
            Err(anyhow!("owner_refused"))
        }
        fn conversation_list(&self, _params: &Value) -> Result<Value> {
            Err(anyhow!("owner_refused"))
        }
        fn conversation_get(&self, _params: &Value) -> Result<Value> {
            Err(anyhow!("owner_refused"))
        }
        fn conversation_events_page(&self, _params: &Value) -> Result<Value> {
            Err(anyhow!("owner_refused"))
        }
        fn conversation_message_post(&self, _params: &Value) -> Result<Value> {
            Err(anyhow!("owner_refused"))
        }
        fn conversation_membership_add(&self, _params: &Value) -> Result<Value> {
            Err(anyhow!("owner_refused"))
        }
        fn conversation_membership_leave(&self, _params: &Value) -> Result<Value> {
            Err(anyhow!("owner_refused"))
        }
    }

    let entry = MobileEntry::new(FailingHost);
    assert!(
        entry
            .dispatch(&json!({ "action": "conversation.list" }))
            .is_err()
    );
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

#[test]
fn the_read_model_reads_the_canonical_conversation_authority() -> Result<()> {
    let entry = entry();
    let store = ConversationStore::open_in_memory()?;
    let owner = principal("human:owner", PrincipalKind::Human, "Owner");
    let conversation = store.create_conversation_with_members(
        "Paired group",
        owner.clone(),
        &[
            (
                principal("human:peer", PrincipalKind::Human, "Peer Device"),
                MembershipAccess::Member,
            ),
            (
                principal("agent:assistant", PrincipalKind::Agent, "Assistant"),
                MembershipAccess::Member,
            ),
        ],
    )?;
    let owner_membership = conversation
        .memberships
        .iter()
        .find(|membership| membership.principal.id == owner.id)
        .map(|membership| membership.id.clone())
        .ok_or_else(|| anyhow!("the owner membership exists"))?;
    store.append_event(
        &conversation.id,
        Some(&owner_membership),
        EventKind::Message,
        &[NewEventPart {
            id: String::new(),
            kind: licoup_conversation::EventPartKind::Text,
            content: "hello from the paired client".to_owned(),
        }],
        None,
        None,
        true,
    )?;

    let list = entry.chat_list(&store.list(false)?);
    assert_eq!(list.conversations.len(), 1);
    assert!(!list.truncated);
    let card = &list.conversations[0];
    assert_eq!(card.id, conversation.id);
    assert_eq!(card.title, "Paired group");
    assert!(card.is_group);
    assert_eq!(card.membership_count, 3);
    // The authority records one `MembershipChanged` join event per added
    // member beside the message, so the card carries the authority's own
    // count rather than the count of what this fixture appended.
    assert_eq!(card.event_count, 3);

    let page = store.page_events(&conversation.id, None, 50)?;
    let thread = entry.thread(&store.get(&conversation.id)?, &page);
    assert_eq!(thread.conversation_id, conversation.id);
    assert_eq!(thread.members.len(), 3);
    assert_eq!(thread.total_count, 3);
    assert_eq!(thread.events.len(), 3);
    let message = thread
        .events
        .iter()
        .find(|event| event.text.is_some())
        .ok_or_else(|| anyhow!("the page carries the posted message"))?;
    assert_eq!(
        message.text.as_deref(),
        Some("hello from the paired client")
    );
    assert_eq!(
        message.author_membership_id.as_deref(),
        Some(owner_membership.as_str())
    );
    assert!(message.finalized);
    Ok(())
}

#[test]
fn the_read_model_pages_by_the_clients_own_bound() -> Result<()> {
    let entry = entry();
    let store = ConversationStore::open_in_memory()?;
    let owner = principal("human:owner", PrincipalKind::Human, "Owner");
    // A paired one-to-one thread. The fixture is about the mobile read bound,
    // not about group admission, so it does not have to seat an Agent
    // membership the way a hosted group does.
    let conversation = store.create_conversation("Bounded", owner.clone())?;
    let owner_membership = conversation
        .memberships
        .first()
        .map(|membership| membership.id.clone())
        .ok_or_else(|| anyhow!("the owner membership exists"))?;
    let bound = entry.settings().bounds().history_page_size;
    for index in 0..bound + 3 {
        store.append_event(
            &conversation.id,
            Some(&owner_membership),
            EventKind::Message,
            &[NewEventPart {
                id: String::new(),
                kind: licoup_conversation::EventPartKind::Text,
                content: format!("message {index}"),
            }],
            None,
            None,
            true,
        )?;
    }
    let page = store.page_events(&conversation.id, None, bound + 3)?;
    assert!(
        page.events.len() > bound,
        "the page must offer more than the mobile bound, or the bound proves nothing"
    );
    let thread = entry.thread(&store.get(&conversation.id)?, &page);
    assert_eq!(
        thread.events.len(),
        bound,
        "the mobile thread must not exceed the client's own history bound"
    );
    Ok(())
}
