use super::CodexParser;
use super::helpers::{
    final_agent_message, matches_current_ids, mcp_tool_application_error, mcp_tool_name,
};
use crate::app_server::model::{ProtocolEffect, ProtocolFailure, ProtocolOutcome, ProtocolPhase};
use serde_json::Value;

impl CodexParser {
    pub(super) fn handle_notification(&mut self, message: &Value) -> Vec<ProtocolEffect> {
        match message.get("method").and_then(Value::as_str) {
            Some("item/started") => {
                self.observe_processing_item(message);
                Vec::new()
            }
            Some("item/completed") => {
                self.capture_completed_item(message);
                Vec::new()
            }
            Some("item/agentMessage/delta") | Some("item/agentMessage/updated") => {
                self.emit_agent_message_delta(message);
                Vec::new()
            }
            Some("turn/completed") => self.handle_turn_completed(message),
            _ => Vec::new(),
        }
    }

    fn capture_completed_item(&mut self, message: &Value) {
        if self.phase != ProtocolPhase::AwaitTurnCompleted {
            return;
        }
        let Some(params) = message.get("params") else {
            return;
        };
        if !matches_current_ids(params, self.thread_id.as_deref(), self.turn_id.as_deref()) {
            return;
        }
        if let Some(item) = params.get("item") {
            self.emit_processing_item_once(item, false);
            self.emit_mcp_tool_error(item);
            self.completed_items.push(item.clone());
            if item.get("type").and_then(Value::as_str) == Some("agentMessage")
                && let Some(text) = item.get("text").and_then(Value::as_str)
            {
                crate::port::turn_event::emit_agent_message_completed(
                    self.thread_id.as_deref().unwrap_or_default(),
                    self.turn_id.as_deref().unwrap_or_default(),
                    text,
                );
            }
        }
    }

    fn observe_processing_item(&mut self, message: &Value) {
        if self.phase != ProtocolPhase::AwaitTurnCompleted {
            return;
        }
        let Some(params) = message.get("params") else {
            return;
        };
        if !matches_current_ids(params, self.thread_id.as_deref(), self.turn_id.as_deref()) {
            return;
        }
        if let Some(item) = params.get("item") {
            self.emit_processing_item_once(item, true);
        }
    }

    fn emit_processing_item_once(&mut self, item: &Value, emit_without_identity: bool) {
        let item_id = item.get("id").and_then(Value::as_str).unwrap_or_default();
        if item_id.is_empty() {
            if emit_without_identity {
                self.unidentified_processing_items += 1;
            } else if self.unidentified_processing_items > 0 {
                self.unidentified_processing_items -= 1;
                return;
            }
        } else if !self.observed_processing_items.insert(item_id.to_owned()) {
            return;
        }
        self.emit_processing_item(item);
    }

    fn emit_processing_item(&self, item: &Value) {
        let item_type = item.get("type").and_then(Value::as_str).unwrap_or_default();
        if item_type == "agentMessage" {
            return;
        }
        let evidence_kind = if item_type.contains("reason") {
            "reasoning"
        } else if item_type.contains("plan") {
            "plan"
        } else if item_type.contains("command")
            || item_type.contains("tool")
            || item_type.contains("mcp")
        {
            "tool"
        } else {
            "activity"
        };
        crate::port::turn_event::emit_agent_processing(
            self.thread_id.as_deref().unwrap_or_default(),
            self.turn_id.as_deref().unwrap_or_default(),
            evidence_kind,
            mcp_tool_name(item),
        );
    }

    fn emit_mcp_tool_error(&self, item: &Value) {
        let Some((tool_name, error_code)) = mcp_tool_application_error(item) else {
            return;
        };
        crate::port::turn_event::emit_agent_tool_error(
            self.thread_id.as_deref().unwrap_or_default(),
            self.turn_id.as_deref().unwrap_or_default(),
            tool_name,
            error_code,
        );
    }

    fn emit_agent_message_delta(&self, message: &Value) {
        if self.phase != ProtocolPhase::AwaitTurnCompleted {
            return;
        }
        let Some(params) = message.get("params") else {
            return;
        };
        if !matches_current_ids(params, self.thread_id.as_deref(), self.turn_id.as_deref()) {
            return;
        }
        let text = params
            .get("delta")
            .and_then(Value::as_str)
            .or_else(|| params.get("text").and_then(Value::as_str))
            .or_else(|| {
                params
                    .get("item")
                    .and_then(|item| item.get("text"))
                    .and_then(Value::as_str)
            })
            .unwrap_or_default();
        if !text.is_empty() {
            crate::port::turn_event::emit_agent_message_chunk(
                self.thread_id.as_deref().unwrap_or_default(),
                self.turn_id.as_deref().unwrap_or_default(),
                text,
            );
        }
    }

    fn handle_turn_completed(&mut self, message: &Value) -> Vec<ProtocolEffect> {
        if self.phase != ProtocolPhase::AwaitTurnCompleted {
            return Vec::new();
        }
        let Some(params) = message.get("params") else {
            return Vec::new();
        };
        let Some(turn) = params.get("turn") else {
            return Vec::new();
        };
        let thread_matches = params
            .get("threadId")
            .and_then(Value::as_str)
            .zip(self.thread_id.as_deref())
            .is_some_and(|(actual, expected)| actual == expected);
        let turn_matches = turn
            .get("id")
            .and_then(Value::as_str)
            .zip(self.turn_id.as_deref())
            .is_some_and(|(actual, expected)| actual == expected);
        if !thread_matches || !turn_matches {
            return Vec::new();
        }

        let status = turn
            .get("status")
            .and_then(Value::as_str)
            .unwrap_or("failed")
            .to_string();
        let final_message = turn
            .get("items")
            .and_then(Value::as_array)
            .and_then(|items| final_agent_message(items))
            .or_else(|| final_agent_message(&self.completed_items));

        self.phase = ProtocolPhase::Finished;
        if status != "completed" {
            let class = closed_codex_error_class(turn);
            let (code, message) = turn_failure(status.as_str(), class);
            let turn_status = turn_status_token(status.as_str(), class);
            crate::port::turn_event::emit_turn_event(
                "dispatch.turn.failed",
                self.thread_id.as_deref().unwrap_or_default(),
                self.turn_id.as_deref().unwrap_or_default(),
                serde_json::json!({
                    "turnStatus": turn_status,
                    "code": code,
                }),
            );
            let failure = ProtocolFailure::new(code, message, "turn/completed");
            let failure = if code == "codex_usage_limit_exceeded" {
                failure.with_resolution(
                    "native_cli",
                    false,
                    "select_available_model_or_wait_for_quota_reset",
                )
            } else {
                failure
            };
            let mut failure = self.contextualize(failure);
            failure.turn_status = Some(turn_status);
            return vec![ProtocolEffect::Fail(failure)];
        }
        let Some(output) = final_message else {
            crate::port::turn_event::emit_turn_event(
                "dispatch.turn.failed",
                self.thread_id.as_deref().unwrap_or_default(),
                self.turn_id.as_deref().unwrap_or_default(),
                serde_json::json!({
                    "turnStatus": status,
                    "code": "codex_final_message_missing",
                }),
            );
            let mut failure = self.contextualize(ProtocolFailure::new(
                "codex_final_message_missing",
                "Codex completed the turn without a final agent message.",
                "turn/completed",
            ));
            failure.turn_status = Some(status);
            return vec![ProtocolEffect::Fail(failure)];
        };
        crate::port::turn_event::emit_turn_event(
            "dispatch.turn.completed",
            self.thread_id.as_deref().unwrap_or_default(),
            self.turn_id.as_deref().unwrap_or_default(),
            serde_json::json!({ "turnStatus": "completed" }),
        );

        vec![ProtocolEffect::Complete(Box::new(ProtocolOutcome {
            output,
            session_id: self.session_id.clone().unwrap_or_default(),
            thread_id: self.thread_id.clone().unwrap_or_default(),
            turn_id: self.turn_id.clone().unwrap_or_default(),
            turn_status: status,
            effective: self.effective.clone(),
        }))]
    }
}

fn closed_codex_error_class(turn: &Value) -> Option<&'static str> {
    let info = turn.get("error")?.get("codexErrorInfo")?;
    if let Some(class) = info
        .as_str()
        .or_else(|| info.get("type").and_then(Value::as_str))
        .and_then(canonical_codex_error_class)
    {
        return Some(class);
    }
    info.as_object()?
        .keys()
        .find_map(|key| canonical_codex_error_class(key))
}

fn canonical_codex_error_class(raw: &str) -> Option<&'static str> {
    match raw {
        "Unauthorized" | "unauthorized" => Some("Unauthorized"),
        "UsageLimitExceeded" | "usageLimitExceeded" => Some("UsageLimitExceeded"),
        "UsageNotIncluded" | "usageNotIncluded" => Some("UsageNotIncluded"),
        "ContextWindowExceeded" | "contextWindowExceeded" => Some("ContextWindowExceeded"),
        "BadRequest" | "badRequest" => Some("BadRequest"),
        "SandboxError" | "sandboxError" => Some("SandboxError"),
        "InternalServerError" | "internalServerError" => Some("InternalServerError"),
        "ServerOverloaded" | "serverOverloaded" => Some("ServerOverloaded"),
        "SessionBudgetExceeded" | "sessionBudgetExceeded" => Some("SessionBudgetExceeded"),
        "CyberPolicy" | "cyberPolicy" => Some("CyberPolicy"),
        "MisalignmentPolicyViolation" | "misalignmentPolicyViolation" => {
            Some("MisalignmentPolicyViolation")
        }
        "ThreadRollbackFailed" | "threadRollbackFailed" => Some("ThreadRollbackFailed"),
        "ResponseStreamConnectionError"
        | "responseStreamConnectionError"
        | "ResponseStreamConnectionFailed"
        | "responseStreamConnectionFailed" => Some("ResponseStreamConnectionFailed"),
        "ResponseTooManyFailedAttempts" | "responseTooManyFailedAttempts" => {
            Some("ResponseTooManyFailedAttempts")
        }
        "ResponseStreamInterrupted"
        | "responseStreamInterrupted"
        | "ResponseStreamDisconnected"
        | "responseStreamDisconnected" => Some("ResponseStreamDisconnected"),
        "HttpConnectionFailed" | "httpConnectionFailed" => Some("HttpConnectionFailed"),
        "ActiveTurnNotSteerable" | "activeTurnNotSteerable" => Some("ActiveTurnNotSteerable"),
        "Other" | "other" => Some("Other"),
        _ => None,
    }
}

fn turn_failure(status: &str, class: Option<&str>) -> (&'static str, &'static str) {
    let message = match (status, class) {
        ("interrupted", _) => "Codex interrupted the requested turn.",
        (_, Some("Unauthorized")) => "Codex rejected the turn as unauthorized.",
        (_, Some("UsageLimitExceeded")) => "Codex usage limit exceeded.",
        (_, Some("UsageNotIncluded")) => "Codex usage is not included for this account.",
        (_, Some("ContextWindowExceeded")) => "Codex context window was exceeded.",
        (_, Some("BadRequest")) => "Codex rejected the turn as a bad request.",
        (_, Some("SandboxError")) => "Codex sandbox rejected the turn.",
        (_, Some("InternalServerError")) => "Codex reported an internal server error.",
        (_, Some("ServerOverloaded")) => "Codex is temporarily overloaded.",
        (_, Some("SessionBudgetExceeded")) => "Codex session budget was exceeded.",
        (_, Some("CyberPolicy" | "MisalignmentPolicyViolation")) => {
            "Codex rejected the turn under its policy."
        }
        (_, Some("ThreadRollbackFailed")) => "Codex could not roll back the thread.",
        (_, Some("ResponseStreamConnectionFailed")) => {
            "Codex could not establish the response stream."
        }
        (_, Some("ResponseTooManyFailedAttempts")) => {
            "Codex stopped after too many failed attempts."
        }
        (_, Some("ResponseStreamDisconnected")) => "Codex response stream disconnected.",
        (_, Some("HttpConnectionFailed")) => "Codex HTTP connection failed.",
        (_, Some("ActiveTurnNotSteerable")) => "Codex could not steer the active turn.",
        _ => "Codex did not complete the requested turn.",
    };
    let code = match class {
        Some("UsageLimitExceeded") => "codex_usage_limit_exceeded",
        _ => "codex_turn_not_completed",
    };
    (code, message)
}

fn turn_status_token(status: &str, class: Option<&str>) -> String {
    match class {
        Some(class) if status == "failed" => format!("{status}/{class}"),
        _ => status.to_string(),
    }
}
