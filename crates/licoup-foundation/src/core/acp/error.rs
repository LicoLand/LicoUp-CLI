use std::error::Error;
use std::fmt;

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum AcpError {
    RequestIdInvalid,
    ImplementationInvalid,
    WorkingDirectoryInvalid,
    AdditionalDirectoryInvalid,
    AdditionalDirectoryLimitExceeded,
    McpServerInvalid,
    McpServerLimitExceeded,
    SessionIdInvalid,
    PromptInvalid,
    MessageTooLarge,
    JsonLineInvalid,
    ResponseEnvelopeInvalid,
    NotificationEnvelopeInvalid,
    NotificationMethodInvalid,
    JsonRpcVersionInvalid,
    ResponseIdInvalid,
    ResponseIdMismatch,
    ResponseOutcomeInvalid,
    RemoteError { code: i64 },
    ResultInvalid,
    ProtocolVersionInvalid,
    UnsupportedProtocolVersion { received: u16 },
    CapabilityInvalid,
    SessionListRequestInvalid,
    SessionListResponseInvalid,
    SessionListLimitExceeded,
    SessionResponseInvalid,
    SessionUpdateInvalid,
    SessionMismatch,
    CloseResponseInvalid,
    PromptResponseInvalid,
    StopReasonInvalid,
}

impl AcpError {
    pub const fn code(&self) -> &'static str {
        match self {
            Self::RequestIdInvalid => "acp_request_id_invalid",
            Self::ImplementationInvalid => "acp_implementation_invalid",
            Self::WorkingDirectoryInvalid => "acp_working_directory_invalid",
            Self::AdditionalDirectoryInvalid => "acp_additional_directory_invalid",
            Self::AdditionalDirectoryLimitExceeded => "acp_additional_directory_limit_exceeded",
            Self::McpServerInvalid => "acp_mcp_server_invalid",
            Self::McpServerLimitExceeded => "acp_mcp_server_limit_exceeded",
            Self::SessionIdInvalid => "acp_session_id_invalid",
            Self::PromptInvalid => "acp_prompt_invalid",
            Self::MessageTooLarge => "acp_message_too_large",
            Self::JsonLineInvalid => "acp_json_line_invalid",
            Self::ResponseEnvelopeInvalid => "acp_response_envelope_invalid",
            Self::NotificationEnvelopeInvalid => "acp_notification_envelope_invalid",
            Self::NotificationMethodInvalid => "acp_notification_method_invalid",
            Self::JsonRpcVersionInvalid => "acp_jsonrpc_version_invalid",
            Self::ResponseIdInvalid => "acp_response_id_invalid",
            Self::ResponseIdMismatch => "acp_response_id_mismatch",
            Self::ResponseOutcomeInvalid => "acp_response_outcome_invalid",
            Self::RemoteError { .. } => "acp_remote_error",
            Self::ResultInvalid => "acp_result_invalid",
            Self::ProtocolVersionInvalid => "acp_protocol_version_invalid",
            Self::UnsupportedProtocolVersion { .. } => "acp_protocol_version_unsupported",
            Self::CapabilityInvalid => "acp_capability_invalid",
            Self::SessionListRequestInvalid => "acp_session_list_request_invalid",
            Self::SessionListResponseInvalid => "acp_session_list_response_invalid",
            Self::SessionListLimitExceeded => "acp_session_list_limit_exceeded",
            Self::SessionResponseInvalid => "acp_session_response_invalid",
            Self::SessionUpdateInvalid => "acp_session_update_invalid",
            Self::SessionMismatch => "acp_session_mismatch",
            Self::CloseResponseInvalid => "acp_close_response_invalid",
            Self::PromptResponseInvalid => "acp_prompt_response_invalid",
            Self::StopReasonInvalid => "acp_stop_reason_invalid",
        }
    }

    pub const fn is_remote_error(&self) -> bool {
        matches!(self, Self::RemoteError { .. })
    }
}

impl fmt::Display for AcpError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.code())
    }
}

impl Error for AcpError {}
