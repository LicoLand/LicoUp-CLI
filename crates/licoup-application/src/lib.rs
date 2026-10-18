//! Protocol-neutral business entry shared by the CLI and MCP.
//!
//! This crate owns the *shape* of a business request and its outcome: typed
//! command families, a normalized result and failure model, actor claims, and
//! stable operation references. It owns no protocol, no storage, no scheduler,
//! no runtime, and no platform access — the ports below are traits the native
//! application implements.
//!
//! Two rules keep the boundary honest:
//!
//! - A caller (CLI or MCP) translates its own wire shape into a command here and
//!   translates the result back. Business semantics live behind the ports.
//! - Nothing in this crate can observe which caller it is serving. If a
//!   decision needs that knowledge, it belongs in the caller or in the native
//!   verification the caller already performed.
//!
//! The tool surface is described by three more modules that follow the same
//! rule — they publish shapes and rules, never a second copy of the host's own
//! registry: [`extension`] for namespaced identity, capability descriptors and
//! the discovered-versus-required rules, [`invocation`] for what one call is
//! once authority is proved, and [`receipt`] for the machine receipts a tool
//! writes (and for the natural output that is not one).

mod actor;
mod command;
mod extension;
mod facade;
mod failure;
mod invocation;
mod ports;
mod receipt;
mod result;

pub use actor::{ActorClaim, ActorClaimError};
pub use command::{
    AUTHORITY_KINDS, ApplicationCommand, ArtifactInputRequest, AssistantCommand, CallbackDecision,
    CancelRequest, CommandFamily, ConversationCommand, DependencyDeclarationRequest,
    DispatchRequest, ExportRequest, ImportRequest, MAX_ARTIFACT_PATH_BYTES, MAX_DISPLAY_NAME_BYTES,
    MAX_PROMPT_BYTES, MAX_QUERY_BYTES, MAX_STABLE_ID_BYTES, Operation, ProjectCommand,
    ProjectRegistrationRequest, SearchRequest, SubagentCommand, TaskType,
};
pub use extension::{
    AUTHORITY_FIELDS, ActivationMode, AdoptedAttributes, CapabilityDescriptor,
    ContractCompatibility, ContractRange, DeclaredAttribute, DiscoveredCapabilities,
    LifecycleSupport, MAX_IMPLEMENTATION_VERSION_BYTES, MAX_NAMESPACED_NAME_BYTES, QuotaShape,
    Requirement, is_authority_field, is_namespaced, is_semver,
};
pub use facade::ApplicationFacade;
pub use failure::{
    ApplicationFailure, EffectCertainty, FailureNormalization, MAX_PRESENTATION_ARGS,
    MAX_PRESENTATION_KEY_BYTES, MAX_PRESENTATION_VALUE_BYTES, PresentationArgs, RecoveryAction,
};
pub use invocation::{
    AuthorityHandle, AuthoritySource, IdempotencyReference, InvocationScope, ToolInvocation,
};
pub use ports::{
    ActorPort, ApplicationPorts, AssistantPort, ConversationPort, ProjectPort, SubagentPort,
};
pub use receipt::{NaturalOutput, ReceiptKind, ToolReceipt};
pub use result::{CommandOutcome, CommandResolution, OperationReference, OperationState};
