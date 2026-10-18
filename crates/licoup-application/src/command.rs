//! Typed command families.
//!
//! These are the business requests both interfaces share. A CLI invocation and
//! an MCP tool call for the same operation decode into the same value here, so
//! the two interfaces cannot drift in what they ask for — only in how they
//! frame it.
//!
//! Field bounds mirror the ones the existing surfaces already enforce, so a
//! command that reaches a port has already been bounded.

use crate::failure::ApplicationFailure;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Largest stable identifier (conversation, membership, run, dispatch).
pub const MAX_STABLE_ID_BYTES: usize = 256;
/// Largest delegated prompt.
pub const MAX_PROMPT_BYTES: usize = 48 * 1024;
/// Largest conversation search query.
pub const MAX_QUERY_BYTES: usize = 512;
/// Largest working directory accepted.
pub const MAX_WORKING_DIRECTORY_BYTES: usize = 4096;
/// Largest model name accepted.
pub const MAX_MODEL_BYTES: usize = 256;
/// Largest reasoning-effort token accepted.
pub const MAX_REASONING_EFFORT_BYTES: usize = 32;
/// Largest project display name accepted.
pub const MAX_DISPLAY_NAME_BYTES: usize = 256;
/// Largest declared artifact location accepted. The owner applies the same
/// bound, so a location this crate admits is one the owner can consider.
pub const MAX_ARTIFACT_PATH_BYTES: usize = 4096;

/// Which family a command belongs to. Ownership follows this, not the caller.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CommandFamily {
    Assistant,
    Subagent,
    Conversation,
    Project,
}

/// One addressable business operation. The strings are the neutral names the
/// product already publishes on receipts (`subagent.delegate` and friends).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Operation {
    AssistantProfiles,
    WorkflowExecute,
    WorkflowInspect,
    WorkflowCancel,
    SubagentsList,
    SubagentProbe,
    SubagentDelegate,
    SubagentContinue,
    SubagentCancel,
    ConversationList,
    ConversationGet,
    ConversationSearch,
    ConversationExport,
    ConversationImport,
    ProjectRegister,
    ProjectRead,
    ProjectList,
    ProjectImportPreview,
    ProjectImportApply,
    ProjectDeclareDependency,
    ProjectDependencies,
    ProjectUnresolvedArtifacts,
    ProjectBlockedConsumers,
}

impl Operation {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::AssistantProfiles => "assistant.profiles",
            Self::WorkflowExecute => "workflow.execute",
            Self::WorkflowInspect => "workflow.inspect",
            Self::WorkflowCancel => "workflow.cancel",
            Self::SubagentsList => "subagent.list",
            Self::SubagentProbe => "subagent.probe",
            Self::SubagentDelegate => "subagent.delegate",
            Self::SubagentContinue => "subagent.continue",
            Self::SubagentCancel => "subagent.cancel",
            Self::ConversationList => "conversation.list",
            Self::ConversationGet => "conversation.get",
            Self::ConversationSearch => "conversation.search",
            Self::ConversationExport => "conversation.export",
            Self::ConversationImport => "conversation.import",
            Self::ProjectRegister => "project.register",
            Self::ProjectRead => "project.read",
            Self::ProjectList => "project.list",
            Self::ProjectImportPreview => "project.import.preview",
            Self::ProjectImportApply => "project.import.apply",
            Self::ProjectDeclareDependency => "project.declare-dependency",
            Self::ProjectDependencies => "project.dependencies",
            Self::ProjectUnresolvedArtifacts => "project.unresolved-artifacts",
            Self::ProjectBlockedConsumers => "project.blocked-consumers",
        }
    }

    /// Operations that can produce an effect on a provider. These are the ones
    /// where effect certainty matters: a failure after the effect is attempted
    /// must be reconciled rather than blindly retried.
    pub const fn produces_effect(self) -> bool {
        matches!(
            self,
            Self::WorkflowExecute
                | Self::WorkflowCancel
                | Self::SubagentDelegate
                | Self::SubagentContinue
                | Self::SubagentCancel
                | Self::ConversationImport
        )
    }
}

/// The Assistant Profile listing and workflow control family.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(tag = "command", rename_all = "kebab-case")]
pub enum AssistantCommand {
    Profiles {
        conversation_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        filters: Option<Value>,
    },
    WorkflowExecute {
        conversation_id: String,
        membership_id: String,
        workflow: Value,
        #[serde(default)]
        bindings: Value,
        /// Candidate binding filters the admission pass ranks against. The
        /// workflow owner reads them, so a command that dropped them would run
        /// a different admission than the one the caller asked for.
        #[serde(default)]
        filters: Value,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        input: Option<Value>,
        idempotency_key: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        decision: Option<CallbackDecision>,
    },
    WorkflowInspect {
        run_id: String,
    },
    WorkflowCancel {
        run_id: String,
    },
}

impl AssistantCommand {
    pub const fn family() -> CommandFamily {
        CommandFamily::Assistant
    }

    pub const fn operation(&self) -> Operation {
        match self {
            Self::Profiles { .. } => Operation::AssistantProfiles,
            Self::WorkflowExecute { .. } => Operation::WorkflowExecute,
            Self::WorkflowInspect { .. } => Operation::WorkflowInspect,
            Self::WorkflowCancel { .. } => Operation::WorkflowCancel,
        }
    }

    pub fn validate(&self) -> Result<(), ApplicationFailure> {
        match self {
            Self::Profiles {
                conversation_id, ..
            } => stable_id("conversation_id", conversation_id),
            Self::WorkflowExecute {
                conversation_id,
                membership_id,
                workflow,
                idempotency_key,
                decision,
                ..
            } => {
                stable_id("conversation_id", conversation_id)?;
                stable_id("membership_id", membership_id)?;
                stable_id("idempotency_key", idempotency_key)?;
                if !workflow.is_object() {
                    return Err(ApplicationFailure::invalid_request("workflow"));
                }
                if let Some(decision) = decision {
                    decision.validate()?;
                }
                Ok(())
            }
            Self::WorkflowInspect { run_id } | Self::WorkflowCancel { run_id } => {
                stable_id("run_id", run_id)
            }
        }
    }
}

/// The answer a parked callback wait accepts. Kept as its own type so the three
/// accepted decisions cannot be invented at a call site.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum CallbackDecision {
    Advance { state_id: String, state_visit: u64 },
    Return { state_id: String, state_visit: u64 },
    Terminate,
}

impl CallbackDecision {
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Advance { .. } => "advance",
            Self::Return { .. } => "return",
            Self::Terminate => "terminate",
        }
    }

    pub fn validate(&self) -> Result<(), ApplicationFailure> {
        match self {
            Self::Advance {
                state_id,
                state_visit,
            }
            | Self::Return {
                state_id,
                state_visit,
            } => {
                stable_id("callback_state_id", state_id)?;
                if *state_visit == 0 {
                    return Err(ApplicationFailure::invalid_request("callback_state_visit"));
                }
                Ok(())
            }
            Self::Terminate => Ok(()),
        }
    }
}

/// The delegated-work family.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(tag = "command", rename_all = "kebab-case")]
pub enum SubagentCommand {
    List,
    Probe { agent_id: String },
    Delegate(DispatchRequest),
    Continue(DispatchRequest),
    Cancel(CancelRequest),
}

impl SubagentCommand {
    pub const fn family() -> CommandFamily {
        CommandFamily::Subagent
    }

    pub const fn operation(&self) -> Operation {
        match self {
            Self::List => Operation::SubagentsList,
            Self::Probe { .. } => Operation::SubagentProbe,
            Self::Delegate(_) => Operation::SubagentDelegate,
            Self::Continue(_) => Operation::SubagentContinue,
            Self::Cancel(_) => Operation::SubagentCancel,
        }
    }

    pub fn validate(&self) -> Result<(), ApplicationFailure> {
        match self {
            Self::List => Ok(()),
            Self::Probe { agent_id } => provider("agent_id", agent_id),
            Self::Delegate(request) | Self::Continue(request) => request.validate(),
            Self::Cancel(request) => request.validate(),
        }
    }
}

/// One bounded unit of work handed to another Membership.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DispatchRequest {
    /// The conversation the target lives in.
    ///
    /// A membership claim already supplies it and must not name a different
    /// one; the facade rejects that. The in-process local admin has no
    /// conversation of its own, so it names one here — exactly as the existing
    /// surfaces do (`conversationId` on the MCP tool, the desktop's selected
    /// conversation).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub conversation_id: Option<String>,
    /// Exactly one of `membership_id` or `agent_id` addresses the target.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub membership_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_id: Option<String>,
    pub prompt: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning_effort: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub working_directory: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task_type: Option<TaskType>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub timeout_unbounded: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_stdout_bytes: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_stderr_bytes: Option<u64>,
}

impl DispatchRequest {
    pub fn validate(&self) -> Result<(), ApplicationFailure> {
        if let Some(conversation_id) = &self.conversation_id {
            stable_id("conversation_id", conversation_id)?;
        }
        if self.membership_id.is_none() && self.agent_id.is_none() {
            return Err(ApplicationFailure::invalid_request("target"));
        }
        if let Some(membership_id) = &self.membership_id {
            stable_id("membership_id", membership_id)?;
        }
        if let Some(agent_id) = &self.agent_id {
            provider("agent_id", agent_id)?;
        }
        bounded_non_empty("prompt", &self.prompt, MAX_PROMPT_BYTES)?;
        if let Some(model) = &self.model {
            bounded_non_empty("model", model, MAX_MODEL_BYTES)?;
        }
        if let Some(effort) = &self.reasoning_effort {
            bounded_non_empty("reasoning_effort", effort, MAX_REASONING_EFFORT_BYTES)?;
        }
        if let Some(directory) = &self.working_directory {
            bounded_non_empty("working_directory", directory, MAX_WORKING_DIRECTORY_BYTES)?;
            // The platform's own notion of absolute, so a Windows path is not
            // rejected by a Unix-shaped check. This is what the native dispatch
            // path already enforces, and Windows is a supported platform.
            if !std::path::Path::new(directory).is_absolute() {
                return Err(ApplicationFailure::invalid_request("working_directory"));
            }
        }
        Ok(())
    }
}

/// Which lane the delegated work belongs to. Providers use it to pick a model
/// when the caller did not name one.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum TaskType {
    Frontend,
    Backend,
    Retrieval,
    Text,
}

/// Stop the active dispatch of one target.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CancelRequest {
    /// The conversation the target lives in. See [`DispatchRequest`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub conversation_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub membership_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_id: Option<String>,
}

impl CancelRequest {
    pub fn validate(&self) -> Result<(), ApplicationFailure> {
        if let Some(conversation_id) = &self.conversation_id {
            stable_id("conversation_id", conversation_id)?;
        }
        if self.membership_id.is_none() && self.agent_id.is_none() {
            return Err(ApplicationFailure::invalid_request("target"));
        }
        if let Some(membership_id) = &self.membership_id {
            stable_id("membership_id", membership_id)?;
        }
        if let Some(agent_id) = &self.agent_id {
            provider("agent_id", agent_id)?;
        }
        Ok(())
    }
}

/// The Canonical Conversation family.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(tag = "command", rename_all = "kebab-case")]
pub enum ConversationCommand {
    List {
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        include_archived: bool,
    },
    Get {
        conversation_id: String,
    },
    Search(SearchRequest),
    Export(ExportRequest),
    Import(ImportRequest),
}

impl ConversationCommand {
    pub const fn family() -> CommandFamily {
        CommandFamily::Conversation
    }

    pub const fn operation(&self) -> Operation {
        match self {
            Self::List { .. } => Operation::ConversationList,
            Self::Get { .. } => Operation::ConversationGet,
            Self::Search(_) => Operation::ConversationSearch,
            Self::Export(_) => Operation::ConversationExport,
            Self::Import(_) => Operation::ConversationImport,
        }
    }

    pub fn validate(&self) -> Result<(), ApplicationFailure> {
        match self {
            Self::List { .. } => Ok(()),
            Self::Get { conversation_id } => stable_id("conversation_id", conversation_id),
            Self::Search(request) => request.validate(),
            Self::Export(request) => request.validate(),
            Self::Import(request) => request.validate(),
        }
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchRequest {
    pub query: String,
    pub limit: u32,
}

impl SearchRequest {
    pub fn validate(&self) -> Result<(), ApplicationFailure> {
        bounded_non_empty("query", &self.query, MAX_QUERY_BYTES)?;
        if self.limit == 0 {
            return Err(ApplicationFailure::invalid_request("limit"));
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExportRequest {
    pub path: String,
    pub conversation_ids: Vec<String>,
}

impl ExportRequest {
    pub fn validate(&self) -> Result<(), ApplicationFailure> {
        if self.path.trim().is_empty() || self.path.contains('\0') {
            return Err(ApplicationFailure::invalid_request("path"));
        }
        if self.conversation_ids.is_empty() {
            return Err(ApplicationFailure::invalid_request("conversation_ids"));
        }
        for conversation_id in &self.conversation_ids {
            stable_id("conversation_id", conversation_id)?;
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportRequest {
    pub path: String,
}

impl ImportRequest {
    pub fn validate(&self) -> Result<(), ApplicationFailure> {
        if self.path.trim().is_empty() || self.path.contains('\0') {
            return Err(ApplicationFailure::invalid_request("path"));
        }
        Ok(())
    }
}

/// The authorized-project family: registration, one read, the listing, the
/// explicit plan import, and the declared artifact inputs a project's work
/// items take from other work items.
///
/// The registration carries every identity the caller declares — project,
/// workspace, plan, authorized root, and the authority reference it registers
/// under. None of it is derived here: this crate bounds the request so a
/// malformed one never reaches the owner, and the owner decides identity,
/// authority and durability.
///
/// The import carries one canonical plan document as an opaque JSON value. This
/// crate is protocol-neutral and does not own the document's schema: the project
/// owner parses it once, resolves its own references and returns every
/// diagnostic before any effect, and this layer only bounds the request's shape.
///
/// The dependency commands carry a declaration, never a resolution: a caller
/// names the consumer work item and the result it takes, and the owner answers
/// with the explicit state of that reference. The three read commands are the
/// queries the declaration model exists for — the declared inputs, the ones
/// whose result is not materialized, and the consumers a blocked producer
/// actually blocks.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(tag = "command", rename_all = "kebab-case")]
pub enum ProjectCommand {
    Register(ProjectRegistrationRequest),
    Read {
        project_id: String,
    },
    List,
    /// What one canonical plan document would change, before it changes it.
    ImportPreview {
        document: Value,
    },
    /// Apply one canonical plan document, expecting the revision it previewed.
    ImportApply {
        document: Value,
        /// The source revision the caller last saw. The owner refuses a value
        /// that is not current, so a concurrent import is never overwritten.
        expected_revision: u64,
    },
    DeclareDependency(DependencyDeclarationRequest),
    Dependencies {
        project_id: String,
    },
    UnresolvedArtifacts {
        project_id: String,
    },
    BlockedConsumers {
        project_id: String,
        work_item_id: String,
    },
}

impl ProjectCommand {
    pub const fn family() -> CommandFamily {
        CommandFamily::Project
    }

    pub const fn operation(&self) -> Operation {
        match self {
            Self::Register(_) => Operation::ProjectRegister,
            Self::Read { .. } => Operation::ProjectRead,
            Self::List => Operation::ProjectList,
            Self::ImportPreview { .. } => Operation::ProjectImportPreview,
            Self::ImportApply { .. } => Operation::ProjectImportApply,
            Self::DeclareDependency(_) => Operation::ProjectDeclareDependency,
            Self::Dependencies { .. } => Operation::ProjectDependencies,
            Self::UnresolvedArtifacts { .. } => Operation::ProjectUnresolvedArtifacts,
            Self::BlockedConsumers { .. } => Operation::ProjectBlockedConsumers,
        }
    }

    pub fn validate(&self) -> Result<(), ApplicationFailure> {
        match self {
            Self::Register(request) => request.validate(),
            Self::Read { project_id } => stable_id("project_id", project_id),
            Self::List => Ok(()),
            Self::ImportPreview { document } | Self::ImportApply { document, .. } => {
                plan_document(document)
            }
            Self::DeclareDependency(request) => request.validate(),
            Self::Dependencies { project_id } | Self::UnresolvedArtifacts { project_id } => {
                stable_id("project_id", project_id)
            }
            Self::BlockedConsumers {
                project_id,
                work_item_id,
            } => {
                stable_id("project_id", project_id)?;
                stable_id("work_item_id", work_item_id)
            }
        }
    }
}

/// Bound one carried plan document to the shape this layer can judge.
///
/// Whether the document is canonical is the project owner's decision, and it
/// answers with every diagnostic before any effect. What this layer refuses is a
/// request that carries something other than the one document, so a malformed
/// envelope never reaches the owner.
fn plan_document(document: &Value) -> Result<(), ApplicationFailure> {
    if document.is_object() {
        Ok(())
    } else {
        Err(ApplicationFailure::invalid_request("document"))
    }
}

/// One explicit registration request, in the names the owner reads.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProjectRegistrationRequest {
    /// The declared project identity. Never resolved from the authorized root.
    pub project_id: String,
    /// The name a person reads.
    pub display_name: String,
    /// The declared absolute root this project is authorized to operate in.
    pub authorized_root: String,
    /// Which existing authority owner the reference points into.
    pub authority_kind: String,
    /// The reference into that owner. A reference, never a credential.
    pub authority_reference: String,
    /// The workspace identity this project belongs to.
    pub workspace_id: String,
    /// The plan identity carried by this registration.
    pub plan_id: String,
}

/// The authority kinds the owner admits.
pub const AUTHORITY_KINDS: &[&str] = &["membership", "role", "grant"];

/// One declared artifact input, in the shape both interfaces send.
///
/// The declaration names the consumer work item and the result it takes. The
/// artifact has exactly two declared shapes and no third that discovers one: a
/// location inside the declaring project's own authorized root, or a work item
/// of another registered project. Nesting the producer inside the artifact is
/// deliberate — an edge cannot name one producer and take the result of another.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DependencyDeclarationRequest {
    /// The project that owns the consumer work item.
    pub project_id: String,
    /// The consumer work item: the one that waits for the result.
    pub work_item_id: String,
    /// The declared result the consumer takes.
    pub artifact: ArtifactInputRequest,
}

impl DependencyDeclarationRequest {
    /// Decode the declared dependency payload one interface received. The wire
    /// shape is the request's own documented fields; a payload of the wrong
    /// shape is one `invalid_request` refusal, never a partially bound request.
    pub fn decode(value: &Value) -> Result<Self, ApplicationFailure> {
        serde_json::from_value(value.clone())
            .map_err(|_| ApplicationFailure::invalid_request("request"))
    }

    pub fn validate(&self) -> Result<(), ApplicationFailure> {
        stable_id("project_id", &self.project_id)?;
        stable_id("work_item_id", &self.work_item_id)?;
        self.artifact.validate()
    }
}

/// The two declared shapes of one artifact reference, bounded at the surface.
///
/// Unknown fields are refused rather than dropped: a payload that carries a
/// credential or a permission set beside the declaration is not a dependency
/// this owner can honour, and silently ignoring the extra field would record a
/// request the caller never made.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(
    tag = "kind",
    rename_all = "kebab-case",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum ArtifactInputRequest {
    /// A declared location inside the declaring project's authorized root,
    /// produced by another work item of the same project.
    Local {
        producer_work_item_id: String,
        path: String,
    },
    /// A result declared by one work item of another registered project.
    ///
    /// The naming project may be the declaring project itself; whether the
    /// reference is authorized then depends only on that project's
    /// registration, exactly as it does for any other project identity.
    CrossProject {
        project_id: String,
        work_item_id: String,
    },
}

impl ArtifactInputRequest {
    pub fn validate(&self) -> Result<(), ApplicationFailure> {
        match self {
            Self::Local {
                producer_work_item_id,
                path,
            } => {
                stable_id("producer_work_item_id", producer_work_item_id)?;
                // Whether the location stays inside the authorized root is the
                // owner's decision, because only the owner holds the declared
                // root; this crate bounds the location's own shape so an
                // oversized or unusable one never reaches that decision.
                bounded_non_empty("path", path, MAX_ARTIFACT_PATH_BYTES)
            }
            Self::CrossProject {
                project_id,
                work_item_id,
            } => {
                stable_id("project_id", project_id)?;
                stable_id("work_item_id", work_item_id)
            }
        }
    }
}

impl ProjectRegistrationRequest {
    /// Decode the declared registration payload one interface received. The
    /// wire shape is the request's own documented fields; a payload of the
    /// wrong shape is one `invalid_request` refusal, never a partially bound
    /// request.
    pub fn decode(value: &Value) -> Result<Self, ApplicationFailure> {
        serde_json::from_value(value.clone())
            .map_err(|_| ApplicationFailure::invalid_request("request"))
    }

    pub fn validate(&self) -> Result<(), ApplicationFailure> {
        stable_id("project_id", &self.project_id)?;
        bounded_non_empty("display_name", &self.display_name, MAX_DISPLAY_NAME_BYTES)?;
        bounded_non_empty(
            "authorized_root",
            &self.authorized_root,
            MAX_WORKING_DIRECTORY_BYTES,
        )?;
        if !std::path::Path::new(&self.authorized_root).is_absolute() {
            return Err(ApplicationFailure::invalid_request("authorized_root"));
        }
        if !AUTHORITY_KINDS.contains(&self.authority_kind.as_str()) {
            return Err(ApplicationFailure::invalid_request("authority_kind"));
        }
        stable_id("authority_reference", &self.authority_reference)?;
        stable_id("workspace_id", &self.workspace_id)?;
        stable_id("plan_id", &self.plan_id)?;
        Ok(())
    }
}

/// Any command either interface can issue.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(tag = "family", rename_all = "kebab-case")]
pub enum ApplicationCommand {
    Assistant(AssistantCommand),
    Subagent(SubagentCommand),
    Conversation(ConversationCommand),
    Project(ProjectCommand),
}

impl ApplicationCommand {
    pub const fn family(&self) -> CommandFamily {
        match self {
            Self::Assistant(_) => CommandFamily::Assistant,
            Self::Subagent(_) => CommandFamily::Subagent,
            Self::Conversation(_) => CommandFamily::Conversation,
            Self::Project(_) => CommandFamily::Project,
        }
    }

    pub const fn operation(&self) -> Operation {
        match self {
            Self::Assistant(command) => command.operation(),
            Self::Subagent(command) => command.operation(),
            Self::Conversation(command) => command.operation(),
            Self::Project(command) => command.operation(),
        }
    }

    /// Decode error codes keep the existing `invalid_request` vocabulary, with
    /// the offending field named through `presentationArgs` on the caller side.
    pub fn decode(value: &Value) -> Result<Self, ApplicationFailure> {
        serde_json::from_value(value.clone())
            .map_err(|_| ApplicationFailure::invalid_request("command"))
    }

    /// Decode a command from the JSON text a machine interface received.
    ///
    /// Malformed JSON and a well-formed payload of the wrong shape are two
    /// different failures, because the caller has to do two different things:
    /// the first is [`ApplicationFailure::invalid_json`] with the
    /// `provide_valid_json` recovery, and the second names the field that is
    /// wrong. Neither turns every other cause into a format error.
    pub fn decode_text(text: &str) -> Result<Self, ApplicationFailure> {
        let value: Value =
            serde_json::from_str(text).map_err(|_| ApplicationFailure::invalid_json("command"))?;
        Self::decode(&value)
    }

    pub fn encode(&self) -> Result<Value, ApplicationFailure> {
        serde_json::to_value(self).map_err(|_| ApplicationFailure::invalid_request("command"))
    }

    /// Structural validation. Runs before any port is reached, so a malformed
    /// command can never produce an effect.
    pub fn validate(&self) -> Result<(), ApplicationFailure> {
        match self {
            Self::Assistant(command) => command.validate(),
            Self::Subagent(command) => command.validate(),
            Self::Conversation(command) => command.validate(),
            Self::Project(command) => command.validate(),
        }
    }

    /// The conversation this command addresses, when it addresses exactly one.
    ///
    /// A membership claim admits exactly one conversation, so the facade uses
    /// this to reject a claim that is bound elsewhere. Commands that are not
    /// conversation-scoped (a listing, a run-scoped inspect) return `None` and
    /// leave binding to native verification.
    pub fn conversation_id(&self) -> Option<&str> {
        match self {
            Self::Assistant(command) => match command {
                AssistantCommand::Profiles {
                    conversation_id, ..
                }
                | AssistantCommand::WorkflowExecute {
                    conversation_id, ..
                } => Some(conversation_id.as_str()),
                AssistantCommand::WorkflowInspect { .. }
                | AssistantCommand::WorkflowCancel { .. } => None,
            },
            // A dispatch addresses a conversation: a membership claim supplies
            // its own and may not name another, while the in-process local admin
            // names one. Either way the facade binds it before verification.
            Self::Subagent(command) => match command {
                SubagentCommand::List | SubagentCommand::Probe { .. } => None,
                SubagentCommand::Delegate(request) | SubagentCommand::Continue(request) => {
                    request.conversation_id.as_deref()
                }
                SubagentCommand::Cancel(request) => request.conversation_id.as_deref(),
            },
            Self::Conversation(command) => match command {
                ConversationCommand::Get { conversation_id } => Some(conversation_id.as_str()),
                ConversationCommand::List { .. }
                | ConversationCommand::Search(_)
                | ConversationCommand::Export(_)
                | ConversationCommand::Import(_) => None,
            },
            // A project identity is not a conversation: the project family
            // addresses one by its own declared identity, so a claim is never
            // bound to a conversation for it.
            Self::Project(_) => None,
        }
    }
}

pub(crate) fn stable_id(field: &'static str, value: &str) -> Result<(), ApplicationFailure> {
    bounded_non_empty(field, value, MAX_STABLE_ID_BYTES)
}

fn provider(field: &'static str, value: &str) -> Result<(), ApplicationFailure> {
    if value.is_empty()
        || value.len() > crate::actor::MAX_PROVIDER_ID_BYTES
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
    {
        return Err(ApplicationFailure::invalid_request(field));
    }
    Ok(())
}

fn bounded_non_empty(
    field: &'static str,
    value: &str,
    max: usize,
) -> Result<(), ApplicationFailure> {
    if value.trim().is_empty() || value.len() > max || value.contains('\0') {
        return Err(ApplicationFailure::invalid_request(field));
    }
    Ok(())
}
