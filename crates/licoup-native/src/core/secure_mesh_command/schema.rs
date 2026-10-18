use super::*;

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SecureCommandRiskClass {
    ReadOnly,
    SafeWrite,
    LocalEffect,
    HighRisk,
}

impl SecureCommandRiskClass {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::ReadOnly => "read_only",
            Self::SafeWrite => "safe_write",
            Self::LocalEffect => "local_effect",
            Self::HighRisk => "high_risk",
        }
    }

    fn from_str(value: &str) -> Result<Self> {
        match value {
            "read_only" => Ok(Self::ReadOnly),
            "safe_write" => Ok(Self::SafeWrite),
            "local_effect" => Ok(Self::LocalEffect),
            "high_risk" => Ok(Self::HighRisk),
            _ => bail!("secure mesh command riskClass is unsupported"),
        }
    }

    pub(super) fn rank(&self) -> u8 {
        match self {
            Self::ReadOnly => 0,
            Self::SafeWrite => 1,
            Self::LocalEffect => 2,
            Self::HighRisk => 3,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SecureCommandSenderIdentity {
    pub endpoint_id: String,
    pub identity_fingerprint: String,
    pub trust_state: DeviceTrustState,
    pub endpoint_kind: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SecureCommandTargetBinding {
    pub target_endpoint_id: String,
    pub target_agent_id: Option<String>,
    pub workspace_id: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SecureCommandPayload {
    pub command_id: String,
    pub command_kind: String,
    pub sender_identity: SecureCommandSenderIdentity,
    pub target_binding: SecureCommandTargetBinding,
    pub risk_class: SecureCommandRiskClass,
    pub requires_user_confirmation: bool,
    pub idempotency_key: String,
    pub created_at: String,
    pub expires_at: String,
    pub(super) created_at_time: OffsetDateTime,
    pub(super) expires_at_time: OffsetDateTime,
    body: Value,
}

impl SecureCommandPayload {
    pub fn from_value(value: &Value) -> Result<Self> {
        let object = value
            .as_object()
            .ok_or_else(|| anyhow!("secure mesh command payload must be a JSON object"))?;
        ensure_allowed_keys(
            "secure mesh command payload",
            object.keys().map(String::as_str),
            &[
                "schema",
                "commandId",
                "commandKind",
                "senderIdentity",
                "targetBinding",
                "riskClass",
                "requiresUserConfirmation",
                "idempotencyKey",
                "createdAt",
                "expiresAt",
                "body",
            ],
        )?;
        let schema = read_required_string(value, "schema", 255)?;
        ensure!(
            schema == SECURE_MESH_COMMAND_PROTOCOL_VERSION,
            "secure mesh command schema is unsupported"
        );
        let command_id = read_required_string(value, "commandId", MAX_COMMAND_ID_BYTES)?;
        let command_kind = read_required_string(value, "commandKind", MAX_COMMAND_KIND_BYTES)?;
        let sender_identity = read_sender_identity(
            value
                .get("senderIdentity")
                .context("secure mesh command senderIdentity is required")?,
        )?;
        let target_binding = read_target_binding(
            value
                .get("targetBinding")
                .context("secure mesh command targetBinding is required")?,
        )?;
        let risk_class =
            SecureCommandRiskClass::from_str(&read_required_string(value, "riskClass", 64)?)?;
        let requires_user_confirmation = value
            .get("requiresUserConfirmation")
            .and_then(Value::as_bool)
            .context("secure mesh command requiresUserConfirmation is required")?;
        let idempotency_key =
            read_required_string(value, "idempotencyKey", MAX_IDEMPOTENCY_KEY_BYTES)?;
        let created_at = read_required_string(value, "createdAt", 64)?;
        let expires_at = read_required_string(value, "expiresAt", 64)?;
        let created_at_time = parse_timestamp("createdAt", &created_at)?;
        let expires_at_time = parse_timestamp("expiresAt", &expires_at)?;
        ensure!(
            expires_at_time > created_at_time,
            "secure mesh command expiresAt must be after createdAt"
        );
        let body = value
            .get("body")
            .cloned()
            .context("secure mesh command body is required")?;
        ensure!(
            serde_json::to_vec(&body)?.len() <= MAX_COMMAND_BODY_BYTES,
            "secure mesh command body is too large"
        );
        Ok(Self {
            command_id,
            command_kind,
            sender_identity,
            target_binding,
            risk_class,
            requires_user_confirmation,
            idempotency_key,
            created_at,
            expires_at,
            created_at_time,
            expires_at_time,
            body,
        })
    }

    pub(super) fn idempotency_fingerprint(&self) -> Result<String> {
        let canonical = json!({
            "schema": SECURE_MESH_COMMAND_PROTOCOL_VERSION,
            "commandKind": self.command_kind,
            "senderIdentity": {
                "endpointId": self.sender_identity.endpoint_id,
                "identityFingerprint": self.sender_identity.identity_fingerprint,
            },
            "targetBinding": {
                "targetEndpointId": self.target_binding.target_endpoint_id,
                "targetAgentId": self.target_binding.target_agent_id,
                "workspaceId": self.target_binding.workspace_id,
            },
            "riskClass": self.risk_class.as_str(),
            "requiresUserConfirmation": self.requires_user_confirmation,
            "body": self.body,
        });
        Ok(hex_digest(&serde_json::to_vec(&canonical)?))
    }

    pub fn body(&self) -> &Value {
        &self.body
    }
}

#[derive(Clone, Debug)]
pub struct SecureCommandEvaluationContext {
    pub local_endpoint_id: String,
    pub sender_endpoint_id: String,
    pub sender_identity_fingerprint: String,
    pub sender_trust_state: DeviceTrustState,
    pub sender_endpoint_kind: String,
    pub sender_roster_active: bool,
    pub target_roster_active: bool,
    pub session_or_epoch_valid: bool,
    pub user_confirmed: bool,
    pub allowed_workspace_ids: BTreeSet<String>,
    pub allowed_agent_ids: BTreeSet<String>,
    pub now: OffsetDateTime,
}

impl SecureCommandEvaluationContext {
    pub fn from_value(value: &Value) -> Result<Self> {
        let object = value
            .as_object()
            .ok_or_else(|| anyhow!("secure mesh command context must be a JSON object"))?;
        ensure_allowed_keys(
            "secure mesh command context",
            object.keys().map(String::as_str),
            &[
                "localEndpointId",
                "senderEndpointId",
                "senderIdentityFingerprint",
                "senderTrustState",
                "senderEndpointKind",
                "senderRosterActive",
                "targetRosterActive",
                "sessionOrEpochValid",
                "userConfirmed",
                "allowedWorkspaceIds",
                "allowedAgentIds",
                "now",
            ],
        )?;
        let now = value
            .get("now")
            .and_then(Value::as_str)
            .map(|raw| parse_timestamp("now", raw))
            .transpose()?
            .unwrap_or_else(OffsetDateTime::now_utc);
        Ok(Self {
            local_endpoint_id: read_required_string(
                value,
                "localEndpointId",
                MAX_ENDPOINT_ID_BYTES,
            )?,
            sender_endpoint_id: read_required_string(
                value,
                "senderEndpointId",
                MAX_ENDPOINT_ID_BYTES,
            )?,
            sender_identity_fingerprint: read_required_string(
                value,
                "senderIdentityFingerprint",
                MAX_FINGERPRINT_BYTES,
            )?,
            sender_trust_state: trust_state_from_str(&read_required_string(
                value,
                "senderTrustState",
                64,
            )?)?,
            sender_endpoint_kind: read_required_string(
                value,
                "senderEndpointKind",
                MAX_ENDPOINT_ID_BYTES,
            )?,
            sender_roster_active: read_required_bool(value, "senderRosterActive")?,
            target_roster_active: read_required_bool(value, "targetRosterActive")?,
            session_or_epoch_valid: read_required_bool(value, "sessionOrEpochValid")?,
            user_confirmed: read_required_bool(value, "userConfirmed")?,
            allowed_workspace_ids: read_string_set(value, "allowedWorkspaceIds")?,
            allowed_agent_ids: read_string_set(value, "allowedAgentIds")?,
            now,
        })
    }
}

fn read_sender_identity(value: &Value) -> Result<SecureCommandSenderIdentity> {
    let object = value
        .as_object()
        .ok_or_else(|| anyhow!("secure mesh command senderIdentity must be a JSON object"))?;
    ensure_allowed_keys(
        "secure mesh command senderIdentity",
        object.keys().map(String::as_str),
        &[
            "endpointId",
            "identityFingerprint",
            "trustState",
            "endpointKind",
        ],
    )?;
    Ok(SecureCommandSenderIdentity {
        endpoint_id: read_required_string(value, "endpointId", MAX_ENDPOINT_ID_BYTES)?,
        identity_fingerprint: read_required_string(
            value,
            "identityFingerprint",
            MAX_FINGERPRINT_BYTES,
        )?,
        trust_state: trust_state_from_str(&read_required_string(value, "trustState", 64)?)?,
        endpoint_kind: read_required_string(value, "endpointKind", MAX_ENDPOINT_ID_BYTES)?,
    })
}

fn read_target_binding(value: &Value) -> Result<SecureCommandTargetBinding> {
    let object = value
        .as_object()
        .ok_or_else(|| anyhow!("secure mesh command targetBinding must be a JSON object"))?;
    ensure_allowed_keys(
        "secure mesh command targetBinding",
        object.keys().map(String::as_str),
        &["targetEndpointId", "targetAgentId", "workspaceId"],
    )?;
    Ok(SecureCommandTargetBinding {
        target_endpoint_id: read_required_string(value, "targetEndpointId", MAX_ENDPOINT_ID_BYTES)?,
        target_agent_id: read_optional_string(value, "targetAgentId", MAX_BINDING_ID_BYTES)?,
        workspace_id: read_optional_string(value, "workspaceId", MAX_BINDING_ID_BYTES)?,
    })
}

fn ensure_allowed_keys<'a>(
    context: &str,
    keys: impl Iterator<Item = &'a str>,
    allowed: &[&str],
) -> Result<()> {
    for key in keys {
        ensure!(
            allowed.contains(&key),
            "{} contains unsupported field {}",
            context,
            key
        );
    }
    Ok(())
}

fn read_required_string(value: &Value, key: &str, max_bytes: usize) -> Result<String> {
    let raw = value
        .get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow!("secure mesh command {} is required", key))?
        .trim();
    ensure!(
        !raw.is_empty(),
        "secure mesh command {} must not be empty",
        key
    );
    ensure!(
        raw.len() <= max_bytes,
        "secure mesh command {} is too large",
        key
    );
    Ok(raw.to_string())
}

fn read_optional_string(value: &Value, key: &str, max_bytes: usize) -> Result<Option<String>> {
    match value.get(key) {
        Some(Value::String(raw)) => {
            let trimmed = raw.trim();
            ensure!(
                !trimmed.is_empty(),
                "secure mesh command {} must not be empty",
                key
            );
            ensure!(
                trimmed.len() <= max_bytes,
                "secure mesh command {} is too large",
                key
            );
            Ok(Some(trimmed.to_string()))
        }
        Some(Value::Null) | None => Ok(None),
        _ => bail!("secure mesh command {} must be a string", key),
    }
}

fn read_required_bool(value: &Value, key: &str) -> Result<bool> {
    value
        .get(key)
        .and_then(Value::as_bool)
        .ok_or_else(|| anyhow!("secure mesh command {} is required", key))
}

fn read_string_set(value: &Value, key: &str) -> Result<BTreeSet<String>> {
    let mut out = BTreeSet::new();
    let Some(raw) = value.get(key) else {
        return Ok(out);
    };
    let values = raw
        .as_array()
        .ok_or_else(|| anyhow!("secure mesh command {} must be an array", key))?;
    for value in values {
        let item = value
            .as_str()
            .ok_or_else(|| anyhow!("secure mesh command {} entries must be strings", key))?
            .trim();
        ensure!(
            !item.is_empty(),
            "secure mesh command {} entries must not be empty",
            key
        );
        ensure!(
            item.len() <= MAX_BINDING_ID_BYTES,
            "secure mesh command {} entry is too large",
            key
        );
        out.insert(item.to_string());
    }
    Ok(out)
}

fn parse_timestamp(key: &str, value: &str) -> Result<OffsetDateTime> {
    OffsetDateTime::parse(value, &Rfc3339)
        .map_err(|error| anyhow!("secure mesh command {} is not RFC3339: {error}", key))
}

fn trust_state_from_str(value: &str) -> Result<DeviceTrustState> {
    match value {
        "unverified" => Ok(DeviceTrustState::Unverified),
        "verified" => Ok(DeviceTrustState::Verified),
        "cross_signed" => Ok(DeviceTrustState::CrossSigned),
        "changed" | "key_changed" => Ok(DeviceTrustState::KeyChanged),
        "revoked" => Ok(DeviceTrustState::Revoked),
        _ => bail!("secure mesh command sender trust state is unsupported"),
    }
}

fn hex_digest(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}
