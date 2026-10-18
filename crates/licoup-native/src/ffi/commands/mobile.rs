// mobile commands: mobile relay config|pairing|pc|commands

use super::{AdmittedCommand, CliExecution, admitted_params};
use anyhow::Result;

pub(super) fn handle_mobile_relay(command: AdmittedCommand) -> Result<CliExecution> {
    let route = command.path();
    let params = command
        .option_json("stdin-json")
        .cloned()
        .unwrap_or_else(|| {
            admitted_params(
                &[
                    ("stationBaseUrl", command.option_text("station-base-url")),
                    ("relayEnabled", command.option_text("relay-enabled")),
                    ("authorize", command.option_text("authorize")),
                    ("hydrateSecrets", command.option_text("hydrate-secrets")),
                    ("pcClientId", command.option_text("pc-client-id")),
                    ("pcClientName", command.option_text("pc-client-name")),
                    ("paired", command.option_text("paired")),
                    ("resetPairing", command.option_text("reset-pairing")),
                    ("pairingId", command.option_text("pairing-id")),
                    ("allowInteraction", command.option_text("allow-interaction")),
                    ("commandId", command.option_text("command-id")),
                    ("idempotencyKey", command.option_text("idempotency-key")),
                    ("clientIntentId", command.option_text("client-intent-id")),
                    ("commandKind", command.option_text("command-kind")),
                    ("targetAgentId", command.option_text("target-agent-id")),
                    ("workspaceId", command.option_text("workspace-id")),
                    (
                        "acknowledgeReceiptId",
                        command.option_text("acknowledge-receipt-id"),
                    ),
                    ("type", command.option_text("type")),
                ],
                &[
                    ("body", command.option_json("body")),
                    (
                        "cleanupConfirmation",
                        command.option_json("cleanup-confirmation"),
                    ),
                ],
                &[],
            )
        });
    let (noun, action) = match route {
        ["mobile", "relay", noun, action] => (*noun, *action),
        _ => {
            return Err(super::handler_error("command_failed", "use_cli_help").into());
        }
    };
    let result = match (noun, action) {
        ("config", "get") => crate::domain::mobile_relay::config_get(&params)?,
        ("config", "set") => crate::domain::mobile_relay::config_set(&params)?,
        ("pairing", "create") => crate::domain::mobile_relay::pairing_create(&params)?,
        ("pairing", "claim") => crate::domain::mobile_relay::pairing_claim(&params)?,
        ("pairing", "status") => crate::domain::mobile_relay::pairing_status(&params)?,
        ("pairing", "revoke") => crate::domain::mobile_relay::pairing_revoke(&params)?,
        ("pc", "check-in") => crate::domain::mobile_relay::pc_check_in(&params)?,
        ("commands", "poll") => crate::domain::mobile_relay::commands_poll(&params)?,
        ("commands", "sync") => crate::domain::mobile_relay::commands_sync(&params)?,
        ("commands", "create") => crate::domain::mobile_relay::command_create(&params)?,
        ("commands", "create-secure") => {
            crate::domain::mobile_relay::command_create_secure(&params)?
        }
        ("commands", "result") => crate::domain::mobile_relay::command_result(&params)?,
        ("commands", "result-secure") => {
            crate::domain::mobile_relay::command_result_secure(&params)?
        }
        ("commands", "result-replay-proof") => {
            crate::domain::mobile_relay::command_result_replay_proof(&params)?
        }
        ("e2ee", "secret-store-cleanup") => {
            crate::domain::mobile_relay::e2ee_secret_store_cleanup(&params)?
        }
        ("e2ee", "secret-store-cleanup-inventory") => {
            crate::domain::mobile_relay::e2ee_secret_store_cleanup_inventory()?
        }
        ("e2ee", "secret-store-self-test") => {
            crate::domain::mobile_relay::e2ee_secret_store_self_test(&params)?
        }
        _ => {
            return Err(super::handler_error("command_failed", "use_cli_help").into());
        }
    };
    Ok(CliExecution::Json(result))
}

#[cfg(test)]
mod tests {
    use crate::domain::mobile_relay::with_mobile_relay_secret_store_override;
    use crate::platform::secure_mesh_secret_store::{EphemeralSecretStore, SecureMeshSecretStore};
    use licoup_foundation::platform::paths::set_portable_data_dir_override;
    use std::fs;
    use std::sync::Arc;
    use std::time::{SystemTime, UNIX_EPOCH};

    /// A well-formed confirmation is not authority: the CLI route must still
    /// reach the custody authority, which refuses without a verified device
    /// trust record for the replacement endpoint.
    #[test]
    fn custody_cleanup_cli_refuses_a_confirmation_without_an_authenticated_replacement_endpoint() {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "lico-mobile-relay-cleanup-cli-{}-{nonce}",
            std::process::id()
        ));
        let previous = set_portable_data_dir_override(Some(root.clone()));
        let store = Arc::new(EphemeralSecretStore::new());
        let store_override: Arc<dyn SecureMeshSecretStore> = store.clone();
        let confirmation = serde_json::json!({
            "schemaVersion": "licoup.custody-cleanup-confirmation.v1",
            "consequence": "irreversibleLocalSecretErasure",
            "subjectEndpointId": "endpoint-unauthenticated",
            "subjectIdentityFingerprint": "sha256:unauthenticated",
            "custodyNamespace": "mobileRelayPqxdhMlKem1024Runtime",
            "replacementEndpointId": "endpoint-unauthenticated",
            "replacementDeviceTrustFingerprint": "sha256:unauthenticated",
            "inventoryDigest": "0".repeat(64),
            "scope": ["secret-handle:unauthenticated:pcToken"],
        })
        .to_string();

        let error = with_mobile_relay_secret_store_override(store_override, || {
            crate::ffi::commands::execute_cli(vec![
                "mobile".to_string(),
                "relay".to_string(),
                "e2ee".to_string(),
                "secret-store-cleanup".to_string(),
                "--cleanup-confirmation".to_string(),
                confirmation,
            ])
        })
        .unwrap_err()
        .to_string();

        assert!(
            error.contains("custody cleanup requires an authenticated replacement endpoint"),
            "unexpected custody cleanup refusal: {error}"
        );
        assert_eq!(store.authorization_session_count(), 0);

        set_portable_data_dir_override(previous);
        let _ = fs::remove_dir_all(root);
    }
}
