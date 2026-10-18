//! Actor claims: who is asking.
//!
//! This module validates the *shape* of a claim. Whether the claim is true is
//! decided natively — the local owner membership must exist, and an agent
//! membership must be active in the conversation and owned by that provider.
//! Keeping verification out of here is what lets one command run through either
//! interface without the interfaces disagreeing about authority.
//!
//! The two kinds are deliberately different, mirroring the product's existing
//! split:
//!
//! - [`ActorClaim::LocalAdmin`] is the in-process caller. It names the local
//!   human owner membership directly, exactly as the CLI and desktop do today.
//! - [`ActorClaim::Membership`] is a caller acting under a delegation scope. It
//!   carries the provider identity and, when the caller has them, the
//!   conversation/membership binding; the transport is what makes it authentic.
//!   A caller that names no conversation can only reach operations that do not
//!   address one — the domain owner refuses it for every operation that does.

use crate::command::MAX_STABLE_ID_BYTES;
use crate::failure::RecoveryAction;
use serde::{Deserialize, Serialize};

/// The largest provider identity accepted, matching the runtime's own bound.
pub const MAX_PROVIDER_ID_BYTES: usize = 64;

/// A structurally valid claim to act.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum ActorClaim {
    /// A local human owner acting in-process.
    LocalAdmin { owner_membership_id: String },
    /// A caller acting through an authenticated transport, or through a local
    /// interface that an authenticated adapter already admitted.
    ///
    /// The conversation and membership are present only when the caller scope
    /// names them: an inventory or a readiness probe carries neither, while a
    /// dispatch always carries both. This mirrors the caller scope the native
    /// domain already verifies, rather than inventing a binding for a request
    /// that has none.
    Membership {
        provider_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        conversation_id: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        membership_id: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        parent_dispatch_id: Option<String>,
    },
}

impl ActorClaim {
    pub fn local_admin(owner_membership_id: impl Into<String>) -> Self {
        Self::LocalAdmin {
            owner_membership_id: owner_membership_id.into(),
        }
    }

    pub fn membership(
        provider_id: impl Into<String>,
        conversation_id: impl Into<String>,
        membership_id: impl Into<String>,
    ) -> Self {
        Self::Membership {
            provider_id: provider_id.into(),
            conversation_id: Some(conversation_id.into()),
            membership_id: Some(membership_id.into()),
            parent_dispatch_id: None,
        }
    }

    pub fn with_parent_dispatch(mut self, dispatch_id: impl Into<String>) -> Self {
        if let Self::Membership {
            parent_dispatch_id, ..
        } = &mut self
        {
            *parent_dispatch_id = Some(dispatch_id.into());
        }
        self
    }

    /// The conversation this claim is bound to, when it names one.
    pub fn conversation_id(&self) -> Option<&str> {
        match self {
            Self::LocalAdmin { .. } => None,
            Self::Membership {
                conversation_id, ..
            } => conversation_id.as_deref(),
        }
    }

    /// The membership this claim acts as, when it names one.
    pub fn membership_id(&self) -> Option<&str> {
        match self {
            Self::LocalAdmin {
                owner_membership_id,
            } => Some(owner_membership_id),
            Self::Membership { membership_id, .. } => membership_id.as_deref(),
        }
    }

    /// Whether this claim is the in-process local-admin kind.
    pub fn is_local_admin(&self) -> bool {
        matches!(self, Self::LocalAdmin { .. })
    }

    /// Structural validation only. This never decides authority.
    pub fn validate(&self) -> Result<(), ActorClaimError> {
        match self {
            Self::LocalAdmin {
                owner_membership_id,
            } => validate_identifier(owner_membership_id, ActorClaimError::OwnerMembershipInvalid),
            Self::Membership {
                provider_id,
                conversation_id,
                membership_id,
                parent_dispatch_id,
            } => {
                if !valid_provider_id(provider_id) {
                    return Err(ActorClaimError::ProviderInvalid);
                }
                if let Some(conversation_id) = conversation_id {
                    validate_identifier(conversation_id, ActorClaimError::ConversationInvalid)?;
                }
                if let Some(membership_id) = membership_id {
                    validate_identifier(membership_id, ActorClaimError::MembershipInvalid)?;
                }
                if let Some(dispatch_id) = parent_dispatch_id {
                    validate_identifier(dispatch_id, ActorClaimError::ParentDispatchInvalid)?;
                }
                Ok(())
            }
        }
    }

    /// Whether this claim may bind to `conversation_id`.
    ///
    /// A claim that names no conversation is not conversation-scoped — the
    /// local-admin owner is not, and neither is a caller scope that carries no
    /// conversation. Either may address any conversation here, because the
    /// domain owner refuses every operation that addresses one without a bound
    /// caller scope, and it does so before any effect.
    pub fn admits_conversation(&self, conversation_id: &str) -> bool {
        match self.conversation_id() {
            None => true,
            Some(bound) => bound == conversation_id,
        }
    }
}

/// Why a claim is structurally unusable. These are shape errors, not authority
/// errors: a claim that passes still has to be verified natively.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ActorClaimError {
    ProviderInvalid,
    OwnerMembershipInvalid,
    ConversationInvalid,
    MembershipInvalid,
    ParentDispatchInvalid,
}

impl ActorClaimError {
    pub const fn code(self) -> &'static str {
        match self {
            Self::ProviderInvalid => "actor_provider_invalid",
            Self::OwnerMembershipInvalid => "actor_owner_membership_invalid",
            Self::ConversationInvalid => "actor_conversation_invalid",
            Self::MembershipInvalid => "actor_membership_invalid",
            Self::ParentDispatchInvalid => "actor_parent_dispatch_invalid",
        }
    }

    pub const fn stage(self) -> &'static str {
        "actor/validate"
    }
}

impl std::fmt::Display for ActorClaimError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.code())
    }
}

impl std::error::Error for ActorClaimError {}

impl From<ActorClaimError> for crate::failure::ApplicationFailure {
    /// A structurally unusable claim is refused with the claim's own code and
    /// stage, so both interfaces report the same reason for the same shape.
    fn from(error: ActorClaimError) -> Self {
        Self::permanent(error.code(), error.stage()).with_recovery(RecoveryAction::CorrectRequest)
    }
}

fn validate_identifier(value: &str, error: ActorClaimError) -> Result<(), ActorClaimError> {
    if value.is_empty() || value.len() > MAX_STABLE_ID_BYTES || value.contains('\0') {
        return Err(error);
    }
    Ok(())
}

/// Provider identities keep the runtime's existing alphabet, so a claim that
/// passes here cannot carry a value the runtime would reject.
fn valid_provider_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= MAX_PROVIDER_ID_BYTES
        && value
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
}
