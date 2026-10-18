use super::*;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SecureCommandEvaluation {
    pub accepted: bool,
    pub should_execute: bool,
    pub replayed: bool,
    pub code: String,
    pub reason: String,
    pub command_id: String,
    pub command_kind: String,
    pub risk_class: String,
    pub requires_user_confirmation: bool,
}

impl SecureCommandEvaluation {
    pub fn to_json(&self) -> Value {
        json!({
            "ok": true,
            "protocolVersion": SECURE_MESH_COMMAND_PROTOCOL_VERSION,
            "accepted": self.accepted,
            "shouldExecute": self.should_execute,
            "replayed": self.replayed,
            "code": self.code,
            "reason": self.reason,
            "commandId": self.command_id,
            "commandKind": self.command_kind,
            "riskClass": self.risk_class,
            "requiresUserConfirmation": self.requires_user_confirmation,
            "bodyRedacted": true,
        })
    }
}

pub fn evaluate_secure_command(
    payload: &SecureCommandPayload,
    context: &SecureCommandEvaluationContext,
    ledger: &mut impl SecureCommandReplayStore,
) -> Result<SecureCommandEvaluation> {
    if payload.sender_identity.endpoint_id != context.sender_endpoint_id {
        return Ok(reject(payload, "sender_identity_mismatch"));
    }
    if payload.sender_identity.identity_fingerprint != context.sender_identity_fingerprint {
        return Ok(reject(payload, "sender_fingerprint_mismatch"));
    }
    if payload.sender_identity.endpoint_kind != context.sender_endpoint_kind {
        return Ok(reject(payload, "sender_endpoint_kind_mismatch"));
    }
    if payload.sender_identity.trust_state != context.sender_trust_state {
        return Ok(reject(payload, "sender_trust_state_mismatch"));
    }
    if matches!(
        context.sender_trust_state,
        DeviceTrustState::KeyChanged | DeviceTrustState::Revoked
    ) {
        return Ok(reject(payload, "sender_device_trust_rejected"));
    }
    if !context.sender_roster_active || !context.target_roster_active {
        return Ok(reject(payload, "roster_inactive"));
    }
    if !context.session_or_epoch_valid {
        return Ok(reject(payload, "session_or_epoch_invalid"));
    }
    if ledger.has_command_id(&payload.command_id)? {
        return Ok(no_execute(payload, "command_replay_rejected", true));
    }
    if payload.created_at_time > context.now + time::Duration::seconds(MAX_CLOCK_SKEW_SECONDS) {
        return Ok(reject(payload, "created_at_clock_skew_rejected"));
    }
    if payload.expires_at_time <= context.now {
        return Ok(reject(payload, "command_expired"));
    }
    if denied_prefix(&payload.command_kind).is_some()
        || !ALLOWED_COMMANDS.contains(&payload.command_kind.as_str())
    {
        return Ok(reject(payload, "command_not_allowlisted"));
    }
    if let Some(minimum_risk) = minimum_risk_class(&payload.command_kind) {
        if payload.risk_class.rank() < minimum_risk.rank() {
            return Ok(reject(payload, "risk_class_understated"));
        }
    }
    if payload.target_binding.target_endpoint_id != context.local_endpoint_id {
        return Ok(reject(payload, "target_endpoint_mismatch"));
    }
    if !binding_allowed(
        payload.target_binding.workspace_id.as_deref(),
        &context.allowed_workspace_ids,
    ) {
        return Ok(reject(payload, "workspace_binding_rejected"));
    }
    if command_requires_agent_binding(&payload.command_kind) {
        if payload.target_binding.target_agent_id.is_none() {
            return Ok(reject(payload, "agent_binding_required"));
        }
        if payload.body().as_object().is_some_and(|body| {
            AGENT_RESOURCE_SELECTOR_FIELDS
                .iter()
                .any(|field| body.contains_key(*field))
        }) {
            return Ok(reject(payload, "agent_selector_must_use_target_binding"));
        }
    }
    if !binding_allowed(
        payload.target_binding.target_agent_id.as_deref(),
        &context.allowed_agent_ids,
    ) {
        return Ok(reject(payload, "agent_binding_rejected"));
    }
    if matches!(payload.risk_class, SecureCommandRiskClass::HighRisk)
        && (!trusted_for_high_risk(&context.sender_trust_state)
            || context.sender_endpoint_kind == "web_limited")
    {
        return Ok(reject(payload, "high_risk_sender_rejected"));
    }
    if confirmation_required(payload) && !context.user_confirmed {
        return Ok(no_execute(payload, "user_confirmation_required", false));
    }
    match ledger.record_execution(payload, context.now)? {
        SecureCommandReplayRecordStatus::Fresh => Ok(SecureCommandEvaluation {
            accepted: true,
            should_execute: true,
            replayed: false,
            code: "execute".to_string(),
            reason: "secure mesh command passed local receive gates".to_string(),
            command_id: payload.command_id.clone(),
            command_kind: payload.command_kind.clone(),
            risk_class: payload.risk_class.as_str().to_string(),
            requires_user_confirmation: payload.requires_user_confirmation,
        }),
        SecureCommandReplayRecordStatus::CommandReplay => {
            Ok(no_execute(payload, "command_replay_rejected", true))
        }
        SecureCommandReplayRecordStatus::IdempotentReplay => {
            Ok(no_execute(payload, "idempotent_replay", true))
        }
        SecureCommandReplayRecordStatus::IdempotencyConflict => {
            Ok(reject(payload, "idempotency_conflict"))
        }
    }
}

fn denied_prefix(command_kind: &str) -> Option<&'static str> {
    DENIED_PREFIXES
        .iter()
        .find(|prefix| command_kind.starts_with(**prefix))
        .copied()
}

fn binding_allowed(value: Option<&str>, allowed_values: &BTreeSet<String>) -> bool {
    match value {
        // An empty set means no bound value is authorized; it is never a
        // wildcard. Unbound commands remain valid only when no binding policy
        // was supplied for that dimension.
        None => allowed_values.is_empty(),
        Some(_) if allowed_values.is_empty() => false,
        Some(candidate) => allowed_values.contains(candidate),
    }
}

fn trusted_for_high_risk(trust_state: &DeviceTrustState) -> bool {
    matches!(
        trust_state,
        DeviceTrustState::Verified | DeviceTrustState::CrossSigned
    )
}

fn minimum_risk_class(command_kind: &str) -> Option<SecureCommandRiskClass> {
    match command_kind {
        "agent.sessions.list" | "agent.sessions.describe" => Some(SecureCommandRiskClass::ReadOnly),
        "agent.message.send" => Some(SecureCommandRiskClass::SafeWrite),
        "secure_mesh.device.verify" => Some(SecureCommandRiskClass::LocalEffect),
        _ => None,
    }
}

fn command_requires_agent_binding(command_kind: &str) -> bool {
    matches!(
        command_kind,
        "agent.sessions.list" | "agent.sessions.describe" | "agent.message.send"
    )
}

fn confirmation_required(payload: &SecureCommandPayload) -> bool {
    payload.requires_user_confirmation
        || matches!(
            payload.risk_class,
            SecureCommandRiskClass::LocalEffect | SecureCommandRiskClass::HighRisk
        )
}

fn reject(payload: &SecureCommandPayload, code: &str) -> SecureCommandEvaluation {
    SecureCommandEvaluation {
        accepted: false,
        should_execute: false,
        replayed: false,
        code: code.to_string(),
        reason: format!("secure mesh command rejected by {}", code),
        command_id: payload.command_id.clone(),
        command_kind: payload.command_kind.clone(),
        risk_class: payload.risk_class.as_str().to_string(),
        requires_user_confirmation: payload.requires_user_confirmation,
    }
}

fn no_execute(
    payload: &SecureCommandPayload,
    code: &str,
    replayed: bool,
) -> SecureCommandEvaluation {
    SecureCommandEvaluation {
        accepted: true,
        should_execute: false,
        replayed,
        code: code.to_string(),
        reason: format!("secure mesh command accepted without execution by {}", code),
        command_id: payload.command_id.clone(),
        command_kind: payload.command_kind.clone(),
        risk_class: payload.risk_class.as_str().to_string(),
        requires_user_confirmation: payload.requires_user_confirmation,
    }
}
