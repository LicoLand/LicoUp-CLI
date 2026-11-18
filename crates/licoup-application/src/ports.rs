//! Grouped backend ports.
//!
//! Each port is one family of business work, implemented by the native
//! application. The ports exist so both interfaces reach the *same*
//! implementation: a CLI call and an MCP call for one operation arrive at one
//! trait method, which is what makes identical authority and a single effect
//! structural rather than a promise.
//!
//! Verification stays native. [`ActorPort::verify`] is the one place that
//! decides whether a structurally valid claim is true, and the facade calls it
//! once before any family port runs.

use crate::actor::ActorClaim;
use crate::command::{AssistantCommand, ConversationCommand, ProjectCommand, SubagentCommand};
use crate::failure::ApplicationFailure;
use crate::result::CommandOutcome;

/// Decide whether a structurally valid claim is true.
///
/// Implementations own the real check the process can make about *this caller*:
/// the in-process owner is the process itself, and a provider claim must be one
/// the local mesh admits. The facade runs this once per command, before any
/// family port.
///
/// Whether a caller is active in a conversation, and whether it owns the
/// membership it acts as, is a durable question. The domain owner answers it
/// against the conversation store before any effect — the facade cannot, and a
/// port that repeated it would only add a second answer that could disagree.
pub trait ActorPort: Send + Sync {
    fn verify(&self, claim: &ActorClaim) -> Result<(), ApplicationFailure>;
}

/// Assistant profiles and workflow control.
pub trait AssistantPort: Send + Sync {
    fn execute(
        &self,
        claim: &ActorClaim,
        command: &AssistantCommand,
    ) -> Result<CommandOutcome, ApplicationFailure>;
}

/// Delegated work: listing targets, probing readiness, and the dispatch
/// lifecycle.
pub trait SubagentPort: Send + Sync {
    fn execute(
        &self,
        claim: &ActorClaim,
        command: &SubagentCommand,
    ) -> Result<CommandOutcome, ApplicationFailure>;
}

/// Canonical Conversation reads and transfer.
pub trait ConversationPort: Send + Sync {
    fn execute(
        &self,
        claim: &ActorClaim,
        command: &ConversationCommand,
    ) -> Result<CommandOutcome, ApplicationFailure>;
}

/// Authorized project and plan identity registration and reads.
pub trait ProjectPort: Send + Sync {
    fn execute(
        &self,
        claim: &ActorClaim,
        command: &ProjectCommand,
    ) -> Result<CommandOutcome, ApplicationFailure>;
}

/// The complete backend behind the facade.
///
/// There is deliberately no notification port. Completion follow-up is not a
/// request either interface makes: the durable host drives it from the runtime
/// settlement it observes, inside the domain owner that already holds the
/// conversation state. A port here would have no production caller and would
/// only put a second path next to the one the host already takes.
#[derive(Clone)]
pub struct ApplicationPorts {
    pub actors: std::sync::Arc<dyn ActorPort>,
    pub assistant: std::sync::Arc<dyn AssistantPort>,
    pub subagent: std::sync::Arc<dyn SubagentPort>,
    pub conversation: std::sync::Arc<dyn ConversationPort>,
    pub project: std::sync::Arc<dyn ProjectPort>,
}

impl ApplicationPorts {
    pub fn new(
        actors: std::sync::Arc<dyn ActorPort>,
        assistant: std::sync::Arc<dyn AssistantPort>,
        subagent: std::sync::Arc<dyn SubagentPort>,
        conversation: std::sync::Arc<dyn ConversationPort>,
        project: std::sync::Arc<dyn ProjectPort>,
    ) -> Self {
        Self {
            actors,
            assistant,
            subagent,
            conversation,
            project,
        }
    }
}
