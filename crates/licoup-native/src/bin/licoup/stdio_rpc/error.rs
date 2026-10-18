use licoup_native::core::secure_mesh_secret_store::SecretStorePresenceError;
use licoup_native::ffi::generated::client_error::{ClientError, ClientErrorCode};
use licoup_native::ffi::generated::client_state::ClientStateFailure;

#[path = "error/metadata.rs"]
mod metadata;

pub(crate) fn stdio_rpc_client_error(code: &str) -> ClientError {
    let code = serde_json::from_value(serde_json::Value::String(code.to_owned()))
        .unwrap_or(ClientErrorCode::CommandFailed);
    let (stage, component, retryable, recovery) = metadata::for_code(&code);
    ClientError::new(code, stage, component, retryable, recovery)
}

pub(crate) fn stdio_rpc_state_failure(error: ClientStateFailure) -> ClientError {
    stdio_rpc_client_error(error.code.as_str())
}

pub(crate) fn stdio_rpc_command_error(error: &anyhow::Error) -> ClientError {
    if let Some(presence) = error.downcast_ref::<SecretStorePresenceError>() {
        match presence.code() {
            "secure_mesh_authorization_required" => {
                return stdio_rpc_client_error("authorization_required");
            }
            "secure_mesh_presence_native_authentication_failed" => {
                return stdio_rpc_client_error("authorization_failed");
            }
            _ => {}
        }
    }
    if let Some(error) = error.downcast_ref::<licoup_native::ffi::commands::CliCommandError>() {
        return stdio_rpc_client_error(error.code());
    }
    stdio_rpc_client_error("command_failed")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn authorization_errors_keep_their_type_through_context() {
        for (presence, code) in [
            (
                SecretStorePresenceError::authorization_required(),
                ClientErrorCode::AuthorizationRequired,
            ),
            (
                SecretStorePresenceError::authorization_failed(),
                ClientErrorCode::AuthorizationFailed,
            ),
        ] {
            let error = anyhow::Error::new(presence).context("synthetic private operation detail");
            assert_eq!(stdio_rpc_command_error(&error).code, code);
        }
    }

    #[test]
    fn untyped_text_cannot_impersonate_an_authorization_failure() {
        for text in [
            "secure_mesh_authorization_required",
            "system authentication failed closed",
            "system authentication timed out",
        ] {
            assert_eq!(
                stdio_rpc_command_error(&anyhow::anyhow!(text)).code,
                ClientErrorCode::CommandFailed
            );
        }
    }
}
