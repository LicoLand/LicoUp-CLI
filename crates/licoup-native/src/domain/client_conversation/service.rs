use super::{
    ConversationStore, DirectTurn, DispatchState, ImageAttachment, ImageAttachmentReference,
    MembershipAccess, MembershipStatus, NewEventPart, Principal, PrincipalKind,
};
use crate::domain::assistant_continuity::ContinuityHost;
use crate::domain::assistant_continuity::cognition::{
    CompleteAdmittedTurn, apply_admitted_runtime_fields,
};
use crate::platform::runtime_adapters::{
    MAX_IMAGE_ATTACHMENT_BYTES_PER_FILE, MAX_IMAGE_ATTACHMENT_BYTES_TOTAL, MAX_IMAGE_ATTACHMENTS,
    attachment_media_type_supported,
};
use anyhow::{Result, anyhow};
use licoup_conversation::continuity::ContinuityReadPort;
use licoup_conversation::{DEFAULT_EVENT_PAGE_SIZE, EventPagePosition};
use serde_json::{Value, json};
use std::{
    fmt,
    path::Path,
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
};

type NativeTurnSender = dyn Fn(&Value) -> std::result::Result<Value, crate::platform::runtime_adapters::RuntimeAdapterError>
    + Send
    + Sync;
type ActiveTurnsLookup = dyn Fn(&str) -> Value + Send + Sync;
type TurnSteer = dyn Fn(&Value) -> std::result::Result<Value, crate::platform::runtime_adapters::RuntimeAdapterError>
    + Send
    + Sync;
type StrategyExecute = dyn Fn(Value) -> Result<Value> + Send + Sync;

/// Persistent host ports attached by the production conversation binder.
pub struct PersistentRuntimePorts {
    start_background: Arc<NativeTurnSender>,
    active: Arc<ActiveTurnsLookup>,
    steer: Arc<TurnSteer>,
    complete_admitted_turn: Arc<CompleteAdmittedTurn>,
    strategy: Arc<StrategyExecute>,
    cancel: Option<Arc<TurnSteer>>,
}

impl PersistentRuntimePorts {
    pub fn new(
        start_background: impl Fn(
            &Value,
        ) -> std::result::Result<
            Value,
            crate::platform::runtime_adapters::RuntimeAdapterError,
        > + Send
        + Sync
        + 'static,
        active: impl Fn(&str) -> Value + Send + Sync + 'static,
        steer: impl Fn(
            &Value,
        ) -> std::result::Result<
            Value,
            crate::platform::runtime_adapters::RuntimeAdapterError,
        > + Send
        + Sync
        + 'static,
        complete_admitted_turn: impl Fn(
            &Value,
        ) -> std::result::Result<
            Value,
            crate::platform::runtime_adapters::RuntimeAdapterError,
        > + Send
        + Sync
        + 'static,
        strategy: impl Fn(Value) -> Result<Value> + Send + Sync + 'static,
    ) -> Self {
        Self {
            start_background: Arc::new(start_background),
            active: Arc::new(active),
            steer: Arc::new(steer),
            complete_admitted_turn: Arc::new(complete_admitted_turn),
            strategy: Arc::new(strategy),
            cancel: None,
        }
    }

    pub fn with_cancel(
        mut self,
        cancel: impl Fn(
            &Value,
        ) -> std::result::Result<
            Value,
            crate::platform::runtime_adapters::RuntimeAdapterError,
        > + Send
        + Sync
        + 'static,
    ) -> Self {
        self.cancel = Some(Arc::new(cancel));
        self
    }
}

/// Upper bound on direct turns dispatched to native runtimes in parallel.
/// Each worker is an independent runtime call; state leases are held only
/// around short local transactions, never across the runtime call itself.
pub const DEFAULT_DIRECT_TURN_WORKERS: usize = 4;

/// The persistent host runtime seams one dispatch can use. Every field is
/// absent on a default-constructed service, so dispatch-type work fails
/// closed with a typed transport rejection instead of running through a
/// one-shot lane no observer can attach.
#[derive(Clone, Default)]
struct HostRuntimePorts {
    native_turn_sender: Option<Arc<NativeTurnSender>>,
    active_turns: Option<Arc<ActiveTurnsLookup>>,
    steer_turn: Option<Arc<TurnSteer>>,
    cancel_turn: Option<Arc<TurnSteer>>,
    complete_admitted_turn: Option<Arc<CompleteAdmittedTurn>>,
    strategy_execute: Option<Arc<StrategyExecute>>,
}

/// One native application service for CLI, FFI and admitted protocol adapters.
/// Transport adapters pass JSON envelopes here; domain validation remains in
/// `ConversationStore`. Strategy execution is a separate native authority.
#[derive(Clone)]
pub struct ConversationService {
    store: ConversationStore,
    host: HostRuntimePorts,
    continuity: Option<Arc<ContinuityHost>>,
}

impl fmt::Debug for ConversationService {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ConversationService")
            .field("store", &self.store)
            .finish_non_exhaustive()
    }
}

fn drain_json(drain: &crate::domain::assistant_continuity::host::WakeDrain) -> Value {
    json!({
        "consumed": drain.consumed,
        "reconciled": drain.reconciled,
        "replayed": drain.replayed,
        "waiting": drain.waiting,
        "reevaluated": drain.reevaluated,
        "preserved": drain.preserved,
        "noOps": drain.no_ops.iter().map(|(id, reason)| json!({
            "logicalWakeId": id,
            "reason": reason,
        })).collect::<Vec<_>>(),
    })
}

impl ConversationService {
    pub fn open(portable_root: &Path) -> Result<Self> {
        let store = ConversationStore::open(portable_root)?;
        store.ensure_default_local_group()?;
        let continuity = ContinuityHost::attach(store.clone())?;
        Ok(Self {
            store,
            host: HostRuntimePorts::default(),
            continuity: Some(continuity),
        })
    }

    pub fn from_store(store: ConversationStore) -> Self {
        let continuity = ContinuityHost::attach(store.clone()).ok();
        Self {
            store,
            host: HostRuntimePorts::default(),
            continuity,
        }
    }

    pub fn continuity(&self) -> Option<&Arc<ContinuityHost>> {
        self.continuity.as_ref()
    }

    pub fn claim_continuity_owner(&self) -> Result<()> {
        let Some(continuity) = &self.continuity else {
            return Ok(());
        };
        continuity
            .claim_continuity_owner()
            .map_err(|err| anyhow!(format!("{:?}", err.code)))
    }

    pub fn drain_continuity(&self, conversation_id: &str) -> Result<Value> {
        let Some(continuity) = &self.continuity else {
            return Ok(json!({ "consumed": [], "reconciled": [], "replayed": 0 }));
        };
        let drain = continuity
            .drain_wakes(conversation_id)
            .map_err(|err| anyhow!(format!("{:?}", err.code)))?;
        Ok(drain_json(&drain))
    }

    pub fn attend_due(&self) -> Result<Value> {
        let Some(continuity) = &self.continuity else {
            return Ok(json!({ "consumed": [], "reconciled": [], "replayed": 0 }));
        };
        let drain = continuity
            .attend_due()
            .map_err(|err| anyhow!(format!("{:?}", err.code)))?;
        Ok(drain_json(&drain))
    }

    pub fn after_runtime_settlement(
        &self,
        conversation_id: &str,
        payload: &Value,
    ) -> Result<Value> {
        let Some(continuity) = &self.continuity else {
            return Ok(json!({ "consumed": [], "reconciled": [], "replayed": 0 }));
        };
        let drain = continuity
            .after_runtime_settlement(conversation_id, payload)
            .map_err(|err| anyhow!(format!("{:?}", err.code)))?;
        Ok(drain_json(&drain))
    }

    /// Route native Agent work through a process-owned coordinator while
    /// keeping Conversation orchestration and persistence in this service.
    pub fn with_native_turn_sender(
        mut self,
        native_turn_sender: impl Fn(
            &Value,
        ) -> std::result::Result<
            Value,
            crate::platform::runtime_adapters::RuntimeAdapterError,
        > + Send
        + Sync
        + 'static,
    ) -> Self {
        self.host.native_turn_sender = Some(Arc::new(native_turn_sender));
        self
    }

    pub fn with_active_turns(
        mut self,
        active_turns: impl Fn(&str) -> Value + Send + Sync + 'static,
    ) -> Self {
        self.host.active_turns = Some(Arc::new(active_turns));
        self
    }

    pub fn with_steer_turn(
        mut self,
        steer_turn: impl Fn(
            &Value,
        ) -> std::result::Result<
            Value,
            crate::platform::runtime_adapters::RuntimeAdapterError,
        > + Send
        + Sync
        + 'static,
    ) -> Self {
        self.host.steer_turn = Some(Arc::new(steer_turn));
        self
    }

    pub fn with_strategy_execute(
        mut self,
        strategy_execute: impl Fn(Value) -> Result<Value> + Send + Sync + 'static,
    ) -> Self {
        self.host.strategy_execute = Some(Arc::new(strategy_execute));
        self
    }

    /// Production composition binder. The persistent host calls this same
    /// function; tests inject only the final complete-turn effect.
    pub fn bind_conversation_runtime(mut self, ports: PersistentRuntimePorts) -> Self {
        let start = Arc::clone(&ports.start_background);
        let steer = Arc::clone(&ports.steer);
        let cancel = ports.cancel.clone();
        self.host.native_turn_sender = Some(Arc::clone(&start));
        self.host.active_turns = Some(ports.active);
        self.host.steer_turn = Some(Arc::clone(&steer));
        self.host.cancel_turn = cancel.clone();
        self.host.complete_admitted_turn = Some(Arc::clone(&ports.complete_admitted_turn));
        self.host.strategy_execute = Some(ports.strategy);
        if let Some(host) = self.continuity.as_ref() {
            host.bind_work_turn_start(start);
            host.bind_persistent_cognition(ports.complete_admitted_turn);
            if let Some(cancel) = cancel {
                host.bind_work_turn_control(steer, cancel);
            }
        }
        self
    }

    #[cfg(test)]
    fn from_store_with_runtime(
        store: ConversationStore,
        native_turn_sender: impl Fn(
            &Value,
        ) -> std::result::Result<
            Value,
            crate::platform::runtime_adapters::RuntimeAdapterError,
        > + Send
        + Sync
        + 'static,
    ) -> Self {
        Self::from_store(store).with_native_turn_sender(native_turn_sender)
    }

    pub fn store(&self) -> &ConversationStore {
        &self.store
    }

    pub fn execute(&self, request: Value) -> Result<Value> {
        let object = request
            .as_object()
            .ok_or_else(|| anyhow!("invalid_request"))?;
        let action = object
            .get("action")
            .and_then(Value::as_str)
            .ok_or_else(|| anyhow!("invalid_request"))?;
        ensure_allowed_fields(action, object)?;
        match action {
            "conversation.create" => {
                let title = required_string(object, "title")?;
                let owner = principal_from_value(
                    object
                        .get("owner")
                        .ok_or_else(|| anyhow!("invalid_request"))?,
                )?;
                let members = object
                    .get("members")
                    .and_then(Value::as_array)
                    .map(|values| {
                        values
                            .iter()
                            .map(|value| {
                                let member = value
                                    .as_object()
                                    .ok_or_else(|| anyhow!("invalid_request"))?;
                                ensure_member_fields(member)?;
                                let principal = principal_from_value(
                                    member
                                        .get("principal")
                                        .ok_or_else(|| anyhow!("invalid_request"))?,
                                )?;
                                let access = serde_json::from_value(
                                    member
                                        .get("access")
                                        .cloned()
                                        .unwrap_or_else(|| json!("member")),
                                )?;
                                Ok((principal, access))
                            })
                            .collect::<Result<Vec<_>>>()
                    })
                    .transpose()?
                    .unwrap_or_default();
                let is_group = object
                    .get("isGroup")
                    .map(|value| value.as_bool().ok_or_else(|| anyhow!("invalid_request")))
                    .transpose()?
                    .unwrap_or(true);
                let created = if is_group {
                    self.store
                        .create_conversation_with_members(title, owner, &members)?
                } else {
                    self.store
                        .create_direct_conversation_with_members(title, owner, &members)?
                };
                Ok(serde_json::to_value(created)?)
            }
            "conversation.rename" => {
                self.store.rename_conversation(
                    required_string(object, "conversationId")?,
                    required_string(object, "title")?,
                )?;
                Ok(json!({"ok": true, "status": "accepted"}))
            }
            "conversation.archive" => {
                let conversation_id = required_string(object, "conversationId")?;
                let archived = object
                    .get("archived")
                    .and_then(Value::as_bool)
                    .ok_or_else(|| anyhow!("invalid_request"))?;
                let reopen = object
                    .get("reopen")
                    .and_then(Value::as_bool)
                    .unwrap_or(false);
                let report =
                    self.store
                        .archive_conversation_tree(conversation_id, archived, reopen)?;
                // Disconnecting the archived conversations' native transports
                // must not hold the response: the foreground refresh returns
                // immediately and the teardown settles off the lane.
                if archived && !report.archived_native_sessions.is_empty() {
                    let sessions = report.archived_native_sessions.clone();
                    std::thread::spawn(move || {
                        for session in sessions {
                            let _ = crate::agent_port::cleanup(&json!({
                                "agent": session.agent_id,
                                "nativeSessionId": session.native_session_id,
                            }));
                        }
                    });
                }
                let mut value = serde_json::to_value(&report)?;
                value["ok"] = json!(true);
                value["status"] = json!("accepted");
                if let Some(successor_id) = &report.successor_conversation_id {
                    value["successor"] = serde_json::to_value(self.store.get(successor_id)?)?;
                }
                Ok(value)
            }
            "conversation.clear" => {
                let conversation_id = required_string(object, "conversationId")?;
                if !self.active_turns_for(conversation_id).is_empty() {
                    return Err(anyhow!("conversation_clear_blocked"));
                }
                let report = self.store.clear_conversation_history(
                    conversation_id,
                    required_string(object, "ownerMembershipId")?,
                )?;
                let mut value = serde_json::to_value(report)?;
                value["ok"] = json!(true);
                Ok(value)
            }
            "conversation.pin.set" => {
                self.store.set_conversation_pinned(
                    required_string(object, "conversationId")?,
                    object
                        .get("pinned")
                        .and_then(Value::as_bool)
                        .ok_or_else(|| anyhow!("invalid_request"))?,
                )?;
                Ok(json!({"ok": true, "status": "accepted"}))
            }
            "conversation.strategy.set" => {
                let strategy_revision = match object.get("strategyRevision") {
                    Some(Value::Null) => None,
                    Some(Value::String(value)) if value.trim().is_empty() => None,
                    Some(Value::String(value)) => Some(value.as_str()),
                    _ => return Err(anyhow!("invalid_request")),
                };
                self.store.set_conversation_strategy_revision(
                    required_string(object, "conversationId")?,
                    strategy_revision,
                )?;
                Ok(json!({"ok": true}))
            }
            "conversation.assistant.set" => {
                let membership_id = match object.get("membershipId") {
                    Some(Value::Null) => None,
                    Some(Value::String(value)) if value.trim().is_empty() => None,
                    Some(Value::String(value)) => Some(value.as_str()),
                    _ => return Err(anyhow!("invalid_request")),
                };
                self.store.set_conversation_assistant(
                    required_string(object, "conversationId")?,
                    required_string(object, "ownerMembershipId")?,
                    required_revision(object, "expectedRevision")?,
                    membership_id,
                )?;
                Ok(json!({"ok": true}))
            }
            "conversation.profile.update" => {
                let intent: super::ProfileIntentUpdate = serde_json::from_value(
                    object
                        .get("intent")
                        .cloned()
                        .ok_or_else(|| anyhow!("invalid_request"))?,
                )?;
                let profile = self.store.set_membership_profile(
                    required_string(object, "conversationId")?,
                    required_string(object, "membershipId")?,
                    required_string(object, "ownerMembershipId")?,
                    required_revision(object, "expectedRevision")?,
                    &intent,
                )?;
                Ok(json!({"ok": true, "profile": serde_json::to_value(profile)?}))
            }
            "conversation.profile.get" => {
                let profile = self
                    .store
                    .membership_profile(required_string(object, "membershipId")?)?;
                Ok(serde_json::to_value(profile)?)
            }
            "conversation.profile.native_roles" => Ok(json!({
                "ok": true,
                "roles": crate::domain::native_roles::list()
                    .into_iter()
                    .map(|role| role.public_projection())
                    .collect::<Vec<_>>(),
            })),
            "conversation.profile.candidates" => {
                let conversation_id = required_string(object, "conversationId")?;
                let filters: super::CandidateFilters = serde_json::from_value(
                    object.get("filters").cloned().unwrap_or_else(|| json!({})),
                )?;
                // The selection policy this host adopted is read once, at the
                // boundary a new task is admitted through, and captured for
                // this request: the ranking below runs under it, and the
                // receipt freezes the revision it ran under.
                let selection_policy = super::selection_policy::current_binding();
                let filters = selection_policy.apply_to_filters(&filters);
                let pairs = self.profile_projection_pairs(conversation_id)?;
                let authority = super::production_snapshot_authority();
                let snapshots = super::project_profile_snapshots(
                    conversation_id,
                    &pairs,
                    &authority,
                    &crate::workflow_host::ProductionModelFacts,
                );
                let candidates =
                    super::rank_candidates(snapshots, &filters).map_err(anyhow::Error::msg)?;
                Ok(json!({
                    "candidates": serde_json::to_value(&candidates)?,
                    "routeReceipt": route_receipt_under(
                        conversation_id,
                        &candidates,
                        &selection_policy,
                    ),
                    "timeoutPolicy": crate::domain::dispatch_timeout_policy::policy_envelope(
                        &crate::domain::dispatch_timeout_policy::load_or_default(),
                    ),
                }))
            }
            "timeout.policy.get" => {
                let mut envelope = crate::domain::dispatch_timeout_policy::policy_envelope(
                    &crate::domain::dispatch_timeout_policy::load_or_default(),
                );
                envelope["ok"] = json!(true);
                Ok(envelope)
            }
            "timeout.policy.set" => {
                let policy = serde_json::from_value(
                    object
                        .get("policy")
                        .cloned()
                        .ok_or_else(|| anyhow!("invalid_request"))?,
                )?;
                let stored = crate::domain::dispatch_timeout_policy::store(&policy)
                    .map_err(anyhow::Error::msg)?;
                let mut envelope = crate::domain::dispatch_timeout_policy::policy_envelope(&stored);
                envelope["ok"] = json!(true);
                Ok(envelope)
            }
            "conversation.list" => {
                let mut listed = serde_json::to_value(
                    self.store.list(
                        object
                            .get("includeArchived")
                            .and_then(Value::as_bool)
                            .unwrap_or(false),
                    )?,
                )?;
                if let Some(continuity) = &self.continuity {
                    continuity.annotate_list(&mut listed);
                }
                Ok(listed)
            }
            "conversation.get" => {
                let conversation_id = required_string(object, "conversationId")?;
                let mut value = serde_json::to_value(self.store.get(conversation_id)?)?;
                if object
                    .get("includeNativeSessionReferences")
                    .and_then(Value::as_bool)
                    == Some(true)
                {
                    value["nativeSessionReferences"] = serde_json::to_value(
                        self.store.native_session_references(conversation_id)?,
                    )?;
                }
                if let Some(continuity) = &self.continuity {
                    continuity.enrich_get(&mut value, conversation_id);
                }
                Ok(value)
            }
            "conversation.events.page" => Ok(serde_json::to_value(
                self.store.page_events_window(
                    required_string(object, "conversationId")?,
                    event_page_position(object)?,
                    object
                        .get("limit")
                        .and_then(Value::as_u64)
                        .map(|limit| limit as usize)
                        .unwrap_or(DEFAULT_EVENT_PAGE_SIZE),
                )?,
            )?),
            "conversation.events.search" => Ok(serde_json::to_value(self.store.search(
                required_string(object, "query")?,
                object.get("limit").and_then(Value::as_u64).unwrap_or(50) as usize,
            )?)?),
            "conversation.event.append" => {
                let parts = object
                    .get("parts")
                    .and_then(Value::as_array)
                    .ok_or_else(|| anyhow!("invalid_request"))?
                    .iter()
                    .map(|part| serde_json::from_value::<NewEventPart>(part.clone()))
                    .collect::<std::result::Result<Vec<_>, _>>()?;
                let event = self.store.append_event(
                    required_string(object, "conversationId")?,
                    object.get("authorMembershipId").and_then(Value::as_str),
                    serde_json::from_value(
                        object
                            .get("kind")
                            .cloned()
                            .ok_or_else(|| anyhow!("invalid_request"))?,
                    )?,
                    &parts,
                    object.get("causationId").and_then(Value::as_str),
                    object.get("correlationId").and_then(Value::as_str),
                    object
                        .get("finalized")
                        .and_then(Value::as_bool)
                        .unwrap_or(false),
                )?;
                Ok(serde_json::to_value(event)?)
            }
            "conversation.message.post" => {
                let conversation_id = required_string(object, "conversationId")?;
                let author = object.get("authorMembershipId").and_then(Value::as_str);
                let attachments = admit_post_attachments(object.get("attachments"))?;
                // The text stays required unless admitted attachments carry the
                // post: an image-only group message posts empty content.
                let content = object
                    .get("content")
                    .and_then(Value::as_str)
                    .ok_or_else(|| anyhow!("invalid_request"))?;
                if content.trim().is_empty() && attachments.is_empty() {
                    return Err(anyhow!("invalid_request"));
                }
                self.persist_posted_message(
                    conversation_id,
                    author,
                    content,
                    object.get("correlationId").and_then(Value::as_str),
                    &attachments,
                )
            }
            "conversation.message.delete" => {
                self.store.delete_posted_message(
                    required_string(object, "conversationId")?,
                    required_string(object, "eventId")?,
                    required_string(object, "ownerMembershipId")?,
                )?;
                Ok(json!({"ok": true}))
            }
            "conversation.dispatch.after-post" => {
                let conversation_id = required_string(object, "conversationId")?;
                let event_id = required_string(object, "eventId")?;
                let suppress_assistant = object
                    .get("suppressAssistant")
                    .map(|value| value.as_bool().ok_or_else(|| anyhow!("invalid_request")))
                    .transpose()?
                    .unwrap_or(false);
                self.dispatch_posted_message(conversation_id, event_id, suppress_assistant)
            }
            "conversation.event.part.append" => {
                let part: NewEventPart = serde_json::from_value(
                    object
                        .get("part")
                        .cloned()
                        .ok_or_else(|| anyhow!("invalid_request"))?,
                )?;
                Ok(serde_json::to_value(self.store.append_event_part(
                    required_string(object, "eventId")?,
                    part,
                )?)?)
            }
            "conversation.event.finalize" => {
                self.store
                    .finalize_event(required_string(object, "eventId")?)?;
                Ok(json!({"ok": true}))
            }
            "conversation.membership.add" => {
                let access: MembershipAccess = serde_json::from_value(
                    object
                        .get("access")
                        .cloned()
                        .unwrap_or_else(|| json!("member")),
                )?;
                let conversation_id = required_string(object, "conversationId")?;
                if let Some(native_role_id) = object
                    .get("nativeRoleId")
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                {
                    return self.admit_native_role(
                        conversation_id,
                        required_string(object, "ownerMembershipId")?,
                        native_role_id,
                        access,
                    );
                }
                let principal = principal_from_value(
                    object
                        .get("principal")
                        .ok_or_else(|| anyhow!("invalid_request"))?,
                )?;
                Ok(serde_json::to_value(self.store.add_member(
                    conversation_id,
                    principal,
                    access,
                )?)?)
            }
            "conversation.membership.leave" => {
                self.store.leave_member(
                    required_string(object, "conversationId")?,
                    required_string(object, "membershipId")?,
                )?;
                Ok(json!({"ok": true}))
            }
            "conversation.membership.access.set" => {
                let access: MembershipAccess = serde_json::from_value(
                    object
                        .get("access")
                        .cloned()
                        .ok_or_else(|| anyhow!("invalid_request"))?,
                )?;
                Ok(serde_json::to_value(self.store.set_member_access(
                    required_string(object, "conversationId")?,
                    required_string(object, "membershipId")?,
                    access,
                )?)?)
            }
            "conversation.export" => {
                let ids = object
                    .get("conversationIds")
                    .and_then(Value::as_array)
                    .map(|values| {
                        values
                            .iter()
                            .filter_map(Value::as_str)
                            .map(str::to_owned)
                            .collect::<Vec<_>>()
                    })
                    .unwrap_or_default();
                self.store
                    .export_bundle(Path::new(required_string(object, "path")?), &ids)
            }
            "conversation.import" => self
                .store
                .import_bundle(Path::new(required_string(object, "path")?)),
            "conversation.subagent.edge" => {
                let edge = self.store.subagent_mesh_edge(
                    required_string(object, "conversationId")?,
                    required_string(object, "callerMembershipId")?,
                    required_string(object, "targetMembershipId")?,
                )?;
                Ok(json!({
                    "inbound": {
                        "delegate": edge.inbound_delegate,
                        "continue": edge.inbound_continue,
                        "cancel": edge.inbound_cancel,
                    },
                    "outcomes": {
                        "delegate": edge.delegate_outcome,
                        "continue": edge.continue_outcome,
                        "cancel": edge.cancel_outcome,
                    },
                    "claimState": edge.claim_state,
                    "dispatchState": edge.dispatch_state,
                }))
            }
            "conversation.subagent.target" => {
                let conversation_id = required_string(object, "conversationId")?;
                let membership_id = required_string(object, "membershipId")?;
                let conversation = self.store.get(conversation_id)?;
                let membership = conversation
                    .memberships
                    .iter()
                    .find(|membership| membership.id == membership_id)
                    .filter(|membership| membership.status == MembershipStatus::Active)
                    .filter(|membership| membership.principal.kind == PrincipalKind::Agent)
                    .ok_or_else(|| anyhow!("subagent_target_membership_inactive"))?;
                let provider_id = membership
                    .principal
                    .agent_id
                    .as_deref()
                    .ok_or_else(|| anyhow!("subagent_target_invalid"))?;
                let profile = self.store.membership_profile(membership_id)?;
                Ok(json!({
                    "providerId": provider_id,
                    "preferredModel": profile.as_ref().and_then(|profile| profile.preferred_model.as_deref()),
                    "preferredReasoningEffort": profile.as_ref().and_then(|profile| profile.preferred_reasoning_effort.as_deref()),
                    "preferredPermissionMode": profile.as_ref().and_then(|profile| profile.preferred_permission_mode.as_deref()),
                }))
            }
            "conversation.subagent.claim" => {
                let claim = self.store.claim_subagent_dispatch(
                    required_string(object, "conversationId")?,
                    required_string(object, "callerMembershipId")?,
                    required_string(object, "targetMembershipId")?,
                    object.get("parentDispatchId").and_then(Value::as_str),
                )?;
                Ok(subagent_claim_json(&claim))
            }
            "conversation.subagent.claim.update" => {
                self.store.update_subagent_claim_state(
                    required_string(object, "dispatchId")?,
                    subagent_claim_state(required_string(object, "state")?)?,
                )?;
                Ok(json!({"ok": true}))
            }
            "conversation.subagent.claim.active" => Ok(self
                .store
                .active_subagent_claim(
                    required_string(object, "conversationId")?,
                    required_string(object, "callerMembershipId")?,
                    required_string(object, "targetMembershipId")?,
                )?
                .as_ref()
                .map(subagent_claim_json)
                .unwrap_or(Value::Null)),
            "conversation.subagent.inbound.record" => {
                self.store.record_subagent_mcp_inbound(
                    required_string(object, "conversationId")?,
                    object.get("callerMembershipId").and_then(Value::as_str),
                    object.get("targetMembershipId").and_then(Value::as_str),
                    required_string(object, "tool")?,
                    required_string(object, "outcome")?,
                )?;
                Ok(json!({"ok": true}))
            }
            "conversation.subagent.binding.get" => {
                let binding = self.store.private_runtime_binding(
                    required_string(object, "conversationId")?,
                    required_string(object, "membershipId")?,
                )?;
                Ok(binding.map_or(Value::Null, |binding| {
                    json!({
                        "runtimeSessionId": binding.runtime_session_id,
                        "runtimeConversationPath": binding.runtime_conversation_path,
                        "workingDirectory": binding.working_directory,
                    })
                }))
            }
            "apply-interpretation"
            | "correct-association"
            | "revise-agreement"
            | "propose-criterion-change"
            | "pause-goal"
            | "resume-goal"
            | "request-cancel"
            | "accept-evidence"
            | "close-goal"
            | "replace-assistant"
            | "admit-task-child"
            | "list-pending-completion-notices"
            | "ack-completion-notices"
            | "resolve-completion-notice"
            | "set-adoption-enabled" => self.execute_continuity_command(action, object),
            _ => Err(anyhow!("unsupported_action")),
        }
    }

    fn persist_posted_message(
        &self,
        conversation_id: &str,
        author: Option<&str>,
        content: &str,
        correlation_id: Option<&str>,
        attachments: &[ImageAttachment],
    ) -> Result<Value> {
        let (event, _) = self.store.post_message_with_attachments(
            conversation_id,
            author,
            content,
            correlation_id,
            &[],
            attachments,
        )?;
        let mut ingress = json!(null);
        if let Some(continuity) = &self.continuity {
            if continuity.uses_scripted_cognition() {
                ingress = match continuity.after_user_event(conversation_id, &event.id) {
                    Ok(outcome) => json!({
                        "committed": outcome.committed,
                        "abstained": outcome.abstained,
                        "childConversationId": outcome.child_conversation_id,
                        "receiptRevision": outcome.receipt_revision,
                        "invocationCount": outcome.invocation_count,
                        "unavailable": outcome.unavailable,
                    }),
                    Err(err) => json!({
                        "committed": false,
                        "abstained": true,
                        "error": format!("{:?}", err.code),
                    }),
                };
            }
        }
        Ok(json!({
            "event": {"id": event.id, "state": "finalized"},
            "directTurns": [],
            "turns": [],
            "dispatchPending": false,
            "continuityIngress": ingress,
            "continuityDrain": json!(null),
        }))
    }

    fn execute_continuity_command(
        &self,
        action: &str,
        object: &serde_json::Map<String, Value>,
    ) -> Result<Value> {
        let Some(continuity) = &self.continuity else {
            return Err(anyhow!("unsupported_action"));
        };
        let conversation_id = required_string(object, "conversationId")?;
        match action {
            "apply-interpretation" | "correct-association" | "propose-criterion-change" => {
                let proposal = serde_json::from_value(
                    object
                        .get("proposal")
                        .cloned()
                        .ok_or_else(|| anyhow!("invalid_request"))?,
                )?;
                let receipt = continuity
                    .commit_fresh(proposal)
                    .map_err(|err| anyhow!(format!("{:?}", err.code)))?;
                Ok(json!({
                    "ok": true,
                    "revision": receipt.revision,
                    "conversationId": receipt.conversation_id,
                }))
            }
            "pause-goal" => {
                let progress = continuity
                    .pause_goal(conversation_id, required_string(object, "goalId")?)
                    .map_err(|err| anyhow!(format!("{:?}", err.code)))?;
                Ok(serde_json::to_value(progress)?)
            }
            "resume-goal" => {
                let progress = continuity
                    .resume_goal(conversation_id, required_string(object, "goalId")?)
                    .map_err(|err| anyhow!(format!("{:?}", err.code)))?;
                Ok(serde_json::to_value(progress)?)
            }
            "request-cancel" => {
                let progress = continuity
                    .request_cancel(conversation_id, required_string(object, "goalId")?)
                    .map_err(|err| anyhow!(format!("{:?}", err.code)))?;
                Ok(serde_json::to_value(progress)?)
            }
            "close-goal" => {
                let transition = serde_json::from_value(
                    object
                        .get("transition")
                        .cloned()
                        .ok_or_else(|| anyhow!("invalid_request"))?,
                )?;
                let progress = serde_json::from_value(
                    object
                        .get("progress")
                        .cloned()
                        .ok_or_else(|| anyhow!("invalid_request"))?,
                )?;
                let notice = continuity
                    .accept_goal_completion(conversation_id, &transition, &progress)
                    .map_err(|err| anyhow!(format!("{:?}", err.code)))?;
                let mut result = json!({
                    "ok": true,
                    "accepted": notice.is_some(),
                    "notificationId": notice.as_ref().map(|item| item.notification_id.clone()),
                    "goalId": notice.as_ref().map(|item| item.goal_id.clone()),
                    "parentConversationId": conversation_id,
                });
                if let Some(item) = &notice {
                    if let Ok(relation) = self.store.relation_for_goal(&item.goal_id) {
                        result["childConversationId"] = json!(relation.child_conversation_id);
                        result["cardEventId"] = json!(relation.card_anchor.event_id);
                        result["cardSequence"] = json!(relation.card_anchor.sequence);
                    }
                }
                Ok(result)
            }
            "list-pending-completion-notices" => {
                let notices = continuity
                    .list_pending_completion_notices(
                        conversation_id,
                        required_string(object, "ownerMembershipId")?,
                    )
                    .map_err(|err| anyhow!(format!("{:?}", err.code)))?;
                Ok(json!({ "pendingCompletionNotices": notices }))
            }
            "ack-completion-notices" => {
                let ids = object
                    .get("notificationIds")
                    .and_then(Value::as_array)
                    .ok_or_else(|| anyhow!("invalid_request"))?
                    .iter()
                    .filter_map(Value::as_str)
                    .map(str::to_owned)
                    .collect::<Vec<_>>();
                let acknowledged = continuity
                    .ack_completion_notices(
                        conversation_id,
                        required_string(object, "ownerMembershipId")?,
                        &ids,
                    )
                    .map_err(|err| anyhow!(format!("{:?}", err.code)))?;
                Ok(json!({ "acknowledgedNotificationIds": acknowledged }))
            }
            "resolve-completion-notice" => continuity
                .resolve_completion_notice(
                    conversation_id,
                    required_string(object, "ownerMembershipId")?,
                    required_string(object, "notificationId")?,
                )
                .map_err(|err| anyhow!(format!("{:?}", err.code))),
            "set-adoption-enabled" => {
                let enabled = object
                    .get("enabled")
                    .and_then(Value::as_bool)
                    .ok_or_else(|| anyhow!("invalid_request"))?;
                let policy = continuity
                    .set_adoption_enabled(
                        conversation_id,
                        required_string(object, "ownerMembershipId")?,
                        enabled,
                    )
                    .map_err(|err| anyhow!(format!("{:?}", err.code)))?;
                Ok(json!({
                    "ok": true,
                    "adoptionPolicy": {
                        "enabled": policy.enabled,
                        "stage": policy.stage,
                        "realModelQualification": "unknown",
                    },
                }))
            }
            "admit-task-child" => {
                let admission = object
                    .get("admission")
                    .cloned()
                    .ok_or_else(|| anyhow!("invalid_request"))?;
                let proposal = serde_json::from_value(json!({
                    "envelope": {
                        "conversationId": conversation_id,
                        "sourceEventRefs": [],
                        "observedRevision": 0,
                        "designationEpoch": 0,
                        "requestId": format!("request:admit-task-child:{conversation_id}"),
                    },
                    "matterAssociations": [],
                    "speechAct": "delegation",
                    "commitmentProposals": [],
                    "agreementProposals": [],
                    "capabilityNeeds": [],
                    "uncertaintyReasons": [],
                    "requestedReads": [],
                    "taskChildAdmission": admission,
                }))?;
                let receipt = continuity
                    .commit_fresh(proposal)
                    .map_err(|err| anyhow!(format!("{:?}", err.code)))?;
                Ok(json!({
                    "ok": true,
                    "revision": receipt.revision,
                }))
            }
            "revise-agreement" => {
                let agreement = serde_json::from_value(
                    object
                        .get("agreement")
                        .cloned()
                        .ok_or_else(|| anyhow!("invalid_request"))?,
                )?;
                let stored = continuity
                    .revise_agreement(conversation_id, agreement)
                    .map_err(|err| anyhow!(format!("{:?}", err.code)))?;
                Ok(json!({
                    "ok": true,
                    "agreementId": stored.id,
                    "effectiveRevision": stored.effective_revision,
                    "revocationGeneration": stored.revocation_generation,
                }))
            }
            "accept-evidence" => {
                let goal_id = required_string(object, "goalId")?;
                let evidence = serde_json::from_value(
                    object
                        .get("evidence")
                        .cloned()
                        .ok_or_else(|| anyhow!("invalid_request"))?,
                )?;
                let progress = continuity
                    .accept_evidence(conversation_id, goal_id, evidence)
                    .map_err(|err| anyhow!(format!("{:?}", err.code)))?;
                Ok(json!({
                    "ok": true,
                    "goalId": progress.goal_id,
                    "revision": progress.revision,
                    "lifecycle": progress.lifecycle,
                    "evidenceCount": progress.criterion_evidence_refs.len(),
                }))
            }
            "replace-assistant" => {
                let membership_id = required_string(object, "membershipId")?;
                let revision = continuity
                    .replace_assistant(conversation_id, membership_id)
                    .map_err(|err| anyhow!(format!("{:?}", err.code)))?;
                Ok(json!({
                    "ok": true,
                    "membershipId": membership_id,
                    "revision": revision,
                }))
            }
            _ => Err(anyhow!("unsupported_action")),
        }
    }

    /// Active Agent Memberships of one Conversation paired with their
    /// persistent Profile intents and the Assistant flag for projection.
    fn profile_projection_pairs(
        &self,
        conversation_id: &str,
    ) -> Result<Vec<(super::Membership, super::ProfileIntent, bool)>> {
        let conversation = self.store.get(conversation_id)?;
        let profiles = self.store.membership_profiles(conversation_id)?;
        Ok(profiles
            .into_iter()
            .map(|(membership, intent)| {
                let is_assistant =
                    conversation.assistant_membership_id.as_deref() == Some(membership.id.as_str());
                (membership, intent, is_assistant)
            })
            .collect())
    }

    /// The single dispatch door. Addressing runs natively from the stored
    /// Event text, registration happens before this response, a non-empty
    /// turn list means an attachable turn, and the error field is reserved
    /// for a start, resume, or dispatch call that actually failed.
    fn dispatch_posted_message(
        &self,
        conversation_id: &str,
        event_id: &str,
        suppress_assistant: bool,
    ) -> Result<Value> {
        let Some(sender) = self.host.native_turn_sender.as_ref() else {
            return Err(anyhow!(super::PERSISTENT_TRANSPORT_REQUIRED));
        };
        let content = self.store.posted_event_text(conversation_id, event_id)?;
        let conversation = self.store.get(conversation_id)?;
        let assistant_membership_id = conversation.assistant_membership_id.as_deref();
        let mut mention_ids = self.resolve_mentions(conversation_id, &content)?;
        // This is one-message addressing intent, not a membership change or a
        // strategy toggle. The durable Event is already stored above this door.
        let suppressed_assistant_mention = suppress_assistant
            && assistant_membership_id.is_some_and(|assistant_id| {
                mention_ids
                    .iter()
                    .any(|membership_id| membership_id == assistant_id)
            });
        if suppress_assistant {
            mention_ids
                .retain(|membership_id| Some(membership_id.as_str()) != assistant_membership_id);
        }
        // Routing only steers into a turn that can still receive work. The
        // host's raw view may still list a turn the store has closed, and
        // steering into one of those would drop the message silently.
        let active = self.routable_turns(conversation_id)?;
        let assistant_ids = if mention_ids.is_empty() && !suppress_assistant {
            conversation
                .assistant_membership_id
                .as_deref()
                .filter(|membership_id| {
                    conversation.memberships.iter().any(|membership| {
                        membership.id == **membership_id
                            && membership.status == MembershipStatus::Active
                            && membership.principal.kind == PrincipalKind::Agent
                    })
                })
                .map(|membership_id| vec![membership_id.to_owned()])
                .unwrap_or_default()
        } else {
            Vec::new()
        };
        let mut addressed = mention_ids.clone();
        for assistant_id in &assistant_ids {
            if !addressed.contains(assistant_id) {
                addressed.push(assistant_id.clone());
            }
        }
        let start_ids = addressed
            .iter()
            .filter(|membership_id| {
                !active
                    .iter()
                    .any(|turn| turn.membership_id == **membership_id)
            })
            .filter(|membership_id| {
                self.store
                    .direct_turn_for_source(conversation_id, event_id, membership_id)
                    .ok()
                    .flatten()
                    .is_none()
            })
            .filter(|membership_id| {
                !self.continuity.as_ref().is_some_and(|host| {
                    host.ingress_already_executed(conversation_id, event_id, membership_id)
                })
            })
            .cloned()
            .collect::<Vec<_>>();
        let pending_turns = if start_ids.is_empty() {
            Vec::new()
        } else {
            self.store
                .enqueue_mention_turns(conversation_id, event_id, &start_ids)?
        };
        let mut live_turns = Vec::new();
        let mut direct_receipts = Vec::new();
        let mut boundary_queue_ids = Vec::new();
        let mut dispatch_error = None;
        if !addressed.is_empty() {
            for membership_id in &addressed {
                if let Some(turn) = active
                    .iter()
                    .find(|candidate| candidate.membership_id == *membership_id)
                {
                    if self
                        .store
                        .direct_turn_for_source(conversation_id, event_id, membership_id)
                        .ok()
                        .flatten()
                        .is_some()
                    {
                        merge_live_turn(&mut live_turns, turn.to_json());
                        continue;
                    }
                    match self.steer_active_turn(turn, &content) {
                        SteerDisposition::Accepted => {}
                        SteerDisposition::QueueAtBoundary => {
                            boundary_queue_ids.push(turn.membership_id.clone());
                        }
                        SteerDisposition::Unknown => {
                            dispatch_error.get_or_insert_with(dispatch_steer_error);
                        }
                    }
                    merge_live_turn(&mut live_turns, turn.to_json());
                }
            }
            let dispatched = self.execute_direct_turns(sender, pending_turns)?;
            for outcome in dispatched {
                direct_receipts.push(json!({
                    "id": outcome.turn.id,
                    "state": outcome.turn.state,
                }));
                if let Some(live) = outcome.live {
                    merge_live_turn(&mut live_turns, live);
                }
            }
        } else {
            let fallback_active = active
                .iter()
                .filter(|turn| {
                    !suppress_assistant
                        || Some(turn.membership_id.as_str()) != assistant_membership_id
                })
                .collect::<Vec<_>>();
            if !suppressed_assistant_mention && fallback_active.len() == 1 {
                for turn in fallback_active {
                    match self.steer_active_turn(turn, &content) {
                        SteerDisposition::Accepted => {}
                        SteerDisposition::QueueAtBoundary => {
                            boundary_queue_ids.push(turn.membership_id.clone());
                        }
                        SteerDisposition::Unknown => {
                            dispatch_error.get_or_insert_with(dispatch_steer_error);
                        }
                    }
                    merge_live_turn(&mut live_turns, turn.to_json());
                }
            } else if !active.is_empty() && fallback_active.len() > 1 {
                dispatch_error = Some(json!({
                    "code": "conversation_address_ambiguous",
                    "stage": "conversation/address",
                }));
            }
        }
        // Active turns remain attachable even when one is deliberately ignored
        // for this message's targeting decision.
        for turn in &active {
            merge_live_turn(&mut live_turns, turn.to_json());
        }
        let strategy_address =
            if addressed.is_empty() && active.is_empty() && !suppressed_assistant_mention {
                self.address_strategy(conversation_id, &content, event_id)?
            } else {
                StrategyAddress::default()
            };
        if let Some(entry_turn) = strategy_address.entry_turn {
            merge_live_turn(&mut live_turns, entry_turn);
        }
        if !boundary_queue_ids.is_empty() {
            for turn in
                self.store
                    .enqueue_mention_turns(conversation_id, event_id, &boundary_queue_ids)?
            {
                direct_receipts.push(json!({"id": turn.id, "state": turn.state}));
            }
        }
        if dispatch_error.is_none() {
            dispatch_error = strategy_address.error;
        }
        if dispatch_error.is_none()
            && direct_receipts
                .iter()
                .any(|receipt| receipt.get("state").and_then(Value::as_str) == Some("failed"))
        {
            dispatch_error = Some(json!({
                "code": "conversation_dispatch_failed",
                "stage": "conversation/dispatch",
            }));
        }
        let dispatch_pending = !live_turns.is_empty();
        let mut payload = json!({
            "event": {"id": event_id, "state": "finalized"},
            "directTurns": direct_receipts,
            "turns": live_turns,
            "dispatchPending": dispatch_pending,
        });
        if let Some(error) = dispatch_error {
            payload["strategyError"] = error;
        }
        Ok(payload)
    }

    /// Resolve mentioned Agent Memberships from the stored Event text. Each
    /// active Agent Membership is addressed by its display name and its Agent
    /// identifier; an alias matches when it follows a start or whitespace
    /// after the mention marker and is followed by whitespace, a sentence
    /// terminator, or the end of the text. Matching is case-insensitive.
    fn resolve_mentions(&self, conversation_id: &str, text: &str) -> Result<Vec<String>> {
        let conversation = self.store.get(conversation_id)?;
        let mut mentioned = Vec::new();
        for membership in &conversation.memberships {
            if membership.status != MembershipStatus::Active
                || membership.principal.kind != PrincipalKind::Agent
            {
                continue;
            }
            let display_name = membership.principal.display_name.trim();
            let agent_id = membership
                .principal
                .agent_id
                .as_deref()
                .map(str::trim)
                .unwrap_or_default();
            let matched = [display_name, agent_id]
                .into_iter()
                .filter(|alias| !alias.is_empty())
                .any(|alias| mention_alias_matches(text, alias));
            if matched {
                mentioned.push(membership.id.clone());
            }
        }
        Ok(mentioned)
    }

    /// The turns the host currently reports as active.
    ///
    /// This is the host's own view, deliberately unreconciled: callers that
    /// guard destructive work want the conservative answer, so a turn this
    /// process still holds counts even if the store has since closed it.
    /// Routing needs the opposite preference and calls [`Self::routable_turns`].
    fn active_turns_for(&self, conversation_id: &str) -> Vec<ActiveTurnRef> {
        let Some(active_turns) = self.host.active_turns.as_ref() else {
            return Vec::new();
        };
        active_turns(conversation_id)
            .get("turns")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|turn| {
                let handle = turn.get("turnHandle").and_then(Value::as_str)?;
                if handle.trim().is_empty() {
                    return None;
                }
                Some(ActiveTurnRef {
                    turn_handle: handle.to_owned(),
                    conversation_id: turn
                        .get("conversationId")
                        .and_then(Value::as_str)
                        .unwrap_or(conversation_id)
                        .to_owned(),
                    membership_id: turn
                        .get("membershipId")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_owned(),
                    agent: turn
                        .get("agent")
                        .or_else(|| turn.get("agentId"))
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_owned(),
                })
            })
            .collect()
    }

    /// The turns that may still receive work, reconciled against the store.
    ///
    /// The host keeps live turns in process memory while the store is the
    /// durable authority on whether a turn is still open, and the two can
    /// disagree: a cold recovery marks every in-flight dispatch interrupted
    /// without reaching into the owning process, so the host keeps offering a
    /// turn the store has already closed. Routing trusts this list to choose
    /// between steering an existing turn and starting one, so an unreconciled
    /// entry silently swallows the message — it is steered into a turn that
    /// will never run. A turn the store has closed is therefore not routable.
    fn routable_turns(&self, conversation_id: &str) -> Result<Vec<ActiveTurnRef>> {
        let mut routable = Vec::new();
        for turn in self.active_turns_for(conversation_id) {
            if !self.store_turn_is_closed(&turn.turn_handle)? {
                routable.push(turn);
            }
        }
        Ok(routable)
    }

    /// Whether the durable record already closed this turn.
    ///
    /// The host names a turn by its dispatch id, so the dispatch record is the
    /// first durable entry for exactly that handle: a settled dispatch is what
    /// a cold-recovered host keeps offering after its process memory stops
    /// matching the store. A refused launch settles the direct turn without
    /// ever opening a dispatch, so the direct record is consulted too.
    ///
    /// A handle no record names counts as open: the store is an authority only
    /// over what it knows, and absence is not evidence that work stopped. A
    /// read that *fails* is reported rather than read as "still open", so a
    /// store that cannot answer can never silently swallow a message.
    fn store_turn_is_closed(&self, turn_handle: &str) -> Result<bool> {
        if let Some(dispatch) = self.store.dispatch_record(turn_handle)? {
            return Ok(matches!(
                dispatch.state,
                DispatchState::Completed | DispatchState::Failed | DispatchState::Cancelled
            ));
        }
        let Some(turn) = self.store.direct_turn_record(turn_handle)? else {
            return Ok(false);
        };
        Ok(turn.state.is_terminal())
    }

    fn steer_active_turn(&self, turn: &ActiveTurnRef, text: &str) -> SteerDisposition {
        if let Some(host) = self.continuity.as_ref() {
            match host.steer_admitted_child_follow_up(
                &turn.conversation_id,
                &turn.membership_id,
                &turn.turn_handle,
                text,
            ) {
                crate::domain::assistant_continuity::host::ChildControlDisposition::Ordinary => {}
                crate::domain::assistant_continuity::host::ChildControlDisposition::Accepted => {
                    return SteerDisposition::Accepted;
                }
                crate::domain::assistant_continuity::host::ChildControlDisposition::Unavailable => {
                    return SteerDisposition::QueueAtBoundary;
                }
                crate::domain::assistant_continuity::host::ChildControlDisposition::Conflict => {
                    return SteerDisposition::Unknown;
                }
            }
        }
        let Some(steer_turn) = self.host.steer_turn.as_ref() else {
            return SteerDisposition::Unknown;
        };
        let Ok(receipt) = steer_turn(&json!({
            "turnHandle": turn.turn_handle,
            "conversationId": turn.conversation_id,
            "text": text,
            "agent": turn.agent,
        })) else {
            return SteerDisposition::Unknown;
        };
        match (
            receipt.get("ok").and_then(Value::as_bool),
            receipt.get("status").and_then(Value::as_str),
        ) {
            (Some(true), Some("accepted")) => SteerDisposition::Accepted,
            (Some(false), Some("unsupported" | "no_active_turn" | "session_unavailable")) => {
                SteerDisposition::QueueAtBoundary
            }
            _ => SteerDisposition::Unknown,
        }
    }

    fn address_strategy(
        &self,
        conversation_id: &str,
        content: &str,
        event_id: &str,
    ) -> Result<StrategyAddress> {
        let conversation = self.store.get(conversation_id)?;
        let Some(revision) = conversation
            .strategy_revision
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
        else {
            return Ok(StrategyAddress::default());
        };
        let Some(execute) = self.host.strategy_execute.as_ref() else {
            return Err(anyhow!(super::PERSISTENT_TRANSPORT_REQUIRED));
        };
        let active = match execute(json!({
            "action": "strategy.run.active",
            "revisionDigest": revision,
            "conversationId": conversation_id,
        })) {
            Ok(value) => match unwrap_strategy_execute(value) {
                Ok(value) => value,
                Err(error) => {
                    return Ok(StrategyAddress {
                        entry_turn: None,
                        error: Some(error),
                    });
                }
            },
            Err(_) => {
                return Ok(StrategyAddress {
                    entry_turn: None,
                    error: Some(
                        json!({"code": "strategy_run_start_failed", "stage": "strategy/start"}),
                    ),
                });
            }
        };
        let run_id = active
            .get("runId")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        let status = active
            .get("status")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let terminal = matches!(
            status,
            "completed"
                | "failed"
                | "cancelled"
                | "blocked"
                | "cancel-requested"
                | "cancel-in-doubt"
        );
        let request = if run_id.is_empty() || terminal {
            json!({
                "action": "strategy.run.start",
                "revisionDigest": revision,
                "input": {"message": content},
                "idempotencyKey": format!("conversation-post-{event_id}"),
                "conversationId": conversation_id,
            })
        } else {
            json!({
                "action": "strategy.run.resume",
                "runId": run_id,
                "conversationId": conversation_id,
            })
        };
        match execute(request) {
            Ok(value) => match unwrap_strategy_execute(value) {
                Ok(result) => Ok(StrategyAddress {
                    entry_turn: entry_turn_projection(&result, conversation_id),
                    error: None,
                }),
                Err(error) => Ok(StrategyAddress {
                    entry_turn: None,
                    error: Some(error),
                }),
            },
            Err(_) => Ok(StrategyAddress {
                entry_turn: None,
                error: Some(
                    json!({"code": "strategy_run_start_failed", "stage": "strategy/start"}),
                ),
            }),
        }
    }

    fn execute_direct_turns(
        &self,
        sender: &Arc<NativeTurnSender>,
        pending_turns: Vec<DirectTurn>,
    ) -> Result<Vec<DirectTurnOutcome>> {
        if pending_turns.is_empty() {
            return Ok(Vec::new());
        }
        // Phase one claims every turn and marks it running. Each step is one
        // short local lease; no lease is held across the runtime dispatch.
        let mut claimed = Vec::with_capacity(pending_turns.len());
        let mut phase_one_error = None;
        for pending in pending_turns {
            match self.prepare_direct_turn(&pending.id) {
                Ok(claimed_turn) => claimed.push(claimed_turn),
                Err(error) => {
                    phase_one_error = Some(error);
                    break;
                }
            }
        }
        // Phase two dispatches the claimed turns with a bounded worker set.
        // Results are collected by input ordinal, so receipts keep the
        // original order regardless of completion timing.
        let results = self.dispatch_direct_turns(sender, &claimed);
        if let Some(error) = phase_one_error {
            return Err(error);
        }
        results.into_iter().collect()
    }

    /// Claim one pending turn and move it to running, or report its current
    /// state without a context when a concurrent claimant already owns it.
    fn prepare_direct_turn(&self, turn_id: &str) -> Result<ClaimedTurn> {
        let Some(context) = self.store.claim_direct_turn(turn_id)? else {
            return Ok(ClaimedTurn {
                turn_id: turn_id.to_owned(),
                context: None,
            });
        };
        if !self.store.mark_direct_turn_running(&context.turn.id)? {
            return Ok(ClaimedTurn {
                turn_id: turn_id.to_owned(),
                context: None,
            });
        }
        Ok(ClaimedTurn {
            turn_id: turn_id.to_owned(),
            context: Some(context),
        })
    }

    fn dispatch_direct_turns(
        &self,
        sender: &Arc<NativeTurnSender>,
        claimed: &[ClaimedTurn],
    ) -> Vec<Result<DirectTurnOutcome>> {
        let results: Mutex<Vec<Option<Result<DirectTurnOutcome>>>> =
            Mutex::new((0..claimed.len()).map(|_| None).collect());
        let next = AtomicUsize::new(0);
        std::thread::scope(|scope| {
            for _ in 0..DEFAULT_DIRECT_TURN_WORKERS.min(claimed.len()) {
                let service = self.clone();
                let sender = Arc::clone(sender);
                let results = &results;
                let next = &next;
                scope.spawn(move || {
                    loop {
                        let index = next.fetch_add(1, Ordering::Relaxed);
                        if index >= claimed.len() {
                            break;
                        }
                        let result = service.run_direct_turn(&sender, &claimed[index]);
                        results.lock().unwrap_or_else(|poison| poison.into_inner())[index] =
                            Some(result);
                    }
                });
            }
        });
        results
            .into_inner()
            .unwrap_or_else(|poison| poison.into_inner())
            .into_iter()
            .map(|result| result.expect("worker filled every slot"))
            .collect()
    }

    fn admit_native_role(
        &self,
        conversation_id: &str,
        owner_membership_id: &str,
        native_role_id: &str,
        access: MembershipAccess,
    ) -> Result<Value> {
        let role = crate::domain::native_roles::find(native_role_id)
            .ok_or_else(|| anyhow!("native_role_not_found"))?;
        let principal = Principal {
            id: role.principal_id(),
            kind: PrincipalKind::Agent,
            display_name: role.name.clone(),
            agent_id: Some(role.host_agent_id.clone()),
            created_at_unix_ms: 0,
        };
        let membership = self.store.add_member(conversation_id, principal, access)?;
        self.store.set_membership_profile(
            conversation_id,
            &membership.id,
            owner_membership_id,
            0,
            &role.profile_intent_update(),
        )?;
        Ok(serde_json::to_value(membership)?)
    }

    fn run_direct_turn(
        &self,
        sender: &Arc<NativeTurnSender>,
        claimed: &ClaimedTurn,
    ) -> Result<DirectTurnOutcome> {
        let Some(context) = claimed.context.as_ref() else {
            return Ok(DirectTurnOutcome {
                turn: self.store.direct_turn(&claimed.turn_id)?,
                live: None,
            });
        };
        let profile = self.store.membership_profile(&context.turn.membership_id)?;
        let native_role = profile.as_ref().and_then(|intent| {
            crate::domain::native_roles::role_from_skill_refs(&intent.skill_references)
        });
        let mut guidance = String::new();
        if let Some(assistant) = context.private_instructions() {
            guidance.push_str(assistant);
        }
        if let Some(instructions) = profile
            .as_ref()
            .and_then(|intent| intent.preferred_instructions.as_deref())
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            if !guidance.is_empty() {
                guidance.push_str("\n\n");
            }
            guidance.push_str(instructions);
        }
        if let Some(role) = native_role.as_ref()
            && !role.instructions.trim().is_empty()
        {
            if !guidance.is_empty() {
                guidance.push_str("\n\n");
            }
            guidance.push_str(&role.instructions);
        }
        if let Some(host) = self.continuity.as_ref()
            && context.is_assistant
        {
            if let Ok(extra) = host.compose_ingress_guidance(
                &context.turn.conversation_id,
                &context.turn.membership_id,
                &context.turn.source_event_id,
            ) {
                if !extra.trim().is_empty() {
                    if !guidance.is_empty() {
                        guidance.push_str("\n\n");
                    }
                    guidance.push_str(&extra);
                }
            }
        }
        // User-authored Event text stays exact. Generated guidance follows the
        // adapter's declared ephemeral policy and never enters Event/Part.
        let delivery = crate::platform::runtime_adapters::compose_generated_instruction_delivery(
            &context.agent_id,
            &context.source_content,
            (!guidance.is_empty()).then_some(guidance.as_str()),
        )
        .map_err(anyhow::Error::msg)?;
        let mut params = json!({
            "agentId": context.agent_id,
            "agent": context.agent_id,
            "text": delivery.text,
            "streamEvents": true,
            "conversationId": context.turn.conversation_id,
            "membershipId": context.turn.membership_id,
            "causationId": context.turn.source_event_id,
            "dispatchId": context.turn.id,
        });
        if self.continuity.is_some() && context.is_assistant {
            params["continuityKind"] =
                json!(crate::domain::assistant_continuity::execution::CONTINUITY_KIND_USER_POSTED);
        }
        if let (Some(field), Some(guidance)) = (delivery.field, delivery.guidance) {
            params[field] = json!(guidance);
        }
        if let Some(role) = native_role.as_ref() {
            params["runtimeAgent"] = json!(role.slug);
        }
        apply_admitted_runtime_fields(
            &mut params,
            &self.store,
            &context.turn.conversation_id,
            &context.turn.membership_id,
            profile.as_ref(),
            (!context.source_attachments.is_empty())
                .then(|| dispatch_attachments_param(&context.source_attachments)),
        );
        #[cfg(test)]
        self.store.counters().begin_turn();
        let dispatched = sender(&params);
        #[cfg(test)]
        self.store.counters().end_turn();
        let mut live = None;
        match dispatched {
            Ok(value) => {
                // An accepted receipt carries the attachable handle. Once the
                // dispatch is open, the dispatch completion authority alone
                // writes its terminal Event, dispatch state, and turn state.
                live = live_turn_from_accepted(&value, context);
            }
            Err(error) => {
                // Pre-dispatch rejection: settle the turn only when its
                // dispatch was never opened. An opened dispatch already
                // belongs to the completion authority.
                let projected = serde_json::to_value(
                    crate::platform::runtime_adapters::client_error::client_error(&error),
                )?;
                let diagnostic = serde_json::to_string(&json!({
                    "code": safe_failure_field(
                        &projected,
                        "code",
                        "agent_conversation_dispatch_failed",
                    ),
                    "stage": safe_failure_field(&projected, "stage", "conversation/dispatch"),
                }))?;
                self.store
                    .fail_direct_turn_unless_dispatched(&claimed.turn_id, &diagnostic)?;
            }
        }
        Ok(DirectTurnOutcome {
            turn: self.store.direct_turn(&claimed.turn_id)?,
            live,
        })
    }
}

struct DirectTurnOutcome {
    turn: DirectTurn,
    live: Option<Value>,
}

/// A turn claimed for this fan-out: either with private execution context, or
/// without context when a concurrent claimant already owns it (its terminal
/// state is read back from the store).
struct ClaimedTurn {
    turn_id: String,
    context: Option<crate::domain::client_conversation::store::DirectTurnExecutionContext>,
}

#[derive(Default)]
struct StrategyAddress {
    entry_turn: Option<Value>,
    error: Option<Value>,
}

struct ActiveTurnRef {
    turn_handle: String,
    conversation_id: String,
    membership_id: String,
    agent: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum SteerDisposition {
    Accepted,
    QueueAtBoundary,
    Unknown,
}

fn dispatch_steer_error() -> Value {
    json!({
        "code": "conversation_dispatch_failed",
        "stage": "conversation/steer",
    })
}

impl ActiveTurnRef {
    fn to_json(&self) -> Value {
        json!({
            "turnHandle": self.turn_handle,
            "conversationId": self.conversation_id,
            "membershipId": self.membership_id,
            "agent": self.agent,
        })
    }
}

fn unwrap_strategy_execute(value: Value) -> std::result::Result<Value, Value> {
    if value.get("ok").and_then(Value::as_bool) == Some(true) {
        return Ok(value.get("result").cloned().unwrap_or_else(|| json!({})));
    }
    let error = value
        .get("error")
        .cloned()
        .unwrap_or_else(|| json!({"code": "strategy_run_start_failed"}));
    let code = error
        .get("code")
        .and_then(Value::as_str)
        .filter(|code| !code.is_empty())
        .unwrap_or("strategy_run_start_failed");
    Err(json!({"code": code, "stage": "strategy/start"}))
}

/// The receipt under the selection policy the caller captured.
///
/// The receipt shape is owned by the runtime that consumes it; this host
/// supplies its own answer for the model facts inside it and the exact
/// selection-policy revision the decision was admitted under. A durable
/// admission stores the receipt it was admitted with, so an in-flight task
/// keeps the revision captured here while a later adoption governs only the
/// next task.
pub(crate) fn route_receipt_under(
    conversation_id: &str,
    snapshots: &[super::MembershipProfileSnapshot],
    selection_policy: &super::selection_policy::SelectionPolicyBinding,
) -> Value {
    licoup_workflow_runtime::ports::route_receipt(
        conversation_id,
        snapshots,
        &crate::workflow_host::ProductionModelFacts,
        selection_policy.revision_name(),
    )
}

fn merge_live_turn(live_turns: &mut Vec<Value>, turn: Value) {
    let Some(handle) = turn
        .get("turnHandle")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|handle| !handle.is_empty())
    else {
        return;
    };
    if live_turns
        .iter()
        .any(|existing| existing.get("turnHandle").and_then(Value::as_str) == Some(handle))
    {
        return;
    }
    live_turns.push(turn);
}

/// The attachable entry handle from a strategy run start/resume response, in
/// the same shape as a live turn entry.
fn entry_turn_projection(result: &Value, conversation_id: &str) -> Option<Value> {
    let entry = result.get("entryTurn")?;
    let handle = entry
        .get("turnHandle")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|handle| !handle.is_empty())?;
    Some(json!({
        "turnHandle": handle,
        "conversationId": conversation_id,
        "membershipId": entry.get("membershipId").and_then(Value::as_str).unwrap_or_default(),
        "agent": entry.get("agent").and_then(Value::as_str).unwrap_or_default(),
    }))
}

/// One mention alias match: the alias follows a start or whitespace after the
/// `@` marker and is followed by whitespace, a sentence terminator, or the
/// end of the text. Matching is case-insensitive.
fn mention_alias_matches(text: &str, alias: &str) -> bool {
    let pattern = format!(
        r"(?i)(?:^|\s)@{}(?:\s|[,.!?;:，。！？；：]|$)",
        regex::escape(alias)
    );
    regex::Regex::new(&pattern)
        .map(|pattern| pattern.is_match(text))
        .unwrap_or(false)
}

fn live_turn_from_accepted(
    value: &Value,
    context: &crate::domain::client_conversation::store::DirectTurnExecutionContext,
) -> Option<Value> {
    if value.get("accepted").and_then(Value::as_bool) != Some(true) {
        return None;
    }
    let handle = value
        .get("turnHandle")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|handle| !handle.is_empty())?;
    Some(json!({
        "turnHandle": handle,
        "conversationId": context.turn.conversation_id,
        "membershipId": context.turn.membership_id,
        "agent": context.agent_id,
    }))
}

fn safe_failure_field<'a>(value: &'a Value, key: &str, fallback: &'a str) -> &'a str {
    value
        .get(key)
        .and_then(Value::as_str)
        .filter(|text| {
            !text.is_empty()
                && text.len() <= 96
                && text.bytes().all(|byte| {
                    byte.is_ascii_lowercase()
                        || byte.is_ascii_digit()
                        || matches!(byte, b'_' | b'-' | b'/' | b'.')
                })
        })
        .unwrap_or(fallback)
}

/// Admit the optional `attachments` array of one `conversation.message.post`
/// request. The shape is the shared local-image shape of the 1:1 lane
/// (`id`, `name`, `mediaType`, `path`; the caller id is accepted and ignored —
/// the stored Event Part identity becomes the dispatch reference). Admission
/// validates image-only media types, the existing count and size limits, and
/// reads each file's real byte size from the filesystem; every failure
/// rejects the whole post before any Event is persisted.
fn admit_post_attachments(raw: Option<&Value>) -> Result<Vec<ImageAttachment>> {
    let Some(raw) = raw else {
        return Ok(Vec::new());
    };
    if raw.is_null() {
        return Ok(Vec::new());
    }
    let items = raw
        .as_array()
        .ok_or_else(|| anyhow!("attachment_invalid"))?;
    if items.len() > MAX_IMAGE_ATTACHMENTS {
        return Err(anyhow!("attachment_limit_exceeded"));
    }
    let mut total_bytes: u64 = 0;
    let mut admitted = Vec::with_capacity(items.len());
    for item in items {
        let object = item
            .as_object()
            .ok_or_else(|| anyhow!("attachment_invalid"))?;
        if object
            .keys()
            .any(|key| !matches!(key.as_str(), "id" | "name" | "mediaType" | "path"))
        {
            return Err(anyhow!("attachment_invalid"));
        }
        let name = required_attachment_string(object, "name")?;
        let media_type = required_attachment_string(object, "mediaType")?;
        let path = required_attachment_string(object, "path")?;
        if !attachment_media_type_supported(&media_type) {
            return Err(anyhow!("attachment_media_unsupported"));
        }
        if path.contains("://") {
            return Err(anyhow!("attachment_remote_unsupported"));
        }
        let metadata =
            std::fs::symlink_metadata(&path).map_err(|_| anyhow!("attachment_unavailable"))?;
        if !metadata.file_type().is_file() {
            return Err(anyhow!("attachment_invalid"));
        }
        let byte_size = metadata.len();
        total_bytes = total_bytes.saturating_add(byte_size);
        if byte_size > MAX_IMAGE_ATTACHMENT_BYTES_PER_FILE
            || total_bytes > MAX_IMAGE_ATTACHMENT_BYTES_TOTAL
        {
            return Err(anyhow!("attachment_size_exceeded"));
        }
        admitted.push(ImageAttachment {
            path,
            name,
            media_type,
            byte_size,
        });
    }
    Ok(admitted)
}

fn required_attachment_string(
    object: &serde_json::Map<String, Value>,
    key: &str,
) -> Result<String> {
    object
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
        .ok_or_else(|| anyhow!("attachment_invalid"))
}

/// The shared local-image wire shape admitted by the runtime-adapter
/// attachment lane. The durable Event Part identity is the reference id;
/// byte size stays in the canonical record and is never sent to adapters.
pub fn dispatch_attachments_param(references: &[ImageAttachmentReference]) -> Value {
    Value::Array(
        references
            .iter()
            .map(|reference| {
                json!({
                    "id": reference.part_id,
                    "name": reference.attachment.name,
                    "mediaType": reference.attachment.media_type,
                    "path": reference.attachment.path,
                })
            })
            .collect(),
    )
}

fn ensure_allowed_fields(action: &str, object: &serde_json::Map<String, Value>) -> Result<()> {
    let allowed: &[&str] = match action {
        "conversation.create" => &["action", "title", "owner", "members", "isGroup"],
        "conversation.rename" => &["action", "conversationId", "title"],
        "conversation.archive" => &["action", "conversationId", "archived", "reopen"],
        "conversation.clear" => &["action", "conversationId", "ownerMembershipId"],
        "conversation.pin.set" => &["action", "conversationId", "pinned"],
        "conversation.strategy.set" => &["action", "conversationId", "strategyRevision"],
        "conversation.assistant.set" => &[
            "action",
            "conversationId",
            "ownerMembershipId",
            "expectedRevision",
            "membershipId",
        ],
        "conversation.profile.update" => &[
            "action",
            "conversationId",
            "membershipId",
            "ownerMembershipId",
            "expectedRevision",
            "intent",
        ],
        "conversation.profile.get" => &["action", "membershipId"],
        "conversation.profile.native_roles" => &["action"],
        "conversation.profile.candidates" => &["action", "conversationId", "filters"],
        "timeout.policy.get" => &["action"],
        "timeout.policy.set" => &["action", "policy"],
        "conversation.list" => &["action", "includeArchived"],
        "conversation.get" => &["action", "conversationId", "includeNativeSessionReferences"],
        "conversation.events.page" => &[
            "action",
            "conversationId",
            "afterSequence",
            "beforeSequence",
            "latest",
            "limit",
        ],
        "conversation.events.search" => &["action", "query", "limit"],
        "conversation.event.append" => &[
            "action",
            "conversationId",
            "authorMembershipId",
            "kind",
            "parts",
            "causationId",
            "correlationId",
            "finalized",
        ],
        "conversation.message.post" => &[
            "action",
            "conversationId",
            "authorMembershipId",
            "content",
            "correlationId",
            "mentionedMembershipIds",
            "attachments",
        ],
        "conversation.message.delete" => {
            &["action", "conversationId", "eventId", "ownerMembershipId"]
        }
        "conversation.dispatch.after-post" => {
            &["action", "conversationId", "eventId", "suppressAssistant"]
        }
        "conversation.event.part.append" => &["action", "eventId", "part"],
        "conversation.event.finalize" => &["action", "eventId"],
        "conversation.membership.add" => &[
            "action",
            "conversationId",
            "principal",
            "access",
            "nativeRoleId",
            "ownerMembershipId",
        ],
        "conversation.membership.access.set" => {
            &["action", "conversationId", "membershipId", "access"]
        }
        "conversation.membership.leave" => &["action", "conversationId", "membershipId"],
        "conversation.export" => &["action", "path", "conversationIds"],
        "conversation.import" => &["action", "path"],
        "conversation.subagent.edge" => &[
            "action",
            "conversationId",
            "callerMembershipId",
            "targetMembershipId",
        ],
        "conversation.subagent.target" => &["action", "conversationId", "membershipId"],
        "conversation.subagent.claim" => &[
            "action",
            "conversationId",
            "callerMembershipId",
            "targetMembershipId",
            "parentDispatchId",
        ],
        "conversation.subagent.claim.update" => &["action", "dispatchId", "state"],
        "conversation.subagent.claim.active" => &[
            "action",
            "conversationId",
            "callerMembershipId",
            "targetMembershipId",
        ],
        "conversation.subagent.inbound.record" => &[
            "action",
            "conversationId",
            "callerMembershipId",
            "targetMembershipId",
            "tool",
            "outcome",
        ],
        "conversation.subagent.binding.get" => &["action", "conversationId", "membershipId"],
        "apply-interpretation" | "correct-association" | "propose-criterion-change" => {
            &["action", "conversationId", "proposal"]
        }
        "pause-goal" | "resume-goal" | "request-cancel" => &["action", "conversationId", "goalId"],
        "close-goal" => &["action", "conversationId", "transition", "progress"],
        "list-pending-completion-notices" => &["action", "conversationId", "ownerMembershipId"],
        "ack-completion-notices" => &[
            "action",
            "conversationId",
            "ownerMembershipId",
            "notificationIds",
        ],
        "resolve-completion-notice" => &[
            "action",
            "conversationId",
            "ownerMembershipId",
            "notificationId",
        ],
        "set-adoption-enabled" => &["action", "conversationId", "ownerMembershipId", "enabled"],
        "admit-task-child" => &["action", "conversationId", "admission"],
        "revise-agreement" => &["action", "conversationId", "agreement"],
        "accept-evidence" => &["action", "conversationId", "goalId", "evidence"],
        "replace-assistant" => &["action", "conversationId", "membershipId"],
        _ => return Err(anyhow!("unsupported_action")),
    };
    if object.keys().any(|key| !allowed.contains(&key.as_str())) {
        return Err(anyhow!("invalid_request"));
    }
    Ok(())
}

fn subagent_claim_json(claim: &licoup_conversation::SubagentDispatchClaim) -> Value {
    json!({
        "id": claim.id,
        "conversationId": claim.conversation_id,
        "callerMembershipId": claim.caller_membership_id,
        "targetMembershipId": claim.target_membership_id,
        "parentDispatchId": claim.parent_dispatch_id,
        "depth": claim.depth,
        "state": claim.state.as_str(),
        "createdAtUnixMs": claim.created_at_unix_ms,
        "updatedAtUnixMs": claim.updated_at_unix_ms,
    })
}

fn subagent_claim_state(value: &str) -> Result<licoup_conversation::SubagentDispatchClaimState> {
    use licoup_conversation::SubagentDispatchClaimState;
    match value {
        "claimed" => Ok(SubagentDispatchClaimState::Claimed),
        "running" => Ok(SubagentDispatchClaimState::Running),
        "cancel-requested" => Ok(SubagentDispatchClaimState::CancelRequested),
        "reconciliation-required" => Ok(SubagentDispatchClaimState::ReconciliationRequired),
        "completed" => Ok(SubagentDispatchClaimState::Completed),
        "failed" => Ok(SubagentDispatchClaimState::Failed),
        "cancelled" => Ok(SubagentDispatchClaimState::Cancelled),
        _ => Err(anyhow!("subagent_dispatch_transition_invalid")),
    }
}

fn ensure_member_fields(object: &serde_json::Map<String, Value>) -> Result<()> {
    if object
        .keys()
        .any(|key| !matches!(key.as_str(), "principal" | "access"))
    {
        return Err(anyhow!("invalid_request"));
    }
    Ok(())
}

fn required_string<'a>(object: &'a serde_json::Map<String, Value>, key: &str) -> Result<&'a str> {
    object
        .get(key)
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| anyhow!("invalid_request"))
}

fn event_page_position(object: &serde_json::Map<String, Value>) -> Result<EventPagePosition> {
    let sequence = |key| {
        object
            .get(key)
            .filter(|value| !value.is_null())
            .map(|value| value.as_i64().ok_or_else(|| anyhow!("invalid_request")))
            .transpose()
    };
    let after = sequence("afterSequence")?;
    let before = sequence("beforeSequence")?;
    let latest = object
        .get("latest")
        .map(|value| value.as_bool().ok_or_else(|| anyhow!("invalid_request")))
        .transpose()?
        .unwrap_or(false);
    if usize::from(after.is_some()) + usize::from(before.is_some()) + usize::from(latest) > 1 {
        return Err(anyhow!("invalid_request"));
    }
    Ok(if latest {
        EventPagePosition::Latest
    } else if let Some(sequence) = before {
        EventPagePosition::Before(sequence)
    } else {
        EventPagePosition::After(after.unwrap_or(0))
    })
}

fn required_revision(object: &serde_json::Map<String, Value>, key: &str) -> Result<i64> {
    object
        .get(key)
        .and_then(Value::as_i64)
        .filter(|revision| *revision >= 0)
        .ok_or_else(|| anyhow!("invalid_request"))
}

fn principal_from_value(value: &Value) -> Result<Principal> {
    let object = value
        .as_object()
        .ok_or_else(|| anyhow!("invalid_request"))?;
    let id = object
        .get("id")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow!("invalid_request"))?;
    let kind: PrincipalKind = serde_json::from_value(
        object
            .get("kind")
            .cloned()
            .unwrap_or_else(|| json!("human")),
    )?;
    Ok(Principal {
        id: id.to_owned(),
        kind,
        display_name: object
            .get("displayName")
            .and_then(Value::as_str)
            .unwrap_or(id)
            .to_owned(),
        agent_id: object
            .get("agentId")
            .and_then(Value::as_str)
            .map(str::to_owned),
        created_at_unix_ms: object
            .get("createdAtUnixMs")
            .and_then(Value::as_i64)
            .unwrap_or(0),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::client_conversation::{DispatchSessionMode, EventKind};
    use std::sync::{Condvar, Mutex};

    /// Releases the runtime barrier even when an assertion fails first, so a
    /// panicking test can never leave the fan-out threads blocked forever.
    struct ReleaseGuard<'a>(&'a (Mutex<bool>, Condvar));

    impl Drop for ReleaseGuard<'_> {
        fn drop(&mut self) {
            let (lock, cvar) = self.0;
            *lock.lock().unwrap_or_else(|poison| poison.into_inner()) = true;
            cvar.notify_all();
        }
    }

    fn group_fixture(service: &ConversationService) -> (String, String, String) {
        let group = service
            .execute(json!({
                "action": "conversation.create",
                "title": "Direct Turn",
                "owner": {"id": "human:local", "kind": "human", "displayName": "You"},
                "members": [
                    {"principal": {"id": "agent:one", "kind": "agent", "displayName": "One", "agentId": "one"}, "access": "member"}
                ]
            }))
            .unwrap();
        let memberships = group["memberships"].as_array().unwrap();
        let owner = memberships
            .iter()
            .find(|membership| membership["principal"]["kind"] == "human")
            .unwrap()["id"]
            .as_str()
            .unwrap()
            .to_owned();
        let agent = memberships
            .iter()
            .find(|membership| membership["principal"]["kind"] == "agent")
            .unwrap()["id"]
            .as_str()
            .unwrap()
            .to_owned();
        (group["id"].as_str().unwrap().to_owned(), owner, agent)
    }

    fn accepted_receipt(params: &Value) -> Value {
        json!({
            "ok": true,
            "accepted": true,
            "turnHandle": params["dispatchId"],
            "conversationId": params["conversationId"],
            "membershipId": params["membershipId"],
        })
    }

    /// One real local image file so post admission can stat it and adapter
    /// admission can verify its signature.
    struct ImageFixture {
        directory: std::path::PathBuf,
        png: std::path::PathBuf,
    }

    impl ImageFixture {
        const PNG_BYTES: [u8; 12] = [0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A, 0, 0, 0, 0];

        fn new() -> Self {
            let directory =
                std::env::temp_dir().join(format!("lico-post-attachment-{}", uuid::Uuid::new_v4()));
            std::fs::create_dir_all(&directory).unwrap();
            let png = directory.join("synthetic.png");
            std::fs::write(&png, Self::PNG_BYTES).unwrap();
            Self { directory, png }
        }

        fn attachment(&self) -> Value {
            json!({
                "id": "sel-1",
                "name": "synthetic.png",
                "mediaType": "image/png",
                "path": self.png.to_string_lossy(),
            })
        }
    }

    impl Drop for ImageFixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.directory);
        }
    }

    /// Persist one human message, then dispatch it by identity alone. The
    /// dispatch request carries no content and no client-computed mentions.
    fn persist_then_dispatch(service: &ConversationService, request: Value) -> Value {
        persist_then_dispatch_with_assistant(service, request, false)
    }

    fn persist_then_dispatch_with_assistant(
        service: &ConversationService,
        request: Value,
        suppress_assistant: bool,
    ) -> Value {
        let persisted = service
            .execute(request.clone())
            .expect("persist posted message");
        let event_id = persisted
            .get("event")
            .and_then(|event| event.get("id"))
            .cloned()
            .expect("persisted event id");
        let conversation_id = request["conversationId"].clone();
        let mut dispatch = json!({
            "action": "conversation.dispatch.after-post",
            "conversationId": conversation_id,
            "eventId": event_id,
        });
        if suppress_assistant {
            dispatch["suppressAssistant"] = Value::Bool(true);
        }
        service.execute(dispatch).expect("dispatch after post")
    }

    #[test]
    fn canonical_event_pages_default_to_twenty_and_preserve_directional_cursors() {
        let service = ConversationService::from_store(
            crate::domain::client_conversation::ConversationStore::open_in_memory().unwrap(),
        );
        let (conversation_id, _, _) = group_fixture(&service);
        for _ in 0..45 {
            service
                .store()
                .append_event(
                    &conversation_id,
                    None,
                    EventKind::Message,
                    &[],
                    None,
                    None,
                    true,
                )
                .unwrap();
        }
        let latest = service
            .execute(json!({
                "action": "conversation.events.page",
                "conversationId": conversation_id,
                "latest": true
            }))
            .unwrap();
        let events = latest["events"].as_array().unwrap();
        assert_eq!(events.len(), 20);
        assert_eq!(latest["hasEarlier"], true);
        assert_eq!(latest["nextBeforeSequence"], events[0]["sequence"]);
        let tail_sequence = events.last().unwrap()["sequence"].as_i64().unwrap();
        assert_eq!(latest["nextCursor"], tail_sequence.to_string());
        let older = service
            .execute(json!({
                "action": "conversation.events.page",
                "conversationId": conversation_id,
                "beforeSequence": latest["nextBeforeSequence"]
            }))
            .unwrap();
        let older_events = older["events"].as_array().unwrap();
        assert_eq!(older_events.len(), 20);
        assert!(
            older_events.last().unwrap()["sequence"].as_i64().unwrap()
                < events[0]["sequence"].as_i64().unwrap()
        );
        assert_eq!(older["totalCount"], latest["totalCount"]);

        let forward = service
            .execute(json!({
                "action": "conversation.events.page",
                "conversationId": conversation_id
            }))
            .unwrap();
        assert_eq!(forward["events"].as_array().unwrap().len(), 20);
        assert_eq!(forward["events"][0]["sequence"], 1);
        assert_eq!(forward["hasEarlier"], false);
        assert!(forward["nextBeforeSequence"].is_null());
        let exact = service
            .execute(json!({
                "action": "conversation.events.page",
                "conversationId": conversation_id,
                "afterSequence": tail_sequence - 1,
                "limit": 1
            }))
            .unwrap();
        assert_eq!(exact["events"].as_array().unwrap().len(), 1);
        assert_eq!(exact["events"][0]["sequence"], tail_sequence);
        assert_eq!(exact["nextCursor"], latest["nextCursor"]);
    }

    #[test]
    fn canonical_event_pages_reject_conflicting_or_malformed_selectors() {
        let service = ConversationService::from_store(
            crate::domain::client_conversation::ConversationStore::open_in_memory().unwrap(),
        );
        let (conversation_id, _, _) = group_fixture(&service);
        for selectors in [
            json!({"latest": true, "afterSequence": 0}),
            json!({"latest": true, "beforeSequence": 20}),
            json!({"beforeSequence": 20, "afterSequence": 0}),
            json!({"latest": "true"}),
            json!({"beforeSequence": "20"}),
            json!({"afterSequence": 1.5}),
        ] {
            let mut request = json!({
                "action": "conversation.events.page",
                "conversationId": conversation_id,
            });
            request
                .as_object_mut()
                .unwrap()
                .extend(selectors.as_object().unwrap().clone());
            assert_eq!(
                service.execute(request).unwrap_err().to_string(),
                "invalid_request"
            );
        }
    }

    #[test]
    fn posted_message_persists_without_a_runtime() {
        let service = ConversationService::from_store(
            crate::domain::client_conversation::ConversationStore::open_in_memory().unwrap(),
        );
        let (conversation_id, owner_id, _) = group_fixture(&service);
        let posted = service
            .execute(json!({
                "action": "conversation.message.post",
                "conversationId": conversation_id,
                "authorMembershipId": owner_id,
                "content": "hello without a host"
            }))
            .unwrap();
        assert_eq!(posted["event"]["state"], "finalized");
        assert!(posted["directTurns"].as_array().unwrap().is_empty());
        assert!(posted["turns"].as_array().unwrap().is_empty());
        assert_eq!(posted["dispatchPending"], false);
        let events = service
            .store()
            .page_events(&conversation_id, None, 20)
            .unwrap()
            .events;
        assert!(events.iter().any(|event| event.id == posted["event"]["id"]));
        assert_eq!(
            events.last().unwrap().parts[0].content,
            "hello without a host"
        );
    }

    /// AC-013: one admitted image attachment persists as an image Event Part
    /// carrying the attachment metadata, with the byte size read from the
    /// real file, and the projected Event exposes it to Dart.
    #[test]
    fn message_post_with_image_attachments_persists_image_parts() {
        let fixture = ImageFixture::new();
        let service = ConversationService::from_store(
            crate::domain::client_conversation::ConversationStore::open_in_memory().unwrap(),
        );
        let (conversation_id, owner_id, _) = group_fixture(&service);
        let posted = service
            .execute(json!({
                "action": "conversation.message.post",
                "conversationId": conversation_id,
                "authorMembershipId": owner_id,
                "content": "see the mockup",
                "attachments": [fixture.attachment()],
            }))
            .unwrap();
        assert_eq!(posted["event"]["state"], "finalized");

        let events = service
            .store()
            .page_events(&conversation_id, None, 20)
            .unwrap()
            .events;
        let event = events
            .iter()
            .find(|event| event.id == posted["event"]["id"])
            .unwrap();
        assert_eq!(event.parts.len(), 2);
        let text = &event.parts[0];
        assert_eq!(text.kind, super::super::EventPartKind::Text);
        assert_eq!(text.content, "see the mockup");
        let image = &event.parts[1];
        assert_eq!(image.kind, super::super::EventPartKind::Image);
        let attachment = image.image_attachment().unwrap();
        assert_eq!(attachment.path, fixture.png.to_string_lossy());
        assert_eq!(attachment.name, "synthetic.png");
        assert_eq!(attachment.media_type, "image/png");
        assert_eq!(attachment.byte_size, ImageFixture::PNG_BYTES.len() as u64);

        // The projected message exposes the attachment to Dart.
        let projected = serde_json::to_value(event).unwrap();
        let parts = projected["parts"].as_array().unwrap();
        let image_json = parts
            .iter()
            .find(|part| part["kind"] == json!("image"))
            .unwrap();
        let content: Value = serde_json::from_str(image_json["content"].as_str().unwrap()).unwrap();
        assert_eq!(content["mediaType"], "image/png");
        assert_eq!(content["name"], "synthetic.png");
        assert_eq!(content["path"], json!(fixture.png.to_string_lossy()));
        assert_eq!(
            content["byteSize"],
            json!(ImageFixture::PNG_BYTES.len() as u64)
        );
    }

    /// Dishonest attachment posts reject with a stable code before any Event
    /// is persisted: no partial writes, no fabricated metadata.
    #[test]
    fn message_post_rejects_invalid_attachments_without_persisting() {
        let fixture = ImageFixture::new();
        let service = ConversationService::from_store(
            crate::domain::client_conversation::ConversationStore::open_in_memory().unwrap(),
        );
        let (conversation_id, owner_id, _) = group_fixture(&service);
        let before = service
            .store()
            .page_events(&conversation_id, None, 50)
            .unwrap()
            .events
            .len();
        let path = fixture.png.to_string_lossy().into_owned();
        let missing = fixture.directory.join("missing.png");
        let cases: Vec<(Value, &str)> = vec![
            (json!({"path": path}), "attachment_invalid"),
            (
                json!([{"name": "a.png", "mediaType": "image/png"}]),
                "attachment_invalid",
            ),
            (
                json!([{"name": "a.png", "mediaType": "image/png", "path": path, "byteSize": 1}]),
                "attachment_invalid",
            ),
            (
                json!([{"name": "a.pdf", "mediaType": "application/pdf", "path": path}]),
                "attachment_media_unsupported",
            ),
            (
                json!([{"name": "a.png", "mediaType": "image/png", "path": "https://example.test/a.png"}]),
                "attachment_remote_unsupported",
            ),
            (
                json!([{"name": "a.png", "mediaType": "image/png", "path": missing.to_string_lossy()}]),
                "attachment_unavailable",
            ),
            (
                json!([
                    {"name": "1.png", "mediaType": "image/png", "path": path},
                    {"name": "2.png", "mediaType": "image/png", "path": path},
                    {"name": "3.png", "mediaType": "image/png", "path": path},
                    {"name": "4.png", "mediaType": "image/png", "path": path},
                    {"name": "5.png", "mediaType": "image/png", "path": path},
                ]),
                "attachment_limit_exceeded",
            ),
        ];
        for (attachments, expected) in cases {
            let error = service
                .execute(json!({
                    "action": "conversation.message.post",
                    "conversationId": conversation_id,
                    "authorMembershipId": owner_id,
                    "content": "must not persist",
                    "attachments": attachments,
                }))
                .expect_err("dishonest attachment post must reject");
            assert_eq!(error.to_string(), expected);
        }
        let after = service
            .store()
            .page_events(&conversation_id, None, 50)
            .unwrap()
            .events
            .len();
        assert_eq!(before, after, "rejected posts must not persist events");
    }

    /// Fake-adapter end-to-end: a group post carrying one image attachment
    /// persists the image part and dispatches the member turn with the
    /// attachment reference in the shared adapter wire shape.
    #[test]
    fn group_dispatch_carries_attachment_references_to_member_turn_params() {
        let fixture = ImageFixture::new();
        let calls = Arc::new(Mutex::new(Vec::<Value>::new()));
        let recorded = Arc::clone(&calls);
        let service = ConversationService::from_store(
            crate::domain::client_conversation::ConversationStore::open_in_memory().unwrap(),
        )
        .with_native_turn_sender(move |params| {
            recorded.lock().unwrap().push(params.clone());
            Ok(accepted_receipt(params))
        });
        let (conversation_id, owner_id, agent_id) = group_fixture(&service);

        let posted = persist_then_dispatch(
            &service,
            json!({
                "action": "conversation.message.post",
                "conversationId": conversation_id,
                "authorMembershipId": owner_id,
                "content": "@One look at this mockup",
                "attachments": [fixture.attachment()],
            }),
        );

        assert_eq!(posted["directTurns"][0]["state"], "running");
        let events = service
            .store()
            .page_events(&conversation_id, None, 20)
            .unwrap()
            .events;
        let event = events
            .iter()
            .find(|event| event.id == posted["event"]["id"])
            .unwrap();
        let image_part = event
            .parts
            .iter()
            .find(|part| part.kind == super::super::EventPartKind::Image)
            .unwrap();
        let calls = calls.lock().unwrap();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0]["membershipId"], agent_id);
        assert_eq!(calls[0]["text"], "@One look at this mockup");
        let attachments = calls[0]["attachments"].as_array().unwrap();
        assert_eq!(attachments.len(), 1);
        // The durable Event Part identity is the dispatch reference id.
        assert_eq!(attachments[0]["id"], json!(image_part.id));
        assert_eq!(attachments[0]["name"], "synthetic.png");
        assert_eq!(attachments[0]["mediaType"], "image/png");
        assert_eq!(attachments[0]["path"], json!(fixture.png.to_string_lossy()));
        assert!(
            attachments[0].get("byteSize").is_none(),
            "byte size stays in the canonical record"
        );
    }

    /// An image-only group post admits empty content, persists only the image
    /// Event Part, and still dispatches to the designated Assistant — the
    /// default dispatch target when no mention exists. The wire text is the
    /// composed Assistant guidance with an empty user-authored portion, and
    /// the turn params carry the attachment reference.
    #[test]
    fn image_only_group_post_persists_and_dispatches_to_the_assistant() {
        let fixture = ImageFixture::new();
        let calls = Arc::new(Mutex::new(Vec::<Value>::new()));
        let recorded = Arc::clone(&calls);
        let service = ConversationService::from_store(
            crate::domain::client_conversation::ConversationStore::open_in_memory().unwrap(),
        )
        .with_native_turn_sender(move |params| {
            recorded.lock().unwrap().push(params.clone());
            Ok(accepted_receipt(params))
        });
        let (conversation_id, owner_id, agent_id) = group_fixture(&service);
        let conversation_revision = service.store().get(&conversation_id).unwrap().revision;
        service
            .execute(json!({
                "action": "conversation.assistant.set",
                "conversationId": conversation_id,
                "ownerMembershipId": owner_id,
                "expectedRevision": conversation_revision,
                "membershipId": agent_id,
            }))
            .unwrap();

        let posted = persist_then_dispatch(
            &service,
            json!({
                "action": "conversation.message.post",
                "conversationId": conversation_id,
                "authorMembershipId": owner_id,
                "content": "",
                "attachments": [fixture.attachment()],
            }),
        );

        assert_eq!(posted["directTurns"][0]["state"], "running");
        let events = service
            .store()
            .page_events(&conversation_id, None, 20)
            .unwrap()
            .events;
        let event = events
            .iter()
            .find(|event| event.id == posted["event"]["id"])
            .unwrap();
        // No empty text part pollutes the record: the image part stands alone.
        assert_eq!(event.parts.len(), 1);
        assert_eq!(event.parts[0].kind, super::super::EventPartKind::Image);
        let calls = calls.lock().unwrap();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0]["membershipId"], agent_id);
        let text = calls[0]["text"].as_str().unwrap();
        assert!(!text.trim().is_empty());
        assert!(calls[0].get("privateInstructions").is_none());
        let attachments = calls[0]["attachments"].as_array().unwrap();
        assert_eq!(attachments.len(), 1);
        assert_eq!(attachments[0]["id"], json!(event.parts[0].id));
        assert_eq!(attachments[0]["path"], json!(fixture.png.to_string_lossy()));
    }

    /// The text-only admission rule is unchanged: empty content without
    /// attachments still rejects before anything persists.
    #[test]
    fn message_post_still_rejects_empty_content_without_attachments() {
        let service = ConversationService::from_store(
            crate::domain::client_conversation::ConversationStore::open_in_memory().unwrap(),
        );
        let (conversation_id, owner_id, _) = group_fixture(&service);
        let before = service
            .store()
            .page_events(&conversation_id, None, 50)
            .unwrap()
            .events
            .len();
        for request in [
            json!({
                "action": "conversation.message.post",
                "conversationId": conversation_id,
                "authorMembershipId": owner_id,
                "content": "",
            }),
            json!({
                "action": "conversation.message.post",
                "conversationId": conversation_id,
                "authorMembershipId": owner_id,
                "content": "   ",
            }),
            json!({
                "action": "conversation.message.post",
                "conversationId": conversation_id,
                "authorMembershipId": owner_id,
                "content": "",
                "attachments": [],
            }),
        ] {
            let error = service
                .execute(request)
                .expect_err("empty content without attachments must reject");
            assert_eq!(error.to_string(), "invalid_request");
        }
        let after = service
            .store()
            .page_events(&conversation_id, None, 50)
            .unwrap()
            .events
            .len();
        assert_eq!(before, after, "rejected posts must not persist events");
    }

    /// AC-014: with one image-capable member and one text-only member, the
    /// capable member's turn carries the attachment reference through the
    /// existing adapter admission while the text-only member rejects honestly
    /// before launch — no partial dispatch, no silent drop.
    #[cfg(unix)]
    #[test]
    fn text_only_member_rejects_attachments_before_launch() {
        use crate::platform::runtime_adapters::{RuntimeAdapterError, send_message};

        let fixture = ImageFixture::new();
        type Recorded = (
            String,
            Value,
            std::result::Result<Value, RuntimeAdapterError>,
        );
        let calls = Arc::new(Mutex::new(Vec::<Recorded>::new()));
        let recorded = Arc::clone(&calls);
        let service = ConversationService::from_store(
            crate::domain::client_conversation::ConversationStore::open_in_memory().unwrap(),
        )
        .with_native_turn_sender(move |params| {
            let agent = params["agentId"].as_str().unwrap_or_default().to_owned();
            // Route through the real 1:1 adapter admission; /bin/sh stands in
            // for the image-capable adapter binary exactly like the adapter
            // dispatch tests do.
            let mut admitted = params.clone();
            admitted["binaryPath"] = json!("/bin/sh");
            let result = send_message(&crate::target_port::agent_target_port(), &admitted);
            recorded
                .lock()
                .unwrap()
                .push((agent, params.clone(), result.clone()));
            result
        });
        let group = service
            .execute(json!({
                "action": "conversation.create",
                "title": "Mixed capability",
                "owner": {"id": "human:local", "kind": "human", "displayName": "You"},
                "members": [
                    {"principal": {"id": "agent:codex", "kind": "agent", "displayName": "Codex", "agentId": "codex"}, "access": "member"},
                    {"principal": {"id": "agent:claude", "kind": "agent", "displayName": "Claude", "agentId": "claude-code"}, "access": "member"}
                ]
            }))
            .unwrap();
        let memberships = group["memberships"].as_array().unwrap();
        let membership_of = |agent: &str| {
            memberships
                .iter()
                .find(|membership| membership["principal"]["agentId"] == json!(agent))
                .unwrap()["id"]
                .as_str()
                .unwrap()
                .to_owned()
        };
        let codex_membership = membership_of("codex");
        let claude_membership = membership_of("claude-code");
        let owner_id = memberships
            .iter()
            .find(|membership| membership["principal"]["kind"] == "human")
            .unwrap()["id"]
            .as_str()
            .unwrap()
            .to_owned();
        let conversation_id = group["id"].as_str().unwrap().to_owned();

        let posted = persist_then_dispatch(
            &service,
            json!({
                "action": "conversation.message.post",
                "conversationId": conversation_id,
                "authorMembershipId": owner_id,
                "content": "@Codex @Claude review this screenshot",
                "attachments": [fixture.attachment()],
            }),
        );

        // Both member turns were attempted with the attachment reference.
        let calls = calls.lock().unwrap();
        assert_eq!(calls.len(), 2);
        for (_, params, _) in calls.iter() {
            let attachments = params["attachments"].as_array().unwrap();
            assert_eq!(attachments.len(), 1);
            assert_eq!(attachments[0]["path"], json!(fixture.png.to_string_lossy()));
            assert_eq!(attachments[0]["mediaType"], "image/png");
        }
        // The image-capable member passed admission and reached its driver
        // (the shell fixture fails at the Codex protocol stage, proving a
        // process was actually launched).
        let codex_call = calls.iter().find(|(agent, _, _)| agent == "codex").unwrap();
        let codex_result = codex_call.2.as_ref().expect("codex send dispatched");
        assert_eq!(codex_result["ok"], false);
        let code = codex_result["error"]["code"].as_str().unwrap_or_default();
        assert!(
            code.starts_with("codex_"),
            "expected a Codex protocol failure, got {code}"
        );
        // The text-only member rejected honestly before launch.
        let claude_call = calls
            .iter()
            .find(|(agent, _, _)| agent == "claude-code")
            .unwrap();
        assert_eq!(
            claude_call.2,
            Err(RuntimeAdapterError::AttachmentUnsupportedForAdapter {
                agent_label: "claude-code".to_owned()
            })
        );
        drop(calls);

        // Turn receipts report the honest per-member outcome.
        let receipts = posted["directTurns"].as_array().unwrap();
        assert_eq!(receipts.len(), 2);
        let state_of = |membership_id: &str| {
            receipts
                .iter()
                .find(|receipt| {
                    service
                        .store()
                        .direct_turn(receipt["id"].as_str().unwrap())
                        .unwrap()
                        .membership_id
                        == membership_id
                })
                .unwrap()["state"]
                .as_str()
                .unwrap()
                .to_owned()
        };
        assert_eq!(state_of(&codex_membership), "running");
        assert_eq!(state_of(&claude_membership), "failed");
        assert_eq!(
            posted["strategyError"]["code"],
            "conversation_dispatch_failed"
        );
    }

    #[test]
    fn dispatch_after_post_without_the_host_runtime_is_fail_closed() {
        let service = ConversationService::from_store(
            crate::domain::client_conversation::ConversationStore::open_in_memory().unwrap(),
        );
        let (conversation_id, owner_id, _) = group_fixture(&service);
        let posted = service
            .execute(json!({
                "action": "conversation.message.post",
                "conversationId": conversation_id,
                "authorMembershipId": owner_id,
                "content": "@One hello"
            }))
            .unwrap();
        let before = service
            .store()
            .page_events(&conversation_id, None, 20)
            .unwrap()
            .events
            .len();
        let error = service
            .execute(json!({
                "action": "conversation.dispatch.after-post",
                "conversationId": conversation_id,
                "eventId": posted["event"]["id"],
            }))
            .expect_err("dispatch without the host runtime must reject");
        assert_eq!(
            error.to_string(),
            super::super::PERSISTENT_TRANSPORT_REQUIRED
        );
        let after = service
            .store()
            .page_events(&conversation_id, None, 20)
            .unwrap()
            .events
            .len();
        assert_eq!(
            before, after,
            "no Agent work and no settlement happened without the host"
        );
    }

    #[test]
    fn owner_can_delete_a_settled_post_through_the_canonical_action() {
        let service = ConversationService::from_store_with_runtime(
            ConversationStore::open_in_memory().unwrap(),
            |_| Err(crate::platform::runtime_adapters::RuntimeAdapterError::ExecutableUnavailable),
        );
        let (conversation_id, owner_id, _) = group_fixture(&service);
        let posted = persist_then_dispatch(
            &service,
            json!({
                "action": "conversation.message.post",
                "conversationId": conversation_id,
                "authorMembershipId": owner_id,
                "content": "@One remove this failed attempt"
            }),
        );
        let event_id = posted["event"]["id"].as_str().unwrap().to_owned();
        let before = service
            .store()
            .page_events(&conversation_id, None, 20)
            .unwrap()
            .events;
        assert!(
            before
                .iter()
                .any(|event| event.causation_id.as_deref() == Some(event_id.as_str())),
            "fixture includes the derived failure"
        );

        service
            .execute(json!({
                "action": "conversation.message.delete",
                "conversationId": conversation_id,
                "eventId": event_id,
                "ownerMembershipId": owner_id,
            }))
            .unwrap();

        let after = service
            .store()
            .page_events(&conversation_id, None, 20)
            .unwrap()
            .events;
        assert!(after.iter().all(|event| {
            event.id != event_id && event.causation_id.as_deref() != Some(event_id.as_str())
        }));
    }

    #[test]
    fn owner_cannot_delete_a_post_while_its_turn_is_active() {
        let service = ConversationService::from_store_with_runtime(
            ConversationStore::open_in_memory().unwrap(),
            |params| Ok(accepted_receipt(params)),
        );
        let (conversation_id, owner_id, _) = group_fixture(&service);
        let posted = persist_then_dispatch(
            &service,
            json!({
                "action": "conversation.message.post",
                "conversationId": conversation_id,
                "authorMembershipId": owner_id,
                "content": "@One keep running"
            }),
        );
        let event_id = posted["event"]["id"].as_str().unwrap();

        let error = service
            .execute(json!({
                "action": "conversation.message.delete",
                "conversationId": conversation_id,
                "eventId": event_id,
                "ownerMembershipId": owner_id,
            }))
            .expect_err("active work must not be deleted");

        assert_eq!(error.to_string(), "message_turn_active");
        assert!(
            service
                .store()
                .page_events(&conversation_id, None, 20)
                .unwrap()
                .events
                .iter()
                .any(|event| event.id == event_id)
        );
    }

    #[test]
    fn clear_empties_group_history_archives_children_and_rotates_assistant() {
        let service = ConversationService::from_store_with_runtime(
            ConversationStore::open_in_memory().unwrap(),
            |_| Err(crate::platform::runtime_adapters::RuntimeAdapterError::ExecutableUnavailable),
        );
        let (conversation_id, owner_id, agent_id) = group_fixture(&service);
        let before = service.store().get(&conversation_id).unwrap();
        service
            .execute(json!({
                "action": "conversation.assistant.set",
                "conversationId": conversation_id,
                "ownerMembershipId": owner_id,
                "expectedRevision": before.revision,
                "membershipId": agent_id,
            }))
            .unwrap();
        service
            .execute(json!({
                "action": "conversation.message.post",
                "conversationId": conversation_id,
                "authorMembershipId": owner_id,
                "content": "remember this"
            }))
            .unwrap();
        let child = service
            .execute(json!({
                "action": "conversation.create",
                "title": "Child task",
                "owner": {"id": "human:local", "kind": "human", "displayName": "You"},
                "members": [{
                    "principal": {
                        "id": "agent:one",
                        "kind": "agent",
                        "displayName": "One",
                        "agentId": "one"
                    },
                    "access": "member"
                }]
            }))
            .unwrap();
        let child_id = child["id"].as_str().unwrap().to_owned();
        service
            .store()
            .register_continuity_child_link(&conversation_id, &child_id, "goal:clear-child")
            .unwrap();

        let cleared = service
            .execute(json!({
                "action": "conversation.clear",
                "conversationId": conversation_id,
                "ownerMembershipId": owner_id,
            }))
            .unwrap();

        assert_eq!(cleared["ok"], true);
        assert_eq!(cleared["archivedChildIds"], json!([child_id]));
        let after = service.store().get(&conversation_id).unwrap();
        assert_eq!(after.event_count, 0);
        assert_ne!(
            after.assistant_membership_id.as_deref(),
            Some(agent_id.as_str())
        );
        assert_eq!(
            after.assistant_membership_id.as_deref(),
            cleared["assistantMembershipId"].as_str()
        );
        let child_after = service.store().get(&child_id).unwrap();
        assert!(child_after.archived);
        assert!(
            after
                .memberships
                .iter()
                .all(|membership| membership.id != agent_id)
        );
        assert!(
            service
                .store()
                .private_runtime_binding(
                    &conversation_id,
                    after.assistant_membership_id.as_deref().unwrap(),
                )
                .unwrap()
                .is_none()
        );
        assert!(
            service
                .store()
                .page_events(&conversation_id, None, 20)
                .unwrap()
                .events
                .is_empty()
        );
    }

    #[test]
    fn archive_reopen_cascades_children_and_opens_a_silent_successor() {
        let service = ConversationService::from_store_with_runtime(
            ConversationStore::open_in_memory().unwrap(),
            |_| Err(crate::platform::runtime_adapters::RuntimeAdapterError::ExecutableUnavailable),
        );
        let (conversation_id, owner_id, agent_id) = group_fixture(&service);
        let before = service.store().get(&conversation_id).unwrap();
        service
            .execute(json!({
                "action": "conversation.assistant.set",
                "conversationId": conversation_id,
                "ownerMembershipId": owner_id,
                "expectedRevision": before.revision,
                "membershipId": agent_id,
            }))
            .unwrap();
        service
            .execute(json!({
                "action": "conversation.message.post",
                "conversationId": conversation_id,
                "authorMembershipId": owner_id,
                "content": "remember this"
            }))
            .unwrap();
        let child = service
            .execute(json!({
                "action": "conversation.create",
                "title": "Child task",
                "owner": {"id": "human:local", "kind": "human", "displayName": "You"},
                "members": [{
                    "principal": {
                        "id": "agent:one",
                        "kind": "agent",
                        "displayName": "One",
                        "agentId": "one"
                    },
                    "access": "member"
                }]
            }))
            .unwrap();
        let child_id = child["id"].as_str().unwrap().to_owned();
        service
            .store()
            .register_continuity_child_link(&conversation_id, &child_id, "goal:archive-child")
            .unwrap();
        let grandchild = service
            .execute(json!({
                "action": "conversation.create",
                "title": "Grandchild task",
                "owner": {"id": "human:local", "kind": "human", "displayName": "You"},
                "members": [{
                    "principal": {
                        "id": "agent:one",
                        "kind": "agent",
                        "displayName": "One",
                        "agentId": "one"
                    },
                    "access": "member"
                }]
            }))
            .unwrap();
        let grandchild_id = grandchild["id"].as_str().unwrap().to_owned();
        service
            .store()
            .register_continuity_child_link(&child_id, &grandchild_id, "goal:archive-grandchild")
            .unwrap();

        let archived = service
            .execute(json!({
                "action": "conversation.archive",
                "conversationId": conversation_id,
                "archived": true,
                "reopen": true,
            }))
            .unwrap();

        assert_eq!(archived["ok"], true);
        assert_eq!(
            archived["archivedChildIds"],
            json!([child_id.clone(), grandchild_id.clone()])
        );
        assert!(service.store().get(&conversation_id).unwrap().archived);
        assert!(service.store().get(&child_id).unwrap().archived);
        assert!(service.store().get(&grandchild_id).unwrap().archived);
        // Archiving keeps the prior history in place for review and restore.
        assert!(service.store().get(&conversation_id).unwrap().event_count > 0);

        let successor = archived["successor"].clone();
        let successor_id = successor["id"].as_str().unwrap().to_owned();
        assert_ne!(successor_id, conversation_id);
        assert_eq!(successor["title"], json!("Direct Turn"));
        assert_eq!(successor["isGroup"], json!(true));
        assert_eq!(successor["archived"], json!(false));
        let successor_members = successor["memberships"].as_array().unwrap();
        assert_eq!(successor_members.len(), 2);
        let successor_agent = successor_members
            .iter()
            .find(|membership| membership["principal"]["kind"] == "agent")
            .unwrap();
        assert_eq!(successor["assistantMembershipId"], successor_agent["id"]);
        let successor_events = service
            .store()
            .page_events(&successor_id, None, 20)
            .unwrap()
            .events;
        assert_eq!(successor_events.len(), 1);
        assert_eq!(successor_events[0].kind, EventKind::ConversationReset);
        assert!(
            successor_events
                .iter()
                .all(|event| event.kind != EventKind::MembershipChanged)
        );
    }

    #[test]
    fn archiving_the_default_local_group_resets_it_in_place() {
        let service = ConversationService::from_store_with_runtime(
            ConversationStore::open_in_memory().unwrap(),
            |_| Err(crate::platform::runtime_adapters::RuntimeAdapterError::ExecutableUnavailable),
        );
        let local = service.store().ensure_default_local_group().unwrap();
        let owner_id = local
            .memberships
            .iter()
            .find(|membership| membership.principal.kind == PrincipalKind::Human)
            .map(|membership| membership.id.clone())
            .unwrap();
        service
            .execute(json!({
                "action": "conversation.message.post",
                "conversationId": local.id,
                "authorMembershipId": owner_id,
                "content": "remember this"
            }))
            .unwrap();

        let archived = service
            .execute(json!({
                "action": "conversation.archive",
                "conversationId": local.id,
                "archived": true,
                "reopen": true,
            }))
            .unwrap();

        assert_eq!(archived["ok"], true);
        assert_eq!(archived["resetInPlace"], true);
        assert!(archived.get("successor").is_none());

        // The same reserved Conversation stays active and pinned with one
        // reset notice; no archived original or reopened copy can duplicate
        // it, and the next service open finds nothing to resurrect.
        let reset = service.store().get(&local.id).unwrap();
        assert!(!reset.archived);
        assert!(reset.pinned);
        let events = service
            .store()
            .page_events(&local.id, None, 20)
            .unwrap()
            .events;
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].kind, EventKind::ConversationReset);
    }

    #[test]
    fn clear_refuses_unfinalized_or_active_group_work() {
        let service = ConversationService::from_store_with_runtime(
            ConversationStore::open_in_memory().unwrap(),
            |params| Ok(accepted_receipt(params)),
        );
        let (conversation_id, owner_id, _) = group_fixture(&service);
        persist_then_dispatch(
            &service,
            json!({
                "action": "conversation.message.post",
                "conversationId": conversation_id,
                "authorMembershipId": owner_id,
                "content": "@One still running"
            }),
        );
        let error = service
            .execute(json!({
                "action": "conversation.clear",
                "conversationId": conversation_id,
                "ownerMembershipId": owner_id,
            }))
            .expect_err("active work must block clear");
        assert_eq!(error.to_string(), "conversation_clear_blocked");
        assert!(
            !service
                .store()
                .page_events(&conversation_id, None, 20)
                .unwrap()
                .events
                .is_empty()
        );
    }

    #[test]
    fn dispatch_after_post_admits_only_identity_and_assistant_suppression() {
        let service = ConversationService::from_store(
            crate::domain::client_conversation::ConversationStore::open_in_memory().unwrap(),
        )
        .with_native_turn_sender(|_| {
            panic!("event validation must run before any runtime dispatch")
        });
        let (conversation_id, owner_id, agent_id) = group_fixture(&service);
        let posted = service
            .execute(json!({
                "action": "conversation.message.post",
                "conversationId": conversation_id,
                "authorMembershipId": owner_id,
                "content": "@One hello"
            }))
            .unwrap();
        for extra in [
            json!({"content": "@One hello"}),
            json!({"mentionedMembershipIds": [agent_id]}),
            json!({"suppressAssistant": "true"}),
        ] {
            let mut request = json!({
                "action": "conversation.dispatch.after-post",
                "conversationId": conversation_id,
                "eventId": posted["event"]["id"],
            });
            request
                .as_object_mut()
                .unwrap()
                .extend(extra.as_object().unwrap().clone());
            let error = service
                .execute(request)
                .expect_err("extra field must reject");
            assert_eq!(error.to_string(), "invalid_request");
        }
        let missing = service.execute(json!({
            "action": "conversation.dispatch.after-post",
            "conversationId": conversation_id,
            "eventId": "event:missing",
        }));
        assert_eq!(
            missing.unwrap_err().to_string(),
            "conversation_event_not_found"
        );
    }

    #[test]
    fn group_native_sessions_are_opt_in_local_get_facts() {
        let service = ConversationService::from_store(ConversationStore::open_in_memory().unwrap());
        let (conversation_id, _, membership_id) = group_fixture(&service);
        let agent_id = service
            .store()
            .get(&conversation_id)
            .unwrap()
            .memberships
            .into_iter()
            .find(|membership| membership.id == membership_id)
            .unwrap()
            .principal
            .agent_id
            .unwrap();
        let scope = service
            .store()
            .prepare_runtime_dispatch(
                &agent_id,
                "synthetic-session",
                "Synthetic request",
                Some(&conversation_id),
                Some(&membership_id),
                None,
                None,
            )
            .unwrap();
        service
            .store()
            .bind_runtime_session(&scope, &agent_id, "synthetic-session", None, None)
            .unwrap();
        let ordinary = service
            .execute(json!({"action":"conversation.get","conversationId":conversation_id}))
            .unwrap();
        assert!(ordinary.get("nativeSessionReferences").is_none());
        let local = service
            .execute(
                json!({"action":"conversation.get","conversationId":conversation_id,
            "includeNativeSessionReferences":true}),
            )
            .unwrap();
        assert_eq!(
            local["nativeSessionReferences"],
            json!([{"membershipId":membership_id,
            "agentId":agent_id,"nativeSessionId":"synthetic-session"}])
        );
        assert!(local.get("runtimeBindings").is_none());
    }

    #[test]
    fn mention_aliases_follow_the_client_boundary_rule() {
        assert!(mention_alias_matches("@One hello", "One"));
        assert!(mention_alias_matches("hello @one", "One"));
        assert!(mention_alias_matches("hello @one.", "One"));
        assert!(mention_alias_matches("hello @one,", "one"));
        assert!(mention_alias_matches("hello @one，", "one"));
        assert!(mention_alias_matches("hello @one。", "one"));
        assert!(mention_alias_matches("hello @ONE?", "one"));
        assert!(mention_alias_matches("ask @one two", "one"));
        assert!(!mention_alias_matches("hello@one", "one"));
        assert!(!mention_alias_matches("@onex", "one"));
        assert!(!mention_alias_matches("@one-two", "one"));
        assert!(!mention_alias_matches(
            "email one@example.com",
            "example.com"
        ));
        assert!(mention_alias_matches("ping @a.b now", "a.b"));
        assert!(!mention_alias_matches("ping @a.bx now", "a.b"));
    }

    #[test]
    fn product_startup_restores_one_canonical_default_local_group() {
        let root = std::env::temp_dir().join(format!(
            "lico-conversation-service-default-{}",
            uuid::Uuid::new_v4()
        ));

        let service = ConversationService::open(&root).unwrap();
        let groups = service.store().list(false).unwrap();
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].id, super::super::DEFAULT_LOCAL_AGENT_GROUP_ID);
        assert_eq!(
            groups[0].title,
            super::super::DEFAULT_LOCAL_AGENT_GROUP_TITLE
        );
        assert!(groups[0].pinned);
        assert!(groups[0].is_group);
        let local = service
            .store()
            .get(super::super::DEFAULT_LOCAL_AGENT_GROUP_ID)
            .unwrap();
        assert_eq!(local.memberships.len(), 1);
        assert_eq!(local.memberships[0].principal.id, "human:local");
        assert_eq!(local.memberships[0].access, MembershipAccess::Owner);
        drop(service);

        let reopened = ConversationService::open(&root).unwrap();
        assert_eq!(reopened.store().list(false).unwrap().len(), 1);
        assert_eq!(
            reopened
                .store()
                .get(super::super::DEFAULT_LOCAL_AGENT_GROUP_ID)
                .unwrap()
                .memberships
                .len(),
            1
        );
        drop(reopened);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn group_strategy_selection_persists_until_explicitly_cleared() {
        let root = std::env::temp_dir().join(format!(
            "lico-conversation-service-strategy-{}",
            uuid::Uuid::new_v4()
        ));
        let conversation_id = super::super::DEFAULT_LOCAL_AGENT_GROUP_ID;

        let service = ConversationService::open(&root).unwrap();
        service
            .execute(json!({
                "action": "conversation.strategy.set",
                "conversationId": conversation_id,
                "strategyRevision": "revision-one"
            }))
            .unwrap();
        let selected = service
            .execute(json!({
                "action": "conversation.get",
                "conversationId": conversation_id
            }))
            .unwrap();
        assert_eq!(selected["strategyRevision"], "revision-one");
        let selected_revision = selected["revision"].as_i64().unwrap();
        service
            .execute(json!({
                "action": "conversation.strategy.set",
                "conversationId": conversation_id,
                "strategyRevision": "revision-one"
            }))
            .unwrap();
        assert_eq!(
            service
                .execute(json!({
                    "action": "conversation.get",
                    "conversationId": conversation_id
                }))
                .unwrap()["revision"],
            selected_revision
        );
        drop(service);

        let reopened = ConversationService::open(&root).unwrap();
        assert_eq!(
            reopened
                .execute(json!({
                    "action": "conversation.get",
                    "conversationId": conversation_id
                }))
                .unwrap()["strategyRevision"],
            "revision-one"
        );
        reopened
            .execute(json!({
                "action": "conversation.strategy.set",
                "conversationId": conversation_id,
                "strategyRevision": null
            }))
            .unwrap();
        assert!(
            reopened
                .execute(json!({
                    "action": "conversation.get",
                    "conversationId": conversation_id
                }))
                .unwrap()
                .get("strategyRevision")
                .is_none()
        );

        drop(reopened);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn direct_turn_fanout_is_bounded_parallel_and_preserves_receipt_order() {
        let in_flight = Arc::new((Mutex::new(0usize), Condvar::new()));
        let release = Arc::new((Mutex::new(false), Condvar::new()));
        let calls = Arc::new(Mutex::new(Vec::<Value>::new()));
        let runtime_in_flight = Arc::clone(&in_flight);
        let runtime_release = Arc::clone(&release);
        let runtime_calls = Arc::clone(&calls);
        let service = ConversationService::from_store_with_runtime(
            ConversationStore::open_in_memory().unwrap(),
            move |params| {
                runtime_calls.lock().unwrap().push(params.clone());
                {
                    let (lock, cvar) = &*runtime_in_flight;
                    let mut count = lock.lock().unwrap_or_else(|poison| poison.into_inner());
                    *count += 1;
                    cvar.notify_all();
                }
                let (lock, cvar) = &*runtime_release;
                let mut flag = lock.lock().unwrap_or_else(|poison| poison.into_inner());
                while !*flag {
                    flag = cvar.wait(flag).unwrap_or_else(|poison| poison.into_inner());
                }
                drop(flag);
                Ok(accepted_receipt(params))
            },
        );
        let conversation = service
            .execute(json!({
                "action": "conversation.create",
                "title": "Fanout",
                "owner": {"id": "human:local", "kind": "human", "displayName": "You"},
                "members": [
                    {"principal": {"id": "agent:one", "kind": "agent", "displayName": "One", "agentId": "one"}, "access": "member"},
                    {"principal": {"id": "agent:two", "kind": "agent", "displayName": "Two", "agentId": "two"}, "access": "member"},
                    {"principal": {"id": "agent:three", "kind": "agent", "displayName": "Three", "agentId": "three"}, "access": "member"},
                    {"principal": {"id": "agent:four", "kind": "agent", "displayName": "Four", "agentId": "four"}, "access": "member"},
                    {"principal": {"id": "agent:five", "kind": "agent", "displayName": "Five", "agentId": "five"}, "access": "member"}
                ]
            }))
            .unwrap();
        let memberships = conversation["memberships"].as_array().unwrap();
        let owner_id = memberships
            .iter()
            .find(|membership| membership["principal"]["kind"] == "human")
            .unwrap()["id"]
            .as_str()
            .unwrap()
            .to_owned();

        let conversation_id = conversation["id"].as_str().unwrap().to_owned();
        let persist_request = json!({
            "action": "conversation.message.post",
            "conversationId": conversation_id,
            "authorMembershipId": owner_id,
            "content": "@One @Two @Three @Four @Five run all"
        });
        let persisted = service.execute(persist_request).unwrap();
        let event_id = persisted["event"]["id"].clone();
        let service_for_post = service.clone();
        let dispatch_conversation_id = conversation_id.clone();
        let post = std::thread::spawn(move || {
            service_for_post
                .execute(json!({
                    "action": "conversation.dispatch.after-post",
                    "conversationId": dispatch_conversation_id,
                    "eventId": event_id,
                }))
                .unwrap()
        });

        {
            let (lock, cvar) = &*in_flight;
            let mut count = lock.lock().unwrap_or_else(|poison| poison.into_inner());
            while *count < DEFAULT_DIRECT_TURN_WORKERS {
                count = cvar
                    .wait(count)
                    .unwrap_or_else(|poison| poison.into_inner());
            }
        }
        let _guard = ReleaseGuard(&release);
        assert_eq!(
            service.store().counters().peak_in_flight_turns(),
            DEFAULT_DIRECT_TURN_WORKERS
        );
        assert_eq!(
            calls.lock().unwrap().len(),
            DEFAULT_DIRECT_TURN_WORKERS,
            "the fifth mention must wait for a worker slot"
        );
        let leases_while_blocked = service.store().counters().leases();
        std::thread::sleep(std::time::Duration::from_millis(60));
        assert_eq!(
            service.store().counters().leases(),
            leases_while_blocked,
            "no SQLite lease may span runtime work"
        );

        drop(_guard);
        let posted = post.join().unwrap();
        let receipts = posted["directTurns"].as_array().unwrap();
        assert_eq!(receipts.len(), 5);
        for (index, receipt) in receipts.iter().enumerate() {
            assert_eq!(receipt["state"], "running");
            let turn_id = receipt["id"].as_str().unwrap();
            let turn = service.store().direct_turn(turn_id).unwrap();
            assert_eq!(turn.ordinal, index as i64, "receipt {index} out of order");
        }
        assert_eq!(posted["turns"].as_array().unwrap().len(), 5);
        assert_eq!(posted["dispatchPending"], true);
        assert!(posted.get("strategyError").is_none());
        assert_eq!(calls.lock().unwrap().len(), 5);
        assert_eq!(
            service.store().counters().peak_in_flight_turns(),
            DEFAULT_DIRECT_TURN_WORKERS
        );
        assert_eq!(
            service.store().counters().in_flight_turns(),
            0,
            "no direct turn may stay in flight after the fanout completes"
        );
        assert!(
            service.store().counters().peak_in_flight()
                <= crate::domain::client_conversation::store::DEFAULT_CONVERSATION_POOL_SIZE,
            "pool leases stay bounded by the configured connection pool"
        );
    }

    #[test]
    fn creates_initial_group_memberships_through_one_service_action() {
        let service = ConversationService::from_store(
            crate::domain::client_conversation::ConversationStore::open_in_memory().unwrap(),
        );
        let result = service
            .execute(json!({
                "action": "conversation.create",
                "title": "Group",
                "owner": {"id": "human:local", "kind": "human", "displayName": "You"},
                "members": [
                    {"principal": {"id": "agent:one", "kind": "agent", "displayName": "One", "agentId": "one"}, "access": "member"}
                ]
            }))
            .unwrap();

        assert_eq!(result["memberships"].as_array().unwrap().len(), 2);
        assert_eq!(result["isGroup"], true);
    }

    #[test]
    fn structured_mention_dispatches_once_and_returns_the_attachable_handle() {
        let calls = Arc::new(Mutex::new(Vec::<Value>::new()));
        let captured_calls = Arc::clone(&calls);
        let service = ConversationService::from_store_with_runtime(
            ConversationStore::open_in_memory().unwrap(),
            move |params| {
                captured_calls.lock().unwrap().push(params.clone());
                Ok(accepted_receipt(params))
            },
        );
        let (conversation_id, owner_id, agent_id) = group_fixture(&service);

        let posted = persist_then_dispatch(
            &service,
            json!({
                "action": "conversation.message.post",
                "conversationId": conversation_id,
                "authorMembershipId": owner_id,
                "content": "@One Please answer"
            }),
        );

        assert_eq!(posted["directTurns"][0]["state"], "running");
        assert_eq!(posted["event"]["state"], "finalized");
        assert!(posted["event"].get("parts").is_none());
        assert!(posted.to_string().len() < 1024);
        assert_eq!(posted["dispatchPending"], true);
        assert_eq!(
            posted["turns"][0]["turnHandle"],
            posted["directTurns"][0]["id"]
        );
        assert_eq!(posted["turns"][0]["membershipId"], agent_id);
        let calls = calls.lock().unwrap();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0]["agentId"], "one");
        assert_eq!(calls[0]["text"], "@One Please answer");
        assert!(calls[0].get("timeoutMs").is_none());
        assert_eq!(calls[0]["streamEvents"], true);
        assert_eq!(calls[0]["conversationId"], conversation_id);
        assert_eq!(calls[0]["membershipId"], agent_id);
        assert_eq!(calls[0]["causationId"], posted["event"]["id"]);
        assert_eq!(calls[0]["dispatchId"], posted["directTurns"][0]["id"]);
        assert!(calls[0].get("maxStdoutBytes").is_none());
        drop(calls);
        let events = service
            .store()
            .page_events(&conversation_id, None, 50)
            .unwrap()
            .events;
        assert!(
            !events
                .iter()
                .any(|event| event.author_membership_id.as_deref() == Some(agent_id.as_str())),
            "the service never writes the agent reply; the completion authority owns it"
        );
    }

    #[test]
    fn persistent_group_dispatch_finalizes_one_canonical_agent_event() {
        let store = ConversationStore::open_in_memory().unwrap();
        let runtime_store = store.clone();
        let service =
            ConversationService::from_store(store).with_native_turn_sender(move |params| {
                let scope = runtime_store
                    .prepare_runtime_dispatch(
                        params["agentId"].as_str().unwrap(),
                        "",
                        params["text"].as_str().unwrap(),
                        params["conversationId"].as_str(),
                        params["membershipId"].as_str(),
                        params["causationId"].as_str(),
                        params["dispatchId"].as_str(),
                    )
                    .unwrap();
                runtime_store
                    .append_runtime_frame(
                        &scope,
                        1,
                        &json!({
                            "event": "agent.message.completed",
                            "sessionId": "session-fixture",
                            "turnId": "turn-fixture",
                            "payload": {"text": "agent answer"}
                        }),
                    )
                    .unwrap();
                runtime_store
                    .finish_runtime_dispatch(
                        &scope,
                        &json!({"ok": true, "output": "agent answer"}),
                        crate::domain::client_conversation::DispatchState::Completed,
                        None,
                    )
                    .unwrap();
                Ok(json!({
                    "ok": true,
                    "accepted": true,
                    "turnHandle": params["dispatchId"],
                    "nativeSessionId": "session-fixture"
                }))
            });
        let (conversation_id, owner_id, _) = group_fixture(&service);

        let posted = persist_then_dispatch(
            &service,
            json!({
                "action": "conversation.message.post",
                "conversationId": conversation_id,
                "authorMembershipId": owner_id,
                "content": "@One one answer"
            }),
        );

        assert_eq!(posted["directTurns"][0]["state"], "succeeded");
        let turn_id = posted["directTurns"][0]["id"].as_str().unwrap();
        let events = service
            .store()
            .page_events(&conversation_id, None, 20)
            .unwrap()
            .events;
        let replies = events
            .iter()
            .filter(|event| event.correlation_id.as_deref() == Some(turn_id))
            .collect::<Vec<_>>();
        assert_eq!(replies.len(), 1);
        assert!(replies[0].finalized);
        assert_eq!(
            replies[0]
                .parts
                .iter()
                .filter(|part| part.kind == super::super::EventPartKind::Text)
                .count(),
            1
        );
        assert_eq!(
            replies[0]
                .parts
                .iter()
                .find(|part| part.kind == super::super::EventPartKind::Text)
                .unwrap()
                .content,
            "agent answer"
        );
        for expected_lifecycle in [
            r#"{"lifecycle":"submitted"}"#,
            r#"{"lifecycle":"accepted"}"#,
            r#"{"lifecycle":"processing"}"#,
            r#"{"lifecycle":"responding"}"#,
            r#"{"lifecycle":"completed"}"#,
        ] {
            assert!(replies[0].parts.iter().any(|part| {
                part.kind == super::super::EventPartKind::Metadata
                    && part.content == expected_lifecycle
            }));
        }
    }

    #[test]
    fn private_continuation_is_membership_scoped_and_non_pending_turns_never_replay() {
        let store = ConversationStore::open_in_memory().unwrap();
        let runtime_store = store.clone();
        let calls = Arc::new(Mutex::new(Vec::<Value>::new()));
        let captured_calls = Arc::clone(&calls);
        let service =
            ConversationService::from_store(store).with_native_turn_sender(move |params| {
                captured_calls.lock().unwrap().push(params.clone());
                let scope = runtime_store
                    .prepare_runtime_dispatch(
                        params["agentId"].as_str().unwrap(),
                        params
                            .get("sessionId")
                            .and_then(Value::as_str)
                            .unwrap_or(""),
                        params["text"].as_str().unwrap(),
                        params["conversationId"].as_str(),
                        params["membershipId"].as_str(),
                        params["causationId"].as_str(),
                        params["dispatchId"].as_str(),
                    )
                    .unwrap();
                runtime_store
                    .bind_runtime_session(
                        &scope,
                        params["agentId"].as_str().unwrap(),
                        "session-fixture",
                        Some("/fixture/session.jsonl"),
                        Some("/fixture/project"),
                    )
                    .unwrap();
                runtime_store
                    .finish_runtime_dispatch(
                        &scope,
                        &json!({
                            "ok": true,
                            "output": "done",
                            "nativeSessionId": "session-fixture",
                            "sourcePath": "/fixture/session.jsonl",
                            "workingDirectory": "/fixture/project"
                        }),
                        crate::domain::client_conversation::DispatchState::Completed,
                        None,
                    )
                    .unwrap();
                Ok(json!({
                    "ok": true,
                    "accepted": true,
                    "turnHandle": params["dispatchId"],
                    "nativeSessionId": "session-fixture"
                }))
            });
        let service = service;
        let (conversation_id, owner_id, _) = group_fixture(&service);
        let post = |content: &str| {
            persist_then_dispatch(
                &service,
                json!({
                    "action": "conversation.message.post",
                    "conversationId": conversation_id,
                    "authorMembershipId": owner_id,
                    "content": content
                }),
            )
        };
        let first = post("@One first");
        let _ = post("@One second");

        let calls = calls.lock().unwrap();
        assert_eq!(calls.len(), 2);
        assert!(calls[0].get("sessionId").is_none());
        assert_eq!(calls[1]["sessionId"], "session-fixture");
        assert_eq!(calls[1]["sourcePath"], "/fixture/session.jsonl");
        assert_eq!(calls[1]["workingDirectory"], "/fixture/project");
        drop(calls);
        let completed_turn = first["directTurns"][0]["id"].as_str().unwrap();
        assert!(
            service
                .store()
                .claim_direct_turn(completed_turn)
                .unwrap()
                .is_none()
        );
        let export_root =
            std::env::temp_dir().join(format!("lico-direct-turn-export-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&export_root).unwrap();
        let bundle = export_root.join("bundle.json");
        service
            .store()
            .export_bundle(&bundle, std::slice::from_ref(&conversation_id))
            .unwrap();
        let exported = std::fs::read_to_string(&bundle).unwrap();
        assert!(!exported.contains("session-fixture"));
        assert!(!exported.contains("/fixture/session.jsonl"));
        assert!(!exported.contains("/fixture/project"));
        std::fs::remove_dir_all(export_root).unwrap();
    }

    #[test]
    fn ordinary_message_does_not_dispatch() {
        let calls = Arc::new(Mutex::new(0usize));
        let captured_calls = Arc::clone(&calls);
        let service = ConversationService::from_store_with_runtime(
            ConversationStore::open_in_memory().unwrap(),
            move |_| {
                *captured_calls.lock().unwrap() += 1;
                Ok(json!({"ok": true, "accepted": true, "turnHandle": "dispatch:unexpected"}))
            },
        );
        let (conversation_id, owner_id, _) = group_fixture(&service);
        let ordinary = persist_then_dispatch(
            &service,
            json!({
                "action": "conversation.message.post",
                "conversationId": conversation_id,
                "authorMembershipId": owner_id,
                "content": "ordinary"
            }),
        );
        assert!(ordinary["directTurns"].as_array().unwrap().is_empty());
        assert_eq!(ordinary["dispatchPending"], false);
        assert!(ordinary["turns"].as_array().unwrap().is_empty());
        assert_eq!(*calls.lock().unwrap(), 0);
    }

    #[test]
    fn pre_dispatch_rejection_settles_the_unopened_turn_with_a_typed_code() {
        let calls = Arc::new(Mutex::new(0usize));
        let captured_calls = Arc::clone(&calls);
        let service = ConversationService::from_store_with_runtime(
            ConversationStore::open_in_memory().unwrap(),
            move |_| {
                *captured_calls.lock().unwrap() += 1;
                Err(crate::platform::runtime_adapters::RuntimeAdapterError::ExecutableUnavailable)
            },
        );
        let (conversation_id, owner_id, agent_id) = group_fixture(&service);
        let failed = persist_then_dispatch(
            &service,
            json!({
                "action": "conversation.message.post",
                "conversationId": conversation_id,
                "authorMembershipId": owner_id,
                "content": "@One run"
            }),
        );
        assert_eq!(failed["directTurns"][0]["state"], "failed");
        assert_eq!(failed["dispatchPending"], false);
        assert_eq!(
            failed["strategyError"]["code"],
            "conversation_dispatch_failed"
        );
        assert_eq!(*calls.lock().unwrap(), 1);
        let events = service
            .store()
            .page_events(&conversation_id, None, 50)
            .unwrap()
            .events;
        let replies = events
            .iter()
            .filter(|event| event.author_membership_id.as_deref() == Some(agent_id.as_str()))
            .collect::<Vec<_>>();
        assert_eq!(replies.len(), 1, "one settlement event for the rejection");
        let diagnostic = replies[0]
            .parts
            .iter()
            .find(|part| part.kind == super::super::EventPartKind::Diagnostic)
            .unwrap();
        assert!(
            diagnostic
                .content
                .contains("native_agent_executable_unavailable")
        );
        assert!(diagnostic.content.contains("process/launch"));
    }

    #[test]
    fn opened_dispatch_rejection_defers_to_the_completion_authority() {
        let store = ConversationStore::open_in_memory().unwrap();
        let runtime_store = store.clone();
        let service =
            ConversationService::from_store(store).with_native_turn_sender(move |params| {
                let scope = runtime_store
                    .prepare_runtime_dispatch(
                        params["agentId"].as_str().unwrap(),
                        "",
                        params["text"].as_str().unwrap(),
                        params["conversationId"].as_str(),
                        params["membershipId"].as_str(),
                        params["causationId"].as_str(),
                        params["dispatchId"].as_str(),
                    )
                    .unwrap();
                runtime_store
                    .finish_runtime_dispatch(
                        &scope,
                        &json!({
                            "ok": false,
                            "error": {
                                "code": "native_agent_executable_unavailable",
                                "stage": "process/launch"
                            }
                        }),
                        crate::domain::client_conversation::DispatchState::Failed,
                        Some("native_agent_executable_unavailable"),
                    )
                    .unwrap();
                Err(crate::platform::runtime_adapters::RuntimeAdapterError::ExecutableUnavailable)
            });
        let (conversation_id, owner_id, agent_id) = group_fixture(&service);
        let failed = persist_then_dispatch(
            &service,
            json!({
                "action": "conversation.message.post",
                "conversationId": conversation_id,
                "authorMembershipId": owner_id,
                "content": "@One run"
            }),
        );
        assert_eq!(failed["directTurns"][0]["state"], "failed");
        let events = service
            .store()
            .page_events(&conversation_id, None, 50)
            .unwrap()
            .events;
        let replies = events
            .iter()
            .filter(|event| event.author_membership_id.as_deref() == Some(agent_id.as_str()))
            .collect::<Vec<_>>();
        assert_eq!(
            replies.len(),
            1,
            "the completion authority wrote the only terminal event"
        );
        assert!(replies[0].finalized);
        assert!(
            replies[0]
                .parts
                .iter()
                .any(|part| part.kind == super::super::EventPartKind::Diagnostic
                    && part.content.contains("native_agent_executable_unavailable"))
        );
    }

    #[test]
    fn accepted_receipt_keeps_mention_turn_running_for_attach() {
        let calls = Arc::new(Mutex::new(Vec::<Value>::new()));
        let captured_calls = Arc::clone(&calls);
        let service = ConversationService::from_store_with_runtime(
            ConversationStore::open_in_memory().unwrap(),
            move |params| {
                captured_calls.lock().unwrap().push(params.clone());
                Ok(json!({
                    "ok": true,
                    "accepted": true,
                    "turnHandle": "dispatch:live",
                    "conversationId": params["conversationId"],
                    "membershipId": params["membershipId"],
                }))
            },
        );
        let (conversation_id, owner_id, agent_id) = group_fixture(&service);
        let posted = persist_then_dispatch(
            &service,
            json!({
                "action": "conversation.message.post",
                "conversationId": conversation_id,
                "authorMembershipId": owner_id,
                "content": "@One Please answer"
            }),
        );
        assert_eq!(posted["directTurns"][0]["state"], "running");
        assert_eq!(posted["dispatchPending"], true);
        assert_eq!(posted["turns"][0]["turnHandle"], "dispatch:live");
        assert_eq!(posted["turns"][0]["membershipId"], agent_id);
        assert_eq!(calls.lock().unwrap()[0]["streamEvents"], true);
        let events = service
            .store()
            .page_events(&conversation_id, None, 50)
            .unwrap()
            .events;
        assert!(
            !events
                .iter()
                .any(|event| event.author_membership_id.as_deref() == Some(agent_id.as_str()))
        );
    }

    #[test]
    fn strategy_bound_plain_post_starts_the_graph_without_mention_turns() {
        let calls = Arc::new(Mutex::new(Vec::<Value>::new()));
        let captured = Arc::clone(&calls);
        let service = ConversationService::from_store(ConversationStore::open_in_memory().unwrap())
            .with_native_turn_sender(|_| {
                panic!("plain strategy post must not start a mention turn")
            })
            .with_active_turns(|_| json!({"turns": []}))
            .with_strategy_execute(move |request| {
                captured.lock().unwrap().push(request.clone());
                let action = request.get("action").and_then(Value::as_str).unwrap_or("");
                if action == "strategy.run.active" {
                    Ok(json!({"ok": true, "result": {"runId": null}}))
                } else {
                    Ok(json!({
                        "ok": true,
                        "result": {
                            "runId": "run-1",
                            "status": "running",
                            "entryTurn": {
                                "turnHandle": "dispatch:entry",
                                "membershipId": "membership:entry",
                                "agent": "one"
                            }
                        }
                    }))
                }
            });
        let (conversation_id, owner_id, _) = group_fixture(&service);
        service
            .execute(json!({
                "action": "conversation.strategy.set",
                "conversationId": conversation_id,
                "strategyRevision": "revision-one"
            }))
            .unwrap();
        let posted = persist_then_dispatch(
            &service,
            json!({
                "action": "conversation.message.post",
                "conversationId": conversation_id,
                "authorMembershipId": owner_id,
                "content": "start the graph"
            }),
        );
        assert!(posted["directTurns"].as_array().unwrap().is_empty());
        assert!(posted.get("strategyError").is_none());
        assert_eq!(posted["turns"][0]["turnHandle"], "dispatch:entry");
        assert_eq!(posted["turns"][0]["membershipId"], "membership:entry");
        assert_eq!(posted["turns"][0]["agent"], "one");
        assert_eq!(posted["dispatchPending"], true);
        let actions = calls
            .lock()
            .unwrap()
            .iter()
            .map(|request| request["action"].as_str().unwrap().to_owned())
            .collect::<Vec<_>>();
        assert_eq!(
            actions,
            vec![
                "strategy.run.active".to_owned(),
                "strategy.run.start".to_owned()
            ]
        );
        assert_eq!(
            calls.lock().unwrap()[1]["input"]["message"],
            "start the graph"
        );
        assert_eq!(calls.lock().unwrap()[1]["conversationId"], conversation_id);
    }

    #[test]
    fn in_flight_follow_up_steers_instead_of_starting_a_mention_turn() {
        let steers = Arc::new(Mutex::new(Vec::<Value>::new()));
        let captured = Arc::clone(&steers);
        let service = ConversationService::from_store(ConversationStore::open_in_memory().unwrap())
            .with_native_turn_sender(|_| panic!("in-flight follow-up must steer"))
            .with_active_turns(|conversation_id| {
                json!({
                    "turns": [{
                        "turnHandle": "dispatch:live",
                        "conversationId": conversation_id,
                        "membershipId": "membership:ignored",
                        "agent": "one"
                    }]
                })
            })
            .with_steer_turn(move |params| {
                captured.lock().unwrap().push(params.clone());
                Ok(json!({"ok": true, "status": "accepted"}))
            });
        let (conversation_id, owner_id, _) = group_fixture(&service);
        let posted = persist_then_dispatch(
            &service,
            json!({
                "action": "conversation.message.post",
                "conversationId": conversation_id,
                "authorMembershipId": owner_id,
                "content": "steer please"
            }),
        );
        assert_eq!(posted["turns"][0]["turnHandle"], "dispatch:live");
        assert_eq!(posted["dispatchPending"], true);
        assert!(posted.get("strategyError").is_none());
        assert_eq!(steers.lock().unwrap()[0]["text"], "steer please");
        assert_eq!(steers.lock().unwrap()[0]["turnHandle"], "dispatch:live");
    }

    #[test]
    fn a_follow_up_never_steers_into_a_turn_the_store_already_closed() {
        let refusing = Arc::new(Mutex::new(true));
        let runtime_mode = Arc::clone(&refusing);
        let started = Arc::new(Mutex::new(Vec::<Value>::new()));
        let captured_started = Arc::clone(&started);
        let steers = Arc::new(Mutex::new(Vec::<Value>::new()));
        let captured_steers = Arc::clone(&steers);
        let host_turns = Arc::new(Mutex::new(Vec::<Value>::new()));
        let host_view = Arc::clone(&host_turns);
        let service = ConversationService::from_store(ConversationStore::open_in_memory().unwrap())
            .with_native_turn_sender(move |params| {
                if *runtime_mode.lock().unwrap() {
                    return Err(
                        crate::platform::runtime_adapters::RuntimeAdapterError::ExecutableUnavailable,
                    );
                }
                captured_started.lock().unwrap().push(params.clone());
                Ok(accepted_receipt(params))
            })
            .with_active_turns(move |_| json!({"turns": host_view.lock().unwrap().clone()}))
            .with_steer_turn(move |params| {
                captured_steers.lock().unwrap().push(params.clone());
                Ok(json!({"ok": true, "status": "accepted"}))
            });
        let (conversation_id, owner_id, agent_id) = group_fixture(&service);
        let closed = persist_then_dispatch(
            &service,
            json!({
                "action": "conversation.message.post",
                "conversationId": conversation_id,
                "authorMembershipId": owner_id,
                "content": "@One run"
            }),
        );
        assert_eq!(closed["directTurns"][0]["state"], "failed");
        // The owning host keeps advertising the turn its own process opened,
        // while cold recovery already closed it in the durable record.
        host_turns.lock().unwrap().push(json!({
            "turnHandle": closed["directTurns"][0]["id"],
            "conversationId": conversation_id,
            "membershipId": agent_id,
            "agent": "one"
        }));
        *refusing.lock().unwrap() = false;
        let follow_up = persist_then_dispatch(
            &service,
            json!({
                "action": "conversation.message.post",
                "conversationId": conversation_id,
                "authorMembershipId": owner_id,
                "content": "@One run"
            }),
        );
        assert!(
            steers.lock().unwrap().is_empty(),
            "a closed turn can never receive a steer"
        );
        assert_eq!(started.lock().unwrap().len(), 1);
        assert_eq!(follow_up["directTurns"][0]["state"], "running");
        assert_ne!(
            follow_up["directTurns"][0]["id"],
            closed["directTurns"][0]["id"]
        );
        assert!(
            !follow_up["turns"]
                .as_array()
                .unwrap()
                .iter()
                .any(|turn| turn["turnHandle"] == closed["directTurns"][0]["id"]),
            "a closed handle must not come back as an attachable turn"
        );
    }

    #[test]
    fn a_follow_up_never_steers_into_a_closed_dispatch_without_a_direct_turn() {
        let started = Arc::new(Mutex::new(Vec::<Value>::new()));
        let captured_started = Arc::clone(&started);
        let steers = Arc::new(Mutex::new(Vec::<Value>::new()));
        let captured_steers = Arc::clone(&steers);
        let host_turns = Arc::new(Mutex::new(Vec::<Value>::new()));
        let host_view = Arc::clone(&host_turns);
        let service = ConversationService::from_store(ConversationStore::open_in_memory().unwrap())
            .with_native_turn_sender(move |params| {
                captured_started.lock().unwrap().push(params.clone());
                Ok(accepted_receipt(params))
            })
            .with_active_turns(move |_| json!({"turns": host_view.lock().unwrap().clone()}))
            .with_steer_turn(move |params| {
                captured_steers.lock().unwrap().push(params.clone());
                Ok(json!({"ok": true, "status": "accepted"}))
            });
        let (conversation_id, owner_id, agent_id) = group_fixture(&service);
        // A dispatch the host can still name that never had a direct turn of
        // its own — the shape child-work and subagent dispatches take — already
        // settled in the durable record.
        let closed = service
            .store()
            .create_dispatch(
                &conversation_id,
                &agent_id,
                "send",
                DispatchSessionMode::New,
            )
            .unwrap();
        service
            .store()
            .update_dispatch(&closed.id, DispatchState::Completed, None, None)
            .unwrap();
        host_turns.lock().unwrap().push(json!({
            "turnHandle": closed.id,
            "conversationId": conversation_id,
            "membershipId": agent_id,
            "agent": "one"
        }));
        let posted = persist_then_dispatch(
            &service,
            json!({
                "action": "conversation.message.post",
                "conversationId": conversation_id,
                "authorMembershipId": owner_id,
                "content": "@One run"
            }),
        );
        assert!(
            steers.lock().unwrap().is_empty(),
            "a settled dispatch can never receive a steer"
        );
        assert_eq!(started.lock().unwrap().len(), 1);
        assert_eq!(posted["directTurns"][0]["state"], "running");
    }

    #[test]
    fn in_flight_steer_failure_is_reported_without_restarting_the_turn() {
        let service = ConversationService::from_store(ConversationStore::open_in_memory().unwrap())
            .with_native_turn_sender(|_| panic!("a failed steer must not restart"))
            .with_active_turns(|conversation_id| {
                json!({
                    "turns": [{
                        "turnHandle": "dispatch:live",
                        "conversationId": conversation_id,
                        "membershipId": "membership:agent",
                        "agent": "one"
                    }]
                })
            })
            .with_steer_turn(|_| {
                Err(crate::platform::runtime_adapters::RuntimeAdapterError::ConversationDispatchFailed)
            });
        let (conversation_id, owner_id, _) = group_fixture(&service);
        let posted = persist_then_dispatch(
            &service,
            json!({
                "action": "conversation.message.post",
                "conversationId": conversation_id,
                "authorMembershipId": owner_id,
                "content": "follow up"
            }),
        );
        assert_eq!(posted["turns"][0]["turnHandle"], "dispatch:live");
        assert_eq!(posted["dispatchPending"], true);
        assert_eq!(
            posted["strategyError"],
            json!({
                "code": "conversation_dispatch_failed",
                "stage": "conversation/steer"
            })
        );
    }

    #[test]
    fn known_unsupported_steer_queues_one_membership_turn_at_the_boundary() {
        let base = ConversationService::from_store(ConversationStore::open_in_memory().unwrap());
        let (conversation_id, owner_id, agent_id) = group_fixture(&base);
        let active_conversation = conversation_id.clone();
        let active_membership = agent_id.clone();
        let service = base
            .with_native_turn_sender(|_| panic!("boundary follow-up must not start early"))
            .with_active_turns(move |_| {
                json!({"turns": [{
                    "turnHandle": "dispatch:live",
                    "conversationId": active_conversation,
                    "membershipId": active_membership,
                    "agent": "one"
                }]})
            })
            .with_steer_turn(|_| Ok(json!({"ok": false, "status": "unsupported"})));
        let posted = persist_then_dispatch(
            &service,
            json!({
                "action": "conversation.message.post",
                "conversationId": conversation_id,
                "authorMembershipId": owner_id,
                "content": "follow after the active turn"
            }),
        );
        assert!(posted.get("strategyError").is_none());
        assert_eq!(posted["directTurns"].as_array().unwrap().len(), 1);
        assert_eq!(posted["directTurns"][0]["state"], "pending");
        assert_eq!(posted["turns"][0]["turnHandle"], "dispatch:live");
    }

    #[test]
    fn waiting_follow_up_resumes_the_run_without_a_new_handle() {
        let calls = Arc::new(Mutex::new(Vec::<Value>::new()));
        let captured = Arc::clone(&calls);
        let service = ConversationService::from_store(ConversationStore::open_in_memory().unwrap())
            .with_native_turn_sender(|_| panic!("waiting follow-up must resume"))
            .with_active_turns(|_| json!({"turns": []}))
            .with_strategy_execute(move |request| {
                captured.lock().unwrap().push(request.clone());
                let action = request.get("action").and_then(Value::as_str).unwrap_or("");
                if action == "strategy.run.active" {
                    Ok(json!({"ok": true, "result": {"runId": "run-1", "status": "waiting"}}))
                } else {
                    assert_eq!(action, "strategy.run.resume");
                    Ok(json!({"ok": true, "result": {"runId": "run-1", "status": "running"}}))
                }
            });
        let (conversation_id, owner_id, _) = group_fixture(&service);
        service
            .execute(json!({
                "action": "conversation.strategy.set",
                "conversationId": conversation_id,
                "strategyRevision": "revision-one"
            }))
            .unwrap();
        let posted = persist_then_dispatch(
            &service,
            json!({
                "action": "conversation.message.post",
                "conversationId": conversation_id,
                "authorMembershipId": owner_id,
                "content": "continue"
            }),
        );
        assert!(posted["turns"].as_array().unwrap().is_empty());
        assert_eq!(posted["dispatchPending"], false);
        assert!(posted.get("strategyError").is_none());
        let actions = calls
            .lock()
            .unwrap()
            .iter()
            .map(|request| request["action"].as_str().unwrap().to_owned())
            .collect::<Vec<_>>();
        assert_eq!(
            actions,
            vec![
                "strategy.run.active".to_owned(),
                "strategy.run.resume".to_owned()
            ]
        );
        assert_eq!(calls.lock().unwrap()[1]["runId"], "run-1");
        assert_eq!(calls.lock().unwrap()[1]["conversationId"], conversation_id);
    }

    #[test]
    fn running_graph_follow_up_resumes_without_synthesizing_a_start_failure() {
        let calls = Arc::new(Mutex::new(Vec::<Value>::new()));
        let captured = Arc::clone(&calls);
        let service = ConversationService::from_store(ConversationStore::open_in_memory().unwrap())
            .with_native_turn_sender(|_| panic!("running graph must not start a mention turn"))
            .with_active_turns(|_| json!({"turns": []}))
            .with_strategy_execute(move |request| {
                captured.lock().unwrap().push(request.clone());
                let action = request.get("action").and_then(Value::as_str).unwrap_or("");
                if action == "strategy.run.active" {
                    Ok(json!({"ok": true, "result": {"runId": "run-1", "status": "running"}}))
                } else {
                    assert_eq!(action, "strategy.run.resume");
                    Ok(json!({"ok": true, "result": {"runId": "run-1", "status": "running"}}))
                }
            });
        let (conversation_id, owner_id, _) = group_fixture(&service);
        service
            .execute(json!({
                "action": "conversation.strategy.set",
                "conversationId": conversation_id,
                "strategyRevision": "revision-one"
            }))
            .unwrap();
        let posted = persist_then_dispatch(
            &service,
            json!({
                "action": "conversation.message.post",
                "conversationId": conversation_id,
                "authorMembershipId": owner_id,
                "content": "still running"
            }),
        );
        assert!(posted["turns"].as_array().unwrap().is_empty());
        assert_eq!(posted["dispatchPending"], false);
        assert!(
            posted.get("strategyError").is_none(),
            "a resume without a fresh entry handle is not a failure"
        );
        let actions = calls
            .lock()
            .unwrap()
            .iter()
            .map(|request| request["action"].as_str().unwrap().to_owned())
            .collect::<Vec<_>>();
        assert_eq!(
            actions,
            vec![
                "strategy.run.active".to_owned(),
                "strategy.run.resume".to_owned()
            ]
        );
        assert_eq!(calls.lock().unwrap()[1]["conversationId"], conversation_id);
    }

    #[test]
    fn strategy_start_failure_returns_an_inline_banner_error() {
        let service = ConversationService::from_store(ConversationStore::open_in_memory().unwrap())
            .with_native_turn_sender(|_| panic!("failed start must not dispatch a mention turn"))
            .with_active_turns(|_| json!({"turns": []}))
            .with_strategy_execute(|request| {
                let action = request.get("action").and_then(Value::as_str).unwrap_or("");
                if action == "strategy.run.active" {
                    Ok(json!({"ok": true, "result": {"runId": null}}))
                } else {
                    Ok(json!({
                        "ok": false,
                        "error": {"code": "strategy_actor_quota_exhausted"}
                    }))
                }
            });
        let (conversation_id, owner_id, _) = group_fixture(&service);
        service
            .execute(json!({
                "action": "conversation.strategy.set",
                "conversationId": conversation_id,
                "strategyRevision": "revision-one"
            }))
            .unwrap();
        let posted = persist_then_dispatch(
            &service,
            json!({
                "action": "conversation.message.post",
                "conversationId": conversation_id,
                "authorMembershipId": owner_id,
                "content": "start"
            }),
        );
        assert_eq!(
            posted["strategyError"]["code"],
            "strategy_actor_quota_exhausted"
        );
        assert_eq!(posted["strategyError"]["stage"], "strategy/start");
        assert_eq!(posted["dispatchPending"], false);
    }

    #[test]
    fn strategy_bound_dispatch_without_the_strategy_port_is_fail_closed() {
        let service = ConversationService::from_store(ConversationStore::open_in_memory().unwrap())
            .with_native_turn_sender(|_| panic!("fail-closed dispatch must not reach the runtime"))
            .with_active_turns(|_| json!({"turns": []}));
        let (conversation_id, owner_id, _) = group_fixture(&service);
        service
            .execute(json!({
                "action": "conversation.strategy.set",
                "conversationId": conversation_id,
                "strategyRevision": "revision-one"
            }))
            .unwrap();
        let error = persist_then_dispatch_error(
            &service,
            json!({
                "action": "conversation.message.post",
                "conversationId": conversation_id,
                "authorMembershipId": owner_id,
                "content": "start"
            }),
        );
        assert_eq!(
            error,
            super::super::PERSISTENT_TRANSPORT_REQUIRED.to_owned()
        );
    }

    fn persist_then_dispatch_error(service: &ConversationService, request: Value) -> String {
        let persisted = service
            .execute(request.clone())
            .expect("persist posted message");
        let event_id = persisted["event"]["id"].clone();
        service
            .execute(json!({
                "action": "conversation.dispatch.after-post",
                "conversationId": request["conversationId"].clone(),
                "eventId": event_id,
            }))
            .expect_err("dispatch must reject")
            .to_string()
    }

    #[test]
    fn clearing_strategy_does_not_cancel_or_execute_a_run() {
        let service = ConversationService::from_store(ConversationStore::open_in_memory().unwrap())
            .with_strategy_execute(|_| panic!("clearing the capsule must not touch the run"));
        let (conversation_id, _, _) = group_fixture(&service);
        service
            .execute(json!({
                "action": "conversation.strategy.set",
                "conversationId": conversation_id,
                "strategyRevision": "revision-one"
            }))
            .unwrap();
        service
            .execute(json!({
                "action": "conversation.strategy.set",
                "conversationId": conversation_id,
                "strategyRevision": null
            }))
            .unwrap();
        let conversation = service.store().get(&conversation_id).unwrap();
        assert!(conversation.strategy_revision.is_none());
    }
    #[test]
    fn assistant_designation_profile_actions_and_candidate_projection_are_bounded() {
        let service = ConversationService::from_store(
            crate::domain::client_conversation::ConversationStore::open_in_memory().unwrap(),
        );
        let (conversation_id, owner_id, agent_one) = group_fixture(&service);
        let agent_two = service
            .execute(json!({
                "action": "conversation.membership.add",
                "conversationId": conversation_id,
                "principal": {
                    "id": "agent:two",
                    "kind": "agent",
                    "displayName": "Two",
                    "agentId": "two",
                },
                "access": "member",
            }))
            .unwrap()["id"]
            .as_str()
            .unwrap()
            .to_owned();

        let conversation_revision = service.store().get(&conversation_id).unwrap().revision;
        service
            .execute(json!({
                "action": "conversation.assistant.set",
                "conversationId": conversation_id,
                "ownerMembershipId": owner_id,
                "expectedRevision": conversation_revision,
                "membershipId": agent_one,
            }))
            .unwrap();
        let conversation = service.store().get(&conversation_id).unwrap();
        assert_eq!(
            conversation.assistant_membership_id.as_deref(),
            Some(agent_one.as_str())
        );

        let intent = json!({
            "requiredCapabilities": [],
            "preferredCapabilities": ["workspace"],
            "skillReferences": [],
            "preferredModel": "model-a",
            "preferredReasoningEffort": "high",
            "preferredEnvironment": "local",
        });
        for membership_id in [&agent_one, &agent_two] {
            let expected_revision = service
                .store()
                .membership_profile(membership_id)
                .unwrap()
                .unwrap()
                .revision;
            let updated = service
                .execute(json!({
                    "action": "conversation.profile.update",
                    "conversationId": conversation_id,
                    "membershipId": membership_id,
                    "ownerMembershipId": owner_id,
                    "expectedRevision": expected_revision,
                    "intent": intent,
                }))
                .unwrap();
            assert_eq!(updated["profile"]["revision"], expected_revision + 1);
        }
        let stored = service
            .execute(json!({
                "action": "conversation.profile.get",
                "membershipId": agent_one,
            }))
            .unwrap();
        assert_eq!(stored["revision"], 2);
        assert_eq!(stored["preferredModel"], "model-a");
        assert_eq!(stored["preferredReasoningEffort"], "high");
        assert_eq!(stored["responsibility"], "assistant");
        assert!(
            stored["skillReferences"]
                .as_array()
                .unwrap()
                .iter()
                .any(|skill| skill == "licoup-guide")
        );

        let candidates = service
            .execute(json!({
                "action": "conversation.profile.candidates",
                "conversationId": conversation_id,
            }))
            .unwrap();
        assert_eq!(candidates["candidates"].as_array().unwrap().len(), 2);
        assert!(candidates["timeoutPolicy"]["policy"]["defaultTimeoutMs"].is_number());
        assert!(candidates["timeoutPolicy"]["minTimeoutMs"].is_number());
        assert_eq!(
            candidates["routeReceipt"]["rankedMembershipIds"]
                .as_array()
                .unwrap()
                .len(),
            2
        );
        assert!(
            candidates["routeReceipt"]["sourceRevisions"]
                .as_array()
                .unwrap()
                .iter()
                .all(|source| source["revision"] != "local-current")
        );

        let hard_failure = service
            .execute(json!({
                "action": "conversation.profile.candidates",
                "conversationId": conversation_id,
                "filters": {"membershipIds": ["membership:missing"]},
            }))
            .expect_err("a missing exact binding must reject before any effect");
        assert_eq!(hard_failure.to_string(), "profile_candidate_rejected");
    }

    #[test]
    fn designated_group_plain_message_dispatches_to_the_assistant_membership_turn() {
        let calls = Arc::new(Mutex::new(Vec::<Value>::new()));
        let captured_calls = Arc::clone(&calls);
        let service = ConversationService::from_store_with_runtime(
            ConversationStore::open_in_memory().unwrap(),
            move |params| {
                captured_calls.lock().unwrap().push(params.clone());
                Ok(accepted_receipt(params))
            },
        );
        let (conversation_id, owner_id, agent_one) = group_fixture(&service);
        let conversation_revision = service.store().get(&conversation_id).unwrap().revision;
        service
            .execute(json!({
                "action": "conversation.assistant.set",
                "conversationId": conversation_id,
                "ownerMembershipId": owner_id,
                "expectedRevision": conversation_revision,
                "membershipId": agent_one,
            }))
            .unwrap();
        let profile_revision = service
            .store()
            .membership_profile(&agent_one)
            .unwrap()
            .unwrap()
            .revision;
        service
            .execute(json!({
                "action": "conversation.profile.update",
                "conversationId": conversation_id,
                "membershipId": agent_one,
                "ownerMembershipId": owner_id,
                "expectedRevision": profile_revision,
                "intent": {
                    "preferredModel": "model-a",
                    "preferredReasoningEffort": "high"
                }
            }))
            .unwrap();

        let posted = persist_then_dispatch(
            &service,
            json!({
                "action": "conversation.message.post",
                "conversationId": conversation_id,
                "authorMembershipId": owner_id,
                "content": "plain message without a mention",
            }),
        );
        assert_eq!(posted["dispatchPending"], true);
        assert_eq!(posted["directTurns"].as_array().unwrap().len(), 1);
        assert_eq!(posted["directTurns"][0]["state"], "running");
        assert_eq!(posted["turns"][0]["membershipId"], agent_one);
        let calls = calls.lock().unwrap();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0]["membershipId"], agent_one);
        let text = calls[0]["text"].as_str().unwrap();
        assert_ne!(text, "plain message without a mention");
        assert!(text.ends_with("plain message without a mention"));
        assert!(calls[0].get("privateInstructions").is_none());
        assert!(calls[0].get("timeoutMs").is_none());
        assert_eq!(calls[0]["streamEvents"], true);
        assert_eq!(calls[0]["model"], "model-a");
        assert_eq!(calls[0]["reasoningEffort"], "high");
    }

    #[test]
    fn suppressed_assistant_is_excluded_while_an_explicit_other_agent_still_starts() {
        let calls = Arc::new(Mutex::new(Vec::<Value>::new()));
        let captured_calls = Arc::clone(&calls);
        let service = ConversationService::from_store_with_runtime(
            ConversationStore::open_in_memory().unwrap(),
            move |params| {
                captured_calls.lock().unwrap().push(params.clone());
                Ok(accepted_receipt(params))
            },
        );
        let (conversation_id, owner_id, assistant_id) = group_fixture(&service);
        let other_id = service
            .execute(json!({
                "action": "conversation.membership.add",
                "conversationId": conversation_id,
                "principal": {
                    "id": "agent:two",
                    "kind": "agent",
                    "displayName": "Two",
                    "agentId": "two",
                },
                "access": "member",
            }))
            .unwrap()["id"]
            .as_str()
            .unwrap()
            .to_owned();
        let revision = service.store().get(&conversation_id).unwrap().revision;
        service
            .execute(json!({
                "action": "conversation.assistant.set",
                "conversationId": conversation_id,
                "ownerMembershipId": owner_id,
                "expectedRevision": revision,
                "membershipId": assistant_id,
            }))
            .unwrap();

        let plain = persist_then_dispatch_with_assistant(
            &service,
            json!({
                "action": "conversation.message.post",
                "conversationId": conversation_id,
                "authorMembershipId": owner_id,
                "content": "plain group note",
            }),
            true,
        );
        assert!(plain["directTurns"].as_array().unwrap().is_empty());
        assert!(calls.lock().unwrap().is_empty());
        assert!(
            service
                .store()
                .page_events(&conversation_id, None, 20)
                .unwrap()
                .events
                .iter()
                .flat_map(|event| event.parts.iter())
                .any(|part| {
                    part.kind == super::super::EventPartKind::Text
                        && part.content == "plain group note"
                })
        );

        let mentioned = persist_then_dispatch_with_assistant(
            &service,
            json!({
                "action": "conversation.message.post",
                "conversationId": conversation_id,
                "authorMembershipId": owner_id,
                "content": "@One @Two review this",
            }),
            true,
        );
        assert_eq!(mentioned["directTurns"].as_array().unwrap().len(), 1);
        let calls = calls.lock().unwrap();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0]["membershipId"], other_id);
    }

    #[test]
    fn suppressed_plain_follow_up_steers_only_the_other_active_turn() {
        let steers = Arc::new(Mutex::new(Vec::<Value>::new()));
        let captured_steers = Arc::clone(&steers);
        let service = ConversationService::from_store(
            crate::domain::client_conversation::ConversationStore::open_in_memory().unwrap(),
        )
        .with_native_turn_sender(|_| panic!("active turns must be steered, not restarted"))
        .with_steer_turn(move |params| {
            captured_steers.lock().unwrap().push(params.clone());
            Ok(json!({"ok": true, "status": "accepted"}))
        });
        let (conversation_id, owner_id, assistant_id) = group_fixture(&service);
        let other_id = service
            .execute(json!({
                "action": "conversation.membership.add",
                "conversationId": conversation_id,
                "principal": {"id": "agent:two", "kind": "agent", "displayName": "Two", "agentId": "two"},
                "access": "member",
            }))
            .unwrap()["id"]
            .as_str()
            .unwrap()
            .to_owned();
        let turns = vec![
            json!({"turnHandle": "turn:assistant", "conversationId": conversation_id, "membershipId": assistant_id, "agent": "one"}),
            json!({"turnHandle": "turn:other", "conversationId": conversation_id, "membershipId": other_id, "agent": "two"}),
        ];
        let service = service.with_active_turns(move |_| json!({"turns": turns}));
        let revision = service.store().get(&conversation_id).unwrap().revision;
        service
            .execute(json!({
                "action": "conversation.assistant.set",
                "conversationId": conversation_id,
                "ownerMembershipId": owner_id,
                "expectedRevision": revision,
                "membershipId": assistant_id,
            }))
            .unwrap();

        let result = persist_then_dispatch_with_assistant(
            &service,
            json!({
                "action": "conversation.message.post",
                "conversationId": conversation_id,
                "authorMembershipId": owner_id,
                "content": "continue",
            }),
            true,
        );
        let steers = steers.lock().unwrap();
        assert_eq!(steers.len(), 1);
        assert_eq!(steers[0]["turnHandle"], "turn:other");
        let handles = result["turns"]
            .as_array()
            .unwrap()
            .iter()
            .map(|turn| turn["turnHandle"].as_str().unwrap())
            .collect::<Vec<_>>();
        assert!(handles.contains(&"turn:assistant"));
        assert!(handles.contains(&"turn:other"));
    }

    #[test]
    fn suppressed_assistant_only_active_turn_stays_attachable_without_steering() {
        let steers = Arc::new(Mutex::new(Vec::<Value>::new()));
        let captured_steers = Arc::clone(&steers);
        let service = ConversationService::from_store(
            crate::domain::client_conversation::ConversationStore::open_in_memory().unwrap(),
        )
        .with_native_turn_sender(|_| panic!("the active Assistant must not restart"))
        .with_steer_turn(move |params| {
            captured_steers.lock().unwrap().push(params.clone());
            Ok(json!({"ok": true, "status": "accepted"}))
        });
        let (conversation_id, owner_id, assistant_id) = group_fixture(&service);
        let turns = vec![json!({
            "turnHandle": "turn:assistant",
            "conversationId": conversation_id,
            "membershipId": assistant_id,
            "agent": "one",
        })];
        let service = service.with_active_turns(move |_| json!({"turns": turns}));
        let revision = service.store().get(&conversation_id).unwrap().revision;
        service
            .execute(json!({
                "action": "conversation.assistant.set",
                "conversationId": conversation_id,
                "ownerMembershipId": owner_id,
                "expectedRevision": revision,
                "membershipId": assistant_id,
            }))
            .unwrap();

        let result = persist_then_dispatch_with_assistant(
            &service,
            json!({
                "action": "conversation.message.post",
                "conversationId": conversation_id,
                "authorMembershipId": owner_id,
                "content": "plain group note",
            }),
            true,
        );
        assert!(steers.lock().unwrap().is_empty());
        assert_eq!(result["turns"].as_array().unwrap().len(), 1);
        assert_eq!(result["turns"][0]["turnHandle"], "turn:assistant");
    }

    #[test]
    fn plain_group_follow_up_steers_only_the_designated_assistant_turn() {
        let steers = Arc::new(Mutex::new(Vec::<Value>::new()));
        let captured_steers = Arc::clone(&steers);
        let service = ConversationService::from_store(
            crate::domain::client_conversation::ConversationStore::open_in_memory().unwrap(),
        )
        .with_native_turn_sender(|_| panic!("active turns must be steered, not restarted"))
        .with_steer_turn(move |params| {
            captured_steers.lock().unwrap().push(params.clone());
            Ok(json!({"ok": true, "status": "accepted"}))
        });
        let (conversation_id, owner_id, agent_one) = group_fixture(&service);
        let agent_two = service
            .execute(json!({
                "action": "conversation.membership.add",
                "conversationId": conversation_id,
                "principal": {"id": "agent:two", "kind": "agent", "displayName": "Two", "agentId": "two"},
                "access": "member",
            }))
            .unwrap()["id"]
            .as_str()
            .unwrap()
            .to_owned();
        let active_turns = vec![
            json!({"turnHandle": "turn:assistant", "conversationId": conversation_id, "membershipId": agent_one, "agent": "one"}),
            json!({"turnHandle": "turn:member", "conversationId": conversation_id, "membershipId": agent_two, "agent": "two"}),
        ];
        let service = service.with_active_turns(move |_| json!({"turns": active_turns}));
        let conversation_revision = service.store().get(&conversation_id).unwrap().revision;
        service
            .execute(json!({
                "action": "conversation.assistant.set",
                "conversationId": conversation_id,
                "ownerMembershipId": owner_id,
                "expectedRevision": conversation_revision,
                "membershipId": agent_one,
            }))
            .unwrap();

        let posted = persist_then_dispatch(
            &service,
            json!({
                "action": "conversation.message.post",
                "conversationId": conversation_id,
                "authorMembershipId": owner_id,
                "content": "continue the goal",
            }),
        );
        assert!(posted.get("strategyError").is_none());
        let recorded = steers.lock().unwrap();
        assert_eq!(recorded.len(), 1);
        assert_eq!(recorded[0]["turnHandle"], "turn:assistant");
        assert_eq!(recorded[0]["text"], "continue the goal");
    }

    #[test]
    fn plain_group_follow_up_without_an_assistant_never_fans_out_to_active_turns() {
        let steers = Arc::new(Mutex::new(Vec::<Value>::new()));
        let captured_steers = Arc::clone(&steers);
        let service = ConversationService::from_store(
            crate::domain::client_conversation::ConversationStore::open_in_memory().unwrap(),
        )
        .with_native_turn_sender(|_| panic!("ambiguous active turns must not be restarted"))
        .with_steer_turn(move |params| {
            captured_steers.lock().unwrap().push(params.clone());
            Ok(json!({"ok": true, "status": "accepted"}))
        });
        let (conversation_id, owner_id, agent_one) = group_fixture(&service);
        let agent_two = service
            .execute(json!({
                "action": "conversation.membership.add",
                "conversationId": conversation_id,
                "principal": {"id": "agent:two", "kind": "agent", "displayName": "Two", "agentId": "two"},
                "access": "member",
            }))
            .unwrap()["id"]
            .as_str()
            .unwrap()
            .to_owned();
        let active_turns = vec![
            json!({"turnHandle": "turn:one", "conversationId": conversation_id, "membershipId": agent_one, "agent": "one"}),
            json!({"turnHandle": "turn:two", "conversationId": conversation_id, "membershipId": agent_two, "agent": "two"}),
        ];
        let service = service.with_active_turns(move |_| json!({"turns": active_turns}));

        let posted = persist_then_dispatch(
            &service,
            json!({
                "action": "conversation.message.post",
                "conversationId": conversation_id,
                "authorMembershipId": owner_id,
                "content": "ambiguous follow up",
            }),
        );
        assert_eq!(
            posted["strategyError"]["code"],
            "conversation_address_ambiguous"
        );
        assert!(steers.lock().unwrap().is_empty());
    }

    #[test]
    fn subagent_edge_projects_inbound_without_identifiers() {
        let service = ConversationService::from_store(
            crate::domain::client_conversation::ConversationStore::open_in_memory().unwrap(),
        );
        let group = service
            .execute(json!({
                "action": "conversation.create",
                "title": "Mesh edge",
                "owner": {"id": "human:local", "kind": "human", "displayName": "You"},
                "members": [
                    {"principal": {"id": "agent:one", "kind": "agent", "displayName": "One", "agentId": "one"}, "access": "member"},
                    {"principal": {"id": "agent:two", "kind": "agent", "displayName": "Two", "agentId": "two"}, "access": "member"}
                ]
            }))
            .unwrap();
        let conversation_id = group["id"].as_str().unwrap().to_owned();
        let memberships = group["memberships"].as_array().unwrap();
        let caller = memberships
            .iter()
            .find(|membership| membership["principal"]["agentId"] == "one")
            .unwrap()["id"]
            .as_str()
            .unwrap()
            .to_owned();
        let target = memberships
            .iter()
            .find(|membership| membership["principal"]["agentId"] == "two")
            .unwrap()["id"]
            .as_str()
            .unwrap()
            .to_owned();
        service
            .store()
            .record_subagent_mcp_inbound(
                &conversation_id,
                Some(&caller),
                Some(&target),
                "lico_subagent_delegate",
                "accepted",
            )
            .unwrap();
        let edge = service
            .execute(json!({
                "action": "conversation.subagent.edge",
                "conversationId": conversation_id,
                "callerMembershipId": caller,
                "targetMembershipId": target,
            }))
            .unwrap();
        assert_eq!(edge["inbound"]["delegate"], true);
        assert_eq!(edge["inbound"]["continue"], false);
        assert_eq!(edge["inbound"]["cancel"], false);
        assert_eq!(edge["outcomes"]["delegate"], "accepted");
        assert_eq!(edge["outcomes"]["continue"], Value::Null);
        assert_eq!(edge["outcomes"]["cancel"], Value::Null);
        assert_eq!(edge["claimState"], Value::Null);
        assert_eq!(edge["dispatchState"], Value::Null);
        let wire = edge.to_string();
        assert!(!wire.contains(&conversation_id));
        assert!(!wire.contains(&caller));
        assert!(!wire.contains(&target));
    }

    #[test]
    fn native_role_membership_maps_profile_and_is_addressable() {
        let _roles = crate::domain::native_roles::install_test_roles(vec![
            crate::domain::native_roles::NativeRole {
                id: "native-role:opencode/reviewer".to_owned(),
                host_agent_id: "opencode".to_owned(),
                slug: "reviewer".to_owned(),
                name: "Reviewer".to_owned(),
                instructions: "Review only the requested files.".to_owned(),
                preferred_model: Some("anthropic/claude-sonnet-4".to_owned()),
                preferred_reasoning_effort: Some("high".to_owned()),
            },
        ]);
        let listed = ConversationService::from_store(ConversationStore::open_in_memory().unwrap())
            .execute(json!({"action": "conversation.profile.native_roles"}))
            .unwrap();
        assert_eq!(listed["roles"][0]["id"], "native-role:opencode/reviewer");
        assert!(listed["roles"][0].get("instructions").is_none());
        assert_eq!(listed["roles"][0]["hasInstructions"], true);

        let calls = Arc::new(Mutex::new(Vec::<Value>::new()));
        let captured = Arc::clone(&calls);
        let service = ConversationService::from_store_with_runtime(
            ConversationStore::open_in_memory().unwrap(),
            move |params| {
                captured.lock().unwrap().push(params.clone());
                Ok(accepted_receipt(params))
            },
        );
        let (conversation_id, owner_id, _) = group_fixture(&service);
        let added = service
            .execute(json!({
                "action": "conversation.membership.add",
                "conversationId": conversation_id,
                "ownerMembershipId": owner_id,
                "nativeRoleId": "native-role:opencode/reviewer",
                "access": "member",
            }))
            .unwrap();
        assert_eq!(added["principal"]["id"], "agent:opencode:reviewer");
        assert_eq!(added["principal"]["agentId"], "opencode");
        assert_eq!(added["principal"]["displayName"], "Reviewer");
        let membership_id = added["id"].as_str().unwrap().to_owned();
        let profile = service
            .execute(json!({
                "action": "conversation.profile.get",
                "membershipId": membership_id,
            }))
            .unwrap();
        assert_eq!(profile["preferredModel"], "anthropic/claude-sonnet-4");
        assert_eq!(profile["preferredReasoningEffort"], "high");
        assert!(
            profile["skillReferences"]
                .as_array()
                .unwrap()
                .iter()
                .any(|skill| skill == "native-role:opencode/reviewer")
        );
        let encoded = profile.to_string();
        assert!(!encoded.contains("Review only the requested files."));

        let posted = persist_then_dispatch(
            &service,
            json!({
                "action": "conversation.message.post",
                "conversationId": conversation_id,
                "authorMembershipId": owner_id,
                "content": "@Reviewer please check the patch",
            }),
        );
        assert_eq!(posted["directTurns"].as_array().unwrap().len(), 1);
        assert_eq!(posted["turns"][0]["membershipId"], membership_id);
        let calls = calls.lock().unwrap();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0]["agentId"], "opencode");
        assert_eq!(calls[0]["model"], "anthropic/claude-sonnet-4");
        assert_eq!(calls[0]["reasoningEffort"], "high");
        assert_eq!(calls[0]["runtimeAgent"], "reviewer");
        assert_eq!(
            calls[0]["privateInstructions"],
            "Review only the requested files."
        );
        assert_eq!(calls[0]["text"], "@Reviewer please check the patch");
    }

    #[test]
    fn create_accepts_non_group_conversations() {
        let service = ConversationService::from_store(ConversationStore::open_in_memory().unwrap());
        let direct = service
            .execute(json!({
                "action": "conversation.create",
                "isGroup": false,
                "title": "Persona",
                "owner": {"id": "human:local", "kind": "human", "displayName": "You"},
            }))
            .unwrap();
        assert_eq!(direct["isGroup"], false);
        assert_eq!(direct["memberships"].as_array().unwrap().len(), 1);

        let typed = service.execute(json!({
            "action": "conversation.create",
            "isGroup": "yes",
            "title": "Typed",
            "owner": {"id": "human:local", "kind": "human", "displayName": "You"},
        }));
        assert!(typed.is_err());

        // An omitted flag keeps the established group contract: an Agent
        // member is required and the record lands as a group.
        let group = service
            .execute(json!({
                "action": "conversation.create",
                "title": "Group",
                "owner": {"id": "human:local", "kind": "human", "displayName": "You"},
                "members": [
                    {"principal": {"id": "agent:one", "kind": "agent", "displayName": "One", "agentId": "one"}, "access": "member"}
                ]
            }))
            .unwrap();
        assert_eq!(group["isGroup"], true);
    }

    #[test]
    fn direct_conversation_dispatches_assistant_with_profile_preferences() {
        let calls = Arc::new(Mutex::new(Vec::<Value>::new()));
        let captured = Arc::clone(&calls);
        let service = ConversationService::from_store_with_runtime(
            ConversationStore::open_in_memory().unwrap(),
            move |params| {
                captured.lock().unwrap().push(params.clone());
                Ok(accepted_receipt(params))
            },
        );
        let conversation = service
            .execute(json!({
                "action": "conversation.create",
                "isGroup": false,
                "title": "Haiku Bot",
                "owner": {"id": "human:local", "kind": "human", "displayName": "You"},
                "members": [
                    {"principal": {"id": "profile:haiku", "kind": "agent", "displayName": "Haiku", "agentId": "one"}, "access": "member"}
                ]
            }))
            .unwrap();
        assert_eq!(conversation["isGroup"], false);
        let conversation_id = conversation["id"].as_str().unwrap().to_owned();
        let owner_id = conversation["memberships"]
            .as_array()
            .unwrap()
            .iter()
            .find(|membership| membership["principal"]["kind"] == "human")
            .unwrap()["id"]
            .as_str()
            .unwrap()
            .to_owned();
        let member_id = conversation["memberships"]
            .as_array()
            .unwrap()
            .iter()
            .find(|membership| membership["principal"]["id"] == "profile:haiku")
            .unwrap()["id"]
            .as_str()
            .unwrap()
            .to_owned();

        let conversation_revision = service.store().get(&conversation_id).unwrap().revision;
        service
            .execute(json!({
                "action": "conversation.assistant.set",
                "conversationId": conversation_id,
                "ownerMembershipId": owner_id,
                "expectedRevision": conversation_revision,
                "membershipId": member_id,
            }))
            .unwrap();
        let profile_revision = service
            .store()
            .membership_profile(&member_id)
            .unwrap()
            .unwrap()
            .revision;
        service
            .execute(json!({
                "action": "conversation.profile.update",
                "conversationId": conversation_id,
                "membershipId": member_id,
                "ownerMembershipId": owner_id,
                "expectedRevision": profile_revision,
                "intent": {
                    "preferredModel": "model-x",
                    "preferredReasoningEffort": "medium",
                    "preferredInstructions": "Always answer in haiku.",
                    "preferredContextWindow": 131072,
                },
            }))
            .unwrap();
        let stored = service
            .execute(json!({
                "action": "conversation.profile.get",
                "membershipId": member_id,
            }))
            .unwrap();
        assert_eq!(stored["preferredInstructions"], "Always answer in haiku.");
        assert_eq!(stored["preferredContextWindow"], 131072);

        let posted = persist_then_dispatch(
            &service,
            json!({
                "action": "conversation.message.post",
                "conversationId": conversation_id,
                "authorMembershipId": owner_id,
                "content": "tell me about rain",
            }),
        );
        assert_eq!(posted["turns"].as_array().unwrap().len(), 1);
        let calls = calls.lock().unwrap();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0]["model"], "model-x");
        assert_eq!(calls[0]["reasoningEffort"], "medium");
        assert_eq!(calls[0]["contextWindow"], 131072);
        let text = calls[0]["text"].as_str().unwrap();
        assert!(text.contains("Always answer in haiku."));
        assert!(text.contains("tell me about rain"));
    }

    #[test]
    fn profile_update_validates_instructions_and_context_window() {
        let service = ConversationService::from_store(ConversationStore::open_in_memory().unwrap());
        let (conversation_id, owner_id, agent_id) = group_fixture(&service);
        let revision = || {
            service
                .store()
                .membership_profile(&agent_id)
                .unwrap()
                .unwrap()
                .revision
        };
        for (intent, expected) in [
            (
                json!({"preferredInstructions": ""}),
                "profile_preferred_instructions_invalid",
            ),
            (
                json!({"preferredInstructions": "x".repeat(8193)}),
                "profile_preferred_instructions_invalid",
            ),
            (
                json!({"preferredContextWindow": 0}),
                "profile_preferred_context_window_invalid",
            ),
            (
                json!({"preferredContextWindow": 8_000_001u64}),
                "profile_preferred_context_window_invalid",
            ),
        ] {
            let failure = service
                .execute(json!({
                    "action": "conversation.profile.update",
                    "conversationId": conversation_id,
                    "membershipId": agent_id,
                    "ownerMembershipId": owner_id,
                    "expectedRevision": revision(),
                    "intent": intent,
                }))
                .expect_err("invalid profile field must reject");
            assert_eq!(failure.to_string(), expected);
        }
        let updated = service
            .execute(json!({
                "action": "conversation.profile.update",
                "conversationId": conversation_id,
                "membershipId": agent_id,
                "ownerMembershipId": owner_id,
                "expectedRevision": revision(),
                "intent": {
                    "preferredInstructions": "Be terse.",
                    "preferredContextWindow": 8_000_000u64,
                },
            }))
            .unwrap();
        assert_eq!(updated["profile"]["preferredInstructions"], "Be terse.");
        assert_eq!(updated["profile"]["preferredContextWindow"], 8_000_000u64);
    }
}
