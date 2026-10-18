//! The endpoint layer's mapping from the host's dispatch error to the generated
//! client error.
//!
//! `RuntimeAdapterError` is the host's own vocabulary: it is what the adapter
//! registry and adapter execution report, and every crate below the endpoint
//! layer may name it. `ClientError` is the opposite: it is generated from the
//! endpoint's wire schema, and its codes, stages, components and recoveries are
//! the endpoint's presentation contract.
//!
//! The two used to be one: `RuntimeAdapterError::client_error` lived beside the
//! error enum and made the host's own error type depend on the endpoint layer's
//! generated DTOs. That is the upward edge REQ-002 forbids, and it is inverted
//! here rather than moved down, because the *mapping* is the endpoint's
//! behaviour: the host does not know how a refusal is presented on the wire.
//! This module is the endpoint layer's half of that seam and travels with the
//! endpoint crate when `ffi/` extracts.

use licoup_agent_drivers::runtime_adapters::RuntimeAdapterError;

use crate::ffi::generated::client_error::{
    ClientError, ClientErrorCode, ClientErrorComponent, ClientErrorRecovery, ClientErrorStage,
};

/// Project one host dispatch refusal onto the client error contract.
pub fn client_error(error: &RuntimeAdapterError) -> ClientError {
    match error {
        RuntimeAdapterError::AgentIdentifierMissing => ClientError::new(
            ClientErrorCode::AgentIdentifierMissing,
            ClientErrorStage::RequestValidation,
            ClientErrorComponent::RuntimeAdapter,
            false,
            ClientErrorRecovery::CorrectRequest,
        )
        .with_presentation_arg("field", "agent"),
        RuntimeAdapterError::MessageMissing => ClientError::new(
            ClientErrorCode::AgentMessageMissing,
            ClientErrorStage::RequestValidation,
            ClientErrorComponent::RuntimeAdapter,
            false,
            ClientErrorRecovery::CorrectRequest,
        )
        .with_presentation_arg("field", "message"),
        RuntimeAdapterError::LegacyLaunchConfiguration => ClientError::new(
            ClientErrorCode::InvalidRequest,
            ClientErrorStage::RequestValidation,
            ClientErrorComponent::RuntimeAdapter,
            false,
            ClientErrorRecovery::CorrectRequest,
        )
        .with_presentation_arg("field", "launch"),
        RuntimeAdapterError::InvalidRuntimeSetting { field } => ClientError::new(
            ClientErrorCode::InvalidRequest,
            ClientErrorStage::RequestValidation,
            ClientErrorComponent::RuntimeAdapter,
            false,
            ClientErrorRecovery::CorrectRequest,
        )
        .with_presentation_arg("field", *field),
        RuntimeAdapterError::AttachmentUnsupportedForAdapter { agent_label } => ClientError::new(
            ClientErrorCode::AgentRuntimeUnsupported,
            ClientErrorStage::DiscoveryAdapter,
            ClientErrorComponent::RuntimeAdapter,
            false,
            ClientErrorRecovery::SelectSupportedAdapter,
        )
        .with_presentation_arg("agentLabel", agent_label),
        // The generated error contract is unchanged: an unsatisfied
        // capability requirement is the same client-facing condition as an
        // unsupported runtime, and only allowlisted presentation keys may
        // cross the bridge. The typed refusal carries the fact name.
        RuntimeAdapterError::AttachmentUnsupportedForTransport { agent_label } => ClientError::new(
            ClientErrorCode::AgentRuntimeUnsupported,
            ClientErrorStage::DiscoveryAdapter,
            ClientErrorComponent::RuntimeAdapter,
            false,
            ClientErrorRecovery::SelectSupportedAdapter,
        )
        .with_presentation_arg("agentLabel", agent_label),
        RuntimeAdapterError::CapabilityRequirementUnsatisfied { .. } => ClientError::new(
            ClientErrorCode::AgentRuntimeUnsupported,
            ClientErrorStage::DiscoveryAdapter,
            ClientErrorComponent::RuntimeAdapter,
            false,
            ClientErrorRecovery::SelectSupportedAdapter,
        )
        .with_presentation_arg("field", "requiredCapabilities"),
        RuntimeAdapterError::AttachmentListExceeded => ClientError::new(
            ClientErrorCode::AgentMessageInputLimit,
            ClientErrorStage::RequestValidation,
            ClientErrorComponent::RuntimeAdapter,
            false,
            ClientErrorRecovery::CorrectRequest,
        )
        .with_presentation_arg("field", "attachments")
        .with_presentation_arg("limit", "4"),
        RuntimeAdapterError::AttachmentInvalid => ClientError::new(
            ClientErrorCode::InvalidRequest,
            ClientErrorStage::RequestValidation,
            ClientErrorComponent::RuntimeAdapter,
            false,
            ClientErrorRecovery::CorrectRequest,
        )
        .with_presentation_arg("field", "attachments"),
        RuntimeAdapterError::AttachmentRemoteUnsupported => ClientError::new(
            ClientErrorCode::InvalidRequest,
            ClientErrorStage::RequestValidation,
            ClientErrorComponent::RuntimeAdapter,
            false,
            ClientErrorRecovery::CorrectRequest,
        )
        .with_presentation_arg("field", "attachments"),
        RuntimeAdapterError::AttachmentMediaUnsupported => ClientError::new(
            ClientErrorCode::InvalidRequest,
            ClientErrorStage::RequestValidation,
            ClientErrorComponent::RuntimeAdapter,
            false,
            ClientErrorRecovery::CorrectRequest,
        )
        .with_presentation_arg("field", "mediaType"),
        RuntimeAdapterError::AttachmentSymlinkRejected => ClientError::new(
            ClientErrorCode::InvalidRequest,
            ClientErrorStage::RequestValidation,
            ClientErrorComponent::RuntimeAdapter,
            false,
            ClientErrorRecovery::CorrectRequest,
        )
        .with_presentation_arg("field", "attachments"),
        RuntimeAdapterError::AttachmentFileUnavailable => ClientError::new(
            ClientErrorCode::AgentConversationDispatchFailed,
            ClientErrorStage::ConversationDispatch,
            ClientErrorComponent::ConversationRuntime,
            true,
            ClientErrorRecovery::PreserveDraftAndRetry,
        ),
        RuntimeAdapterError::AttachmentSizeLimit => ClientError::new(
            ClientErrorCode::AgentMessageInputLimit,
            ClientErrorStage::RequestValidation,
            ClientErrorComponent::RuntimeAdapter,
            false,
            ClientErrorRecovery::CorrectRequest,
        )
        .with_presentation_arg("field", "attachments"),
        RuntimeAdapterError::AttachmentSignatureMismatch => ClientError::new(
            ClientErrorCode::InvalidRequest,
            ClientErrorStage::RequestValidation,
            ClientErrorComponent::RuntimeAdapter,
            false,
            ClientErrorRecovery::CorrectRequest,
        )
        .with_presentation_arg("field", "mediaType"),
        RuntimeAdapterError::UnsupportedAdapter { agent_label } => ClientError::new(
            ClientErrorCode::AgentRuntimeUnsupported,
            ClientErrorStage::DiscoveryAdapter,
            ClientErrorComponent::RuntimeAdapter,
            false,
            ClientErrorRecovery::SelectSupportedAdapter,
        )
        .with_presentation_arg("agentLabel", agent_label),
        RuntimeAdapterError::RuntimeProfileUnavailable => ClientError::new(
            ClientErrorCode::NativeAgentRuntimeProfileUnavailable,
            ClientErrorStage::DiscoveryDriver,
            ClientErrorComponent::RuntimeAdapter,
            true,
            ClientErrorRecovery::InstallOrRetryRuntime,
        ),
        RuntimeAdapterError::ExecutableUnavailable => ClientError::new(
            ClientErrorCode::NativeAgentExecutableUnavailable,
            ClientErrorStage::ProcessLaunch,
            ClientErrorComponent::RuntimeProcess,
            true,
            ClientErrorRecovery::InstallOrRetryRuntime,
        ),
        RuntimeAdapterError::ConversationDispatchFailed => ClientError::new(
            ClientErrorCode::AgentConversationDispatchFailed,
            ClientErrorStage::ConversationDispatch,
            ClientErrorComponent::ConversationRuntime,
            true,
            ClientErrorRecovery::PreserveDraftAndRetry,
        ),
    }
}
