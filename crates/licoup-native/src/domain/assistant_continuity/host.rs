//! Production host for M1 continuity leaves on the existing Conversation store.

use std::cell::Cell;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use anyhow::{Result, anyhow};
use licoup_agent_runtime::work_context::{
    ChildBinding, HermeticProtocol, NativeControlRequest, NativeWorkContextKey, ProtocolFamily,
    WorkContextConfig, WorkContextRuntime,
};
use licoup_conversation::continuity::lifecycle::{is_terminal, suppresses_new_work};
use licoup_conversation::continuity::{
    CHILD_WORK_PENDING_DESIGNATION, ChildWorkIdentity, ContextCompositionPort, ContinuityAgreement,
    ContinuityCandidateIdentity, ContinuityCommitPort, ContinuityCommitReceipt,
    ContinuityContextCompositionRequest, ContinuityContextManifest, ContinuityEffectStatus,
    ContinuityEvidenceRef, ContinuityEvidenceResult, ContinuityFailure, ContinuityFailureCode,
    ContinuityGoalCompletionTransition, ContinuityGoalControl, ContinuityGoalEvent,
    ContinuityGoalLifecycle, ContinuityGoalProgress, ContinuityInterpretationProposal,
    ContinuityNextAttention, ContinuityQualificationResult, ContinuityReadPort,
    ContinuitySourceOwnerKind, ContinuitySourceRef, ContinuitySourceValidity,
    ContinuityTaskConversationRelation, ContinuityVerificationKind, ContinuityVisibilityScope,
    ContinuityWake, INGRESS_USER_POSTED_DESIGNATION, PENDING_OBLIGATION_PAGE_SIZE,
    StoredEvaluationCase, StoredEvaluationCorpus, ack_completion_notices, admit_evaluation_corpus,
    admit_evaluation_session, append_criterion_evidence, apply_goal_control, bump_host_generation,
    bump_revocation, child_work_identity_from_payload, child_work_identity_payload,
    child_work_named_key, child_work_operation_id, child_work_started, claim_collection_operation,
    clear_child_work_live, commit_collected_qualification, commit_user_posted_proposal,
    consume_logical_wake, continuity_now_ms, current_host_generation, enqueue_review_wake,
    hand_off_conversation_goals, ingress_execution_recorded, list_all_pending_wakes,
    list_child_work_live, list_due_goals, list_pending_completion_notices,
    list_qualification_evidence, list_unacked_child_work_page, list_unapplied_settlements,
    list_unknown_effect_ids, load_adoption_policy_values, load_effect_status,
    load_evaluation_corpus, load_evaluation_session, load_qualification_evidence_for,
    persist_adoption_enabled, persist_adoption_stage, put_agreement, put_qualification_evidence,
    read_agreements, read_child_links, read_child_work_accepted, read_child_work_intent,
    read_child_work_live, read_goal, read_goal_bundle, read_oldest_pending_child_work,
    read_pending_wakes, read_qualification_invalidations, read_relation_for_child,
    read_settlement_applied, read_settlement_pending, record_child_work_accepted,
    record_child_work_intent, record_child_work_live, record_child_work_started,
    record_ingress_execution, record_qualification_invalidation, record_settlement_applied,
    record_settlement_pending, release_collection_operation, replay_effect,
    resolve_completion_notice, resolve_stored_owner_authority, schedule_goal_due,
    settlement_applied, update_wake_host_generation, wake_repeats_recorded_sources,
};
use licoup_conversation::{
    Conversation, ConversationStore, DispatchState, EventPartKind, MembershipStatus, PrincipalKind,
    ProfileIntent,
};
use serde_json::{Value, json};

use crate::domain::agent_intelligence_catalog::qualification::{
    EvidenceBundle, EvidenceClass, LiveAdmission, LiveProvenance, QualificationObservation,
    QualificationService, RequestKind, query_record,
};
use crate::platform::runtime_adapters::{
    RuntimeAdapter, RuntimeAdapterError, adapter_for_agent_public,
};
use licoup_agent_drivers::AdapterTransport;

use super::adoption::{AdoptionPolicy, stage_from_coverage};
use super::cognition::{
    AdmittedAssistantInvoker, AdmittedTurnRequest, AssemblySnapshot, CognitionIntent,
    CognitionInvoker, CognitionRequest, CompleteAdmittedTurn, PersistentTurnCognition,
    ScriptedAgent, ScriptedCognitionInvoker, SemanticScript, WakeReviewContext,
    collect_admitted_granted_facts, compose_admitted_turn_params, compose_continuity_guidance,
    proposal_creates_new_advancement, proposal_from_assistant_turn_response,
    proposal_from_turn_output, proposal_has_business_effect,
};
#[cfg(any(test, feature = "test-support"))]
use super::collection::HermeticEvaluationObserver;
use super::collection::{
    AdmittedRuntimeEvaluationObserver, CollectedEvaluation, ExternalEvaluationObserver,
    produce_collected_evaluation, validate_collection_receipt,
};
use super::context::{
    ContinuityWorkspace, FrozenContextStore, UnavailableContextCompositionService,
};
use super::execution::{
    AdmittedGrantedFact, AdmittedRecipientFacts, CONTINUITY_KIND_CHILD_WORK,
    CONTINUITY_KIND_WAKE_REVIEW, child_settlement_identity, child_settlement_is_unknown,
    compose_child_work_brief, compose_wake_review_brief, event_source_ref, is_wake_review_payload,
    review_cause_refs, settlement_identity, strip_review_unsafe_effects,
};
use super::live::{insert_task_subject, populate_live_store};
use super::observation::{
    ObservationFact, ObservationIndex, ObservationRecordOutcome, load_observation_index,
    record_observation,
};

#[derive(Clone, Debug, Default)]
pub struct IngressOutcome {
    pub committed: bool,
    pub abstained: bool,
    pub child_conversation_id: Option<String>,
    pub receipt_revision: Option<i64>,
    pub invocation_count: u64,
    pub unavailable: bool,
    pub qualification_denied: bool,
}

#[derive(Clone, Debug, Default)]
pub struct WakeDrain {
    pub consumed: Vec<String>,
    pub reconciled: Vec<String>,
    pub replayed: usize,
    pub waiting: Vec<String>,
    pub reevaluated: Vec<String>,
    pub preserved: Vec<String>,
    pub no_ops: Vec<(String, String)>,
}

#[derive(Clone, Debug)]
pub struct CompletionNotice {
    pub notification_id: String,
    pub goal_id: String,
    pub conversation_id: String,
}

struct PreparedIngress {
    request: ContinuityContextCompositionRequest,
    manifest: ContinuityContextManifest,
    assembly: AssemblySnapshot,
    workspace: Arc<ContinuityWorkspace>,
    composer: UnavailableContextCompositionService,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
struct WorkRuntimeKey {
    child_conversation_id: String,
    goal_id: String,
    membership_id: String,
    generation: i64,
}

type WorkTurnStart =
    dyn Fn(&Value) -> std::result::Result<Value, RuntimeAdapterError> + Send + Sync;
type WorkTurnControl =
    dyn Fn(&Value) -> std::result::Result<Value, RuntimeAdapterError> + Send + Sync;
type WorkTurnInspect = dyn Fn(&str) -> Option<(String, String)> + Send + Sync;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ChildControlDisposition {
    Ordinary,
    Accepted,
    Unavailable,
    Conflict,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ChildControlKind<'a> {
    Steer(&'a str),
    Cancel,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ChildWorkFault {
    FailStart,
    FailAfterAccepted,
    FailAfterEvidence,
}

thread_local! {
    static CHILD_WORK_FAULT: Cell<Option<ChildWorkFault>> = const { Cell::new(None) };
    static CHILD_ASSEMBLY_RECHECK_FAILURES: Cell<u32> = const { Cell::new(0) };
}

pub fn set_child_work_fault(fault: Option<ChildWorkFault>) {
    CHILD_WORK_FAULT.with(|cell| cell.set(fault));
}

fn take_child_work_fault(expected: ChildWorkFault) -> bool {
    CHILD_WORK_FAULT.with(|cell| {
        if cell.get() == Some(expected) {
            cell.set(None);
            true
        } else {
            false
        }
    })
}

pub fn set_child_assembly_recheck_failures(count: u32) {
    CHILD_ASSEMBLY_RECHECK_FAILURES.with(|cell| cell.set(count));
}

fn take_child_assembly_recheck_failure() -> bool {
    CHILD_ASSEMBLY_RECHECK_FAILURES.with(|cell| {
        let remaining = cell.get();
        if remaining > 0 {
            cell.set(remaining - 1);
            true
        } else {
            false
        }
    })
}

enum ChildSettlementOutcome {
    Retained(licoup_conversation::continuity::ContinuitySourceRef),
    PendingUnknown,
    Rejected,
}

pub struct ContinuityHost {
    store: ConversationStore,
    qualification: Mutex<QualificationService>,
    production: Arc<AdmittedAssistantInvoker>,
    cognition: Mutex<Arc<dyn CognitionInvoker>>,
    scripted: Arc<ScriptedCognitionInvoker>,
    pending_scripts: Mutex<Vec<SemanticScript>>,
    work_runtimes: Mutex<BTreeMap<WorkRuntimeKey, Arc<WorkContextRuntime>>>,
    work_start: Mutex<Option<Arc<WorkTurnStart>>>,
    work_steer: Mutex<Option<Arc<WorkTurnControl>>>,
    work_cancel: Mutex<Option<Arc<WorkTurnControl>>>,
    work_inspect: Mutex<Option<Arc<WorkTurnInspect>>>,
    handed_notices: Mutex<BTreeSet<String>>,
    unknown_effects: Mutex<Vec<(String, String, Option<String>)>>,
    host_generation: Mutex<i64>,
    adoption: Mutex<AdoptionPolicy>,
    evaluation_observer: Mutex<Option<Arc<dyn ExternalEvaluationObserver>>>,
    complete_turn: Mutex<Option<Arc<CompleteAdmittedTurn>>>,
    owner_claimed: AtomicBool,
    runtime_bound: AtomicBool,
    using_script: AtomicBool,
}

impl ContinuityHost {
    pub fn attach(store: ConversationStore) -> Result<Arc<Self>> {
        Self::attach_observer(store)
    }

    pub fn attach_observer(store: ConversationStore) -> Result<Arc<Self>> {
        store
            .ensure_continuity_migrated()
            .map_err(|err| anyhow!(err.to_string()))?;
        let generation = current_host_generation(&store).map_err(attach_err)?;
        let unknown = list_unknown_effect_ids(&store).map_err(attach_err)?;
        let adoption = load_adoption_policy_values(&store)
            .map(|(enabled, stage)| AdoptionPolicy::from_stored(enabled, &stage))
            .unwrap_or_default();
        let scripted = Arc::new(ScriptedCognitionInvoker::new());
        let production = Arc::new(AdmittedAssistantInvoker::new());
        let cognition: Arc<dyn CognitionInvoker> = production.clone();
        let host = Arc::new(Self {
            store,
            qualification: Mutex::new(QualificationService::draft_port()),
            production,
            cognition: Mutex::new(cognition),
            scripted,
            pending_scripts: Mutex::new(Vec::new()),
            work_runtimes: Mutex::new(BTreeMap::new()),
            work_start: Mutex::new(None),
            work_steer: Mutex::new(None),
            work_cancel: Mutex::new(None),
            work_inspect: Mutex::new(None),
            handed_notices: Mutex::new(BTreeSet::new()),
            unknown_effects: Mutex::new(unknown),
            host_generation: Mutex::new(generation),
            adoption: Mutex::new(adoption),
            evaluation_observer: Mutex::new(None),
            complete_turn: Mutex::new(None),
            owner_claimed: AtomicBool::new(false),
            runtime_bound: AtomicBool::new(false),
            using_script: AtomicBool::new(false),
        });
        host.reload_qualification().map_err(attach_err)?;
        host.sync_adoption_stage().map_err(attach_err)?;
        Ok(host)
    }

    pub fn claim_continuity_owner(&self) -> Result<(), ContinuityFailure> {
        let generation = bump_host_generation(&self.store)?;
        *lock(&self.host_generation) = generation;
        self.owner_claimed.store(true, Ordering::SeqCst);
        if self.effects_runtime_ready() {
            self.cold_recover()
        } else {
            self.coalesce_wake_generations()
        }
    }

    pub fn store(&self) -> &ConversationStore {
        &self.store
    }

    pub fn host_generation(&self) -> i64 {
        *lock(&self.host_generation)
    }

    pub fn unknown_effect_ids(&self) -> Vec<String> {
        lock(&self.unknown_effects)
            .iter()
            .map(|(id, _, _)| id.clone())
            .collect()
    }

    pub fn cognition_invocation_count(&self) -> u64 {
        lock(&self.cognition).invocation_count()
    }

    /// Record one already-admitted run fact without changing execution state.
    /// Observation is deliberately separate from admission and permission.
    pub fn record_observation(
        &self,
        fact: &ObservationFact,
    ) -> Result<ObservationRecordOutcome, ContinuityFailure> {
        record_observation(&self.store, fact)
    }

    pub fn observation_index(
        &self,
        conversation_id: &str,
    ) -> Result<ObservationIndex, ContinuityFailure> {
        load_observation_index(&self.store, conversation_id)
    }

    pub fn install_script(&self, script: SemanticScript) {
        if script.event_opaque_id.trim().is_empty() {
            lock(&self.pending_scripts).push(script);
        } else {
            self.scripted.insert(script);
        }
        *lock(&self.cognition) = self.scripted.clone();
        self.using_script.store(true, Ordering::SeqCst);
    }

    pub fn uses_scripted_cognition(&self) -> bool {
        // The post path still asks under this name. Inline admission runs when
        // it cannot block the caller, which is the unbound host. A test script
        // is not the switch.
        !self.admitted_turn_blocks_caller()
    }

    /// Whether admitting a user post inline would block on the persistent turn.
    ///
    /// This observes the bound transport, not a test script. A bound transport
    /// runs the admitted turn to completion in the caller's thread, so the post
    /// stays persistence-only and the turn's worker admits. An unbound host
    /// admits inline and cannot block.
    pub fn admitted_turn_blocks_caller(&self) -> bool {
        self.production.is_bound()
    }

    pub fn compose_ingress_guidance(
        &self,
        conversation_id: &str,
        membership_id: &str,
        event_id: &str,
    ) -> Result<String, ContinuityFailure> {
        let Some(prepared) =
            self.prepare_ingress_assembly(conversation_id, event_id, membership_id, None)
        else {
            return Ok(String::new());
        };
        Ok(compose_continuity_guidance(
            &self.store,
            &prepared.assembly,
            &prepared.manifest,
            false,
            None,
        ))
    }

    pub fn bind_persistent_cognition(&self, complete_turn: Arc<CompleteAdmittedTurn>) {
        // The admitted turn invokes the host driver, which reads its dispatch
        // deadline from the host composition. Dispatch installs that composition
        // for itself; binding the turn is the other production entry that
        // reaches the driver, so it installs the same answers. The first
        // installation wins, and an unbound host stays fail-closed.
        crate::agent_port::install();
        *lock(&self.complete_turn) = Some(Arc::clone(&complete_turn));
        self.production.bind(Arc::new(PersistentTurnCognition::new(
            self.store.clone(),
            complete_turn,
        )));
        self.runtime_bound.store(true, Ordering::SeqCst);
        if self.owner_claimed.load(Ordering::SeqCst) {
            let _ = self.cold_recover();
        }
    }

    pub fn bind_work_turn_start(&self, start: Arc<WorkTurnStart>) {
        *lock(&self.work_start) = Some(start);
    }

    pub fn bind_work_turn_control(
        &self,
        steer: Arc<WorkTurnControl>,
        cancel: Arc<WorkTurnControl>,
    ) {
        *lock(&self.work_steer) = Some(steer);
        *lock(&self.work_cancel) = Some(cancel);
    }

    pub fn bind_work_turn_inspect(&self, inspect: Arc<WorkTurnInspect>) {
        *lock(&self.work_inspect) = Some(inspect);
    }

    pub fn ingress_already_executed(
        &self,
        conversation_id: &str,
        event_id: &str,
        membership_id: &str,
    ) -> bool {
        ingress_execution_recorded(
            &self.store,
            conversation_id,
            event_id,
            membership_id,
            INGRESS_USER_POSTED_DESIGNATION,
        )
        .unwrap_or(false)
    }

    fn record_applied_user_posted(
        &self,
        conversation_id: &str,
        event_id: &str,
        membership_id: &str,
    ) -> Result<(), ContinuityFailure> {
        record_ingress_execution(
            &self.store,
            conversation_id,
            event_id,
            membership_id,
            INGRESS_USER_POSTED_DESIGNATION,
        )
    }

    pub fn bind_hermetic(&self, protocol: HermeticProtocol, config: WorkContextConfig) {
        let key = work_runtime_key_from_binding(config.child.clone(), 0);
        let runtime = Arc::new(crate::agent_port::bind_hermetic_work_context(
            protocol, config,
        ));
        lock(&self.work_runtimes).insert(key, runtime);
    }

    pub fn bind_adapter_for_goal(
        &self,
        parent_conversation_id: &str,
        goal_id: &str,
        family: ProtocolFamily,
        transport: Arc<dyn AdapterTransport>,
    ) -> Result<(), ContinuityFailure> {
        let generation = self.admitted_work_generation(parent_conversation_id, goal_id)?;
        self.bind_adapter_for_goal_at(
            parent_conversation_id,
            goal_id,
            family,
            transport,
            generation,
        )
    }

    fn bind_adapter_for_goal_at(
        &self,
        parent_conversation_id: &str,
        goal_id: &str,
        family: ProtocolFamily,
        transport: Arc<dyn AdapterTransport>,
        generation: i64,
    ) -> Result<(), ContinuityFailure> {
        let binding = self.child_binding(parent_conversation_id, goal_id)?;
        let key = work_runtime_key_from_binding(binding.clone(), generation);
        let config = WorkContextConfig::child(binding);
        let runtime = Arc::new(crate::agent_port::bind_work_context(
            family,
            config,
            transport,
            Some(self.store.clone()),
        ));
        lock(&self.work_runtimes).insert(key, runtime);
        Ok(())
    }

    pub fn ensure_child_runtime(
        &self,
        parent_conversation_id: &str,
        goal_id: &str,
        family: ProtocolFamily,
    ) -> Result<(), ContinuityFailure> {
        let generation = self.admitted_work_generation(parent_conversation_id, goal_id)?;
        self.ensure_child_runtime_at(parent_conversation_id, goal_id, family, generation)
    }

    fn ensure_child_runtime_at(
        &self,
        parent_conversation_id: &str,
        goal_id: &str,
        family: ProtocolFamily,
        generation: i64,
    ) -> Result<(), ContinuityFailure> {
        let binding = self.child_binding(parent_conversation_id, goal_id)?;
        let key = work_runtime_key_from_binding(binding, generation);
        if lock(&self.work_runtimes).contains_key(&key) {
            return Ok(());
        }
        self.bind_adapter_for_goal_at(
            parent_conversation_id,
            goal_id,
            family,
            crate::agent_port::work_context_transport(family),
            generation,
        )
    }

    pub fn work_runtime(
        &self,
        child_conversation_id: &str,
        goal_id: &str,
        membership_id: &str,
        generation: i64,
    ) -> Option<Arc<WorkContextRuntime>> {
        lock(&self.work_runtimes)
            .get(&WorkRuntimeKey {
                child_conversation_id: child_conversation_id.to_owned(),
                goal_id: goal_id.to_owned(),
                membership_id: membership_id.to_owned(),
                generation,
            })
            .cloned()
    }

    pub fn work_runtime_binding(
        &self,
        child_conversation_id: &str,
        goal_id: &str,
        membership_id: &str,
    ) -> Option<ChildBinding> {
        let matches: Vec<ChildBinding> = lock(&self.work_runtimes)
            .iter()
            .filter(|(key, _)| {
                key.child_conversation_id == child_conversation_id
                    && key.goal_id == goal_id
                    && key.membership_id == membership_id
            })
            .map(|(_, runtime)| runtime.child_binding().clone())
            .collect();
        match matches.as_slice() {
            [binding] => Some(binding.clone()),
            _ => None,
        }
    }

    fn admitted_work_generation(
        &self,
        parent_conversation_id: &str,
        goal_id: &str,
    ) -> Result<i64, ContinuityFailure> {
        if let Some(identity) = self.live_child_work_identity(parent_conversation_id, goal_id)? {
            return Ok(identity.work_generation);
        }
        if let Some((_, pending)) =
            read_oldest_pending_child_work(&self.store, parent_conversation_id, goal_id)?
        {
            if let Some(identity) =
                child_work_identity_from_payload(&pending, parent_conversation_id, goal_id)
            {
                return Ok(identity.work_generation);
            }
            let revision = pending
                .get("admittedRevision")
                .or_else(|| pending.get("revision"))
                .and_then(Value::as_i64)
                .unwrap_or(0);
            return Ok(revision.max(1));
        }
        Ok(read_goal(&self.store, goal_id)?
            .map(|progress| progress.revision.max(1))
            .unwrap_or(1))
    }

    fn live_child_work_identity(
        &self,
        parent_conversation_id: &str,
        goal_id: &str,
    ) -> Result<Option<ChildWorkIdentity>, ContinuityFailure> {
        let Some(payload) = read_child_work_live(&self.store, parent_conversation_id, goal_id)?
        else {
            return Ok(None);
        };
        Ok(
            child_work_identity_from_payload(&payload, parent_conversation_id, goal_id).filter(
                |identity| {
                    identity.parent_conversation_id == parent_conversation_id
                        && identity.goal_id == goal_id
                },
            ),
        )
    }

    pub fn after_user_event(
        &self,
        conversation_id: &str,
        event_id: &str,
    ) -> Result<IngressOutcome, ContinuityFailure> {
        self.interpret_and_commit(conversation_id, event_id, CognitionIntent::UserPosted)
    }

    pub fn commit_fresh(
        &self,
        mut proposal: ContinuityInterpretationProposal,
    ) -> Result<ContinuityCommitReceipt, ContinuityFailure> {
        let basis = self
            .store
            .commit_basis(&proposal.envelope.conversation_id)?;
        proposal.envelope.observed_revision = basis.revision;
        proposal.envelope.designation_epoch = basis.designation_epoch;
        self.store.commit(&proposal)
    }

    fn commit_fresh_user_posted(
        &self,
        mut proposal: ContinuityInterpretationProposal,
        event_id: &str,
        membership_id: &str,
    ) -> Result<ContinuityCommitReceipt, ContinuityFailure> {
        let basis = self
            .store
            .commit_basis(&proposal.envelope.conversation_id)?;
        proposal.envelope.observed_revision = basis.revision;
        proposal.envelope.designation_epoch = basis.designation_epoch;
        commit_user_posted_proposal(&self.store, &proposal, event_id, membership_id)
    }

    pub fn pause_goal(
        &self,
        conversation_id: &str,
        goal_id: &str,
    ) -> Result<ContinuityGoalProgress, ContinuityFailure> {
        apply_goal_control(
            &self.store,
            conversation_id,
            goal_id,
            ContinuityGoalEvent::Pause,
        )
    }

    pub fn request_cancel(
        &self,
        conversation_id: &str,
        goal_id: &str,
    ) -> Result<ContinuityGoalProgress, ContinuityFailure> {
        let progress = apply_goal_control(
            &self.store,
            conversation_id,
            goal_id,
            ContinuityGoalEvent::CancelRequest,
        )?;
        if let Ok(relation) = self.store.relation_for_goal(goal_id) {
            if let Ok(binding) = self.child_binding(conversation_id, goal_id) {
                let handle = self
                    .live_child_work_identity(conversation_id, goal_id)
                    .ok()
                    .flatten()
                    .and_then(|identity| identity.dispatch_id);
                if let Some(handle) = handle {
                    let _ = self.cancel_admitted_child_turn(
                        &relation.child_conversation_id,
                        &binding.membership_id,
                        &handle,
                    );
                }
            }
        }
        Ok(progress)
    }

    pub fn resume_goal(
        &self,
        conversation_id: &str,
        goal_id: &str,
    ) -> Result<ContinuityGoalProgress, ContinuityFailure> {
        apply_goal_control(
            &self.store,
            conversation_id,
            goal_id,
            ContinuityGoalEvent::Resume,
        )
    }

    pub fn accept_goal_completion(
        &self,
        conversation_id: &str,
        transition: &ContinuityGoalCompletionTransition,
        progress: &ContinuityGoalProgress,
    ) -> Result<Option<CompletionNotice>, ContinuityFailure> {
        let first = licoup_conversation::continuity::accept_completion(
            &self.store,
            conversation_id,
            transition,
            progress,
        )?;
        if !first {
            return Ok(None);
        }
        let mut handed = lock(&self.handed_notices);
        if !handed.insert(transition.notification_id.clone()) {
            return Ok(None);
        }
        Ok(Some(CompletionNotice {
            notification_id: transition.notification_id.clone(),
            goal_id: transition.goal_id.clone(),
            conversation_id: conversation_id.to_owned(),
        }))
    }

    pub fn revise_agreement(
        &self,
        conversation_id: &str,
        mut agreement: ContinuityAgreement,
    ) -> Result<ContinuityAgreement, ContinuityFailure> {
        if agreement.id.trim().is_empty() {
            agreement.id = format!(
                "agreement:{conversation_id}:{}",
                agreement.effective_revision
            );
        }
        if agreement.supersedes.is_none() {
            if let Some(current) = read_agreements(&self.store, conversation_id)?
                .into_iter()
                .find(|item| item.scope == agreement.scope)
            {
                agreement.supersedes = Some(current.effective_revision);
                if agreement.effective_revision <= current.effective_revision {
                    agreement.effective_revision = current.effective_revision + 1;
                }
            }
        }
        let generation = bump_revocation(&self.store, conversation_id)?;
        agreement.revocation_generation = generation;
        put_agreement(&self.store, conversation_id, &agreement)?;
        Ok(agreement)
    }

    pub fn accept_evidence(
        &self,
        conversation_id: &str,
        goal_id: &str,
        evidence: ContinuityEvidenceRef,
    ) -> Result<ContinuityGoalProgress, ContinuityFailure> {
        append_criterion_evidence(&self.store, conversation_id, goal_id, evidence)
    }

    pub fn replace_assistant(
        &self,
        conversation_id: &str,
        membership_id: &str,
    ) -> Result<i64, ContinuityFailure> {
        let conversation = self
            .store
            .get(conversation_id)
            .map_err(|_| source_unavailable())?;
        let owner = local_owner_membership(&conversation).ok_or_else(source_unavailable)?;
        self.store
            .set_conversation_assistant(
                conversation_id,
                &owner,
                conversation.revision,
                Some(membership_id),
            )
            .map_err(|_| source_unavailable())?;
        // The work follows the new owner: unfinished Goals keep their contract,
        // criteria, evidence and in-flight executions, advance one revision so
        // the previous owner's proposals are stale, and are announced once.
        hand_off_conversation_goals(&self.store, conversation_id, membership_id)?;
        let updated = self
            .store
            .get(conversation_id)
            .map_err(|_| source_unavailable())?;
        Ok(updated.revision)
    }

    pub fn drain_wakes(&self, conversation_id: &str) -> Result<WakeDrain, ContinuityFailure> {
        let reconciled = self.reconcile_unknown_effects()?;
        let mut wakes = read_pending_wakes(&self.store, conversation_id)?;
        wakes.sort_by(|left, right| right.settlement.is_some().cmp(&left.settlement.is_some()));
        let unknown = lock(&self.unknown_effects).clone();
        let generation = self.host_generation();
        let now = continuity_now_ms();
        let mut drain = WakeDrain {
            reconciled,
            ..WakeDrain::default()
        };
        let mut seen_goals = BTreeSet::new();
        for mut wake in wakes {
            if let Some(due) = wake.due_at {
                if due > now {
                    drain.preserved.push(wake.logical_wake_id);
                    continue;
                }
            }
            if unknown.iter().any(|(id, _, goal)| {
                goal.as_deref() == Some(wake.goal_id.as_str()) || id == &wake.logical_wake_id
            }) {
                drain.waiting.push(wake.logical_wake_id.clone());
                drain.preserved.push(wake.logical_wake_id);
                continue;
            }
            if wake.host_generation != 0 && wake.host_generation != generation {
                update_wake_host_generation(&self.store, &wake.logical_wake_id, generation)?;
                wake.host_generation = generation;
            }
            if !seen_goals.insert((wake.goal_id.clone(), wake.goal_revision)) {
                drain.preserved.push(wake.logical_wake_id);
                continue;
            }
            let Some((contract, progress)) = read_goal_bundle(&self.store, &wake.goal_id)? else {
                drain.preserved.push(wake.logical_wake_id);
                continue;
            };
            if progress.lifecycle == ContinuityGoalLifecycle::Achieved
                || progress.lifecycle == ContinuityGoalLifecycle::Cancelled
            {
                if consume_logical_wake(&self.store, &wake.logical_wake_id)? {
                    drain.consumed.push(wake.logical_wake_id.clone());
                    drain
                        .no_ops
                        .push((wake.logical_wake_id, "goal-terminal".into()));
                }
                continue;
            }
            if progress.control == ContinuityGoalControl::Paused
                || progress.control == ContinuityGoalControl::CancelRequested
            {
                drain.preserved.push(wake.logical_wake_id);
                continue;
            }
            // A Goal that is already waiting re-reads the same sources on every
            // reminder. Settling that reminder deterministically keeps duplicate
            // notifications from buying a model call, and leaves the Goal, its
            // revision and its pause/resume/cancel controls intact.
            if progress.lifecycle == ContinuityGoalLifecycle::Waiting
                && wake_repeats_recorded_sources(&wake, &contract, &progress)
            {
                if consume_logical_wake(&self.store, &wake.logical_wake_id)? {
                    drain.consumed.push(wake.logical_wake_id.clone());
                    drain
                        .no_ops
                        .push((wake.logical_wake_id, "dependency-wait".into()));
                }
                continue;
            }
            if !self.effects_runtime_ready() {
                drain.preserved.push(wake.logical_wake_id);
                continue;
            }
            match self.interpret_wake_review(conversation_id, &wake) {
                Ok(outcome) if outcome.committed => {
                    if consume_logical_wake(&self.store, &wake.logical_wake_id)? {
                        drain.consumed.push(wake.logical_wake_id.clone());
                    }
                    drain.reevaluated.push(wake.logical_wake_id);
                }
                Ok(outcome)
                    if outcome.unavailable
                        || outcome.qualification_denied
                        || !outcome.abstained && !outcome.committed =>
                {
                    if outcome.qualification_denied || outcome.unavailable {
                        drain.reevaluated.push(wake.logical_wake_id.clone());
                    }
                    drain.preserved.push(wake.logical_wake_id);
                }
                Ok(outcome) if outcome.abstained && has_valid_next_responsibility(&progress) => {
                    if consume_logical_wake(&self.store, &wake.logical_wake_id)? {
                        drain.consumed.push(wake.logical_wake_id.clone());
                    }
                    drain
                        .no_ops
                        .push((wake.logical_wake_id, "semantic-noop".into()));
                }
                Ok(_) => {
                    drain.preserved.push(wake.logical_wake_id);
                }
                Err(err) => {
                    drain
                        .no_ops
                        .push((wake.logical_wake_id.clone(), format!("{:?}", err.code)));
                    drain.preserved.push(wake.logical_wake_id);
                }
            }
        }
        drain.replayed = 0;
        Ok(drain)
    }

    pub fn attend_due(&self) -> Result<WakeDrain, ContinuityFailure> {
        if !self.owner_claimed.load(Ordering::SeqCst) {
            return Ok(WakeDrain::default());
        }
        let recovered = self.recover_pending_settlements()?;
        let _ = self.recover_pending_child_work()?;
        self.enqueue_missed_due_once()?;
        let mut combined = WakeDrain::default();
        combined.replayed += recovered;
        let now = continuity_now_ms();
        let mut conversations = BTreeSet::new();
        for (_, conversation_id, _) in list_due_goals(&self.store, now)? {
            conversations.insert(conversation_id);
        }
        for (conversation_id, wake) in list_all_pending_wakes(&self.store)? {
            if wake.due_at.is_some_and(|due| due <= now) {
                conversations.insert(conversation_id);
                continue;
            }
            // An undated wake is the admission reminder. Attendance filters a
            // waiting Goal's reminder without a model call. Other undated wakes
            // stay with cold recovery, so one review does not chain into the
            // next revision's wake in the same attendance.
            if wake.due_at.is_none()
                && read_goal_bundle(&self.store, &wake.goal_id)
                    .ok()
                    .flatten()
                    .is_some_and(|(_, progress)| {
                        progress.lifecycle == ContinuityGoalLifecycle::Waiting
                    })
            {
                conversations.insert(conversation_id);
            }
        }
        for conversation_id in conversations {
            let drain = self.drain_wakes(&conversation_id)?;
            combined.consumed.extend(drain.consumed);
            combined.reconciled.extend(drain.reconciled);
            combined.waiting.extend(drain.waiting);
            combined.reevaluated.extend(drain.reevaluated);
            combined.preserved.extend(drain.preserved);
            combined.no_ops.extend(drain.no_ops);
            combined.replayed += drain.replayed;
        }
        Ok(combined)
    }

    pub fn after_runtime_settlement(
        &self,
        conversation_id: &str,
        payload: &Value,
    ) -> Result<WakeDrain, ContinuityFailure> {
        self.apply_runtime_settlement(conversation_id, payload, true)
    }

    pub fn schedule_review(
        &self,
        conversation_id: &str,
        goal_id: &str,
        due_at: i64,
    ) -> Result<bool, ContinuityFailure> {
        let (contract, progress) =
            read_goal_bundle(&self.store, goal_id)?.ok_or_else(source_unavailable)?;
        let responsible = self
            .store
            .relation_for_goal(goal_id)
            .ok()
            .and_then(|relation| {
                self.child_binding(conversation_id, goal_id)
                    .ok()
                    .map(|binding| binding.membership_id)
                    .or(Some(relation.child_conversation_id))
            })
            .unwrap_or_else(|| "host".to_owned());
        let _ = schedule_goal_due(
            &self.store,
            conversation_id,
            goal_id,
            due_at,
            "review-due",
            &responsible,
        );
        let wake = ContinuityWake {
            logical_wake_id: format!("wake:{goal_id}:{}:due:{due_at}", progress.revision),
            goal_id: goal_id.to_owned(),
            cause_refs: review_cause_refs(&contract, &progress, None),
            due_at: Some(due_at),
            review_policy: "review-due".into(),
            goal_revision: progress.revision,
            epoch: 0,
            host_generation: self.host_generation(),
            claim: None,
            settlement: None,
        };
        enqueue_review_wake(&self.store, conversation_id, &wake)
    }

    pub fn ingest_test_qualification(
        &self,
        bundle: EvidenceBundle,
    ) -> Result<(), ContinuityFailure> {
        if bundle.evidence_class == EvidenceClass::LiveAuthorized {
            return Err(source_unavailable());
        }
        let identity_key = identity_key(&bundle.identity);
        let payload = serde_json::to_string(&json!({
            "responsibilityId": bundle.responsibility_id,
            "identity": bundle.identity,
            "observations": bundle.observations,
            "evidenceClass": "synthetic",
        }))
        .map_err(|_| source_unavailable())?;
        put_qualification_evidence(
            &self.store,
            &bundle.responsibility_id,
            &identity_key,
            &payload,
            "synthetic",
        )?;
        lock(&self.qualification).ingest_immutable(bundle)?;
        self.sync_adoption_stage()
    }

    pub fn ingest_test_live_qualification(
        &self,
        bundle: EvidenceBundle,
        admission: LiveAdmission,
    ) -> Result<(), ContinuityFailure> {
        if !admission.provenance().is_test_evidence() {
            return Err(source_unavailable());
        }
        self.persist_live_bundle(bundle.authorize_live(admission)?)
    }

    pub fn admit_live_evaluation_session(
        &self,
        conversation_id: &str,
        owner_membership_id: &str,
        recipient_membership_id: &str,
    ) -> Result<String, ContinuityFailure> {
        let conversation = self
            .store
            .get(conversation_id)
            .map_err(|_| source_unavailable())?;
        let identity = self.candidate_identity(&conversation, recipient_membership_id);
        let responsibility = responsibility_id(&conversation, recipient_membership_id);
        let session = admit_evaluation_session(
            &self.store,
            conversation_id,
            owner_membership_id,
            recipient_membership_id,
            &responsibility,
            identity,
            &lock(&self.qualification).policy().policy_revision,
        )?;
        Ok(session.session_id)
    }

    pub fn admit_live_evaluation_corpus(
        &self,
        conversation_id: &str,
        owner_membership_id: &str,
        dataset_id: &str,
        cases: Vec<StoredEvaluationCase>,
    ) -> Result<StoredEvaluationCorpus, ContinuityFailure> {
        admit_evaluation_corpus(
            &self.store,
            conversation_id,
            owner_membership_id,
            dataset_id,
            cases,
        )
    }

    #[cfg(any(test, feature = "test-support"))]
    pub fn bind_hermetic_evaluation_observer(&self, observations: Vec<QualificationObservation>) {
        *lock(&self.evaluation_observer) =
            Some(Arc::new(HermeticEvaluationObserver::new(observations)));
    }

    /// Production collection. Always runs the trusted producer, then evaluates
    /// and commits atomically. Only the external observer may be replaced.
    pub fn collect_admitted_qualification(
        &self,
        session_id: &str,
    ) -> Result<(), ContinuityFailure> {
        let session =
            load_evaluation_session(&self.store, session_id)?.ok_or_else(invalid_request)?;
        if session.consumed {
            return Err(idempotency_conflict());
        }
        let authority = resolve_stored_owner_authority(
            &self.store,
            &session.conversation_id,
            &session.owner_membership_id,
        )?;
        if authority.owner_principal_id() != session.owner_principal_id {
            return Err(invalid_request());
        }
        let conversation = self
            .store
            .get(&session.conversation_id)
            .map_err(|_| source_unavailable())?;
        let expected = self.candidate_identity(&conversation, &session.recipient_membership_id);
        if expected != session.identity
            || responsibility_id(&conversation, &session.recipient_membership_id)
                != session.responsibility_id
        {
            return Err(invalid_request());
        }
        if lock(&self.qualification).has_evidence(&session.responsibility_id, &session.identity) {
            return Err(idempotency_conflict());
        }
        let identity_key = identity_key(&session.identity);
        if load_qualification_evidence_for(&self.store, &session.responsibility_id, &identity_key)?
            .is_some()
        {
            return Err(idempotency_conflict());
        }
        claim_collection_operation(&self.store, &session, &identity_key)?;
        let outcome = (|| {
            let collected = self.produce_admitted_session_evaluation(&session)?;
            validate_collection_receipt(&collected.receipt, &session, &collected.observations)?;
            let admission = LiveAdmission::from_admitted_session(
                session.identity.clone(),
                authority,
                session.session_id.clone(),
                lock(&self.qualification).policy().policy_revision.clone(),
                collected.receipt.collector_id.clone(),
                collected.receipt.digest.clone(),
            )?;
            let bundle = EvidenceBundle {
                responsibility_id: session.responsibility_id.clone(),
                identity: session.identity.clone(),
                observations: collected.observations.clone(),
                evidence_class: EvidenceClass::Synthetic,
                provenance: None,
            }
            .authorize_live(admission)?;
            let payload = serde_json::to_string(&json!({
                "responsibilityId": bundle.responsibility_id,
                "identity": bundle.identity,
                "observations": bundle.observations,
                "evidenceClass": "live-authorized",
                "provenance": bundle.provenance,
                "collectionReceipt": collected.receipt,
            }))
            .map_err(|_| source_unavailable())?;
            commit_collected_qualification(
                &self.store,
                &session,
                &session.responsibility_id,
                &identity_key,
                &payload,
                "live-authorized",
            )?;
            lock(&self.qualification).ingest_immutable(bundle)?;
            self.sync_adoption_stage()
        })();
        if outcome.is_err() {
            let _ = release_collection_operation(&self.store, &session.session_id);
        }
        outcome
    }

    fn produce_admitted_session_evaluation(
        &self,
        session: &licoup_conversation::continuity::StoredEvaluationSession,
    ) -> Result<CollectedEvaluation, ContinuityFailure> {
        if let Some(bound) = lock(&self.evaluation_observer).clone() {
            return produce_collected_evaluation(session, bound.as_ref());
        }
        let observer = AdmittedRuntimeEvaluationObserver::new(
            self.store.clone(),
            lock(&self.complete_turn).clone(),
        );
        produce_collected_evaluation(session, &observer)
    }

    fn persist_live_bundle(&self, bundle: EvidenceBundle) -> Result<(), ContinuityFailure> {
        let Some(provenance) = bundle.provenance.clone() else {
            return Err(source_unavailable());
        };
        let identity_key = identity_key(&bundle.identity);
        if lock(&self.qualification).has_evidence(&bundle.responsibility_id, &bundle.identity) {
            return Err(idempotency_conflict());
        }
        if load_qualification_evidence_for(&self.store, &bundle.responsibility_id, &identity_key)?
            .is_some()
        {
            return Err(idempotency_conflict());
        }
        let payload = serde_json::to_string(&json!({
            "responsibilityId": bundle.responsibility_id,
            "identity": bundle.identity,
            "observations": bundle.observations,
            "evidenceClass": "live-authorized",
            "provenance": provenance,
        }))
        .map_err(|_| source_unavailable())?;
        put_qualification_evidence(
            &self.store,
            &bundle.responsibility_id,
            &identity_key,
            &payload,
            "live-authorized",
        )?;
        lock(&self.qualification).ingest_immutable(bundle)?;
        self.sync_adoption_stage()
    }

    pub fn adoption_policy(&self) -> AdoptionPolicy {
        lock(&self.adoption).clone()
    }

    pub fn set_adoption_enabled(
        &self,
        conversation_id: &str,
        owner_membership_id: &str,
        enabled: bool,
    ) -> Result<AdoptionPolicy, ContinuityFailure> {
        persist_adoption_enabled(&self.store, conversation_id, owner_membership_id, enabled)?;
        lock(&self.adoption).enabled = enabled;
        if enabled {
            self.sync_adoption_stage()?;
        }
        Ok(self.adoption_policy())
    }

    pub fn admit_request(
        &self,
        conversation_id: &str,
        membership_id: &str,
        kind: RequestKind,
    ) -> Result<(), ContinuityFailure> {
        let conversation = self
            .store
            .get(conversation_id)
            .map_err(|_| source_unavailable())?;
        self.qualify(&conversation, membership_id, kind)
    }

    pub fn annotate_list(&self, value: &mut Value) {
        let Ok(links) = read_child_links(&self.store) else {
            return;
        };
        let Some(items) = value.as_array_mut() else {
            return;
        };
        for item in items {
            let Some(id) = item.get("id").and_then(Value::as_str).map(str::to_owned) else {
                continue;
            };
            if let Some((_, parent, goal)) = links.iter().find(|(child, _, _)| child == &id) {
                item["parentConversationId"] = json!(parent);
                item["taskGoalId"] = json!(goal);
                item["listingKind"] = json!("child-task");
            }
        }
    }

    pub fn enrich_get(&self, value: &mut Value, conversation_id: &str) {
        let relations = self
            .store
            .list_child_relations(conversation_id, None, 50)
            .unwrap_or_default();
        let views: Vec<Value> = relations
            .iter()
            .filter_map(|relation| task_view_json(self, relation))
            .collect();
        value["taskViews"] = json!(views);
        value["unknownEffectIds"] = json!(self.unknown_effect_ids());
        let policy = self.adoption_policy();
        value["adoptionPolicy"] = json!({
            "enabled": policy.enabled,
            "stage": policy.stage,
            "realModelQualification": "unknown",
        });
    }

    pub fn list_pending_completion_notices(
        &self,
        conversation_id: &str,
        owner_membership_id: &str,
    ) -> Result<Vec<Value>, ContinuityFailure> {
        let notices =
            list_pending_completion_notices(&self.store, conversation_id, owner_membership_id)?;
        Ok(notices
            .into_iter()
            .map(|notice| {
                json!({
                    "notificationId": notice.notification_id,
                    "goalId": notice.goal_id,
                    "parentConversationId": notice.parent_conversation_id,
                    "childConversationId": notice.child_conversation_id,
                    "cardEventId": notice.card_event_id,
                    "cardSequence": notice.card_sequence,
                })
            })
            .collect())
    }

    pub fn ack_completion_notices(
        &self,
        conversation_id: &str,
        owner_membership_id: &str,
        notification_ids: &[String],
    ) -> Result<Vec<String>, ContinuityFailure> {
        ack_completion_notices(
            &self.store,
            conversation_id,
            owner_membership_id,
            notification_ids,
        )
    }

    pub fn resolve_completion_notice(
        &self,
        conversation_id: &str,
        owner_membership_id: &str,
        notification_id: &str,
    ) -> Result<Value, ContinuityFailure> {
        let notice = resolve_completion_notice(
            &self.store,
            conversation_id,
            owner_membership_id,
            notification_id,
        )?;
        Ok(json!({
            "ok": true,
            "notificationId": notice.notification_id,
            "goalId": notice.goal_id,
            "parentConversationId": notice.parent_conversation_id,
            "childConversationId": notice.child_conversation_id,
            "cardEventId": notice.card_event_id,
            "cardSequence": notice.card_sequence,
        }))
    }

    pub fn child_binding(
        &self,
        parent_conversation_id: &str,
        goal_id: &str,
    ) -> Result<ChildBinding, ContinuityFailure> {
        let relation = self.store.relation_for_goal(goal_id)?;
        if relation.parent_conversation_id != parent_conversation_id {
            return Err(source_unavailable());
        }
        let child = self
            .store
            .get(&relation.child_conversation_id)
            .map_err(|_| source_unavailable())?;
        let membership = child
            .assistant_membership_id
            .or_else(|| {
                child.memberships.iter().find_map(|membership| {
                    (membership.status == MembershipStatus::Active
                        && membership.principal.kind == PrincipalKind::Agent)
                        .then(|| membership.id.clone())
                })
            })
            .ok_or_else(source_unavailable)?;
        Ok(ChildBinding {
            child_conversation_id: relation.child_conversation_id,
            membership_id: membership,
            source_task_id: relation.goal_id,
            parent_conversation_id: relation.parent_conversation_id,
        })
    }

    fn bind_child_runtime_from_relation(
        &self,
        parent_conversation_id: &str,
        relation: &ContinuityTaskConversationRelation,
    ) -> Result<(), ContinuityFailure> {
        let child = self
            .store
            .get(&relation.child_conversation_id)
            .map_err(|_| source_unavailable())?;
        let agent_id = child
            .memberships
            .iter()
            .find(|membership| {
                child.assistant_membership_id.as_deref() == Some(membership.id.as_str())
                    || (child.assistant_membership_id.is_none()
                        && membership.status == MembershipStatus::Active
                        && membership.principal.kind == PrincipalKind::Agent)
            })
            .and_then(|membership| membership.principal.agent_id.clone())
            .filter(|agent_id| !agent_id.trim().is_empty());
        let Some(family) = agent_id.as_deref().and_then(protocol_family_for_agent) else {
            return Ok(());
        };
        let generation =
            self.admitted_work_generation(parent_conversation_id, &relation.goal_id)?;
        self.ensure_child_runtime_at(
            parent_conversation_id,
            &relation.goal_id,
            family,
            generation,
        )
    }

    fn bind_child_runtime_from_relation_at(
        &self,
        parent_conversation_id: &str,
        relation: &ContinuityTaskConversationRelation,
        generation: i64,
    ) -> Result<(), ContinuityFailure> {
        let child = self
            .store
            .get(&relation.child_conversation_id)
            .map_err(|_| source_unavailable())?;
        let agent_id = child
            .memberships
            .iter()
            .find(|membership| {
                child.assistant_membership_id.as_deref() == Some(membership.id.as_str())
                    || (child.assistant_membership_id.is_none()
                        && membership.status == MembershipStatus::Active
                        && membership.principal.kind == PrincipalKind::Agent)
            })
            .and_then(|membership| membership.principal.agent_id.clone())
            .filter(|agent_id| !agent_id.trim().is_empty());
        let Some(family) = agent_id.as_deref().and_then(protocol_family_for_agent) else {
            return Ok(());
        };
        self.ensure_child_runtime_at(
            parent_conversation_id,
            &relation.goal_id,
            family,
            generation,
        )
    }

    fn bind_live_child_control(
        &self,
        binding: &ChildBinding,
        goal_id: &str,
        generation: i64,
        host_handle: &str,
        native_turn_id: &str,
    ) -> bool {
        let Some(runtime) = self.work_runtime(
            &binding.child_conversation_id,
            goal_id,
            &binding.membership_id,
            generation,
        ) else {
            return false;
        };
        let key = NativeWorkContextKey {
            conversation_id: binding.child_conversation_id.clone(),
            membership_id: binding.membership_id.clone(),
            matter_id: goal_id.to_owned(),
            generation: generation.max(1),
        };
        runtime
            .bind_live_control(&key, host_handle, native_turn_id)
            .is_ok()
    }

    pub fn update_live_native_turn(
        &self,
        conversation_id: &str,
        membership_id: &str,
        dispatch_id: &str,
        native_turn_id: &str,
    ) {
        let Some(relation) = read_relation_for_child(&self.store, conversation_id)
            .ok()
            .flatten()
        else {
            return;
        };
        let Ok(binding) = self.child_binding(&relation.parent_conversation_id, &relation.goal_id)
        else {
            return;
        };
        if binding.membership_id != membership_id
            || binding.child_conversation_id != conversation_id
        {
            return;
        }
        if dispatch_id.trim().is_empty() || native_turn_id.trim().is_empty() {
            return;
        }
        let Some(mut identity) = self
            .live_child_work_identity(&relation.parent_conversation_id, &relation.goal_id)
            .ok()
            .flatten()
        else {
            return;
        };
        if identity.child_conversation_id != conversation_id
            || identity.membership_id != membership_id
            || identity.dispatch_id.as_deref() != Some(dispatch_id)
        {
            return;
        }
        if identity
            .native_turn_id
            .as_deref()
            .is_some_and(|existing| !existing.is_empty() && existing != native_turn_id)
        {
            return;
        }
        identity.native_turn_id = Some(native_turn_id.to_owned());
        let _ = self.bind_child_runtime_from_relation_at(
            &relation.parent_conversation_id,
            &relation,
            identity.work_generation,
        );
        if !self.bind_live_child_control(
            &binding,
            &relation.goal_id,
            identity.work_generation,
            dispatch_id,
            native_turn_id,
        ) {
            return;
        }
        let _ = record_child_work_live(
            &self.store,
            &relation.parent_conversation_id,
            &relation.goal_id,
            &child_work_identity_payload(&identity, "child-work-live"),
        );
    }

    fn cold_recover(&self) -> Result<(), ContinuityFailure> {
        self.reconcile_unknown_effects()?;
        self.coalesce_wake_generations()?;
        let _ = self.recover_pending_settlements();
        let _ = self.recover_admitted_live_operations();
        let _ = self.recover_pending_child_work();
        self.enqueue_missed_due_once()?;
        if !self.effects_runtime_ready() {
            return Ok(());
        }
        let now = continuity_now_ms();
        let mut conversations = BTreeSet::new();
        for (_, conversation_id, _) in list_due_goals(&self.store, now)? {
            conversations.insert(conversation_id);
        }
        for (conversation_id, wake) in list_all_pending_wakes(&self.store)? {
            if wake.due_at.is_none() || wake.due_at.is_some_and(|due| due <= now) {
                conversations.insert(conversation_id);
            }
        }
        for conversation_id in conversations {
            let _ = self.drain_wakes(&conversation_id);
        }
        Ok(())
    }

    fn coalesce_wake_generations(&self) -> Result<(), ContinuityFailure> {
        let generation = self.host_generation();
        for (_, wake) in list_all_pending_wakes(&self.store)? {
            if wake.host_generation != 0 && wake.host_generation != generation {
                let _ = update_wake_host_generation(&self.store, &wake.logical_wake_id, generation);
            }
        }
        Ok(())
    }

    fn enqueue_missed_due_once(&self) -> Result<(), ContinuityFailure> {
        let now = continuity_now_ms();
        let pending = list_all_pending_wakes(&self.store)?;
        let pending_keys: BTreeSet<_> = pending
            .iter()
            .map(|(_, wake)| (wake.goal_id.clone(), wake.goal_revision))
            .collect();
        for (goal_id, parent, due) in list_due_goals(&self.store, now)? {
            let Some((contract, progress)) = read_goal_bundle(&self.store, &goal_id)? else {
                continue;
            };
            if progress.lifecycle == ContinuityGoalLifecycle::Achieved
                || progress.lifecycle == ContinuityGoalLifecycle::Cancelled
                || progress.control == ContinuityGoalControl::Paused
                || progress.control == ContinuityGoalControl::CancelRequested
            {
                continue;
            }
            if due > now || pending_keys.contains(&(goal_id.clone(), progress.revision)) {
                continue;
            }
            let wake = ContinuityWake {
                logical_wake_id: format!("wake:{goal_id}:{}:missed-due", progress.revision),
                goal_id: goal_id.clone(),
                cause_refs: review_cause_refs(&contract, &progress, None),
                due_at: Some(due),
                review_policy: "missed-due".into(),
                goal_revision: progress.revision,
                epoch: 0,
                host_generation: self.host_generation(),
                claim: None,
                settlement: None,
            };
            let _ = enqueue_review_wake(&self.store, &parent, &wake);
        }
        Ok(())
    }

    fn enqueue_goal_rethink(
        &self,
        conversation_id: &str,
        goal_id: &str,
        settlement_id: Option<&str>,
        settlement_source: Option<&licoup_conversation::continuity::ContinuitySourceRef>,
        review_policy: &str,
    ) -> Result<(), ContinuityFailure> {
        let Some((contract, progress)) = read_goal_bundle(&self.store, goal_id)? else {
            return Ok(());
        };
        if progress.lifecycle == ContinuityGoalLifecycle::Achieved
            || progress.lifecycle == ContinuityGoalLifecycle::Cancelled
        {
            return Ok(());
        }
        let wake = ContinuityWake {
            logical_wake_id: format!("wake:{goal_id}:{}:{review_policy}", progress.revision),
            goal_id: goal_id.to_owned(),
            cause_refs: review_cause_refs(&contract, &progress, settlement_source),
            due_at: None,
            review_policy: review_policy.to_owned(),
            goal_revision: progress.revision,
            epoch: 0,
            host_generation: self.host_generation(),
            claim: None,
            settlement: settlement_id.map(str::to_owned),
        };
        let _ = enqueue_review_wake(&self.store, conversation_id, &wake);
        Ok(())
    }

    fn reconcile_unknown_effects(&self) -> Result<Vec<String>, ContinuityFailure> {
        let listed = list_unknown_effect_ids(&self.store)?;
        let mut still_unknown = Vec::new();
        let mut reconciled = Vec::new();
        for (effect_id, conversation_id, goal_id) in listed {
            let _ = replay_effect(&self.store, &effect_id);
            match load_effect_status(&self.store, &effect_id)? {
                Some(ContinuityEffectStatus::Executed)
                | Some(ContinuityEffectStatus::NotExecuted) => reconciled.push(effect_id),
                _ => still_unknown.push((effect_id, conversation_id, goal_id)),
            }
        }
        *lock(&self.unknown_effects) = still_unknown;
        Ok(reconciled)
    }

    fn effects_runtime_ready(&self) -> bool {
        self.runtime_bound.load(Ordering::SeqCst)
            || self.using_script.load(Ordering::SeqCst)
            || self.production.is_bound()
    }

    fn reload_qualification(&self) -> Result<(), ContinuityFailure> {
        let rows = list_qualification_evidence(&self.store)?;
        let policy_revision = lock(&self.qualification).policy().policy_revision.clone();
        let mut service = QualificationService::draft_port();
        for (_, _, payload, _) in rows {
            let value: Value = serde_json::from_str(&payload).map_err(|_| source_unavailable())?;
            let Some(bundle) = bundle_from_stored(self, &value, &policy_revision) else {
                continue;
            };
            let _ = service.ingest_immutable(bundle);
        }
        *lock(&self.qualification) = service;
        Ok(())
    }

    fn sync_adoption_stage(&self) -> Result<(), ContinuityFailure> {
        let rows = list_qualification_evidence(&self.store)?;
        let service = lock(&self.qualification);
        let policy_revision = service.policy().policy_revision.clone();
        let mut admitted = BTreeSet::new();
        let mut qualified = BTreeSet::new();
        for (_, _, payload, _) in rows {
            let Ok(value) = serde_json::from_str::<Value>(&payload) else {
                continue;
            };
            let Some(bundle) = bundle_from_stored(self, &value, &policy_revision) else {
                continue;
            };
            if bundle.evidence_class != EvidenceClass::LiveAuthorized {
                continue;
            }
            admitted.insert(bundle.responsibility_id.clone());
            if let Ok(assessment) = service.assess(&bundle.responsibility_id, &bundle.identity) {
                if assessment.result == ContinuityQualificationResult::Qualified {
                    qualified.insert(bundle.responsibility_id);
                }
            }
        }
        drop(service);
        let stage = stage_from_coverage(admitted.len() as u64, qualified.len() as u64);
        let mut policy = lock(&self.adoption);
        if policy.stage != stage {
            policy.stage = stage;
            persist_adoption_stage(&self.store, stage.as_str())?;
        }
        Ok(())
    }

    fn interpret_and_commit(
        &self,
        conversation_id: &str,
        event_id: &str,
        intent: CognitionIntent,
    ) -> Result<IngressOutcome, ContinuityFailure> {
        self.bind_pending_scripts(event_id);
        let conversation = self
            .store
            .get(conversation_id)
            .map_err(|_| source_unavailable())?;
        let Some(recipient) = recipient_membership(&conversation) else {
            return Ok(IngressOutcome {
                abstained: true,
                ..IngressOutcome::default()
            });
        };
        if intent == CognitionIntent::UserPosted
            && self.ingress_already_executed(conversation_id, event_id, &recipient)
        {
            return Ok(IngressOutcome {
                abstained: true,
                ..IngressOutcome::default()
            });
        }
        if intent == CognitionIntent::UserPosted {
            let _ = self.qualify(&conversation, &recipient, RequestKind::ExplicitlyRequested);
        }
        let Some(prepared) =
            self.prepare_ingress_assembly(conversation_id, event_id, &recipient, None)
        else {
            return Ok(IngressOutcome {
                abstained: true,
                ..IngressOutcome::default()
            });
        };
        let invoker = lock(&self.cognition).clone();
        let mut reply = invoker.invoke(&CognitionRequest {
            conversation_id: conversation_id.to_owned(),
            recipient_membership_id: recipient.clone(),
            event_id: event_id.to_owned(),
            intent,
            orientation: prepared.manifest.clone(),
            assembly: prepared.assembly.clone(),
            review: None,
        })?;
        if !reply.proposal.requested_reads.is_empty() {
            match prepared
                .composer
                .refine_authorized(&prepared.request, &reply.proposal)
            {
                Ok(refined) => {
                    if let Ok(refined_assembly) =
                        prepared.workspace.assembly_snapshot(&refined.invocation_id)
                    {
                        reply = invoker.invoke(&CognitionRequest {
                            conversation_id: conversation_id.to_owned(),
                            recipient_membership_id: recipient.clone(),
                            event_id: event_id.to_owned(),
                            intent,
                            orientation: refined,
                            assembly: refined_assembly,
                            review: None,
                        })?;
                    }
                }
                Err(_) => {
                    reply.proposal.requested_reads.clear();
                    if !proposal_has_business_effect(&reply.proposal) {
                        return Ok(IngressOutcome {
                            abstained: true,
                            invocation_count: invoker.invocation_count(),
                            unavailable: reply.unavailable,
                            ..IngressOutcome::default()
                        });
                    }
                }
            }
        }
        let invocation_count = invoker.invocation_count();
        if intent == CognitionIntent::WakeReevaluation
            && (proposal_creates_new_advancement(&reply.proposal)
                || proposal_has_business_effect(&reply.proposal))
        {
            if let Err(err) =
                self.qualify(&conversation, &recipient, RequestKind::AutomaticAdvancement)
            {
                if matches!(
                    err.code,
                    ContinuityFailureCode::QualificationUnknown
                        | ContinuityFailureCode::QualificationStale
                ) {
                    return Ok(IngressOutcome {
                        abstained: true,
                        qualification_denied: true,
                        invocation_count,
                        unavailable: reply.unavailable,
                        ..IngressOutcome::default()
                    });
                }
                return Err(err);
            }
        }
        self.finish_ingress_proposal(
            conversation_id,
            event_id,
            &recipient,
            intent == CognitionIntent::UserPosted && !reply.unavailable,
            reply.proposal,
            invocation_count,
            reply.unavailable,
        )
    }

    fn interpret_settled_turn(
        &self,
        conversation_id: &str,
        payload: &Value,
    ) -> Result<IngressOutcome, ContinuityFailure> {
        let membership_id = payload
            .get("membershipId")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_owned();
        let event_id = payload
            .get("causationId")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_owned();
        if membership_id.is_empty() || event_id.is_empty() {
            return Ok(IngressOutcome {
                abstained: true,
                ..IngressOutcome::default()
            });
        }
        if payload.get("ok").and_then(Value::as_bool) == Some(false)
            || matches!(
                payload
                    .get("turnStatus")
                    .or_else(|| payload
                        .get("error")
                        .and_then(|error| error.get("turnStatus")))
                    .and_then(Value::as_str)
                    .map(str::trim),
                Some("failed" | "cancelled")
            )
        {
            return Ok(IngressOutcome {
                abstained: true,
                ..IngressOutcome::default()
            });
        }
        let conversation = self
            .store
            .get(conversation_id)
            .map_err(|_| source_unavailable())?;
        if conversation.assistant_membership_id.as_deref() != Some(membership_id.as_str()) {
            return Ok(IngressOutcome {
                abstained: true,
                ..IngressOutcome::default()
            });
        }
        if self.ingress_already_executed(conversation_id, &event_id, &membership_id) {
            return Ok(IngressOutcome {
                abstained: true,
                ..IngressOutcome::default()
            });
        }
        let Some(prepared) =
            self.prepare_ingress_assembly(conversation_id, &event_id, &membership_id, None)
        else {
            return Ok(IngressOutcome {
                abstained: true,
                ..IngressOutcome::default()
            });
        };
        let output = settled_turn_output(payload);
        let mut proposal = proposal_from_assistant_turn_response(&prepared.assembly, &output)?;
        let mut unavailable = false;
        if !proposal.requested_reads.is_empty() {
            match prepared
                .composer
                .refine_authorized(&prepared.request, &proposal)
            {
                Ok(refined) => {
                    if let Ok(refined_assembly) =
                        prepared.workspace.assembly_snapshot(&refined.invocation_id)
                    {
                        let invoker = lock(&self.cognition).clone();
                        let reply = invoker.invoke(&CognitionRequest {
                            conversation_id: conversation_id.to_owned(),
                            recipient_membership_id: membership_id.clone(),
                            event_id: event_id.clone(),
                            intent: CognitionIntent::UserPosted,
                            orientation: refined,
                            assembly: refined_assembly,
                            review: None,
                        })?;
                        proposal = reply.proposal;
                        unavailable = reply.unavailable;
                    }
                }
                Err(_) => {
                    return Ok(IngressOutcome {
                        abstained: true,
                        ..IngressOutcome::default()
                    });
                }
            }
        }
        let invocation_count = lock(&self.cognition).invocation_count();
        self.finish_ingress_proposal(
            conversation_id,
            &event_id,
            &membership_id,
            !unavailable,
            proposal,
            invocation_count,
            unavailable,
        )
    }

    fn finish_ingress_proposal(
        &self,
        conversation_id: &str,
        event_id: &str,
        membership_id: &str,
        record_applied: bool,
        proposal: licoup_conversation::continuity::ContinuityInterpretationProposal,
        invocation_count: u64,
        unavailable: bool,
    ) -> Result<IngressOutcome, ContinuityFailure> {
        if unavailable {
            return Ok(IngressOutcome {
                abstained: true,
                invocation_count,
                unavailable: true,
                ..IngressOutcome::default()
            });
        }
        if !proposal_has_business_effect(&proposal) {
            if record_applied {
                self.record_applied_user_posted(conversation_id, event_id, membership_id)?;
            }
            return Ok(IngressOutcome {
                abstained: true,
                invocation_count,
                unavailable,
                ..IngressOutcome::default()
            });
        }
        let admitted_goal = proposal
            .task_child_admission
            .as_ref()
            .map(|admission| admission.goal_id.clone());
        let receipt = if record_applied {
            self.commit_fresh_user_posted(proposal, event_id, membership_id)?
        } else {
            self.commit_fresh(proposal)?
        };
        let relation = admitted_goal
            .as_deref()
            .and_then(|goal_id| self.store.relation_for_goal(goal_id).ok());
        if let Some(relation) = relation.as_ref() {
            let _ = self.bind_child_runtime_from_relation(conversation_id, relation);
            if record_applied {
                let _ = self.start_admitted_child_work(conversation_id, &relation.goal_id);
            }
        }
        let child = relation.map(|item| item.child_conversation_id);
        Ok(IngressOutcome {
            committed: true,
            abstained: false,
            child_conversation_id: child,
            receipt_revision: Some(receipt.revision),
            invocation_count,
            unavailable,
            qualification_denied: false,
        })
    }

    fn prepare_ingress_assembly(
        &self,
        conversation_id: &str,
        event_id: &str,
        recipient: &str,
        subject: Option<&ContinuitySourceRef>,
    ) -> Option<PreparedIngress> {
        let current = if event_id.trim().is_empty() {
            None
        } else {
            Some(event_id)
        };
        let live = populate_live_store(&self.store, conversation_id, current);
        if let Some(subject) = subject {
            insert_task_subject(&live, conversation_id, subject);
        }
        let workspace = ContinuityWorkspace::new(live, ScriptedAgent::new(), Default::default());
        let composer = UnavailableContextCompositionService::from_workspace(workspace.clone());
        let revocation = workspace
            .store
            .recipient_revocation(conversation_id, recipient);
        let request = ContinuityContextCompositionRequest {
            conversation_id: conversation_id.to_owned(),
            recipient_membership_id: recipient.to_owned(),
            authorized_scopes: vec![ContinuityVisibilityScope::Conversation],
            revocation_generation: revocation,
            after: None,
            limit: 32,
        };
        let manifest = composer.compose_authorized(&request).ok()?;
        let assembly = workspace.assembly_snapshot(&manifest.invocation_id).ok()?;
        Some(PreparedIngress {
            request,
            manifest,
            assembly,
            workspace,
            composer,
        })
    }

    fn prepare_child_recipient_assembly(
        &self,
        child_conversation_id: &str,
        event_id: &str,
        recipient: &str,
        subject: Option<&ContinuitySourceRef>,
    ) -> Option<PreparedIngress> {
        let prepared =
            self.prepare_ingress_assembly(child_conversation_id, event_id, recipient, subject)?;
        let first_failed = prepared
            .composer
            .recheck_dispatch(&prepared.manifest)
            .is_err()
            || take_child_assembly_recheck_failure();
        if !first_failed {
            return Some(prepared);
        }
        let retried =
            self.prepare_ingress_assembly(child_conversation_id, event_id, recipient, subject)?;
        let second_failed = retried
            .composer
            .recheck_dispatch(&retried.manifest)
            .is_err()
            || take_child_assembly_recheck_failure();
        if second_failed {
            return None;
        }
        Some(retried)
    }

    fn interpret_wake_review(
        &self,
        parent_conversation_id: &str,
        wake: &ContinuityWake,
    ) -> Result<IngressOutcome, ContinuityFailure> {
        self.bind_pending_scripts(
            wake.cause_refs
                .first()
                .map(|source| source.opaque_id.as_str())
                .unwrap_or(""),
        );
        let Some((contract, progress)) = read_goal_bundle(&self.store, &wake.goal_id)? else {
            return Ok(IngressOutcome {
                abstained: true,
                ..IngressOutcome::default()
            });
        };
        let relation = self.store.relation_for_goal(&wake.goal_id)?;
        if relation.parent_conversation_id != parent_conversation_id {
            return Err(source_unavailable());
        }
        let binding = self.child_binding(parent_conversation_id, &wake.goal_id)?;
        let settlement = wake.settlement.as_deref().and_then(|id| {
            load_settlement_receipt(
                &self.store,
                &relation.child_conversation_id,
                parent_conversation_id,
                id,
            )
        });
        let subject = wake
            .cause_refs
            .iter()
            .find(|source| source.owner_kind == ContinuitySourceOwnerKind::Event)
            .cloned()
            .filter(|source| !source.opaque_id.trim().is_empty())
            .or_else(|| {
                let created = &contract.created_event;
                (!created.opaque_id.trim().is_empty()).then(|| created.clone())
            });
        let cause_event = subject
            .as_ref()
            .map(|source| source.opaque_id.clone())
            .unwrap_or_default();
        let assembly_event =
            child_local_event(&self.store, &binding.child_conversation_id, &cause_event);
        let Some(prepared) = self.prepare_child_recipient_assembly(
            &binding.child_conversation_id,
            assembly_event,
            &binding.membership_id,
            subject.as_ref(),
        ) else {
            return Ok(IngressOutcome {
                abstained: true,
                unavailable: true,
                ..IngressOutcome::default()
            });
        };
        let facts = admitted_facts_from_assembly(&self.store, &prepared.assembly);
        let brief = compose_wake_review_brief(
            &contract,
            &progress,
            &facts,
            &relation,
            settlement.as_ref(),
            &wake.review_policy,
            &wake.review_policy,
        );
        let invoker = lock(&self.cognition).clone();
        let review = WakeReviewContext {
            goal_id: wake.goal_id.clone(),
            goal_revision: progress.revision,
            brief,
            review_policy: wake.review_policy.clone(),
            parent_conversation_id: parent_conversation_id.to_owned(),
            continuity_kind: CONTINUITY_KIND_WAKE_REVIEW,
        };
        let reply = invoker.invoke(&CognitionRequest {
            conversation_id: binding.child_conversation_id.clone(),
            recipient_membership_id: binding.membership_id.clone(),
            event_id: cause_event.clone(),
            intent: CognitionIntent::WakeReevaluation,
            orientation: prepared.manifest.clone(),
            assembly: prepared.assembly.clone(),
            review: Some(review),
        })?;
        let invocation_count = invoker.invocation_count();
        if reply.unavailable {
            return Ok(IngressOutcome {
                abstained: true,
                invocation_count,
                unavailable: true,
                ..IngressOutcome::default()
            });
        }
        let mut proposal = reply.proposal;
        self.retain_existing_parent_sources(parent_conversation_id, &mut proposal);
        restamp_review_onto_parent(&mut proposal, parent_conversation_id);
        proposal.envelope.request_id = format!(
            "request:review:{}:{}",
            wake.logical_wake_id, progress.revision
        );
        if proposal_creates_new_advancement(&proposal) {
            let child_conversation = self
                .store
                .get(&binding.child_conversation_id)
                .map_err(|_| source_unavailable())?;
            if let Err(err) = self.qualify(
                &child_conversation,
                &binding.membership_id,
                RequestKind::AutomaticAdvancement,
            ) {
                if matches!(
                    err.code,
                    ContinuityFailureCode::QualificationUnknown
                        | ContinuityFailureCode::QualificationStale
                ) {
                    return Ok(IngressOutcome {
                        abstained: true,
                        qualification_denied: true,
                        invocation_count,
                        unavailable: reply.unavailable,
                        ..IngressOutcome::default()
                    });
                }
                return Err(err);
            }
        }
        proposal = strip_review_unsafe_effects(proposal);
        self.finish_ingress_proposal(
            parent_conversation_id,
            &cause_event,
            &binding.membership_id,
            false,
            proposal,
            invocation_count,
            reply.unavailable,
        )
    }

    fn apply_runtime_settlement(
        &self,
        conversation_id: &str,
        payload: &Value,
        persist_pending: bool,
    ) -> Result<WakeDrain, ContinuityFailure> {
        self.reconcile_unknown_effects()?;
        let child_relation = read_relation_for_child(&self.store, conversation_id)?;
        if let Some(relation) = child_relation {
            if is_wake_review_payload(payload) {
                let settlement_id = settlement_identity(payload);
                if settlement_applied(&self.store, conversation_id, &settlement_id)? {
                    return Ok(WakeDrain::default());
                }
                if persist_pending {
                    record_settlement_pending(
                        &self.store,
                        conversation_id,
                        &settlement_id,
                        payload,
                    )?;
                }
                let outcome = self.apply_wake_review_settlement(&relation, payload)?;
                if outcome.unavailable || outcome.qualification_denied {
                    return Ok(WakeDrain {
                        preserved: vec![settlement_id],
                        ..WakeDrain::default()
                    });
                }
                record_settlement_applied(&self.store, conversation_id, &settlement_id, payload)?;
                return Ok(WakeDrain::default());
            }
            let Some(settlement_id) = child_settlement_identity(payload) else {
                return Ok(WakeDrain::default());
            };
            if settlement_applied(&self.store, conversation_id, &settlement_id)? {
                return Ok(WakeDrain::default());
            }
            let preview = self.preview_child_settlement(&relation, payload, &settlement_id)?;
            if matches!(preview, ChildSettlementOutcome::Rejected) {
                return Ok(WakeDrain::default());
            }
            if persist_pending {
                record_settlement_pending(&self.store, conversation_id, &settlement_id, payload)?;
            }
            if matches!(preview, ChildSettlementOutcome::PendingUnknown) {
                return Ok(WakeDrain {
                    preserved: vec![settlement_id],
                    ..WakeDrain::default()
                });
            }
            match self.retain_child_settlement_evidence(&relation, payload, &settlement_id)? {
                ChildSettlementOutcome::Rejected | ChildSettlementOutcome::PendingUnknown => {
                    return Ok(WakeDrain {
                        preserved: vec![settlement_id],
                        ..WakeDrain::default()
                    });
                }
                ChildSettlementOutcome::Retained(source) => {
                    if take_child_work_fault(ChildWorkFault::FailAfterEvidence) {
                        return Err(source_unavailable());
                    }
                    self.enqueue_goal_rethink(
                        &relation.parent_conversation_id,
                        &relation.goal_id,
                        Some(&settlement_id),
                        Some(&source),
                        "runtime-terminal",
                    )?;
                    record_settlement_applied(
                        &self.store,
                        conversation_id,
                        &settlement_id,
                        payload,
                    )?;
                    return self.drain_wakes(&relation.parent_conversation_id);
                }
            }
        }
        let settlement_id = settlement_identity(payload);
        if settlement_applied(&self.store, conversation_id, &settlement_id)? {
            return Ok(WakeDrain::default());
        }
        if persist_pending {
            record_settlement_pending(&self.store, conversation_id, &settlement_id, payload)?;
        }
        let outcome = if is_wake_review_payload(payload) {
            IngressOutcome {
                abstained: true,
                ..IngressOutcome::default()
            }
        } else {
            self.interpret_settled_turn(conversation_id, payload)?
        };
        if outcome.unavailable || outcome.qualification_denied {
            return Ok(WakeDrain {
                preserved: vec![settlement_id],
                ..WakeDrain::default()
            });
        }
        record_settlement_applied(&self.store, conversation_id, &settlement_id, payload)?;
        Ok(WakeDrain::default())
    }

    fn recover_pending_settlements(&self) -> Result<usize, ContinuityFailure> {
        if !self.effects_runtime_ready() {
            return Ok(0);
        }
        let mut recovered = 0;
        for (conversation_id, _settlement_id, payload) in list_unapplied_settlements(&self.store)? {
            match self.apply_runtime_settlement(&conversation_id, &payload, false) {
                Ok(_) => recovered += 1,
                Err(_) => {}
            }
        }
        Ok(recovered)
    }

    fn recover_admitted_live_operations(&self) -> Result<usize, ContinuityFailure> {
        if !self.effects_runtime_ready() {
            return Ok(0);
        }
        let mut restored = 0;
        for (parent, goal_id, payload) in list_child_work_live(&self.store)? {
            let Some(identity) = child_work_identity_from_payload(&payload, &parent, &goal_id)
            else {
                continue;
            };
            match self.restore_admitted_live_operation(&parent, &goal_id, &identity) {
                Ok(true) => restored += 1,
                Ok(false) | Err(_) => {}
            }
        }
        Ok(restored)
    }

    fn recover_pending_child_work(&self) -> Result<usize, ContinuityFailure> {
        if !self.effects_runtime_ready() {
            return Ok(0);
        }
        let mut recovered = 0;
        let mut after_conversation: Option<String> = None;
        let mut after_goal: Option<String> = None;
        let mut after_key: Option<String> = None;
        loop {
            let page = list_unacked_child_work_page(
                &self.store,
                after_conversation.as_deref(),
                after_goal.as_deref(),
                after_key.as_deref(),
                PENDING_OBLIGATION_PAGE_SIZE,
            )?;
            let page_len = page.len();
            if let Some((parent, goal_id, revision, _)) = page.last() {
                after_conversation = Some(parent.clone());
                after_goal = Some(goal_id.clone());
                after_key = Some(child_work_named_key(
                    goal_id,
                    *revision,
                    CHILD_WORK_PENDING_DESIGNATION,
                ));
            }
            let mut seen = BTreeSet::new();
            for (parent, goal_id, _, _) in page {
                if !seen.insert((parent.clone(), goal_id.clone())) {
                    continue;
                }
                match self.start_admitted_child_work(&parent, &goal_id) {
                    Ok(()) => recovered += 1,
                    Err(_) => {}
                }
            }
            if page_len < PENDING_OBLIGATION_PAGE_SIZE as usize {
                break;
            }
        }
        Ok(recovered)
    }

    fn start_admitted_child_work(
        &self,
        parent_conversation_id: &str,
        goal_id: &str,
    ) -> Result<(), ContinuityFailure> {
        if self.using_script.load(Ordering::SeqCst) {
            return Ok(());
        }
        let Some((contract, progress)) = read_goal_bundle(&self.store, goal_id)? else {
            return Ok(());
        };
        if is_terminal(progress.lifecycle) {
            return Ok(());
        }
        let relation = self.store.relation_for_goal(goal_id)?;
        if relation.parent_conversation_id != parent_conversation_id {
            return Err(source_unavailable());
        }
        let binding = self.child_binding(parent_conversation_id, goal_id)?;
        let child = self
            .store
            .get(&binding.child_conversation_id)
            .map_err(|_| source_unavailable())?;
        if !active_agent_membership(&child, &binding.membership_id) {
            return Ok(());
        }
        let live_identity = self.live_child_work_identity(parent_conversation_id, goal_id)?;
        let pending = read_oldest_pending_child_work(&self.store, parent_conversation_id, goal_id)?;
        let pending_is_same_live = match (&live_identity, &pending) {
            (Some(live), Some((_, payload))) => {
                child_work_identity_from_payload(payload, parent_conversation_id, goal_id)
                    .is_some_and(|pending_identity| {
                        pending_identity.operation_id == live.operation_id
                    })
            }
            _ => false,
        };
        if let Some(identity) = live_identity {
            if pending.is_none() || pending_is_same_live {
                if self.restore_admitted_live_operation(
                    parent_conversation_id,
                    goal_id,
                    &identity,
                )? {
                    return Ok(());
                }
            }
        }
        if suppresses_new_work(progress.control) {
            return Ok(());
        }
        let Some((admitted_revision, pending)) = pending else {
            return Ok(());
        };
        let identity = child_work_identity_from_payload(&pending, parent_conversation_id, goal_id)
            .unwrap_or_else(|| ChildWorkIdentity {
                parent_conversation_id: parent_conversation_id.to_owned(),
                child_conversation_id: binding.child_conversation_id.clone(),
                goal_id: goal_id.to_owned(),
                membership_id: binding.membership_id.clone(),
                admitted_revision,
                work_generation: admitted_revision.max(1),
                operation_id: child_work_operation_id(goal_id, admitted_revision),
                dispatch_id: None,
                native_turn_id: None,
            });
        if identity.child_conversation_id != binding.child_conversation_id
            || identity.membership_id != binding.membership_id
            || identity.parent_conversation_id != parent_conversation_id
            || identity.goal_id != goal_id
        {
            return Ok(());
        }
        let operation_id = self.ensure_child_work_operation(
            parent_conversation_id,
            goal_id,
            identity.admitted_revision,
            identity.work_generation,
            &binding,
        )?;
        if operation_id != identity.operation_id && !identity.operation_id.is_empty() {
            return Ok(());
        }
        let mut identity = identity;
        identity.operation_id = operation_id.clone();
        if self.reconcile_registered_child_work(
            parent_conversation_id,
            goal_id,
            &identity,
            &binding,
        )? {
            return Ok(());
        }
        let Some(start) = lock(&self.work_start).clone() else {
            return Ok(());
        };
        let Some(prepared) = self.prepare_child_recipient_assembly(
            &binding.child_conversation_id,
            "",
            &binding.membership_id,
            None,
        ) else {
            return Ok(());
        };
        let facts = admitted_facts_from_assembly(&self.store, &prepared.assembly);
        let brief = compose_child_work_brief(&contract, &progress, &facts, &relation);
        if take_child_work_fault(ChildWorkFault::FailStart) {
            return Err(source_unavailable());
        }
        let _ = self.bind_child_runtime_from_relation_at(
            parent_conversation_id,
            &relation,
            identity.work_generation,
        );
        let subject_causation = (!contract.created_event.opaque_id.trim().is_empty())
            .then_some(contract.created_event.opaque_id.as_str());
        let params = compose_admitted_turn_params(&AdmittedTurnRequest {
            store: &self.store,
            conversation_id: &binding.child_conversation_id,
            membership_id: &binding.membership_id,
            text: &brief,
            causation_id: subject_causation,
            dispatch_id: Some(&operation_id),
            continuity_kind: CONTINUITY_KIND_CHILD_WORK,
            goal_id: Some(goal_id),
            goal_revision: Some(identity.admitted_revision),
            review_policy: None,
            parent_conversation_id: Some(parent_conversation_id),
            assembly: Some(&prepared.assembly),
            orientation: Some(&prepared.manifest),
            include_licoup_guide: true,
        })?;
        let started = start(&params).map_err(|_| source_unavailable())?;
        let dispatch_id = started
            .get("dispatchId")
            .or_else(|| started.get("turnHandle"))
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .ok_or_else(source_unavailable)?;
        let native_turn_id = started
            .get("turnId")
            .or_else(|| started.get("nativeTurnId"))
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .unwrap_or("");
        identity.dispatch_id = Some(dispatch_id.to_owned());
        if !native_turn_id.is_empty() {
            identity.native_turn_id = Some(native_turn_id.to_owned());
        }
        record_child_work_accepted(
            &self.store,
            parent_conversation_id,
            goal_id,
            identity.admitted_revision,
            &child_work_identity_payload(&identity, "child-work-accepted"),
        )?;
        if take_child_work_fault(ChildWorkFault::FailAfterAccepted) {
            return Err(source_unavailable());
        }
        if self
            .work_runtime(
                &binding.child_conversation_id,
                goal_id,
                &binding.membership_id,
                identity.work_generation,
            )
            .is_none()
        {
            let _ = self.bind_child_runtime_from_relation_at(
                parent_conversation_id,
                &relation,
                identity.work_generation,
            );
        }
        if !self.bind_live_child_control(
            &binding,
            goal_id,
            identity.work_generation,
            dispatch_id,
            native_turn_id,
        ) {
            return Err(source_unavailable());
        }
        record_child_work_live(
            &self.store,
            parent_conversation_id,
            goal_id,
            &child_work_identity_payload(&identity, "child-work-live"),
        )?;
        record_child_work_started(
            &self.store,
            parent_conversation_id,
            goal_id,
            identity.admitted_revision,
        )?;
        Ok(())
    }

    fn ensure_child_work_operation(
        &self,
        parent_conversation_id: &str,
        goal_id: &str,
        revision: i64,
        work_generation: i64,
        binding: &ChildBinding,
    ) -> Result<String, ContinuityFailure> {
        if let Some(existing) =
            read_child_work_intent(&self.store, parent_conversation_id, goal_id, revision)?
        {
            if let Some(operation_id) = existing
                .get("operationId")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
            {
                return Ok(operation_id.to_owned());
            }
        }
        let operation_id = child_work_operation_id(goal_id, revision);
        record_child_work_intent(
            &self.store,
            parent_conversation_id,
            goal_id,
            revision,
            &child_work_identity_payload(
                &ChildWorkIdentity {
                    parent_conversation_id: parent_conversation_id.to_owned(),
                    child_conversation_id: binding.child_conversation_id.clone(),
                    goal_id: goal_id.to_owned(),
                    membership_id: binding.membership_id.clone(),
                    admitted_revision: revision,
                    work_generation,
                    operation_id: operation_id.clone(),
                    dispatch_id: None,
                    native_turn_id: None,
                },
                "child-work-intent",
            ),
        )?;
        Ok(operation_id)
    }

    fn reconcile_registered_child_work(
        &self,
        parent_conversation_id: &str,
        goal_id: &str,
        identity: &ChildWorkIdentity,
        binding: &ChildBinding,
    ) -> Result<bool, ContinuityFailure> {
        if let Some(accepted) = read_child_work_accepted(
            &self.store,
            parent_conversation_id,
            goal_id,
            identity.admitted_revision,
        )? {
            let Some(accepted_identity) =
                child_work_identity_from_payload(&accepted, parent_conversation_id, goal_id)
            else {
                return Ok(true);
            };
            if !identities_match(identity, &accepted_identity)
                || accepted_identity.child_conversation_id != binding.child_conversation_id
                || accepted_identity.membership_id != binding.membership_id
            {
                return Ok(true);
            }
            let _ = self.restore_admitted_live_operation(
                parent_conversation_id,
                goal_id,
                &accepted_identity,
            )?;
            return Ok(true);
        }
        let Ok(Some(dispatch)) = self.store.dispatch_record(&identity.operation_id) else {
            return Ok(false);
        };
        if dispatch.id != identity.operation_id
            || dispatch.conversation_id != binding.child_conversation_id
            || dispatch.membership_id != binding.membership_id
            || dispatch.operation != "send"
        {
            return Ok(false);
        }
        if dispatch.state == DispatchState::Failed {
            return Ok(true);
        }
        let mut accepted = identity.clone();
        accepted.dispatch_id = Some(dispatch.id.clone());
        record_child_work_accepted(
            &self.store,
            parent_conversation_id,
            goal_id,
            identity.admitted_revision,
            &child_work_identity_payload(&accepted, "child-work-accepted"),
        )?;
        let _ = self.restore_admitted_live_operation(parent_conversation_id, goal_id, &accepted)?;
        Ok(true)
    }

    fn inspect_turn(&self, dispatch_id: &str) -> Option<(String, String)> {
        lock(&self.work_inspect)
            .as_ref()
            .and_then(|inspect| inspect(dispatch_id))
    }

    fn restore_admitted_live_operation(
        &self,
        parent_conversation_id: &str,
        goal_id: &str,
        identity: &ChildWorkIdentity,
    ) -> Result<bool, ContinuityFailure> {
        let Ok(binding) = self.child_binding(parent_conversation_id, goal_id) else {
            return Ok(true);
        };
        if identity.child_conversation_id != binding.child_conversation_id
            || identity.membership_id != binding.membership_id
            || identity.goal_id != goal_id
            || identity.parent_conversation_id != parent_conversation_id
        {
            return Ok(true);
        }
        let Some(accepted) = read_child_work_accepted(
            &self.store,
            parent_conversation_id,
            goal_id,
            identity.admitted_revision,
        )?
        else {
            return Ok(true);
        };
        let Some(accepted_identity) =
            child_work_identity_from_payload(&accepted, parent_conversation_id, goal_id)
        else {
            return Ok(true);
        };
        if !identities_match(identity, &accepted_identity) {
            return Ok(true);
        }
        let Some(dispatch_id) = accepted_identity
            .dispatch_id
            .as_deref()
            .filter(|value| !value.is_empty() && *value != "turn:accepted")
        else {
            return Ok(true);
        };
        let relation = self.store.relation_for_goal(goal_id)?;
        let _ = self.bind_child_runtime_from_relation_at(
            parent_conversation_id,
            &relation,
            identity.work_generation,
        );
        match self.inspect_turn(dispatch_id) {
            Some((_, native)) if !native.trim().is_empty() => {
                if !self.bind_live_child_control(
                    &binding,
                    goal_id,
                    identity.work_generation,
                    dispatch_id,
                    &native,
                ) {
                    return Ok(true);
                }
                let mut live = accepted_identity.clone();
                live.native_turn_id = Some(native);
                record_child_work_live(
                    &self.store,
                    parent_conversation_id,
                    goal_id,
                    &child_work_identity_payload(&live, "child-work-live"),
                )?;
                if !child_work_started(
                    &self.store,
                    parent_conversation_id,
                    goal_id,
                    identity.admitted_revision,
                )? {
                    record_child_work_started(
                        &self.store,
                        parent_conversation_id,
                        goal_id,
                        identity.admitted_revision,
                    )?;
                }
                Ok(true)
            }
            _ => {
                let Some(dispatch) = self
                    .store
                    .dispatch_record(dispatch_id)
                    .map_err(|_| source_unavailable())?
                else {
                    return Ok(true);
                };
                match dispatch.state {
                    DispatchState::Completed | DispatchState::Cancelled => {
                        clear_child_work_live(&self.store, parent_conversation_id, goal_id)?;
                        Ok(false)
                    }
                    DispatchState::Failed => {
                        clear_child_work_live(&self.store, parent_conversation_id, goal_id)?;
                        Ok(true)
                    }
                    DispatchState::Accepted
                    | DispatchState::Running
                    | DispatchState::CancelRequested => {
                        if !child_work_started(
                            &self.store,
                            parent_conversation_id,
                            goal_id,
                            identity.admitted_revision,
                        )? {
                            record_child_work_started(
                                &self.store,
                                parent_conversation_id,
                                goal_id,
                                identity.admitted_revision,
                            )?;
                        }
                        record_child_work_live(
                            &self.store,
                            parent_conversation_id,
                            goal_id,
                            &child_work_identity_payload(&accepted_identity, "child-work-live"),
                        )?;
                        Ok(true)
                    }
                }
            }
        }
    }

    pub fn steer_admitted_child_follow_up(
        &self,
        conversation_id: &str,
        membership_id: &str,
        turn_handle: &str,
        text: &str,
    ) -> ChildControlDisposition {
        self.control_admitted_child(
            conversation_id,
            membership_id,
            turn_handle,
            ChildControlKind::Steer(text),
        )
    }

    pub fn cancel_admitted_child_turn(
        &self,
        conversation_id: &str,
        membership_id: &str,
        turn_handle: &str,
    ) -> ChildControlDisposition {
        self.control_admitted_child(
            conversation_id,
            membership_id,
            turn_handle,
            ChildControlKind::Cancel,
        )
    }

    fn control_admitted_child(
        &self,
        conversation_id: &str,
        membership_id: &str,
        turn_handle: &str,
        kind: ChildControlKind<'_>,
    ) -> ChildControlDisposition {
        let Some(relation) = read_relation_for_child(&self.store, conversation_id)
            .ok()
            .flatten()
        else {
            return ChildControlDisposition::Ordinary;
        };
        if read_goal(&self.store, &relation.goal_id)
            .ok()
            .flatten()
            .is_none()
        {
            return ChildControlDisposition::Unavailable;
        };
        let Ok(binding) = self.child_binding(&relation.parent_conversation_id, &relation.goal_id)
        else {
            return ChildControlDisposition::Unavailable;
        };
        if binding.membership_id != membership_id
            || binding.child_conversation_id != conversation_id
        {
            return ChildControlDisposition::Conflict;
        }
        let Some(identity) = self
            .live_child_work_identity(&relation.parent_conversation_id, &relation.goal_id)
            .ok()
            .flatten()
        else {
            return ChildControlDisposition::Unavailable;
        };
        if identity.child_conversation_id != conversation_id
            || identity.membership_id != membership_id
        {
            return ChildControlDisposition::Conflict;
        }
        if identity
            .dispatch_id
            .as_deref()
            .is_some_and(|dispatch_id| dispatch_id != turn_handle)
        {
            return ChildControlDisposition::Conflict;
        }
        let generation = identity.work_generation;
        let Some(runtime) = self.work_runtime(
            &binding.child_conversation_id,
            &relation.goal_id,
            &binding.membership_id,
            generation,
        ) else {
            return ChildControlDisposition::Unavailable;
        };
        if matches!(kind, ChildControlKind::Cancel) && runtime.family() == ProtocolFamily::Pi {
            return ChildControlDisposition::Unavailable;
        }
        let key = NativeWorkContextKey {
            conversation_id: binding.child_conversation_id.clone(),
            membership_id: binding.membership_id.clone(),
            matter_id: relation.goal_id.clone(),
            generation: generation.max(1),
        };
        let Some((host_handle, native_turn)) = runtime.live_control(&key).ok().flatten() else {
            return ChildControlDisposition::Unavailable;
        };
        if host_handle != turn_handle {
            return ChildControlDisposition::Conflict;
        }
        if native_turn.trim().is_empty() {
            return ChildControlDisposition::Unavailable;
        }
        let request = match kind {
            ChildControlKind::Steer(text) => {
                NativeControlRequest::steer(key, text, host_handle.clone(), native_turn.clone())
            }
            ChildControlKind::Cancel => {
                NativeControlRequest::cancel(key, host_handle.clone(), native_turn.clone())
            }
        };
        match runtime.admit_live_control(&request) {
            Err(error)
                if error.code
                    == licoup_agent_runtime::work_context::ContinuityFailureCode::IdentityConflict =>
            {
                return ChildControlDisposition::Conflict;
            }
            Err(_) => return ChildControlDisposition::Unavailable,
            Ok(()) => {}
        }
        let child = self.store.get(&binding.child_conversation_id).ok();
        let agent = child
            .as_ref()
            .and_then(|conversation| {
                conversation.memberships.iter().find_map(|membership| {
                    (membership.id == binding.membership_id)
                        .then(|| membership.principal.agent_id.clone())
                        .flatten()
                })
            })
            .unwrap_or_default();
        let params = match kind {
            ChildControlKind::Steer(text) => json!({
                "turnHandle": host_handle,
                "conversationId": binding.child_conversation_id,
                "text": text,
                "agent": agent,
                "turnId": native_turn,
            }),
            ChildControlKind::Cancel => json!({
                "turnHandle": host_handle,
                "conversationId": binding.child_conversation_id,
                "agent": agent,
                "turnId": native_turn,
            }),
        };
        let port = match kind {
            ChildControlKind::Steer(_) => lock(&self.work_steer).clone(),
            ChildControlKind::Cancel => lock(&self.work_cancel).clone(),
        };
        let Some(port) = port else {
            return ChildControlDisposition::Unavailable;
        };
        match port(&params) {
            Ok(receipt)
                if receipt.get("ok").and_then(Value::as_bool) == Some(true)
                    && matches!(
                        receipt.get("status").and_then(Value::as_str),
                        Some("accepted") | Some("cancel_requested") | None
                    ) =>
            {
                ChildControlDisposition::Accepted
            }
            Ok(receipt)
                if matches!(
                    receipt.get("status").and_then(Value::as_str),
                    Some("unsupported" | "no_active_turn" | "session_unavailable")
                ) =>
            {
                ChildControlDisposition::Unavailable
            }
            _ => ChildControlDisposition::Unavailable,
        }
    }

    fn preview_child_settlement(
        &self,
        relation: &ContinuityTaskConversationRelation,
        payload: &Value,
        settlement_id: &str,
    ) -> Result<ChildSettlementOutcome, ContinuityFailure> {
        match self.admitted_child_turn(relation, payload, settlement_id)? {
            None => Ok(ChildSettlementOutcome::Rejected),
            Some(_) if child_settlement_is_unknown(payload) => {
                Ok(ChildSettlementOutcome::PendingUnknown)
            }
            Some(_) => Ok(ChildSettlementOutcome::Retained(
                licoup_conversation::continuity::ContinuitySourceRef {
                    owner_kind: licoup_conversation::continuity::ContinuitySourceOwnerKind::Event,
                    opaque_id: settlement_id.to_owned(),
                    part_id: None,
                    span: None,
                    source_revision: 0,
                    digest: format!("event:{settlement_id}"),
                    visibility_scope: ContinuityVisibilityScope::Conversation,
                    validity: ContinuitySourceValidity::Current,
                },
            )),
        }
    }

    fn admitted_child_turn(
        &self,
        relation: &ContinuityTaskConversationRelation,
        payload: &Value,
        settlement_id: &str,
    ) -> Result<Option<licoup_conversation::ConversationEvent>, ContinuityFailure> {
        let Some(dispatch) = self
            .store
            .dispatch_record(settlement_id)
            .map_err(|_| source_unavailable())?
        else {
            return Ok(None);
        };
        if dispatch.conversation_id != relation.child_conversation_id
            || dispatch.operation != "send"
        {
            return Ok(None);
        }
        let Some(payload_member) = payload
            .get("membershipId")
            .and_then(Value::as_str)
            .filter(|value| !value.trim().is_empty())
        else {
            return Ok(None);
        };
        if payload_member != dispatch.membership_id {
            return Ok(None);
        }
        let child = self
            .store
            .get(&relation.child_conversation_id)
            .map_err(|_| source_unavailable())?;
        if !active_agent_membership(&child, payload_member) {
            return Ok(None);
        }
        self.store
            .agent_turn_event_for_dispatch(&relation.child_conversation_id, settlement_id)
            .map_err(|_| source_unavailable())
    }

    fn retain_child_settlement_evidence(
        &self,
        relation: &ContinuityTaskConversationRelation,
        payload: &Value,
        settlement_id: &str,
    ) -> Result<ChildSettlementOutcome, ContinuityFailure> {
        let Some(event) = self.admitted_child_turn(relation, payload, settlement_id)? else {
            return Ok(ChildSettlementOutcome::Rejected);
        };
        if child_settlement_is_unknown(payload) {
            return Ok(ChildSettlementOutcome::PendingUnknown);
        }
        let artifact_part = event
            .parts
            .iter()
            .find(|part| part.kind == EventPartKind::Artifact);
        let source = event_source_ref(
            &event.id,
            artifact_part
                .map(|part| part.id.clone())
                .or_else(|| event.parts.first().map(|part| part.id.clone())),
            event.sequence,
        );
        if let (Some(_), Some((contract, progress))) = (
            artifact_part,
            read_goal_bundle(&self.store, &relation.goal_id)?,
        ) {
            let criterion_id = contract
                .criteria
                .first()
                .map(|criterion| criterion.id.clone())
                .unwrap_or_else(|| "settlement".to_owned());
            let already = progress
                .criterion_evidence_refs
                .iter()
                .any(|item| item.source.opaque_id == event.id && item.criterion_id == criterion_id);
            if !already {
                let issuer = event
                    .author_membership_id
                    .clone()
                    .unwrap_or_else(|| dispatch_membership(payload));
                append_criterion_evidence(
                    &self.store,
                    &relation.parent_conversation_id,
                    &relation.goal_id,
                    ContinuityEvidenceRef {
                        source: source.clone(),
                        issuer,
                        subject_version: progress.revision,
                        criterion_id,
                        observed_at: continuity_now_ms(),
                        result: ContinuityEvidenceResult::Unknown,
                        verification_kind: ContinuityVerificationKind::Judgment,
                        scope: ContinuityVisibilityScope::Goal,
                        validity: ContinuitySourceValidity::Current,
                    },
                )?;
            }
        }
        Ok(ChildSettlementOutcome::Retained(source))
    }

    fn apply_wake_review_settlement(
        &self,
        relation: &ContinuityTaskConversationRelation,
        payload: &Value,
    ) -> Result<IngressOutcome, ContinuityFailure> {
        let binding = self.child_binding(&relation.parent_conversation_id, &relation.goal_id)?;
        let recipient = payload
            .get("membershipId")
            .and_then(Value::as_str)
            .filter(|value| !value.trim().is_empty())
            .and_then(|membership| {
                self.store
                    .get(&relation.child_conversation_id)
                    .ok()
                    .filter(|child| active_agent_membership(child, membership))
                    .map(|_| membership.to_owned())
            })
            .unwrap_or_else(|| binding.membership_id.clone());
        let output = settled_turn_output(payload);
        let causation = payload
            .get("causationId")
            .and_then(Value::as_str)
            .unwrap_or("");
        let assembly_event =
            child_local_event(&self.store, &relation.child_conversation_id, causation);
        let subject = read_goal_bundle(&self.store, &relation.goal_id)
            .ok()
            .flatten()
            .map(|(contract, _)| contract.created_event)
            .filter(|source| !source.opaque_id.trim().is_empty());
        let Some(prepared) = self.prepare_child_recipient_assembly(
            &relation.child_conversation_id,
            assembly_event,
            &recipient,
            subject.as_ref(),
        ) else {
            return Ok(IngressOutcome {
                abstained: true,
                unavailable: true,
                ..IngressOutcome::default()
            });
        };
        let mut proposal = proposal_from_turn_output(&prepared.assembly, &output)?;
        self.retain_existing_parent_sources(&relation.parent_conversation_id, &mut proposal);
        restamp_review_onto_parent(&mut proposal, &relation.parent_conversation_id);
        proposal.envelope.request_id = format!(
            "request:review:{}:{}",
            relation.goal_id,
            settlement_identity(payload)
        );
        if proposal_creates_new_advancement(&proposal) {
            let child = self
                .store
                .get(&binding.child_conversation_id)
                .map_err(|_| source_unavailable())?;
            if let Err(err) = self.qualify(
                &child,
                &binding.membership_id,
                RequestKind::AutomaticAdvancement,
            ) {
                if matches!(
                    err.code,
                    ContinuityFailureCode::QualificationUnknown
                        | ContinuityFailureCode::QualificationStale
                ) {
                    return Ok(IngressOutcome {
                        abstained: true,
                        qualification_denied: true,
                        ..IngressOutcome::default()
                    });
                }
                return Err(err);
            }
        }
        proposal = strip_review_unsafe_effects(proposal);
        self.finish_ingress_proposal(
            &relation.parent_conversation_id,
            payload
                .get("causationId")
                .and_then(Value::as_str)
                .unwrap_or(""),
            &binding.membership_id,
            false,
            proposal,
            lock(&self.cognition).invocation_count(),
            false,
        )
    }

    fn retain_existing_parent_sources(
        &self,
        conversation_id: &str,
        proposal: &mut ContinuityInterpretationProposal,
    ) {
        proposal.envelope.source_event_refs.retain(|source| {
            if source.owner_kind
                != licoup_conversation::continuity::ContinuitySourceOwnerKind::Event
            {
                return true;
            }
            self.store
                .event(conversation_id, &source.opaque_id)
                .ok()
                .flatten()
                .is_some()
        });
    }

    fn bind_pending_scripts(&self, event_id: &str) {
        let pending = std::mem::take(&mut *lock(&self.pending_scripts));
        if pending.is_empty() {
            return;
        }
        for mut script in pending {
            script.event_opaque_id = event_id.to_owned();
            for axis in &mut script.axes {
                if axis.source_ref.opaque_id.trim().is_empty() {
                    axis.source_ref.opaque_id = event_id.to_owned();
                }
            }
            self.scripted.insert(script);
        }
        *lock(&self.cognition) = self.scripted.clone();
    }

    fn qualify(
        &self,
        conversation: &Conversation,
        membership_id: &str,
        kind: RequestKind,
    ) -> Result<(), ContinuityFailure> {
        if kind == RequestKind::AutomaticAdvancement {
            if !lock(&self.adoption).automatic_interpretation_allowed() {
                return Err(qualification_unknown_failure());
            }
            if !self
                .responsibility_live_qualification_currently_valid(conversation, membership_id)?
            {
                return Err(qualification_unknown_failure());
            }
        }
        let identity = self.candidate_identity(conversation, membership_id);
        let responsibility = responsibility_id(conversation, membership_id);
        self.invalidate_if_identity_changed(&responsibility, &identity)?;
        let mut record = query_record(&responsibility, identity.clone());
        if self.lookup_is_invalidated(&responsibility, &identity) {
            record.revoked = true;
        }
        lock(&self.qualification).admit_execution(&record, kind)
    }

    fn candidate_identity(
        &self,
        conversation: &Conversation,
        membership_id: &str,
    ) -> ContinuityCandidateIdentity {
        self.candidate_identity_at(
            conversation,
            membership_id,
            &lock(&self.qualification).policy().policy_revision,
        )
    }

    fn candidate_identity_at(
        &self,
        conversation: &Conversation,
        membership_id: &str,
        policy_revision: &str,
    ) -> ContinuityCandidateIdentity {
        let profile = self.store.membership_profile(membership_id).ok().flatten();
        let agent_id = conversation
            .memberships
            .iter()
            .find(|membership| membership.id == membership_id)
            .and_then(|membership| membership.principal.agent_id.clone());
        let adapter = agent_id
            .as_deref()
            .and_then(protocol_family_for_agent)
            .map(|family| match family {
                ProtocolFamily::Codex => "adapter:codex",
                ProtocolFamily::Pi => "adapter:pi",
            });
        let mut identity = candidate_identity_from_selection(
            membership_id,
            agent_id.as_deref(),
            profile.as_ref(),
            adapter,
            policy_revision,
        );
        if let Ok(Some(corpus)) = load_evaluation_corpus(&self.store, &conversation.id) {
            if !corpus.version_digest.is_empty() {
                identity.dataset_version = corpus.version_digest;
            }
        }
        identity
    }

    fn invalidate_if_identity_changed(
        &self,
        responsibility: &str,
        current: &ContinuityCandidateIdentity,
    ) -> Result<(), ContinuityFailure> {
        let current_key = identity_key(current);
        for (stored_responsibility, stored_key, payload, _) in
            list_qualification_evidence(&self.store)?
        {
            if stored_responsibility != responsibility || stored_key == current_key {
                continue;
            }
            record_qualification_invalidation(&self.store, &stored_responsibility, &stored_key)?;
            let Ok(value) = serde_json::from_str::<Value>(&payload) else {
                continue;
            };
            if let Some(bundle) = bundle_from_stored(
                self,
                &value,
                &lock(&self.qualification).policy().policy_revision,
            ) {
                let _ = lock(&self.qualification).withdraw_for_identity_change(
                    &stored_responsibility,
                    &bundle.identity,
                    current,
                );
            }
        }
        Ok(())
    }

    fn responsibility_live_qualification_currently_valid(
        &self,
        conversation: &Conversation,
        membership_id: &str,
    ) -> Result<bool, ContinuityFailure> {
        let identity = self.candidate_identity(conversation, membership_id);
        let responsibility = responsibility_id(conversation, membership_id);
        let key = identity_key(&identity);
        let Some((_, _, payload, class)) =
            load_qualification_evidence_for(&self.store, &responsibility, &key)?
        else {
            return Ok(false);
        };
        if class != "live-authorized" {
            return Ok(false);
        }
        let value: Value = serde_json::from_str(&payload).map_err(|_| source_unavailable())?;
        let policy_revision = lock(&self.qualification).policy().policy_revision.clone();
        let Some(bundle) = bundle_from_stored(self, &value, &policy_revision) else {
            return Ok(false);
        };
        let assessment =
            lock(&self.qualification).assess(&bundle.responsibility_id, &bundle.identity)?;
        Ok(assessment.result == ContinuityQualificationResult::Qualified)
    }

    fn lookup_is_invalidated(
        &self,
        responsibility: &str,
        identity: &ContinuityCandidateIdentity,
    ) -> bool {
        let key = identity_key(identity);
        read_qualification_invalidations(&self.store)
            .ok()
            .is_some_and(|rows| {
                rows.iter()
                    .any(|(item, stored)| item == responsibility && stored == &key)
            })
    }
}

fn protocol_family_for_agent(agent_id: &str) -> Option<ProtocolFamily> {
    match adapter_for_agent_public(agent_id)? {
        RuntimeAdapter::Codex => Some(ProtocolFamily::Codex),
        RuntimeAdapter::Pi => Some(ProtocolFamily::Pi),
        _ => None,
    }
}

fn candidate_identity_from_selection(
    membership_id: &str,
    agent_id: Option<&str>,
    profile: Option<&ProfileIntent>,
    adapter: Option<&str>,
    policy_revision: &str,
) -> ContinuityCandidateIdentity {
    let selected = |value: Option<&str>, label: &str| {
        value
            .map(str::trim)
            .filter(|item| !item.is_empty())
            .map(|item| format!("identity:{label}:{item}"))
            .unwrap_or_else(|| format!("identity:unselected-{label}"))
    };
    let skills = profile
        .map(|intent| intent.skill_references.join(","))
        .filter(|value| !value.is_empty());
    let tools = profile.map(|intent| {
        let mut caps = intent.required_capabilities.clone();
        caps.extend(intent.preferred_capabilities.iter().cloned());
        caps.join(",")
    });
    ContinuityCandidateIdentity {
        model_digest: selected(
            profile.and_then(|intent| intent.preferred_model.as_deref()),
            "model",
        ),
        reasoning_digest: selected(
            profile.and_then(|intent| intent.preferred_reasoning_effort.as_deref()),
            "reasoning",
        ),
        prompt_digest: format!(
            "identity:prompt:{}:{}",
            membership_id,
            profile.map(|intent| intent.revision).unwrap_or(0)
        ),
        skill_digest: selected(skills.as_deref(), "skills"),
        context_policy_digest: selected(
            profile.and_then(|intent| intent.preferred_environment.as_deref()),
            "context",
        ),
        tool_contract_digest: selected(tools.as_deref().filter(|value| !value.is_empty()), "tools"),
        adapter_runtime_digest: selected(adapter, "adapter"),
        dataset_version: format!(
            "identity:membership:{membership_id}:{}",
            agent_id.unwrap_or("unselected-agent")
        ),
        policy_revision: policy_revision.to_owned(),
    }
}

fn identity_key(identity: &ContinuityCandidateIdentity) -> String {
    format!(
        "{}|{}|{}|{}|{}|{}|{}|{}|{}",
        identity.model_digest,
        identity.reasoning_digest,
        identity.prompt_digest,
        identity.skill_digest,
        identity.context_policy_digest,
        identity.tool_contract_digest,
        identity.adapter_runtime_digest,
        identity.dataset_version,
        identity.policy_revision
    )
}

fn responsibility_id(conversation: &Conversation, membership_id: &str) -> String {
    format!("{}:{membership_id}", conversation.id)
}

fn has_valid_next_responsibility(progress: &ContinuityGoalProgress) -> bool {
    match &progress.next_attention {
        Some(ContinuityNextAttention::Wait {
            responsible_party, ..
        }) => !responsible_party.trim().is_empty(),
        Some(ContinuityNextAttention::ActiveExecution { execution_ref }) => {
            !execution_ref.trim().is_empty()
        }
        Some(ContinuityNextAttention::DispatchableStep { step_ref }) => !step_ref.trim().is_empty(),
        None => false,
    }
}

fn bundle_from_stored(
    host: &ContinuityHost,
    value: &Value,
    policy_revision: &str,
) -> Option<EvidenceBundle> {
    let stored_responsibility = value.get("responsibilityId")?.as_str()?.to_owned();
    let identity: ContinuityCandidateIdentity =
        serde_json::from_value(value.get("identity")?.clone()).ok()?;
    let observations: Vec<QualificationObservation> =
        serde_json::from_value(value.get("observations").cloned().unwrap_or(json!([]))).ok()?;
    let stored_class = value
        .get("evidenceClass")
        .and_then(Value::as_str)
        .unwrap_or("synthetic");
    if stored_class == "live-authorized" {
        let provenance: LiveProvenance =
            serde_json::from_value(value.get("provenance")?.clone()).ok()?;
        let admission = if provenance.is_test_evidence() {
            LiveAdmission::admit_test_evidence(identity.clone(), provenance).ok()?
        } else {
            let session = load_evaluation_session(host.store(), &provenance.session_id)
                .ok()
                .flatten()?;
            let authority = resolve_stored_owner_authority(
                host.store(),
                &session.conversation_id,
                &session.owner_membership_id,
            )
            .ok()?;
            if authority.owner_principal_id() != session.owner_principal_id
                || session.session_id != provenance.session_id
                || session.conversation_id != provenance.conversation_id
                || session.owner_membership_id != provenance.source_id
                || session.owner_principal_id != provenance.owner_principal_id
                || session.responsibility_id != stored_responsibility
                || session.identity != identity
            {
                return None;
            }
            let conversation = host.store().get(&session.conversation_id).ok()?;
            let current = host.candidate_identity_at(
                &conversation,
                &session.recipient_membership_id,
                policy_revision,
            );
            if current != identity
                || responsibility_id(&conversation, &session.recipient_membership_id)
                    != stored_responsibility
            {
                return None;
            }
            if provenance.collection_digest.trim().is_empty()
                || provenance.collector_id.trim().is_empty()
                || provenance.collector_id == "collector:synthetic"
            {
                return None;
            }
            let receipt_value = value.get("collectionReceipt").cloned()?;
            let receipt: super::collection::CollectionReceipt =
                serde_json::from_value(receipt_value).ok()?;
            if receipt.digest != provenance.collection_digest
                || receipt.collector_id != provenance.collector_id
                || receipt.session_id != session.session_id
                || receipt.responsibility_id != stored_responsibility
            {
                return None;
            }
            validate_collection_receipt(&receipt, &session, &observations).ok()?;
            LiveAdmission::from_admitted_session(
                current,
                authority,
                session.session_id,
                session.policy_revision,
                provenance.collector_id,
                provenance.collection_digest,
            )
            .ok()?
        };
        return Some(EvidenceBundle {
            responsibility_id: stored_responsibility,
            identity,
            observations,
            evidence_class: EvidenceClass::LiveAuthorized,
            provenance: Some(admission.provenance().clone()),
        });
    }
    Some(EvidenceBundle {
        responsibility_id: stored_responsibility,
        identity,
        observations,
        evidence_class: EvidenceClass::Synthetic,
        provenance: None,
    })
}

fn settled_turn_output(payload: &Value) -> String {
    if let Some(text) = payload
        .get("output")
        .and_then(Value::as_str)
        .or_else(|| payload.get("text").and_then(Value::as_str))
        .or_else(|| payload.get("message").and_then(Value::as_str))
    {
        return text.to_owned();
    }
    if let Some(proposal) = payload.get("interpretationProposal") {
        return proposal.to_string();
    }
    payload.to_string()
}

fn recipient_membership(conversation: &Conversation) -> Option<String> {
    conversation.assistant_membership_id.clone().or_else(|| {
        conversation
            .memberships
            .iter()
            .find(|membership| {
                membership.status == MembershipStatus::Active
                    && membership.principal.kind == PrincipalKind::Agent
            })
            .map(|membership| membership.id.clone())
    })
}

fn local_owner_membership(conversation: &Conversation) -> Option<String> {
    conversation
        .memberships
        .iter()
        .find(|membership| {
            membership.status == MembershipStatus::Active
                && membership.principal.kind == PrincipalKind::Human
        })
        .map(|membership| membership.id.clone())
}

fn task_view_json(
    host: &ContinuityHost,
    relation: &ContinuityTaskConversationRelation,
) -> Option<Value> {
    let progress = read_goal(&host.store, &relation.goal_id).ok().flatten()?;
    serde_json::to_value(json!({
        "relation": relation,
        "progress": progress,
    }))
    .ok()
}

fn attach_err(failure: ContinuityFailure) -> anyhow::Error {
    anyhow!(format!("{:?}", failure.code))
}

fn active_agent_membership(conversation: &Conversation, membership_id: &str) -> bool {
    conversation.memberships.iter().any(|membership| {
        membership.id == membership_id
            && membership.status == MembershipStatus::Active
            && membership.principal.kind == PrincipalKind::Agent
    })
}

fn restamp_review_onto_parent(
    proposal: &mut ContinuityInterpretationProposal,
    parent_conversation_id: &str,
) {
    proposal.envelope.conversation_id = parent_conversation_id.to_owned();
    if let Some(admission) = proposal.task_child_admission.as_mut() {
        admission.parent_conversation_id = parent_conversation_id.to_owned();
    }
}

fn load_settlement_receipt(
    store: &ConversationStore,
    child_conversation_id: &str,
    parent_conversation_id: &str,
    settlement_id: &str,
) -> Option<Value> {
    read_settlement_pending(store, child_conversation_id, settlement_id)
        .ok()
        .flatten()
        .or_else(|| {
            read_settlement_pending(store, parent_conversation_id, settlement_id)
                .ok()
                .flatten()
        })
        .or_else(|| {
            read_settlement_applied(store, child_conversation_id, settlement_id)
                .ok()
                .flatten()
        })
        .or_else(|| {
            read_settlement_applied(store, parent_conversation_id, settlement_id)
                .ok()
                .flatten()
        })
}

fn child_local_event<'a>(
    store: &ConversationStore,
    child_conversation_id: &str,
    event_id: &'a str,
) -> &'a str {
    if event_id.trim().is_empty() {
        return "";
    }
    if store
        .event(child_conversation_id, event_id)
        .ok()
        .flatten()
        .is_some()
    {
        event_id
    } else {
        ""
    }
}

fn admitted_facts_from_assembly(
    store: &ConversationStore,
    assembly: &AssemblySnapshot,
) -> AdmittedRecipientFacts {
    AdmittedRecipientFacts {
        granted_source_texts: collect_admitted_granted_facts(store, assembly)
            .into_iter()
            .map(|(source_id, text)| AdmittedGrantedFact { source_id, text })
            .collect(),
    }
}

fn dispatch_membership(payload: &Value) -> String {
    payload
        .get("membershipId")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned()
}

fn identities_match(expected: &ChildWorkIdentity, actual: &ChildWorkIdentity) -> bool {
    expected.operation_id == actual.operation_id
        && expected.goal_id == actual.goal_id
        && expected.child_conversation_id == actual.child_conversation_id
        && expected.membership_id == actual.membership_id
        && expected.parent_conversation_id == actual.parent_conversation_id
        && expected.admitted_revision == actual.admitted_revision
        && expected.work_generation == actual.work_generation
}

fn idempotency_conflict() -> ContinuityFailure {
    crate::domain::assistant_continuity::cognition::continuity_failure(
        ContinuityFailureCode::IdempotencyConflict,
        licoup_conversation::continuity::ContinuityFailureStage::ContinuityCommit,
    )
}

fn invalid_request() -> ContinuityFailure {
    crate::domain::assistant_continuity::cognition::continuity_failure(
        ContinuityFailureCode::InvalidRequest,
        licoup_conversation::continuity::ContinuityFailureStage::ContinuityAdmission,
    )
}

fn source_unavailable() -> ContinuityFailure {
    crate::domain::assistant_continuity::cognition::continuity_failure(
        ContinuityFailureCode::SourceUnavailable,
        licoup_conversation::continuity::ContinuityFailureStage::ContinuityAdmission,
    )
}

fn qualification_unknown_failure() -> ContinuityFailure {
    crate::domain::assistant_continuity::cognition::continuity_failure(
        ContinuityFailureCode::QualificationUnknown,
        licoup_conversation::continuity::ContinuityFailureStage::ContinuityAdmission,
    )
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn work_runtime_key_from_binding(binding: ChildBinding, generation: i64) -> WorkRuntimeKey {
    WorkRuntimeKey {
        child_conversation_id: binding.child_conversation_id,
        goal_id: binding.source_task_id,
        membership_id: binding.membership_id,
        generation,
    }
}

pub fn live_workspace(
    store: &ConversationStore,
    conversation_id: &str,
    current_event_id: Option<&str>,
) -> FrozenContextStore {
    populate_live_store(store, conversation_id, current_event_id)
}
