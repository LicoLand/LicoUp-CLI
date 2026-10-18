use super::*;

pub fn evaluate_secure_command_json(
    payload: &Value,
    context: &Value,
    ledger: &mut SecureCommandReplayLedger,
) -> Result<Value> {
    let payload = SecureCommandPayload::from_value(payload)?;
    let context = SecureCommandEvaluationContext::from_value(context)?;
    Ok(evaluate_secure_command(&payload, &context, ledger)?.to_json())
}

pub fn execute_secure_command_json(
    payload: &Value,
    context: &Value,
    ledger: &mut impl SecureCommandReplayStore,
    executor: &mut impl SecureCommandLocalExecutor,
    completed_at: impl Into<String>,
) -> Result<Value> {
    let payload = SecureCommandPayload::from_value(payload)?;
    let context = SecureCommandEvaluationContext::from_value(context)?;
    match ledger.prior_execution(&payload)? {
        SecureCommandPriorExecution::Completed(outcome) => return Ok(outcome),
        SecureCommandPriorExecution::Reserved => {
            return Ok(reserved_execution_outcome(&payload));
        }
        SecureCommandPriorExecution::Conflict => {
            bail!("secure mesh command replay binding conflicts with reserved execution");
        }
        SecureCommandPriorExecution::Missing => {}
    }
    let evaluation = evaluate_secure_command(&payload, &context, ledger)?;
    let execution_reserved = evaluation.should_execute;
    let outcome =
        execute_evaluated_secure_command(&payload, &evaluation, executor, completed_at.into())?;
    let result = json!({
        "ok": true,
        "protocolVersion": SECURE_MESH_COMMAND_PROTOCOL_VERSION,
        "evaluation": evaluation.to_json(),
        "execution": command_execution_outcome_json(outcome),
        "bodyRedacted": true,
    });
    if execution_reserved {
        ledger.record_completed_outcome(&payload, &result)?;
    }
    Ok(result)
}

fn reserved_execution_outcome(payload: &SecureCommandPayload) -> Value {
    json!({
        "ok": true,
        "protocolVersion": SECURE_MESH_COMMAND_PROTOCOL_VERSION,
        "evaluation": {
            "ok": true,
            "protocolVersion": SECURE_MESH_COMMAND_PROTOCOL_VERSION,
            "accepted": true,
            "shouldExecute": false,
            "replayed": true,
            "code": "execution_outcome_unknown",
            "reason": "secure mesh command execution was reserved before an interrupted outcome",
            "commandId": payload.command_id,
            "commandKind": payload.command_kind,
            "riskClass": payload.risk_class.as_str(),
            "requiresUserConfirmation": payload.requires_user_confirmation,
            "bodyRedacted": true,
        },
        "execution": {
            "outcome": "error",
            "commandId": payload.command_id,
            "idempotencyKey": payload.idempotency_key,
            "errorCode": "execution_outcome_unknown",
            "retryable": false,
            "errorDetail": LOCAL_EXECUTION_FAILED_REMOTE_DETAIL,
        },
        "bodyRedacted": true,
    })
}

fn command_execution_outcome_json(outcome: SecureCommandExecutionOutcome) -> Value {
    match outcome {
        SecureCommandExecutionOutcome::Result(result) => json!({
            "outcome": "result",
            "commandId": result.command_id,
            "idempotencyKey": result.idempotency_key,
            "outputContentType": result.output_content_type,
            "completedAt": result.completed_at,
            "output": response_output_json(&result.output),
        }),
        SecureCommandExecutionOutcome::Error(error) => json!({
            "outcome": "error",
            "commandId": error.command_id,
            "idempotencyKey": error.idempotency_key,
            "errorCode": error.error_code,
            "retryable": error.retryable,
            "occurredAt": error.occurred_at,
            "errorDetail": error.error_detail,
        }),
    }
}

fn response_output_json(output: &[u8]) -> Value {
    serde_json::from_slice(output).unwrap_or_else(|_| {
        json!({
            "base64": general_purpose::URL_SAFE_NO_PAD.encode(output),
        })
    })
}
