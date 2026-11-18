//! Shared work-context records. Private native locations never appear here.

use super::{
    ContinuityDecisionLayer, ContinuityEffectClass, ContinuityFailureCode, ContinuityFailureStage,
    ContinuityRecoveryClass, NativeCapabilitySnapshot, NativeCapabilitySupport,
    NativeWorkContextFailure, NativeWorkContextKey,
};

pub const fn native_binding_lost() -> NativeWorkContextFailure {
    NativeWorkContextFailure {
        code: ContinuityFailureCode::NativeBindingLost,
        stage: ContinuityFailureStage::ContinuityNative,
        recovery: ContinuityRecoveryClass::ReviewOrWait,
        effect_class: ContinuityEffectClass::None,
        decision_layer: ContinuityDecisionLayer::Effects,
        retryable: false,
    }
}

pub const fn writer_busy() -> NativeWorkContextFailure {
    NativeWorkContextFailure {
        code: ContinuityFailureCode::WriterBusy,
        stage: ContinuityFailureStage::ContinuityNative,
        recovery: ContinuityRecoveryClass::ReviewOrWait,
        effect_class: ContinuityEffectClass::None,
        decision_layer: ContinuityDecisionLayer::Effects,
        retryable: true,
    }
}

pub const fn isolation_unverified() -> NativeWorkContextFailure {
    NativeWorkContextFailure {
        code: ContinuityFailureCode::NativeIsolationUnverified,
        stage: ContinuityFailureStage::ContinuityNative,
        recovery: ContinuityRecoveryClass::ReviewOrWait,
        effect_class: ContinuityEffectClass::None,
        decision_layer: ContinuityDecisionLayer::Effects,
        retryable: false,
    }
}

pub const fn identity_conflict() -> NativeWorkContextFailure {
    NativeWorkContextFailure {
        code: ContinuityFailureCode::IdentityConflict,
        stage: ContinuityFailureStage::ContinuityNative,
        recovery: ContinuityRecoveryClass::CorrectRequest,
        effect_class: ContinuityEffectClass::None,
        decision_layer: ContinuityDecisionLayer::Effects,
        retryable: false,
    }
}

pub const fn invalid_request() -> NativeWorkContextFailure {
    NativeWorkContextFailure {
        code: ContinuityFailureCode::InvalidRequest,
        stage: ContinuityFailureStage::ContinuityNative,
        recovery: ContinuityRecoveryClass::CorrectRequest,
        effect_class: ContinuityEffectClass::None,
        decision_layer: ContinuityDecisionLayer::Effects,
        retryable: false,
    }
}

pub const fn reconciliation_required() -> NativeWorkContextFailure {
    NativeWorkContextFailure {
        code: ContinuityFailureCode::ReconciliationRequired,
        stage: ContinuityFailureStage::ContinuityNative,
        recovery: ContinuityRecoveryClass::ReconcileEffects,
        effect_class: ContinuityEffectClass::Unknown,
        decision_layer: ContinuityDecisionLayer::Effects,
        retryable: true,
    }
}

pub const fn stale_revision() -> NativeWorkContextFailure {
    NativeWorkContextFailure {
        code: ContinuityFailureCode::StaleRevision,
        stage: ContinuityFailureStage::ContinuityNative,
        recovery: ContinuityRecoveryClass::CorrectRequest,
        effect_class: ContinuityEffectClass::None,
        decision_layer: ContinuityDecisionLayer::Effects,
        retryable: false,
    }
}

/// Closed log tokens. Never carry native session, path, or credential material.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SafeReason {
    WriterBusy,
    NativeBindingLost,
    NativeIsolationUnverified,
    UnsupportedCapability,
    IdentityConflict,
    ReconciliationRequired,
    Queued,
    EffectUnknown,
    InvalidRequest,
    StaleRevision,
}

impl SafeReason {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::WriterBusy => "writer_busy",
            Self::NativeBindingLost => "native_binding_lost",
            Self::NativeIsolationUnverified => "native_isolation_unverified",
            Self::UnsupportedCapability => "unsupported_capability",
            Self::IdentityConflict => "identity_conflict",
            Self::ReconciliationRequired => "reconciliation_required",
            Self::Queued => "queued",
            Self::EffectUnknown => "effect_unknown",
            Self::InvalidRequest => "invalid_request",
            Self::StaleRevision => "stale_revision",
        }
    }

    pub fn from_failure(failure: &NativeWorkContextFailure) -> Self {
        match failure.code {
            ContinuityFailureCode::WriterBusy => Self::WriterBusy,
            ContinuityFailureCode::NativeBindingLost => Self::NativeBindingLost,
            ContinuityFailureCode::NativeIsolationUnverified => Self::NativeIsolationUnverified,
            ContinuityFailureCode::UnsupportedCapability => Self::UnsupportedCapability,
            ContinuityFailureCode::IdentityConflict => Self::IdentityConflict,
            ContinuityFailureCode::ReconciliationRequired => Self::ReconciliationRequired,
            ContinuityFailureCode::InvalidRequest => Self::InvalidRequest,
            ContinuityFailureCode::StaleRevision => Self::StaleRevision,
            _ => Self::ReconciliationRequired,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProtocolFamily {
    Codex,
    Pi,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CapabilityProfile {
    High,
    Low,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SessionPresence {
    Present,
    Archived,
    Lost,
    Unknown,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProtocolEffect {
    Applied,
    None,
    Unknown,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OperationKind {
    ExactResume,
    Rehydrate,
    NewBinding,
    Fork,
    Compact,
    Steer,
    Cancel,
    ClaimWriter,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BindingStatus {
    Bound,
    Lost,
    Replaced,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum IsolationVerdict {
    Clean,
    Inherited,
    Unknown,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct IsolationReview {
    pub memory: IsolationVerdict,
    pub workspace: IsolationVerdict,
    pub environment_tools: IsolationVerdict,
}

impl IsolationReview {
    pub fn unknown() -> Self {
        Self {
            memory: IsolationVerdict::Unknown,
            workspace: IsolationVerdict::Unknown,
            environment_tools: IsolationVerdict::Unknown,
        }
    }

    /// Unknown memory or environment cannot be reported as a clean isolation.
    pub fn claims_clean(&self) -> bool {
        self.memory == IsolationVerdict::Clean
            && self.workspace == IsolationVerdict::Clean
            && self.environment_tools == IsolationVerdict::Clean
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ForkInheritance {
    pub history: bool,
    pub tools: bool,
    pub environment: bool,
    pub authorization: bool,
    pub memory: IsolationVerdict,
}

impl ForkInheritance {
    pub fn explicit(
        history: bool,
        tools: bool,
        environment: bool,
        authorization: bool,
        memory: IsolationVerdict,
    ) -> Self {
        Self {
            history,
            tools,
            environment,
            authorization,
            memory,
        }
    }

    pub fn is_isolation(&self) -> bool {
        !self.history && self.memory == IsolationVerdict::Clean && self.environment && !self.tools
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NativeSurface {
    pub name: String,
    pub revision: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AuthorIdentity {
    pub membership_id: String,
    pub conversation_id: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NativeFidelity {
    pub tools: NativeSurface,
    pub config: NativeSurface,
    pub skills: NativeSurface,
    pub hooks: NativeSurface,
    pub model: NativeSurface,
    pub approval: NativeSurface,
    pub environment: NativeSurface,
    pub author: AuthorIdentity,
}

impl NativeFidelity {
    pub fn for_child(child: &ChildBinding, family: ProtocolFamily) -> Self {
        let prefix = match family {
            ProtocolFamily::Codex => "codex",
            ProtocolFamily::Pi => "pi",
        };
        Self {
            tools: NativeSurface {
                name: format!("{prefix}.tools"),
                revision: "native".into(),
            },
            config: NativeSurface {
                name: format!("{prefix}.config"),
                revision: "native".into(),
            },
            skills: NativeSurface {
                name: format!("{prefix}.skills"),
                revision: "native".into(),
            },
            hooks: NativeSurface {
                name: format!("{prefix}.hooks"),
                revision: "native".into(),
            },
            model: NativeSurface {
                name: format!("{prefix}.model"),
                revision: "native".into(),
            },
            approval: NativeSurface {
                name: format!("{prefix}.approval"),
                revision: "native".into(),
            },
            environment: NativeSurface {
                name: format!("{prefix}.environment"),
                revision: "native".into(),
            },
            author: AuthorIdentity {
                membership_id: child.membership_id.clone(),
                conversation_id: child.child_conversation_id.clone(),
            },
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CoordinatorKind {
    DesignatedAssistant,
    DelegatedCoordinator,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ChildBinding {
    pub child_conversation_id: String,
    pub membership_id: String,
    pub source_task_id: String,
    pub parent_conversation_id: String,
}

impl ChildBinding {
    pub fn admits_key(&self, key: &NativeWorkContextKey) -> bool {
        key.conversation_id == self.child_conversation_id && key.membership_id == self.membership_id
    }

    pub fn is_parent_or_foreign(&self, conversation_id: &str) -> bool {
        conversation_id == self.parent_conversation_id
            || conversation_id != self.child_conversation_id
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SourceCheckpoint {
    pub matter_id: String,
    pub conversation_id: String,
    pub generation: i64,
    pub failed_operation_id: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WorkContextOperation {
    pub operation_id: String,
    pub kind: OperationKind,
    pub generation: i64,
    pub binding_generation: i64,
    pub source_checkpoint: Option<SourceCheckpoint>,
    pub protocol_method: &'static str,
    pub succeeded: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HandoffRecord {
    pub from_operation_id: String,
    pub to_operation_id: String,
    pub from_generation: i64,
    pub to_generation: i64,
    pub source_checkpoint: SourceCheckpoint,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BindingRecord {
    pub key: NativeWorkContextKey,
    pub operation_id: String,
    pub binding_generation: i64,
    pub status: BindingStatus,
    pub source_checkpoint: Option<SourceCheckpoint>,
    pub fidelity: NativeFidelity,
    pub child: ChildBinding,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NativeAttemptRef {
    pub conversation_id: String,
    pub membership_id: String,
    pub matter_id: String,
    pub generation: i64,
    pub attempt: u32,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LateResult {
    pub source_matter_id: String,
    pub source_conversation_id: String,
    pub source_generation: i64,
    pub attempt: NativeAttemptRef,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TurnExit {
    Completed,
    Failed,
    Cancelled,
    Eof,
    Disconnected,
    Unknown,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum GoalInference {
    NotInferred,
}

pub fn goal_inference_from_turn(_exit: TurnExit) -> GoalInference {
    GoalInference::NotInferred
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ParallelPolicy {
    ParallelSessions,
    HonestQueue,
}

pub fn default_snapshot(
    family: ProtocolFamily,
    profile: CapabilityProfile,
) -> NativeCapabilitySnapshot {
    match (family, profile) {
        (ProtocolFamily::Codex, CapabilityProfile::High) => NativeCapabilitySnapshot {
            exact_resume: NativeCapabilitySupport::Supported,
            fork: NativeCapabilitySupport::Unsupported,
            compact: NativeCapabilitySupport::Unverified,
            steer: NativeCapabilitySupport::Supported,
            cancel: NativeCapabilitySupport::Supported,
            tools: NativeCapabilitySupport::Supported,
            isolated_context: NativeCapabilitySupport::Unverified,
            parallel_contexts: NativeCapabilitySupport::Supported,
        },
        (ProtocolFamily::Codex, CapabilityProfile::Low) => NativeCapabilitySnapshot {
            exact_resume: NativeCapabilitySupport::Supported,
            fork: NativeCapabilitySupport::Unsupported,
            compact: NativeCapabilitySupport::Unsupported,
            steer: NativeCapabilitySupport::Unsupported,
            cancel: NativeCapabilitySupport::Supported,
            tools: NativeCapabilitySupport::Supported,
            isolated_context: NativeCapabilitySupport::Unverified,
            parallel_contexts: NativeCapabilitySupport::Unsupported,
        },
        (ProtocolFamily::Pi, CapabilityProfile::High) => NativeCapabilitySnapshot {
            exact_resume: NativeCapabilitySupport::Supported,
            fork: NativeCapabilitySupport::Unsupported,
            compact: NativeCapabilitySupport::Unverified,
            steer: NativeCapabilitySupport::Supported,
            cancel: NativeCapabilitySupport::Supported,
            tools: NativeCapabilitySupport::Supported,
            isolated_context: NativeCapabilitySupport::Unverified,
            parallel_contexts: NativeCapabilitySupport::Unsupported,
        },
        (ProtocolFamily::Pi, CapabilityProfile::Low) => NativeCapabilitySnapshot {
            exact_resume: NativeCapabilitySupport::TemporarilyUnavailable,
            fork: NativeCapabilitySupport::Unsupported,
            compact: NativeCapabilitySupport::Unsupported,
            steer: NativeCapabilitySupport::Unsupported,
            cancel: NativeCapabilitySupport::Supported,
            tools: NativeCapabilitySupport::Supported,
            isolated_context: NativeCapabilitySupport::Unverified,
            parallel_contexts: NativeCapabilitySupport::Unsupported,
        },
    }
}

pub fn protocol_methods(family: ProtocolFamily) -> ProtocolMethods {
    match family {
        ProtocolFamily::Codex => ProtocolMethods {
            exact_resume: "thread/resume",
            start_new: "thread/start",
            unarchive: "thread/unarchive",
            fork: "thread/fork",
            compact: "thread/compact",
            steer: "turn/steer",
            cancel: "turn/interrupt",
        },
        ProtocolFamily::Pi => ProtocolMethods {
            exact_resume: "session/resume",
            start_new: "session/new",
            unarchive: "session/unarchive",
            fork: "session/fork",
            compact: "session/compact",
            steer: "session/steer",
            cancel: "session/cancel",
        },
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ProtocolMethods {
    pub exact_resume: &'static str,
    pub start_new: &'static str,
    pub unarchive: &'static str,
    pub fork: &'static str,
    pub compact: &'static str,
    pub steer: &'static str,
    pub cancel: &'static str,
}

pub fn validate_key(key: &NativeWorkContextKey) -> Result<(), NativeWorkContextFailure> {
    if key.conversation_id.is_empty()
        || key.membership_id.is_empty()
        || key.matter_id.is_empty()
        || key.generation < 1
        || key.conversation_id.len() > 128
        || key.membership_id.len() > 128
        || key.matter_id.len() > 128
    {
        return Err(invalid_request());
    }
    Ok(())
}

/// Full steer/cancel intent. Session identity is never accepted from the caller.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NativeControlRequest {
    pub key: NativeWorkContextKey,
    pub intent: NativeControlIntent,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum NativeControlIntent {
    Steer {
        content: String,
        host_handle: String,
        native_turn_id: String,
    },
    Cancel {
        host_handle: String,
        native_turn_id: String,
    },
}

impl NativeControlRequest {
    pub fn steer(
        key: NativeWorkContextKey,
        content: impl Into<String>,
        host_handle: impl Into<String>,
        native_turn_id: impl Into<String>,
    ) -> Self {
        Self {
            key,
            intent: NativeControlIntent::Steer {
                content: content.into(),
                host_handle: host_handle.into(),
                native_turn_id: native_turn_id.into(),
            },
        }
    }

    pub fn cancel(
        key: NativeWorkContextKey,
        host_handle: impl Into<String>,
        native_turn_id: impl Into<String>,
    ) -> Self {
        Self {
            key,
            intent: NativeControlIntent::Cancel {
                host_handle: host_handle.into(),
                native_turn_id: native_turn_id.into(),
            },
        }
    }

    pub fn host_handle(&self) -> &str {
        match &self.intent {
            NativeControlIntent::Steer { host_handle, .. }
            | NativeControlIntent::Cancel { host_handle, .. } => host_handle,
        }
    }

    pub fn native_turn_id(&self) -> &str {
        match &self.intent {
            NativeControlIntent::Steer { native_turn_id, .. }
            | NativeControlIntent::Cancel { native_turn_id, .. } => native_turn_id,
        }
    }

    pub fn steer_content(&self) -> Option<&str> {
        match &self.intent {
            NativeControlIntent::Steer { content, .. } => Some(content.as_str()),
            NativeControlIntent::Cancel { .. } => None,
        }
    }
}

fn valid_control_token(value: &str) -> bool {
    let trimmed = value.trim();
    !trimmed.is_empty() && trimmed.len() <= 128 && !trimmed.chars().any(char::is_control)
}

pub fn validate_control_request(
    request: &NativeControlRequest,
) -> Result<(), NativeWorkContextFailure> {
    validate_key(&request.key)?;
    if !valid_control_token(request.host_handle()) || !valid_control_token(request.native_turn_id())
    {
        return Err(invalid_request());
    }
    match &request.intent {
        NativeControlIntent::Steer { content, .. } => {
            let trimmed = content.trim();
            if trimmed.is_empty() || content.len() > 1024 * 1024 {
                return Err(invalid_request());
            }
        }
        NativeControlIntent::Cancel { .. } => {}
    }
    Ok(())
}
