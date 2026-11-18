//! One-shot effect capabilities bound to one approved security workflow and
//! the current security generation. Opaque Provider key handles never export
//! key material.

use std::collections::BTreeMap;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SecurityWorkflow {
    AppUnlock,
    ProviderTrust,
    KeyCustody,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct EffectCapability {
    id: u64,
    workflow: SecurityWorkflow,
    security_generation: u64,
}

impl EffectCapability {
    pub const fn workflow(self) -> SecurityWorkflow {
        self.workflow
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct ProviderKeyHandle {
    id: u64,
    generation: u32,
}

impl ProviderKeyHandle {
    #[cfg(test)]
    pub(super) const fn minted(id: u64, generation: u32) -> Self {
        Self { id, generation }
    }

    pub const fn id(self) -> u64 {
        self.id
    }

    pub const fn generation(self) -> u32 {
        self.generation
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EffectError {
    SessionLocked,
    UnknownCapability,
    WorkflowMismatch,
    GenerationMismatch,
    AlreadyConsumed,
}

impl EffectError {
    pub const fn code(self) -> &'static str {
        match self {
            Self::SessionLocked => "effect_session_locked",
            Self::UnknownCapability => "effect_unknown",
            Self::WorkflowMismatch => "effect_workflow_mismatch",
            Self::GenerationMismatch => "effect_generation_mismatch",
            Self::AlreadyConsumed => "effect_already_consumed",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum CapabilityState {
    Issued,
    Consumed,
}

pub(super) struct CapabilityTable {
    next_id: u64,
    #[cfg(test)]
    next_key: u64,
    live: BTreeMap<u64, (EffectCapability, CapabilityState)>,
}

impl CapabilityTable {
    pub(super) fn new() -> Self {
        Self {
            next_id: 1,
            #[cfg(test)]
            next_key: 1,
            live: BTreeMap::new(),
        }
    }

    pub(super) fn issue(
        &mut self,
        workflow: SecurityWorkflow,
        security_generation: u64,
    ) -> Result<EffectCapability, EffectError> {
        let capability = EffectCapability {
            id: self.next_id,
            workflow,
            security_generation,
        };
        self.next_id = self.next_id.saturating_add(1);
        self.live
            .insert(capability.id, (capability, CapabilityState::Issued));
        Ok(capability)
    }

    pub(super) fn consume(
        &mut self,
        capability: EffectCapability,
        workflow: SecurityWorkflow,
        security_generation: u64,
    ) -> Result<(), EffectError> {
        let entry = self
            .live
            .get_mut(&capability.id)
            .ok_or(EffectError::UnknownCapability)?;
        if entry.1 == CapabilityState::Consumed {
            return Err(EffectError::AlreadyConsumed);
        }
        if entry.0.workflow != workflow || capability.workflow != workflow {
            return Err(EffectError::WorkflowMismatch);
        }
        if entry.0.security_generation != security_generation
            || capability.security_generation != security_generation
        {
            return Err(EffectError::GenerationMismatch);
        }
        entry.1 = CapabilityState::Consumed;
        Ok(())
    }

    pub(super) fn invalidate_all(&mut self) {
        self.live.clear();
    }

    #[cfg(test)]
    pub(super) fn mint_provider_key(&mut self) -> ProviderKeyHandle {
        let handle = ProviderKeyHandle::minted(self.next_key, 1);
        self.next_key = self.next_key.saturating_add(1);
        handle
    }
}

/// Visible, revocable Agent authorization bound to exact conversation, target,
/// operation family, and content range.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AgentScope {
    pub conversation_id: String,
    pub target_id: String,
    pub operation_family: super::policy::OperationFamily,
    pub content_range: super::review::ContentRange,
    pub expires_at_unix_ms: Option<i64>,
    pub revoked: bool,
}

impl AgentScope {
    pub fn grant(
        conversation_id: impl Into<String>,
        target_id: impl Into<String>,
        operation_family: super::policy::OperationFamily,
        content_range: super::review::ContentRange,
    ) -> Self {
        Self {
            conversation_id: conversation_id.into(),
            target_id: target_id.into(),
            operation_family,
            content_range,
            expires_at_unix_ms: None,
            revoked: false,
        }
    }

    pub fn revoke(&mut self) {
        self.revoked = true;
    }

    pub fn admits(&self, now_unix_ms: i64) -> bool {
        if self.revoked {
            return false;
        }
        self.expires_at_unix_ms
            .map(|expires| now_unix_ms < expires)
            .unwrap_or(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::session_policy::policy::OperationFamily;
    use crate::domain::session_policy::review::ContentRange;

    #[test]
    fn capability_is_single_use_and_generation_bound() {
        let mut table = CapabilityTable::new();
        let issued = table.issue(SecurityWorkflow::KeyCustody, 1).expect("issue");
        table
            .consume(issued, SecurityWorkflow::KeyCustody, 1)
            .expect("consume");
        assert_eq!(
            table
                .consume(issued, SecurityWorkflow::KeyCustody, 1)
                .expect_err("second")
                .code(),
            "effect_already_consumed"
        );
    }

    #[test]
    fn provider_key_handle_is_opaque() {
        let mut table = CapabilityTable::new();
        let handle = table.mint_provider_key();
        assert_eq!(handle.id(), 1);
        assert_eq!(handle.generation(), 1);
    }

    #[test]
    fn agent_scope_is_visible_revocable_and_precise() {
        let mut scope = AgentScope::grant(
            "conversation-1",
            "codex",
            OperationFamily::Chat,
            ContentRange::ConversationMetadata,
        );
        assert!(scope.admits(10));
        scope.revoke();
        assert!(!scope.admits(11));
    }
}
