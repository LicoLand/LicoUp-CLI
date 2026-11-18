//! Client session reducer, interaction policy, Review, one-shot effect
//! capabilities, opaque Provider key handles, and visible Agent scope.

mod capability;
mod policy;
mod review;
mod session;

pub use capability::{
    AgentScope, EffectCapability, EffectError, ProviderKeyHandle, SecurityWorkflow,
};
pub use policy::{AuthenticationRequirement, ConfirmationPolicy, OperationFamily, OperationPolicy};
pub use review::{ReviewDecision, ReviewDescriptor, ReviewError, ReviewId};
pub use session::{ClientSession, LockReason, SessionCommand, SessionError};

use capability::CapabilityTable;
use review::ReviewTable;

/// Trusted use-case entry. Flutter cannot supply origin, risk, or policy.
pub struct InteractionAuthority {
    session: ClientSession,
    reviews: ReviewTable,
    capabilities: CapabilityTable,
}

impl InteractionAuthority {
    pub fn new() -> Self {
        Self {
            session: ClientSession::default_unlocked(),
            reviews: ReviewTable::new(),
            capabilities: CapabilityTable::new(),
        }
    }

    pub fn session(&self) -> ClientSession {
        self.session
    }

    pub fn apply(&mut self, command: SessionCommand) -> Result<(), SessionError> {
        let previous = self.session;
        self.session = self.session.reduce(command)?;
        if self.session.invalidates_native_context(previous) {
            self.reviews.invalidate_all();
            self.capabilities.invalidate_all();
        }
        Ok(())
    }

    pub fn policy_for(&self, family: OperationFamily) -> Result<OperationPolicy, SessionError> {
        OperationPolicy::evaluate(self.session, family)
    }

    pub fn open_review(&mut self, descriptor: ReviewDescriptor) -> Result<ReviewId, ReviewError> {
        if self.session.is_locked() {
            return Err(ReviewError::SessionLocked);
        }
        self.reviews.open(descriptor)
    }

    pub fn decide_review(
        &mut self,
        decision: ReviewDecision,
    ) -> Result<ReviewDescriptor, ReviewError> {
        self.reviews.decide(decision)
    }

    pub fn issue_effect(
        &mut self,
        workflow: SecurityWorkflow,
    ) -> Result<EffectCapability, EffectError> {
        if self.session.is_locked() {
            return Err(EffectError::SessionLocked);
        }
        self.capabilities
            .issue(workflow, self.session.security_generation())
    }

    pub fn consume_effect(
        &mut self,
        capability: EffectCapability,
        workflow: SecurityWorkflow,
    ) -> Result<(), EffectError> {
        self.capabilities
            .consume(capability, workflow, self.session.security_generation())
    }
}

impl Default for InteractionAuthority {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::session_policy::review::ReviewDescriptor;

    #[test]
    fn ordinary_chat_is_direct_interaction_while_unlocked() {
        let authority = InteractionAuthority::new();
        let policy = authority
            .policy_for(OperationFamily::Chat)
            .expect("unlocked");
        assert_eq!(policy.confirmation, ConfirmationPolicy::DirectInteraction);
        assert_eq!(
            policy.authentication,
            AuthenticationRequirement::DeviceUnlocked
        );
    }

    #[test]
    fn lock_invalidates_open_review_and_unconsumed_capability() {
        let mut authority = InteractionAuthority::new();
        let review = authority
            .open_review(ReviewDescriptor::contact_identity(
                "contact-1",
                "conversation-1",
            ))
            .expect("review");
        let capability = authority
            .issue_effect(SecurityWorkflow::AppUnlock)
            .expect("issue");
        authority
            .apply(SessionCommand::Lock(LockReason::Explicit))
            .expect("lock");
        assert!(
            authority
                .decide_review(ReviewDecision::approve(review))
                .is_err()
        );
        assert!(
            authority
                .consume_effect(capability, SecurityWorkflow::AppUnlock)
                .is_err()
        );
        assert!(authority.policy_for(OperationFamily::Send).is_err());
    }
}
