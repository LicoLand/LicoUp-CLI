use anyhow::{Result, ensure};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use super::schema::{
    LifecycleServiceActionKind, MAX_MISSING_MESSAGE_IDS, MAX_TTL_SECONDS, read_bool,
    read_required_text, read_u64, validate_text,
};

pub(super) struct LifecyclePolicyDecision {
    pub(super) action_kind: &'static str,
    pub(super) scope: Value,
    pub(super) service_policy: Value,
}

pub(super) fn evaluate(params: &Value) -> Result<LifecyclePolicyDecision> {
    let action_kind = LifecycleServiceActionKind::parse(params)?;
    let service_policy = match action_kind {
        LifecycleServiceActionKind::MessageTtlSet => {
            let ttl_seconds = read_u64(params, &["ttlSeconds", "ttl_seconds"])?;
            ensure!(
                (1..=MAX_TTL_SECONDS).contains(&ttl_seconds),
                "secure mesh message TTL is outside the supported range"
            );
            json!({
                "ttlSeconds": ttl_seconds,
                "expiresExistingMessages": read_bool(params, "expiresExistingMessages", false),
                "localTimerRequired": true,
                "remoteServiceNoticeRequired": true
            })
        }
        LifecycleServiceActionKind::MessageDelete => {
            ensure!(
                read_bool(params, "userConfirmed", false),
                "secure mesh delete service action requires local user confirmation"
            );
            json!({
                "localDeleteRequired": true,
                "remoteDeleteNoticeRequired": true,
                "purgeLocalPlaintext": true,
                "purgeLocalCiphertextAfterAck": true
            })
        }
        LifecycleServiceActionKind::ScreenshotDetected => json!({
            "userVisibleWarningRequired": true,
            "remoteServiceNoticeRequired": true,
            "screenshotContentIncluded": false
        }),
        LifecycleServiceActionKind::ResendRequest => {
            let missing = missing_message_digests(params)?;
            ensure!(
                !missing.is_empty(),
                "secure mesh resend service action requires missing message ids"
            );
            json!({
                "resendRequestRequired": true,
                "missingMessageCount": missing.len(),
                "missingMessageDigests": missing,
                "missingMessageIdsRedacted": true
            })
        }
        LifecycleServiceActionKind::TypingState => {
            let typing_state = read_required_text(params, &["typingState", "typing_state"])?;
            ensure!(
                matches!(typing_state.as_str(), "started" | "stopped"),
                "secure mesh typing service action state is unsupported"
            );
            json!({
                "typingState": typing_state,
                "typingNoticeRequired": true,
                "typingStateEncrypted": true,
                "typingContentIncluded": false,
                "remoteServiceNoticeRequired": true
            })
        }
        LifecycleServiceActionKind::ReadReceipt => {
            let read_up_to = required_digest(
                params,
                &[
                    "readUpToMessageId",
                    "read_up_to_message_id",
                    "messageId",
                    "message_id",
                ],
                "read receipt message id",
            )?;
            json!({
                "readReceiptRequired": true,
                "readUpToMessageDigest": read_up_to,
                "readMessageIdsRedacted": true,
                "remoteServiceNoticeRequired": true,
                "localUnreadStateUpdateRequired": true
            })
        }
        LifecycleServiceActionKind::AckPurge => {
            let acknowledged = read_bool(params, "acknowledged", false);
            let transfer_complete = read_bool(params, "transferComplete", false);
            json!({
                "ackRequired": !acknowledged,
                "purgeLocalCiphertext": acknowledged && transfer_complete,
                "purgeLocalPlaintext": true,
                "transferComplete": transfer_complete
            })
        }
    };

    Ok(LifecyclePolicyDecision {
        action_kind: action_kind.as_str(),
        scope: service_action_scope(params)?,
        service_policy,
    })
}

fn service_action_scope(params: &Value) -> Result<Value> {
    Ok(json!({
        "endpointHash": optional_digest(params, &["endpointId", "endpoint_id", "deviceId", "device_id"] )?,
        "conversationHash": optional_digest(params, &["conversationId", "conversation_id", "chatId", "chat_id"] )?,
        "messageHash": optional_digest(params, &["messageId", "message_id"] )?,
        "fileTransferHash": optional_digest(params, &["fileTransferId", "file_transfer_id", "fileId", "file_id"] )?,
        "scopeIdsRedacted": true
    }))
}

fn missing_message_digests(params: &Value) -> Result<Vec<String>> {
    let Some(values) = params
        .get("missingMessageIds")
        .or_else(|| params.get("missing_message_ids"))
        .and_then(Value::as_array)
    else {
        return Ok(Vec::new());
    };
    ensure!(
        values.len() <= MAX_MISSING_MESSAGE_IDS,
        "secure mesh resend service action has too many missing message ids"
    );
    values
        .iter()
        .map(|value| {
            let text = value.as_str().unwrap_or_default().trim().to_string();
            validate_text(&text, "missing message id")?;
            Ok(hash_text(&text))
        })
        .collect()
}

fn optional_digest(params: &Value, keys: &[&str]) -> Result<Option<String>> {
    keys.iter()
        .find_map(|key| params.get(*key))
        .and_then(Value::as_str)
        .map(|value| {
            let text = value.trim().to_string();
            validate_text(&text, "scope id")?;
            Ok(hash_text(&text))
        })
        .transpose()
}

fn required_digest(params: &Value, keys: &[&str], label: &str) -> Result<String> {
    let value = keys
        .iter()
        .find_map(|key| params.get(*key))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim()
        .to_string();
    validate_text(&value, label)?;
    Ok(hash_text(&value))
}

fn hash_text(value: &str) -> String {
    let digest = Sha256::digest(value.as_bytes());
    format!(
        "sha256:{}",
        digest
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>()
    )
}
