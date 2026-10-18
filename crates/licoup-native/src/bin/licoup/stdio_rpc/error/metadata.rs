use licoup_native::ffi::generated::client_error::{
    ClientErrorCode, ClientErrorComponent, ClientErrorRecovery, ClientErrorStage,
};

pub(super) fn for_code(
    code: &ClientErrorCode,
) -> (
    ClientErrorStage,
    ClientErrorComponent,
    bool,
    ClientErrorRecovery,
) {
    match code {
        ClientErrorCode::InvalidRequest
        | ClientErrorCode::RequestTooLarge
        | ClientErrorCode::InvalidJson
        | ClientErrorCode::InvalidProtocol
        | ClientErrorCode::InvalidRequestId
        | ClientErrorCode::InvalidWorkflowId
        | ClientErrorCode::InvalidMethod
        | ClientErrorCode::InvalidArgs
        | ClientErrorCode::InvalidPortableDataDir
        | ClientErrorCode::InvalidParams
        | ClientErrorCode::InvalidCollection
        | ClientErrorCode::InvalidDocument
        | ClientErrorCode::WorkflowMismatch => (
            ClientErrorStage::RequestValidation,
            ClientErrorComponent::StdioRpc,
            false,
            ClientErrorRecovery::CorrectRequest,
        ),
        ClientErrorCode::StreamProtocolFailed => (
            ClientErrorStage::ConversationStreamReceive,
            ClientErrorComponent::StdioRpc,
            true,
            ClientErrorRecovery::PreserveDraftAndRetry,
        ),
        ClientErrorCode::CliCommandMissing
        | ClientErrorCode::CliCommandUnknown
        | ClientErrorCode::CliOperationUnsupported => (
            ClientErrorStage::CliAdmission,
            ClientErrorComponent::NativeCli,
            false,
            ClientErrorRecovery::UseCliHelp,
        ),
        ClientErrorCode::CliRequiredArgumentMissing
        | ClientErrorCode::CliArgumentUnexpected
        | ClientErrorCode::CliOptionUnknown
        | ClientErrorCode::CliRequiredOptionMissing
        | ClientErrorCode::CliOptionValueMissing
        | ClientErrorCode::CliOptionDuplicate
        | ClientErrorCode::CliOptionConstraintViolation => (
            ClientErrorStage::CliAdmission,
            ClientErrorComponent::NativeCli,
            false,
            ClientErrorRecovery::CorrectCommandArguments,
        ),
        ClientErrorCode::CliJsonInvalid => (
            ClientErrorStage::CliAdmission,
            ClientErrorComponent::NativeCli,
            false,
            ClientErrorRecovery::ProvideValidJson,
        ),
        ClientErrorCode::CliArgumentCountExceeded | ClientErrorCode::CliArgumentBytesExceeded => (
            ClientErrorStage::CliAdmission,
            ClientErrorComponent::NativeCli,
            false,
            ClientErrorRecovery::ReduceCommandArguments,
        ),
        _ => (
            ClientErrorStage::StdioRpcResponse,
            ClientErrorComponent::NativeCli,
            false,
            ClientErrorRecovery::RetryOrReviewRequest,
        ),
    }
}
