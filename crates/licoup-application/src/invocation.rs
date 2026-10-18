//! The invocation envelope: what one tool call is once a trusted source has
//! proved who is asking.
//!
//! An invocation carries the request identity a receipt answers
//! (`requestId`), the operation, the scope it targets, the revision the caller
//! decided against, the caller's replay key, and the authority handle. The
//! handle is the part that matters most: it is assembled by the host from what a
//! trusted source already verified, and it is deliberately neither serializable
//! nor deserializable, so tool arguments, a peer payload or a free-form
//! extension attribute cannot carry — and therefore cannot replace, extend or
//! forge — the authority an invocation runs as.
//!
//! Tool *arguments* are a separate thing from an invocation: they decode into
//! [`crate::ApplicationCommand`], whose fields are all command fields. That
//! separation is what keeps a caller from writing the host's own authority into
//! a request.

use crate::actor::ActorClaim;
use crate::command::{Operation, stable_id};
use crate::failure::{ApplicationFailure, RecoveryAction};
use crate::result::OperationReference;

/// The scope one invocation targets.
///
/// These are the identifiers the domain already verifies — the conversation and
/// the membership — rather than a second scope taxonomy. An installation-wide
/// read such as an inventory or a readiness probe names neither.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct InvocationScope {
    pub conversation_id: Option<String>,
    pub membership_id: Option<String>,
}

impl InvocationScope {
    /// A call that addresses the installation as a whole.
    pub fn installation() -> Self {
        Self::default()
    }

    pub fn conversation(conversation_id: impl Into<String>) -> Self {
        Self {
            conversation_id: Some(conversation_id.into()),
            membership_id: None,
        }
    }

    pub fn with_membership(mut self, membership_id: impl Into<String>) -> Self {
        self.membership_id = Some(membership_id.into());
        self
    }

    /// Whether this scope addresses the installation rather than a caller's
    /// conversation.
    pub fn is_installation_wide(&self) -> bool {
        self.conversation_id.is_none() && self.membership_id.is_none()
    }

    pub fn validate(&self) -> Result<(), ApplicationFailure> {
        if let Some(conversation_id) = &self.conversation_id {
            stable_id("conversation_id", conversation_id)?;
        }
        if let Some(membership_id) = &self.membership_id {
            stable_id("membership_id", membership_id)?;
        }
        Ok(())
    }
}

/// Where the authority for one invocation came from.
///
/// A handle is only ever assembled from a source that already proved the caller,
/// so the source is part of the handle rather than something a request asserts
/// about itself.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AuthoritySource {
    /// The in-process caller: the local owner of this installation.
    LocalProcess,
    /// A transport that verified the caller's own identity.
    VerifiedTransport,
    /// A local interface an authenticated adapter already admitted.
    AdmittedAdapter,
}

/// The authority a trusted source proved for one invocation.
///
/// The claim inside is the same [`ActorClaim`] the facade verifies, so an
/// invocation cannot introduce a second kind of caller. The handle is not
/// serializable and not deserializable on purpose: authority never travels as
/// data, so no payload can supply `principal`, `authorized` or a grant.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AuthorityHandle {
    claim: ActorClaim,
    source: AuthoritySource,
}

impl AuthorityHandle {
    /// Assemble a handle from what a trusted source already verified.
    ///
    /// The caller of this function is the surface that performed the
    /// verification — the transport, or the adapter an authenticated peer
    /// reached — not the request that verification was about.
    pub fn from_verified_source(claim: ActorClaim, source: AuthoritySource) -> Self {
        Self { claim, source }
    }

    /// The claim this invocation runs as.
    pub fn claim(&self) -> &ActorClaim {
        &self.claim
    }

    pub fn source(&self) -> AuthoritySource {
        self.source
    }

    /// Structural validation of the claim. Whether it is *true* stays native.
    pub fn validate(&self) -> Result<(), ApplicationFailure> {
        self.claim.validate().map_err(ApplicationFailure::from)
    }
}

/// The caller's replay reference for one invocation.
///
/// Within the same authorized operation and scope, a repeated key must return
/// the existing reference rather than starting a second effect. The host owns
/// the durable lookup and must also bind the caller and command input.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct IdempotencyReference {
    pub key: String,
}

impl IdempotencyReference {
    pub fn new(key: impl Into<String>) -> Self {
        Self { key: key.into() }
    }

    pub fn key(&self) -> &str {
        self.key.as_str()
    }
}

/// One tool invocation, once a trusted source has proved who is asking.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ToolInvocation {
    /// The identity a receipt answers with.
    pub request_id: String,
    pub operation: Operation,
    pub scope: InvocationScope,
    /// The revision the caller saw when it decided to call, when it named one.
    pub expected_revision: Option<u64>,
    pub idempotency: Option<IdempotencyReference>,
    pub authority: AuthorityHandle,
}

impl ToolInvocation {
    pub fn new(
        request_id: impl Into<String>,
        operation: Operation,
        scope: InvocationScope,
        authority: AuthorityHandle,
    ) -> Self {
        Self {
            request_id: request_id.into(),
            operation,
            scope,
            expected_revision: None,
            idempotency: None,
            authority,
        }
    }

    pub fn with_expected_revision(mut self, revision: u64) -> Self {
        self.expected_revision = Some(revision);
        self
    }

    pub fn with_idempotency(mut self, key: impl Into<String>) -> Self {
        self.idempotency = Some(IdempotencyReference::new(key));
        self
    }

    /// Structural validation, before any port runs.
    pub fn validate(&self) -> Result<(), ApplicationFailure> {
        stable_id("request_id", &self.request_id)?;
        self.scope.validate()?;
        if self.expected_revision == Some(0) {
            return Err(ApplicationFailure::invalid_request("expected_revision"));
        }
        if let Some(idempotency) = &self.idempotency {
            stable_id("idempotency_key", idempotency.key())?;
        }
        self.authority.validate()
    }

    /// Refuse an invocation that was decided against state the host has moved
    /// past.
    ///
    /// The caller re-reads the state and calls again; nothing has happened yet,
    /// so this refusal costs nothing and produces no effect.
    pub fn fence(&self, current_revision: u64) -> Result<(), ApplicationFailure> {
        let Some(expected) = self.expected_revision else {
            return Ok(());
        };
        if expected == current_revision {
            return Ok(());
        }
        Err(
            ApplicationFailure::permanent("stale_revision", "invocation/fence")
                .with_component("invocation")
                .with_field("expected_revision")
                .with_recovery(RecoveryAction::RetryOrReviewRequest)
                .with_presentation_arg("expectedRevision", &expected.to_string())
                .with_presentation_arg("currentRevision", &current_revision.to_string()),
        )
    }

    /// Whether `reference` is already the answer to this invocation's replay key,
    /// in which case it must be returned instead of starting the work again.
    /// The host must first restrict the lookup to this authorized caller/input;
    /// a reference carries no authority and this comparison cannot grant any.
    pub fn replays(&self, reference: &OperationReference) -> bool {
        reference.operation == self.operation.as_str()
            && reference.conversation_id == self.scope.conversation_id
            && reference.membership_id == self.scope.membership_id
            && self.idempotency.as_ref().is_some_and(|idempotency| {
                reference.idempotency_key.as_deref() == Some(idempotency.key())
            })
    }
}
