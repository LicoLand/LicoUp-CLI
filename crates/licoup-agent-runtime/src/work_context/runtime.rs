//! Stateful work-context engine. Implements the frozen port plus inspectable records.

use super::protocol::{HermeticProtocol, NativeProtocolAdapter, ProtocolOutcome};
use super::types::{
    BindingRecord, BindingStatus, CapabilityProfile, ChildBinding, CoordinatorKind,
    ForkInheritance, GoalInference, HandoffRecord, IsolationReview, IsolationVerdict, LateResult,
    NativeAttemptRef, NativeControlRequest, NativeFidelity, OperationKind, ParallelPolicy,
    ProtocolFamily, SafeReason, SourceCheckpoint, TurnExit, WorkContextOperation,
    goal_inference_from_turn, identity_conflict, invalid_request, isolation_unverified,
    reconciliation_required, stale_revision, validate_control_request, validate_key, writer_busy,
};
use super::{
    NativeCapabilitySnapshot, NativeWorkContextFailure, NativeWorkContextKey, NativeWorkContextPort,
};
use std::collections::BTreeMap;
use std::sync::Mutex;

#[derive(Clone, Debug, Eq, PartialEq)]
struct SessionSlot {
    id: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct WriterClaim {
    matter_id: String,
    conversation_id: String,
    membership_id: String,
    generation: i64,
    conflict_key: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct QueuedMatter {
    matter_id: String,
    conversation_id: String,
}

struct LiveControlBinding {
    host_handle: String,
    native_turn_id: String,
}

struct RuntimeInner {
    next_operation: u64,
    next_slot: u64,
    bindings: BTreeMap<BindingIndex, BindingRecord>,
    live_controls: BTreeMap<BindingIndex, LiveControlBinding>,
    writers: BTreeMap<u64, WriterClaim>,
    queue: Vec<QueuedMatter>,
    operations: Vec<WorkContextOperation>,
    handoffs: Vec<HandoffRecord>,
    late_results: Vec<LateResult>,
    safe_log: Vec<SafeReason>,
    attempts: BTreeMap<String, u32>,
    last_failed_resume: Option<WorkContextOperation>,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
struct BindingIndex {
    conversation_id: String,
    membership_id: String,
    matter_id: String,
    generation: i64,
}

impl BindingIndex {
    fn from_key(key: &NativeWorkContextKey) -> Self {
        Self {
            conversation_id: key.conversation_id.clone(),
            membership_id: key.membership_id.clone(),
            matter_id: key.matter_id.clone(),
            generation: key.generation,
        }
    }
}

pub struct WorkContextConfig {
    pub child: ChildBinding,
    pub coordinator: CoordinatorKind,
    pub admitted_memberships: Vec<String>,
    pub knowledge_injected: bool,
}

impl WorkContextConfig {
    pub fn child(child: ChildBinding) -> Self {
        let membership = child.membership_id.clone();
        Self {
            child,
            coordinator: CoordinatorKind::DesignatedAssistant,
            admitted_memberships: vec![membership],
            knowledge_injected: false,
        }
    }

    pub fn with_coordinator(mut self, coordinator: CoordinatorKind) -> Self {
        self.coordinator = coordinator;
        self
    }

    pub fn with_admitted(mut self, memberships: Vec<String>) -> Self {
        self.admitted_memberships = memberships;
        self
    }

    pub fn with_knowledge_injected(mut self, injected: bool) -> Self {
        self.knowledge_injected = injected;
        self
    }
}

pub struct WorkContextRuntime {
    adapter: Box<dyn NativeProtocolAdapter>,
    config: WorkContextConfig,
    inner: Mutex<RuntimeInner>,
}

impl WorkContextRuntime {
    pub fn new(adapter: impl NativeProtocolAdapter + 'static, config: WorkContextConfig) -> Self {
        Self {
            adapter: Box::new(adapter),
            config,
            inner: Mutex::new(RuntimeInner {
                next_operation: 1,
                next_slot: 1,
                bindings: BTreeMap::new(),
                live_controls: BTreeMap::new(),
                writers: BTreeMap::new(),
                queue: Vec::new(),
                operations: Vec::new(),
                handoffs: Vec::new(),
                late_results: Vec::new(),
                safe_log: Vec::new(),
                attempts: BTreeMap::new(),
                last_failed_resume: None,
            }),
        }
    }

    pub fn hermetic_codex(profile: CapabilityProfile, config: WorkContextConfig) -> Self {
        let knowledge = config.knowledge_injected;
        Self::new(
            HermeticProtocol::codex(profile).with_knowledge_injected(knowledge),
            config,
        )
    }

    pub fn hermetic_pi(profile: CapabilityProfile, config: WorkContextConfig) -> Self {
        let knowledge = config.knowledge_injected;
        Self::new(
            HermeticProtocol::pi(profile).with_knowledge_injected(knowledge),
            config,
        )
    }

    pub fn from_hermetic(protocol: HermeticProtocol, config: WorkContextConfig) -> Self {
        Self::new(protocol, config)
    }

    pub fn family(&self) -> ProtocolFamily {
        self.adapter.family()
    }

    pub fn isolation_review(&self) -> IsolationReview {
        self.adapter.isolation()
    }

    pub fn fidelity(&self) -> NativeFidelity {
        let baseline = NativeFidelity::for_child(&self.config.child, self.adapter.family());
        self.adapter.fidelity(&baseline)
    }

    pub fn child_binding(&self) -> &ChildBinding {
        &self.config.child
    }

    pub fn coordinator(&self) -> CoordinatorKind {
        self.config.coordinator
    }

    pub fn operations(&self) -> Result<Vec<WorkContextOperation>, NativeWorkContextFailure> {
        Ok(self.lock()?.operations.clone())
    }

    pub fn handoffs(&self) -> Result<Vec<HandoffRecord>, NativeWorkContextFailure> {
        Ok(self.lock()?.handoffs.clone())
    }

    pub fn bindings(&self) -> Result<Vec<BindingRecord>, NativeWorkContextFailure> {
        Ok(self.lock()?.bindings.values().cloned().collect())
    }

    pub fn safe_log(&self) -> Result<Vec<SafeReason>, NativeWorkContextFailure> {
        Ok(self.lock()?.safe_log.clone())
    }

    pub fn queued_matters(&self) -> Result<Vec<String>, NativeWorkContextFailure> {
        Ok(self
            .lock()?
            .queue
            .iter()
            .map(|item| item.matter_id.clone())
            .collect())
    }

    pub fn admitted_late_results(&self) -> Result<Vec<LateResult>, NativeWorkContextFailure> {
        Ok(self.lock()?.late_results.clone())
    }

    pub fn source_task_id(&self) -> &str {
        &self.config.child.source_task_id
    }

    pub fn release_writer(&self) -> Result<(), NativeWorkContextFailure> {
        let mut inner = self.lock()?;
        inner.writers.clear();
        Ok(())
    }

    pub fn pin_attempt(
        &self,
        key: &NativeWorkContextKey,
    ) -> Result<NativeAttemptRef, NativeWorkContextFailure> {
        self.admit_key(key)?;
        let mut inner = self.lock()?;
        let attempts = inner.attempts.entry(key.matter_id.clone()).or_insert(0);
        *attempts += 1;
        Ok(NativeAttemptRef {
            conversation_id: key.conversation_id.clone(),
            membership_id: key.membership_id.clone(),
            matter_id: key.matter_id.clone(),
            generation: key.generation,
            attempt: *attempts,
        })
    }

    pub fn admit_late_result(&self, result: LateResult) -> Result<(), NativeWorkContextFailure> {
        self.admit_conversation(&result.source_conversation_id)?;
        if result.source_conversation_id != result.attempt.conversation_id
            || result.source_matter_id != result.attempt.matter_id
            || self
                .config
                .child
                .is_parent_or_foreign(&result.source_conversation_id)
        {
            self.record_safe(SafeReason::IdentityConflict)?;
            return Err(identity_conflict());
        }
        let mut inner = self.lock()?;
        inner.late_results.push(result);
        Ok(())
    }

    pub fn turn_exit_goal_inference(&self, exit: TurnExit) -> GoalInference {
        goal_inference_from_turn(exit)
    }

    pub fn start_new(&self, key: &NativeWorkContextKey) -> Result<i64, NativeWorkContextFailure> {
        start_new_binding(self, key)
    }

    /// Record the exact PersistentTurn / native-turn handles for later control.
    /// Empty native turn is allowed until the live turn binds.
    pub fn bind_live_control(
        &self,
        key: &NativeWorkContextKey,
        host_handle: &str,
        native_turn_id: &str,
    ) -> Result<(), NativeWorkContextFailure> {
        self.admit_key(key)?;
        if host_handle.trim().is_empty()
            || host_handle.len() > 128
            || host_handle.chars().any(char::is_control)
            || native_turn_id.len() > 128
            || native_turn_id.chars().any(char::is_control)
        {
            return Err(invalid_request());
        }
        let mut inner = self.lock()?;
        inner.live_controls.insert(
            BindingIndex::from_key(key),
            LiveControlBinding {
                host_handle: host_handle.to_owned(),
                native_turn_id: native_turn_id.to_owned(),
            },
        );
        Ok(())
    }

    pub fn live_control(
        &self,
        key: &NativeWorkContextKey,
    ) -> Result<Option<(String, String)>, NativeWorkContextFailure> {
        let inner = self.lock()?;
        Ok(inner
            .live_controls
            .get(&BindingIndex::from_key(key))
            .map(|binding| (binding.host_handle.clone(), binding.native_turn_id.clone())))
    }

    pub fn admit_live_control(
        &self,
        request: &NativeControlRequest,
    ) -> Result<(), NativeWorkContextFailure> {
        self.admit_control(request)
    }

    fn admit_control(
        &self,
        request: &NativeControlRequest,
    ) -> Result<(), NativeWorkContextFailure> {
        self.admit_key(&request.key)?;
        validate_control_request(request)?;
        let inner = self.lock()?;
        let Some(live) = inner
            .live_controls
            .get(&BindingIndex::from_key(&request.key))
        else {
            return Err(reconciliation_required());
        };
        if live.host_handle != request.host_handle() {
            return Err(identity_conflict());
        }
        if live.native_turn_id.is_empty() {
            return Err(reconciliation_required());
        }
        if live.native_turn_id != request.native_turn_id() {
            return Err(identity_conflict());
        }
        Ok(())
    }

    pub fn fork_with_inheritance(
        &self,
        key: &NativeWorkContextKey,
        inheritance: &ForkInheritance,
    ) -> Result<i64, NativeWorkContextFailure> {
        self.admit_key(key)?;
        if self.adapter.capabilities().fork != super::NativeCapabilitySupport::Supported {
            return self.fail_op(
                key,
                OperationKind::Fork,
                self.adapter.methods().fork,
                super::unsupported_capability(),
            );
        }
        if inheritance.memory == IsolationVerdict::Unknown {
            return self.fail_op(
                key,
                OperationKind::Fork,
                self.adapter.methods().fork,
                isolation_unverified(),
            );
        }
        let outcome = self.adapter.fork(key, inheritance);
        self.apply_generation_op(key, OperationKind::Fork, outcome)
    }

    fn lock(&self) -> Result<std::sync::MutexGuard<'_, RuntimeInner>, NativeWorkContextFailure> {
        self.inner.lock().map_err(|_| reconciliation_required())
    }

    fn record_safe(&self, reason: SafeReason) -> Result<(), NativeWorkContextFailure> {
        self.lock()?.safe_log.push(reason);
        Ok(())
    }

    fn admit_conversation(&self, conversation_id: &str) -> Result<(), NativeWorkContextFailure> {
        if self.config.child.is_parent_or_foreign(conversation_id) {
            self.record_safe(SafeReason::IdentityConflict)?;
            return Err(identity_conflict());
        }
        Ok(())
    }

    fn admit_key(&self, key: &NativeWorkContextKey) -> Result<(), NativeWorkContextFailure> {
        validate_key(key)?;
        if !self.config.child.admits_key(key) {
            self.record_safe(SafeReason::IdentityConflict)?;
            return Err(identity_conflict());
        }
        if !self
            .config
            .admitted_memberships
            .iter()
            .any(|membership| membership == &key.membership_id)
        {
            self.record_safe(SafeReason::IdentityConflict)?;
            return Err(identity_conflict());
        }
        Ok(())
    }

    fn next_operation_id(inner: &mut RuntimeInner) -> String {
        let id = format!("op:{}", inner.next_operation);
        inner.next_operation += 1;
        id
    }

    fn allocate_slot(inner: &mut RuntimeInner) -> SessionSlot {
        let slot = SessionSlot {
            id: inner.next_slot,
        };
        inner.next_slot += 1;
        slot
    }

    fn record_operation(
        inner: &mut RuntimeInner,
        key: &NativeWorkContextKey,
        kind: OperationKind,
        binding_generation: i64,
        checkpoint: Option<SourceCheckpoint>,
        method: &'static str,
        succeeded: bool,
    ) -> WorkContextOperation {
        let operation = WorkContextOperation {
            operation_id: Self::next_operation_id(inner),
            kind,
            generation: key.generation,
            binding_generation,
            source_checkpoint: checkpoint,
            protocol_method: method,
            succeeded,
        };
        inner.operations.push(operation.clone());
        operation
    }

    fn fail_op(
        &self,
        key: &NativeWorkContextKey,
        kind: OperationKind,
        method: &'static str,
        failure: NativeWorkContextFailure,
    ) -> Result<i64, NativeWorkContextFailure> {
        let mut inner = self.lock()?;
        let operation =
            Self::record_operation(&mut inner, key, kind, key.generation, None, method, false);
        if kind == OperationKind::ExactResume {
            inner.last_failed_resume = Some(operation);
        }
        inner.safe_log.push(SafeReason::from_failure(&failure));
        Err(failure)
    }

    fn apply_generation_op(
        &self,
        key: &NativeWorkContextKey,
        kind: OperationKind,
        outcome: ProtocolOutcome,
    ) -> Result<i64, NativeWorkContextFailure> {
        if let Some(failure) = outcome.failure {
            return self.fail_op(key, kind, outcome.method, failure);
        }
        if outcome.effect == super::types::ProtocolEffect::Unknown {
            return self.fail_op(key, kind, outcome.method, reconciliation_required());
        }
        let new_generation = key.generation + 1;
        let fidelity = self.fidelity();
        let child = self.config.child.clone();
        let mut inner = self.lock()?;
        let checkpoint = Some(SourceCheckpoint {
            matter_id: key.matter_id.clone(),
            conversation_id: key.conversation_id.clone(),
            generation: key.generation,
            failed_operation_id: None,
        });
        let operation = Self::record_operation(
            &mut inner,
            key,
            kind,
            new_generation,
            checkpoint.clone(),
            outcome.method,
            true,
        );
        let new_key = NativeWorkContextKey {
            conversation_id: key.conversation_id.clone(),
            membership_id: key.membership_id.clone(),
            matter_id: key.matter_id.clone(),
            generation: new_generation,
        };
        inner.bindings.insert(
            BindingIndex::from_key(&new_key),
            BindingRecord {
                key: new_key,
                operation_id: operation.operation_id,
                binding_generation: new_generation,
                status: BindingStatus::Bound,
                source_checkpoint: checkpoint,
                fidelity,
                child,
            },
        );
        let _ = Self::allocate_slot(&mut inner);
        Ok(new_generation)
    }

    fn occupy_writer(&self, key: &NativeWorkContextKey) -> Result<(), NativeWorkContextFailure> {
        let policy = self.adapter.parallel_policy();
        let isolation = self.adapter.isolation();
        let conflict_key = self.adapter.session_conflict_key(key);
        let mut inner = self.lock()?;
        if self.config.knowledge_injected && !isolation.claims_clean() {
            let occupied_other = inner.writers.values().any(|claim| {
                claim.matter_id != key.matter_id || claim.conversation_id != key.conversation_id
            });
            if occupied_other && policy == ParallelPolicy::HonestQueue {
                inner.safe_log.push(SafeReason::NativeIsolationUnverified);
                inner.safe_log.push(SafeReason::WriterBusy);
                inner.safe_log.push(SafeReason::Queued);
                inner.queue.push(QueuedMatter {
                    matter_id: key.matter_id.clone(),
                    conversation_id: key.conversation_id.clone(),
                });
                Self::record_operation(
                    &mut inner,
                    key,
                    OperationKind::ClaimWriter,
                    key.generation,
                    None,
                    "writer/claim",
                    false,
                );
                return Err(writer_busy());
            }
        }
        if let Some(ref session_key) = conflict_key {
            let session_busy = inner
                .writers
                .values()
                .any(|claim| claim.conflict_key.as_deref() == Some(session_key.as_str()));
            if session_busy {
                inner.safe_log.push(SafeReason::WriterBusy);
                if policy == ParallelPolicy::HonestQueue {
                    inner.safe_log.push(SafeReason::Queued);
                    inner.queue.push(QueuedMatter {
                        matter_id: key.matter_id.clone(),
                        conversation_id: key.conversation_id.clone(),
                    });
                }
                Self::record_operation(
                    &mut inner,
                    key,
                    OperationKind::ClaimWriter,
                    key.generation,
                    None,
                    "writer/claim",
                    false,
                );
                return Err(writer_busy());
            }
        }
        if policy == ParallelPolicy::HonestQueue {
            if let Some(existing) = inner.writers.values().next() {
                if existing.matter_id != key.matter_id
                    || existing.conversation_id != key.conversation_id
                {
                    inner.safe_log.push(SafeReason::WriterBusy);
                    inner.safe_log.push(SafeReason::Queued);
                    inner.queue.push(QueuedMatter {
                        matter_id: key.matter_id.clone(),
                        conversation_id: key.conversation_id.clone(),
                    });
                    Self::record_operation(
                        &mut inner,
                        key,
                        OperationKind::ClaimWriter,
                        key.generation,
                        None,
                        "writer/claim",
                        false,
                    );
                    return Err(writer_busy());
                }
                // Same matter and conversation: already occupied, only one writer per session
                inner.safe_log.push(SafeReason::WriterBusy);
                Self::record_operation(
                    &mut inner,
                    key,
                    OperationKind::ClaimWriter,
                    key.generation,
                    None,
                    "writer/claim",
                    false,
                );
                return Err(writer_busy());
            }
        } else {
            for claim in inner.writers.values() {
                if claim.matter_id == key.matter_id
                    && claim.conversation_id == key.conversation_id
                    && claim.membership_id == key.membership_id
                {
                    inner.safe_log.push(SafeReason::WriterBusy);
                    Self::record_operation(
                        &mut inner,
                        key,
                        OperationKind::ClaimWriter,
                        key.generation,
                        None,
                        "writer/claim",
                        false,
                    );
                    return Err(writer_busy());
                }
            }
        }
        let slot = Self::allocate_slot(&mut inner);
        inner.writers.insert(
            slot.id,
            WriterClaim {
                matter_id: key.matter_id.clone(),
                conversation_id: key.conversation_id.clone(),
                membership_id: key.membership_id.clone(),
                generation: key.generation,
                conflict_key,
            },
        );
        Self::record_operation(
            &mut inner,
            key,
            OperationKind::ClaimWriter,
            key.generation,
            None,
            "writer/claim",
            true,
        );
        Ok(())
    }
}

impl NativeWorkContextPort for WorkContextRuntime {
    fn negotiate(
        &self,
        key: &NativeWorkContextKey,
    ) -> Result<NativeCapabilitySnapshot, NativeWorkContextFailure> {
        self.admit_key(key)?;
        let snapshot = self.adapter.capabilities();
        let _ = super::negotiate::dimensions_are_independent(&snapshot);
        if self.config.knowledge_injected && self.adapter.isolation().claims_clean() {
            return Err(isolation_unverified());
        }
        Ok(snapshot)
    }

    fn exact_resume(&self, key: &NativeWorkContextKey) -> Result<(), NativeWorkContextFailure> {
        resume_exact(self, key)
    }

    fn rehydrate(&self, key: &NativeWorkContextKey) -> Result<i64, NativeWorkContextFailure> {
        rehydrate_explicit(self, key)
    }

    fn fork(&self, key: &NativeWorkContextKey) -> Result<i64, NativeWorkContextFailure> {
        self.fork_with_inheritance(
            key,
            &ForkInheritance::explicit(true, true, true, true, IsolationVerdict::Unknown),
        )
    }

    fn compact(&self, key: &NativeWorkContextKey) -> Result<(), NativeWorkContextFailure> {
        self.admit_key(key)?;
        let outcome = self.adapter.compact(key);
        finish_unit_op(self, key, OperationKind::Compact, outcome)
    }

    fn steer(&self, request: &NativeControlRequest) -> Result<(), NativeWorkContextFailure> {
        self.admit_control(request)?;
        let outcome = self.adapter.steer(request);
        finish_unit_op(self, &request.key, OperationKind::Steer, outcome)
    }

    fn cancel(&self, request: &NativeControlRequest) -> Result<(), NativeWorkContextFailure> {
        self.admit_control(request)?;
        let outcome = self.adapter.cancel(request);
        finish_unit_op(self, &request.key, OperationKind::Cancel, outcome)
    }

    fn claim_writer(&self, key: &NativeWorkContextKey) -> Result<(), NativeWorkContextFailure> {
        self.admit_key(key)?;
        self.occupy_writer(key)
    }
}

pub(super) fn resume_exact(
    runtime: &WorkContextRuntime,
    key: &NativeWorkContextKey,
) -> Result<(), NativeWorkContextFailure> {
    runtime.admit_key(key)?;
    if runtime.adapter.capabilities().exact_resume == super::NativeCapabilitySupport::Unsupported {
        return runtime
            .fail_op(
                key,
                OperationKind::ExactResume,
                runtime.adapter.methods().exact_resume,
                super::unsupported_capability(),
            )
            .map(|_| ());
    }
    let outcome = runtime.adapter.exact_resume(key);
    if let Some(failure) = outcome.failure {
        return runtime
            .fail_op(key, OperationKind::ExactResume, outcome.method, failure)
            .map(|_| ());
    }
    if outcome.effect == super::types::ProtocolEffect::Unknown {
        return runtime
            .fail_op(
                key,
                OperationKind::ExactResume,
                outcome.method,
                reconciliation_required(),
            )
            .map(|_| ());
    }
    let fidelity = runtime.fidelity();
    let child = runtime.config.child.clone();
    let mut inner = runtime.lock()?;
    if inner.bindings.values().any(|binding| {
        binding.key.matter_id == key.matter_id
            && binding.key.conversation_id == key.conversation_id
            && binding.binding_generation != key.generation
            && binding.status == BindingStatus::Bound
    }) {
        inner.safe_log.push(SafeReason::StaleRevision);
        return Err(stale_revision());
    }
    let operation = WorkContextRuntime::record_operation(
        &mut inner,
        key,
        OperationKind::ExactResume,
        key.generation,
        None,
        outcome.method,
        true,
    );
    inner.bindings.insert(
        BindingIndex::from_key(key),
        BindingRecord {
            key: key.clone(),
            operation_id: operation.operation_id,
            binding_generation: key.generation,
            status: BindingStatus::Bound,
            source_checkpoint: None,
            fidelity,
            child,
        },
    );
    let _ = outcome.cache_miss;
    Ok(())
}

pub(super) fn rehydrate_explicit(
    runtime: &WorkContextRuntime,
    key: &NativeWorkContextKey,
) -> Result<i64, NativeWorkContextFailure> {
    runtime.admit_key(key)?;
    if runtime.config.knowledge_injected && runtime.adapter.isolation().claims_clean() {
        return runtime.fail_op(
            key,
            OperationKind::Rehydrate,
            runtime.adapter.methods().start_new,
            isolation_unverified(),
        );
    }
    let outcome = runtime.adapter.start_new(key);
    if let Some(failure) = outcome.failure {
        return runtime.fail_op(key, OperationKind::Rehydrate, outcome.method, failure);
    }
    if outcome.effect != super::types::ProtocolEffect::Applied {
        return runtime.fail_op(
            key,
            OperationKind::Rehydrate,
            outcome.method,
            reconciliation_required(),
        );
    }
    if outcome.method == runtime.adapter.methods().exact_resume {
        return runtime.fail_op(
            key,
            OperationKind::Rehydrate,
            outcome.method,
            identity_conflict(),
        );
    }
    let new_generation = key.generation + 1;
    let fidelity = runtime.fidelity();
    let child = runtime.config.child.clone();
    let mut inner = runtime.lock()?;
    let failed = inner.last_failed_resume.clone();
    let checkpoint = Some(SourceCheckpoint {
        matter_id: key.matter_id.clone(),
        conversation_id: key.conversation_id.clone(),
        generation: key.generation,
        failed_operation_id: failed.as_ref().map(|op| op.operation_id.clone()),
    });
    let operation = WorkContextRuntime::record_operation(
        &mut inner,
        key,
        OperationKind::Rehydrate,
        new_generation,
        checkpoint.clone(),
        outcome.method,
        true,
    );
    if let (Some(failed), Some(checkpoint_value)) = (failed, checkpoint.clone()) {
        inner.handoffs.push(HandoffRecord {
            from_operation_id: failed.operation_id.clone(),
            to_operation_id: operation.operation_id.clone(),
            from_generation: failed.generation,
            to_generation: new_generation,
            source_checkpoint: checkpoint_value,
        });
        if let Some(previous) = inner.bindings.get_mut(&BindingIndex::from_key(key)) {
            previous.status = BindingStatus::Replaced;
        }
    }
    let new_key = NativeWorkContextKey {
        conversation_id: key.conversation_id.clone(),
        membership_id: key.membership_id.clone(),
        matter_id: key.matter_id.clone(),
        generation: new_generation,
    };
    inner.bindings.insert(
        BindingIndex::from_key(&new_key),
        BindingRecord {
            key: new_key,
            operation_id: operation.operation_id,
            binding_generation: new_generation,
            status: BindingStatus::Bound,
            source_checkpoint: checkpoint,
            fidelity,
            child,
        },
    );
    let _ = WorkContextRuntime::allocate_slot(&mut inner);
    Ok(new_generation)
}

pub(super) fn start_new_binding(
    runtime: &WorkContextRuntime,
    key: &NativeWorkContextKey,
) -> Result<i64, NativeWorkContextFailure> {
    runtime.admit_key(key)?;
    if runtime.config.knowledge_injected && !runtime.adapter.isolation().claims_clean() {
        return runtime.fail_op(
            key,
            OperationKind::NewBinding,
            runtime.adapter.methods().start_new,
            isolation_unverified(),
        );
    }
    let outcome = runtime.adapter.start_new(key);
    runtime.apply_generation_op(key, OperationKind::NewBinding, outcome)
}

fn finish_unit_op(
    runtime: &WorkContextRuntime,
    key: &NativeWorkContextKey,
    kind: OperationKind,
    outcome: ProtocolOutcome,
) -> Result<(), NativeWorkContextFailure> {
    if let Some(failure) = outcome.failure {
        return runtime
            .fail_op(key, kind, outcome.method, failure)
            .map(|_| ());
    }
    match outcome.effect {
        super::types::ProtocolEffect::Applied => {
            let mut inner = runtime.lock()?;
            WorkContextRuntime::record_operation(
                &mut inner,
                key,
                kind,
                key.generation,
                None,
                outcome.method,
                true,
            );
            Ok(())
        }
        super::types::ProtocolEffect::None => runtime
            .fail_op(key, kind, outcome.method, invalid_request())
            .map(|_| ()),
        super::types::ProtocolEffect::Unknown => runtime
            .fail_op(key, kind, outcome.method, reconciliation_required())
            .map(|_| ()),
    }
}
