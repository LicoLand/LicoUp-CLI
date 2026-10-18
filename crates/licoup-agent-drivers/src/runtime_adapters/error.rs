use std::fmt;

// The generated ClientError carries code, stage, component, retryable,
// recovery, and presentationArgs as one immutable source-selected value.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RuntimeAdapterError {
    AgentIdentifierMissing,
    MessageMissing,
    LegacyLaunchConfiguration,
    InvalidRuntimeSetting {
        field: &'static str,
    },
    AttachmentUnsupportedForAdapter {
        agent_label: String,
    },
    /// A capability the Agent's owner declares, carried over a transport that
    /// cannot move the local attachment. Stated separately from an Agent that
    /// does not declare image input at all.
    AttachmentUnsupportedForTransport {
        agent_label: String,
    },
    /// A Profile requirement that no declared capability fact satisfies. The
    /// named capability is the fact that could not be satisfied.
    CapabilityRequirementUnsatisfied {
        capability: String,
    },
    AttachmentListExceeded,
    AttachmentInvalid,
    AttachmentRemoteUnsupported,
    AttachmentMediaUnsupported,
    AttachmentSymlinkRejected,
    AttachmentFileUnavailable,
    AttachmentSizeLimit,
    AttachmentSignatureMismatch,
    UnsupportedAdapter {
        agent_label: String,
    },
    RuntimeProfileUnavailable,
    ExecutableUnavailable,
    ConversationDispatchFailed,
}

impl fmt::Display for RuntimeAdapterError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::AgentIdentifierMissing => {
                "agent conversation request requires an agent identifier"
            }
            Self::MessageMissing => "agent message request requires message text",
            Self::LegacyLaunchConfiguration => {
                "legacy command and argument launch configuration is not supported"
            }
            Self::InvalidRuntimeSetting { .. } => {
                "agent conversation request contains an invalid runtime setting"
            }
            Self::AttachmentUnsupportedForAdapter { .. } => {
                "image attachments are not supported by this runtime adapter"
            }
            Self::AttachmentUnsupportedForTransport { .. } => {
                "image attachments are not supported over this runtime transport"
            }
            Self::CapabilityRequirementUnsatisfied { .. } => {
                "the request requires a capability this runtime adapter does not declare"
            }
            Self::AttachmentListExceeded => {
                "agent message request exceeds the image attachment limit"
            }
            Self::AttachmentInvalid => "agent message request contains an invalid image attachment",
            Self::AttachmentRemoteUnsupported => {
                "image attachments must be local files, not remote URLs"
            }
            Self::AttachmentMediaUnsupported => "image attachment media type is not supported",
            Self::AttachmentSymlinkRejected => {
                "image attachment must be a regular file, not a symbolic link"
            }
            Self::AttachmentFileUnavailable => "image attachment file is unavailable",
            Self::AttachmentSizeLimit => "image attachment exceeds the size limit",
            Self::AttachmentSignatureMismatch => {
                "image attachment content does not match its declared media type"
            }
            Self::UnsupportedAdapter { .. } => "unsupported runtime adapter",
            Self::RuntimeProfileUnavailable => "native agent runtime profile is unavailable",
            Self::ExecutableUnavailable => "native agent executable is unavailable",
            Self::ConversationDispatchFailed => "agent conversation dispatch failed",
        })
    }
}

impl std::error::Error for RuntimeAdapterError {}
