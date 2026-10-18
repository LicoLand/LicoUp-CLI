//! The single business entry both interfaces call.
//!
//! Order matters and is fixed here, once:
//!
//! 1. the claim is structurally valid;
//! 2. the command is structurally valid;
//! 3. the claim is bound to the conversation the command addresses;
//! 4. the claim is verified natively;
//! 5. the family port runs.
//!
//! Steps 1–4 cannot produce an effect, so a malformed or unbound request costs
//! nothing and leaves no state behind. Running them here rather than in each
//! interface is what stops the CLI and the MCP from drifting in the order they
//! check things — or in what they check at all.

use crate::actor::ActorClaim;
use crate::command::ApplicationCommand;
use crate::failure::ApplicationFailure;
use crate::ports::ApplicationPorts;
use crate::result::CommandOutcome;

/// One backend, one entry, one effect.
#[derive(Clone)]
pub struct ApplicationFacade {
    ports: ApplicationPorts,
}

impl ApplicationFacade {
    pub fn new(ports: ApplicationPorts) -> Self {
        Self { ports }
    }

    /// Run one command as one actor.
    pub fn execute(
        &self,
        claim: &ActorClaim,
        command: &ApplicationCommand,
    ) -> Result<CommandOutcome, ApplicationFailure> {
        claim.validate().map_err(ApplicationFailure::from)?;
        command.validate()?;
        if let Some(conversation_id) = command.conversation_id()
            && !claim.admits_conversation(conversation_id)
        {
            return Err(ApplicationFailure::permanent(
                "actor_conversation_mismatch",
                "actor/validate",
            ));
        }
        self.ports.actors.verify(claim)?;
        match command {
            ApplicationCommand::Assistant(command) => self.ports.assistant.execute(claim, command),
            ApplicationCommand::Subagent(command) => self.ports.subagent.execute(claim, command),
            ApplicationCommand::Conversation(command) => {
                self.ports.conversation.execute(claim, command)
            }
            ApplicationCommand::Project(command) => self.ports.project.execute(claim, command),
        }
    }
}
