use crate::domain::mobile_relay::endpoint_trust::decode_key_32;
use crate::domain::mobile_relay::secret_custody::{
    MobileRelayE2eeSecretField, RuntimeSecretMaterial,
};
use anyhow::{Result, anyhow};
use licoup_protocol_bindings::licoarc_relay::{
    SECURE_MESH_MAILBOX_ROTATION_WINDOW_SECONDS, SecureMeshDeliverySecret,
    SecureMeshMailboxDirection, SecureMeshMailboxSchedule, SecureMeshRelayChannelBinding,
};
use serde_json::Value;
use sha2::{Digest, Sha256};
use time::OffsetDateTime;

pub(in crate::domain::mobile_relay) fn current_mailbox_rotation_epoch() -> Result<u64> {
    let now = u64::try_from(OffsetDateTime::now_utc().unix_timestamp())
        .map_err(|_| anyhow!("Lico Arc mailbox clock is before unix epoch"))?;
    Ok(now / SECURE_MESH_MAILBOX_ROTATION_WINDOW_SECONDS)
}

pub(in crate::domain::mobile_relay) fn canonical_mailbox_token(
    secret_material: &RuntimeSecretMaterial,
    endpoint_id: &str,
    endpoint_kind: &str,
    rotation_epoch: u64,
) -> Result<String> {
    let pairing_secret = secret_material
        .e2ee_secret(MobileRelayE2eeSecretField::PairingSecret)
        .ok_or_else(|| anyhow!("Lico Arc mailbox delivery secret is missing"))?
        .expose_utf8()
        .map_err(|_| anyhow!("Lico Arc mailbox delivery secret is invalid"))?;
    let delivery_secret = SecureMeshDeliverySecret::from_bytes(decode_key_32(
        pairing_secret,
        "Lico Arc mailbox delivery secret",
    )?);
    let direction = if endpoint_kind == "mobile" {
        SecureMeshMailboxDirection::PairwiseInitiatorToResponder
    } else {
        SecureMeshMailboxDirection::PairwiseResponderToInitiator
    };
    let binding: [u8; 32] = Sha256::digest(
        format!("licoup.licoarc.mailbox-channel.v1:{endpoint_kind}:{endpoint_id}").as_bytes(),
    )
    .into();
    let schedule = SecureMeshMailboxSchedule::new(
        delivery_secret,
        direction,
        SecureMeshRelayChannelBinding::from_bytes(binding),
    );
    let epoch_seconds = rotation_epoch
        .checked_mul(SECURE_MESH_MAILBOX_ROTATION_WINDOW_SECONDS)
        .ok_or_else(|| anyhow!("Lico Arc mailbox rotation epoch overflow"))?;
    Ok(schedule
        .token_for_unix_seconds(epoch_seconds)?
        .as_str()
        .to_string())
}

pub(in crate::domain::mobile_relay) fn local_canonical_mailbox_tokens(
    config: &Value,
    secret_material: &RuntimeSecretMaterial,
) -> Result<Vec<String>> {
    local_canonical_mailbox_tokens_at_epoch(
        config,
        secret_material,
        current_mailbox_rotation_epoch()?,
    )
}

pub(in crate::domain::mobile_relay) fn local_canonical_mailbox_tokens_at_epoch(
    config: &Value,
    secret_material: &RuntimeSecretMaterial,
    current_epoch: u64,
) -> Result<Vec<String>> {
    let state = config
        .get("mobileRelayE2ee")
        .ok_or_else(|| anyhow!("mobile relay E2EE endpoint state is missing"))?;
    let endpoint_id = state
        .get("endpointId")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow!("mobile relay endpoint id is missing"))?;
    let endpoint_kind = state
        .get("endpointKind")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow!("mobile relay endpoint kind is missing"))?;
    let mut tokens = Vec::with_capacity(2);
    for epoch in [Some(current_epoch), current_epoch.checked_sub(1)]
        .into_iter()
        .flatten()
    {
        let token = canonical_mailbox_token(secret_material, endpoint_id, endpoint_kind, epoch)?;
        if !tokens.contains(&token) {
            tokens.push(token);
        }
    }
    Ok(tokens)
}
