//! Lico-owned agent process: stdio JSONL RPC, Gateway models, base|plan profiles.

use licoup_foundation::platform::file_security::{append_private_line, ensure_private_dir};
use licoup_foundation::platform::paths::portable_data_dir;
use licoup_native::domain::lico_agent::{
    Agent, AgentConfig, AgentEvent, AgentProfileKind, GatewayChatTransport, LlmTransport,
};
use serde_json::{Value, json};
use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::{env, process::ExitCode};
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;
use uuid::Uuid;

fn main() -> ExitCode {
    match run(env::args().skip(1).collect()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(code) => {
            let _ = writeln!(std::io::stderr(), "{}", json!({"type":"error","code":code}));
            ExitCode::FAILURE
        }
    }
}

#[derive(Debug)]
struct Args {
    profile: AgentProfileKind,
    gateway_base_url: String,
    model: String,
    workspace: PathBuf,
    plan_path: Option<PathBuf>,
    session_id: String,
    resume: bool,
}

fn run(raw: Vec<String>) -> Result<(), &'static str> {
    if raw.iter().any(|a| a == "--help" || a == "-h") {
        println!(
            "lico-agent --mode rpc --profile base|plan --gateway-base-url http://127.0.0.1:PORT --model ID --workspace ABS --session-id UUID [--resume] [--plan-path ABS]"
        );
        return Ok(());
    }
    let _data_home_access =
        licoup_foundation::platform::data_home_access::acquire_process_data_home_access()
            .map_err(|_| "data_home_access_unavailable")?;
    let args = parse_args(&raw)?;
    let transport = GatewayChatTransport::from_base_url(&args.gateway_base_url)
        .map_err(|_| "gateway_base_url_must_be_loopback")?;
    let transport: Arc<dyn LlmTransport> = Arc::new(transport);
    let transcript_path = transcript_path(&args.session_id)?;
    let history = if args.resume {
        Agent::load_persisted_history(&transcript_path, &args.session_id)?
    } else {
        if transcript_path.exists() {
            return Err("lico_agent_session_already_exists");
        }
        Vec::new()
    };
    let mut agent = Agent::new(
        AgentConfig {
            profile: args.profile,
            model: args.model,
            workspace: args.workspace.clone(),
            plan_path: args.plan_path,
        },
        transport,
    )
    .map_err(|_| "agent_config_invalid")?;
    agent.inject_history(history);
    let agent = Arc::new(Mutex::new(agent));
    let stdin = std::io::stdin();
    let mut stdout = std::io::stdout();
    let reader = BufReader::new(stdin.lock());
    for line in reader.lines() {
        let line = line.map_err(|_| "stdin_read_failed")?;
        if line.trim().is_empty() {
            continue;
        }
        let request: Value = serde_json::from_str(&line).map_err(|_| "rpc_request_invalid")?;
        let id = request.get("id").cloned().unwrap_or(Value::Null);
        let req_type = request.get("type").and_then(Value::as_str).unwrap_or("");
        match req_type {
            "get_state" => {
                let profile = agent.lock().unwrap().profile().as_str();
                write_json(
                    &mut stdout,
                    &json!({
                        "id": id,
                        "type": "response",
                        "success": true,
                        "data": {
                            "isRunning": false,
                            "profile": profile,
                            "sessionId": args.session_id,
                        }
                    }),
                )?;
            }
            "prompt" => {
                let message = request
                    .get("message")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string();
                write_json(
                    &mut stdout,
                    &json!({"id": id, "type": "response", "success": true}),
                )?;
                run_prompt(
                    &agent,
                    &mut stdout,
                    &message,
                    &transcript_path,
                    &args.session_id,
                    &args.workspace,
                )?;
            }
            "abort" => {
                agent.lock().unwrap().abort();
                write_json(
                    &mut stdout,
                    &json!({"id": id, "type": "response", "success": true}),
                )?;
            }
            "steer" => {
                // v1: treat steer as a follow-up prompt when idle.
                let message = request
                    .get("message")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string();
                write_json(
                    &mut stdout,
                    &json!({"id": id, "type": "response", "success": true}),
                )?;
                run_prompt(
                    &agent,
                    &mut stdout,
                    &message,
                    &transcript_path,
                    &args.session_id,
                    &args.workspace,
                )?;
            }
            _ => {
                write_json(
                    &mut stdout,
                    &json!({
                        "id": id,
                        "type": "response",
                        "success": false,
                        "error": "unsupported_request"
                    }),
                )?;
            }
        }
    }
    Ok(())
}

fn run_prompt(
    agent: &Arc<Mutex<Agent>>,
    out: &mut impl Write,
    message: &str,
    transcript_path: &std::path::Path,
    session_id: &str,
    workspace: &std::path::Path,
) -> Result<(), &'static str> {
    let mut assistant_output = String::new();
    let mut usage_records = Vec::new();
    let result = {
        let mut guard = agent.lock().unwrap();
        guard.prompt(&licoup_native::agent_target_port(), message, |event| {
            if let AgentEvent::MessageUpdate { role, delta } = &event
                && role == "assistant"
            {
                assistant_output.push_str(delta);
            }
            if let AgentEvent::Usage { model, usage } = &event {
                usage_records.push((OffsetDateTime::now_utc(), usage_record(model, usage)));
            }
            if !matches!(event, AgentEvent::AgentEnd) {
                let _ = emit_event(out, &event);
            }
        })
    };
    if let Err(err) = result {
        // Earlier model calls still consumed tokens when a later call failed.
        if persist_usage_records(transcript_path, session_id, workspace, &usage_records).is_err() {
            emit_persist_error(out)?;
        }
        write_json(
            out,
            &json!({"type":"error","code":"prompt_failed","message":err}),
        )?;
        return Ok(());
    }
    if persist_turn(
        transcript_path,
        session_id,
        workspace,
        message,
        &assistant_output,
        &usage_records,
    )
    .is_err()
    {
        emit_persist_error(out)?;
        return Ok(());
    }
    emit_event(out, &AgentEvent::AgentEnd)
}

fn transcript_path(session_id: &str) -> Result<PathBuf, &'static str> {
    let canonical = Uuid::parse_str(session_id)
        .map_err(|_| "lico_agent_session_id_invalid")?
        .to_string();
    if canonical != session_id {
        return Err("lico_agent_session_id_invalid");
    }
    let sessions = portable_data_dir()
        .map_err(|_| "lico_agent_session_store_unavailable")?
        .join("client-state")
        .join("lico-agent")
        .join("sessions");
    ensure_private_dir(&sessions).map_err(|_| "lico_agent_session_store_unavailable")?;
    Ok(sessions.join(format!("{session_id}.jsonl")))
}

fn persist_turn(
    path: &std::path::Path,
    session_id: &str,
    workspace: &std::path::Path,
    prompt: &str,
    output: &str,
    usage_records: &[(OffsetDateTime, Value)],
) -> Result<(), ()> {
    let timestamp = OffsetDateTime::now_utc().format(&Rfc3339).map_err(|_| ())?;
    persist_session_header(path, session_id, workspace, &timestamp)?;
    append_private_line(
        path,
        &json!({
            "type": "message",
            "role": "user",
            "text": prompt,
            "timestamp": timestamp,
        })
        .to_string(),
    )
    .map_err(|_| ())?;
    append_private_line(
        path,
        &json!({
            "type": "message",
            "role": "assistant",
            "text": output,
            "timestamp": timestamp,
        })
        .to_string(),
    )
    .map_err(|_| ())?;
    persist_usage_records(path, session_id, workspace, usage_records)
}

fn persist_session_header(
    path: &std::path::Path,
    session_id: &str,
    workspace: &std::path::Path,
    timestamp: &str,
) -> Result<(), ()> {
    if !path.exists() {
        append_private_line(
            path,
            &json!({
                "type": "session",
                "id": session_id,
                "cwd": workspace.to_string_lossy(),
                "timestamp": timestamp,
            })
            .to_string(),
        )
        .map_err(|_| ())?;
    }
    Ok(())
}

fn persist_usage_records(
    path: &std::path::Path,
    session_id: &str,
    workspace: &std::path::Path,
    records: &[(OffsetDateTime, Value)],
) -> Result<(), ()> {
    for (observed_at, record) in records {
        let timestamp = observed_at.format(&Rfc3339).map_err(|_| ())?;
        persist_session_header(path, session_id, workspace, &timestamp)?;
        let mut record = record.clone();
        record["timestamp"] = json!(timestamp);
        append_private_line(path, &record.to_string()).map_err(|_| ())?;
    }
    Ok(())
}

fn usage_record(model: &Option<String>, usage: &Value) -> Value {
    json!({"type": "usage.record", "model": model, "usage": usage})
}

fn emit_persist_error(out: &mut impl Write) -> Result<(), &'static str> {
    write_json(
        out,
        &json!({
            "type":"error",
            "code":"lico_agent_transcript_persist_failed",
            "message":"Lico Agent could not persist the completed turn."
        }),
    )
}

fn emit_event(out: &mut impl Write, event: &AgentEvent) -> Result<(), &'static str> {
    let value = match event {
        AgentEvent::AgentStart => json!({"type":"agent_start"}),
        AgentEvent::AgentEnd => json!({"type":"agent_end"}),
        AgentEvent::TurnStart => json!({"type":"turn_start"}),
        AgentEvent::TurnEnd => json!({"type":"turn_end"}),
        AgentEvent::MessageUpdate { role, delta } if role == "assistant" => json!({
            "type": "message_update",
            "assistantMessageEvent": { "type": "text_delta", "delta": delta }
        }),
        AgentEvent::MessageEnd { role, content } if role == "assistant" => json!({
            "type": "message_end",
            "message": { "role": "assistant", "content": content }
        }),
        AgentEvent::Usage { model, usage } => usage_record(model, usage),
        AgentEvent::ToolExecutionStart { name, call_id } => json!({
            "type": "tool_execution_start",
            "toolName": name,
            "toolCallId": call_id
        }),
        AgentEvent::ToolExecutionEnd {
            name,
            call_id,
            ok,
            output,
        } => json!({
            "type": "tool_execution_end",
            "toolName": name,
            "toolCallId": call_id,
            "ok": ok,
            "output": output
        }),
        AgentEvent::Error { code, message } => {
            json!({"type":"error","code":code,"message":message})
        }
        _ => return Ok(()),
    };
    write_json(out, &value)
}

fn write_json(out: &mut impl Write, value: &Value) -> Result<(), &'static str> {
    let mut line = serde_json::to_string(value).map_err(|_| "json_encode_failed")?;
    line.push('\n');
    out.write_all(line.as_bytes())
        .map_err(|_| "stdout_write_failed")?;
    out.flush().map_err(|_| "stdout_flush_failed")?;
    Ok(())
}

fn parse_args(raw: &[String]) -> Result<Args, &'static str> {
    let mut profile = AgentProfileKind::Base;
    let mut gateway_base_url = "http://127.0.0.1:15722".to_string();
    let mut model = String::new();
    let mut workspace = None;
    let mut plan_path = None;
    let mut session_id = None;
    let mut resume = false;
    let mut i = 0;
    while i < raw.len() {
        match raw[i].as_str() {
            "--mode" => {
                i += 1;
                // rpc is the only supported mode
            }
            "--profile" => {
                i += 1;
                profile = AgentProfileKind::parse(raw.get(i).map(String::as_str).unwrap_or(""))
                    .ok_or("profile_invalid")?;
            }
            "--gateway-base-url" => {
                i += 1;
                gateway_base_url = raw.get(i).cloned().ok_or("gateway_base_url_missing")?;
            }
            "--model" => {
                i += 1;
                model = raw.get(i).cloned().ok_or("model_missing")?;
            }
            "--workspace" => {
                i += 1;
                workspace = Some(PathBuf::from(raw.get(i).ok_or("workspace_missing")?));
            }
            "--plan-path" => {
                i += 1;
                plan_path = Some(PathBuf::from(raw.get(i).ok_or("plan_path_missing")?));
            }
            "--session-id" => {
                i += 1;
                session_id = Some(raw.get(i).cloned().ok_or("session_id_missing")?);
            }
            "--resume" => resume = true,
            _ => {}
        }
        i += 1;
    }
    if model.trim().is_empty() {
        return Err("model_required");
    }
    let workspace = workspace.ok_or("workspace_missing")?;
    if !workspace.is_absolute() {
        return Err("workspace_must_be_absolute");
    }
    let session_id = session_id.ok_or("session_id_missing")?;
    let canonical_session_id = Uuid::parse_str(&session_id)
        .map_err(|_| "session_id_invalid")?
        .to_string();
    if session_id != canonical_session_id {
        return Err("session_id_invalid");
    }
    if profile == AgentProfileKind::Plan {
        let path = plan_path.ok_or("plan_path_required")?;
        if !path.is_absolute() {
            return Err("plan_path_must_be_absolute");
        }
        plan_path = Some(path);
    }
    Ok(Args {
        profile,
        gateway_base_url,
        model,
        workspace,
        plan_path,
        session_id,
        resume,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use licoup_native::domain::lico_agent::TransportError;
    use std::collections::VecDeque;

    struct FixtureTransport {
        responses: Mutex<VecDeque<Result<Value, TransportError>>>,
    }

    impl LlmTransport for FixtureTransport {
        fn complete(
            &self,
            model: &str,
            _messages: &[Value],
            _tools: &[Value],
        ) -> Result<Value, TransportError> {
            assert_eq!(model, "requested-model");
            self.responses.lock().unwrap().pop_front().unwrap()
        }
    }

    fn fixture_agent(
        workspace: &std::path::Path,
        responses: Vec<Result<Value, TransportError>>,
    ) -> Arc<Mutex<Agent>> {
        Arc::new(Mutex::new(
            Agent::new(
                AgentConfig {
                    profile: AgentProfileKind::Base,
                    model: "requested-model".into(),
                    workspace: workspace.to_owned(),
                    plan_path: None,
                },
                Arc::new(FixtureTransport {
                    responses: Mutex::new(responses.into()),
                }),
            )
            .unwrap(),
        ))
    }

    fn fixture_response(message: Value) -> Value {
        json!({
            "model": "actual-model",
            "reasoning_effort": "high",
            "usage": {
                "prompt_tokens": 12,
                "completion_tokens": 3,
                "total_tokens": 15,
                "prompt_tokens_details": {"cached_tokens": 4},
                "private": "private-usage-canary"
            },
            "choices": [{"message": message}]
        })
    }

    fn fixture_tool_call() -> Value {
        json!({
            "role": "assistant",
            "content": null,
            "tool_calls": [{
                "id": "fixture-call",
                "type": "function",
                "function": {"name": "read", "arguments": "{\"path\":\"input.txt\"}"}
            }]
        })
    }

    #[test]
    fn cli_requires_fixed_native_session_identity_and_explicit_workspace() {
        let session_id = Uuid::new_v4().to_string();
        let workspace = std::env::temp_dir().join("lico-agent-cli-workspace-fixture");
        let args = parse_args(&[
            "--model".into(),
            "synthetic-model".into(),
            "--workspace".into(),
            workspace.to_string_lossy().into_owned(),
            "--session-id".into(),
            session_id.clone(),
            "--resume".into(),
        ])
        .unwrap();
        assert_eq!(args.session_id, session_id);
        assert!(args.resume);
        assert_eq!(args.workspace, workspace);
        assert_eq!(
            parse_args(&[
                "--model".into(),
                "synthetic-model".into(),
                "--workspace".into(),
                workspace.to_string_lossy().into_owned(),
            ])
            .unwrap_err(),
            "session_id_missing"
        );
    }

    #[test]
    fn completed_turn_is_persisted_before_terminal_success() {
        let dir = std::env::temp_dir().join(format!("lico-agent-bin-{}", Uuid::new_v4()));
        ensure_private_dir(&dir).unwrap();
        let session_id = Uuid::new_v4().to_string();
        let path = dir.join(format!("{session_id}.jsonl"));
        persist_turn(
            &path,
            &session_id,
            &dir,
            "synthetic prompt",
            "synthetic response",
            &[],
        )
        .unwrap();
        let history = Agent::load_persisted_history(&path, &session_id).unwrap();
        assert_eq!(history.len(), 2);
        assert_eq!(history[0]["content"], "synthetic prompt");
        assert_eq!(history[1]["content"], "synthetic response");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn tool_loop_persists_each_identical_usage_call_without_estimate_double_counting() {
        let dir = std::env::temp_dir().join(format!("lico-agent-usage-{}", Uuid::new_v4()));
        let sessions = dir.join("sessions");
        ensure_private_dir(&sessions).unwrap();
        std::fs::write(dir.join("input.txt"), "synthetic tool output").unwrap();
        let session_id = Uuid::new_v4().to_string();
        let path = sessions.join(format!("{session_id}.jsonl"));
        let agent = fixture_agent(
            &dir,
            vec![
                Ok(fixture_response(fixture_tool_call())),
                Ok(fixture_response(
                    json!({"role": "assistant", "content": "synthetic answer"}),
                )),
            ],
        );
        let mut output = Vec::new();
        run_prompt(
            &agent,
            &mut output,
            "synthetic prompt",
            &path,
            &session_id,
            &dir,
        )
        .unwrap();

        let events = String::from_utf8(output)
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str::<Value>(line).unwrap())
            .collect::<Vec<_>>();
        let live_usage = events
            .iter()
            .filter(|event| event["type"] == "usage.record")
            .collect::<Vec<_>>();
        assert_eq!(live_usage.len(), 2);
        assert_eq!(live_usage[0], live_usage[1]);
        assert_eq!(events.last().unwrap()["type"], "agent_end");
        assert!(
            events
                .iter()
                .any(|event| event["type"] == "tool_execution_end" && event["ok"] == true)
        );
        assert_eq!(
            agent.lock().unwrap().history()[2]["content"],
            "synthetic tool output"
        );

        let records = std::fs::read_to_string(&path)
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str::<Value>(line).unwrap())
            .collect::<Vec<_>>();
        assert_eq!(records.len(), 5);
        assert_eq!(records[1]["type"], "message");
        assert_eq!(records[2]["type"], "message");
        for record in &records[3..] {
            assert_eq!(record["type"], "usage.record");
            assert_eq!(record["model"], "actual-model");
            assert_eq!(record["usage"]["reasoningEffort"], "high");
            assert_eq!(record["usage"]["totalTokens"], 15);
            assert!(record["usage"].get("fast").is_none());
            assert!(!record.to_string().contains("synthetic tool output"));
            assert!(!record.to_string().contains("private-usage-canary"));
        }
        let history = Agent::load_persisted_history(&path, &session_id).unwrap();
        assert_eq!(
            history,
            vec![
                json!({"role": "user", "content": "synthetic prompt"}),
                json!({"role": "assistant", "content": "synthetic answer"}),
            ]
        );

        licoup_native::domain::agent_usage::host::install()
            .expect("statistics host installs once per process");
        let params = json!({
            "agent": "lico-agent",
            "root": sessions,
            "stateRoot": dir.join("state"),
            "now": OffsetDateTime::now_utc().format(&Rfc3339).unwrap(),
            "forceRefresh": true,
        });
        for _ in 0..2 {
            let report = licoup_native::domain::agent_usage::scan(&params).unwrap();
            assert_eq!(report["summary"]["promptTokens"], 24);
            assert_eq!(report["summary"]["completionTokens"], 6);
            assert_eq!(report["summary"]["totalTokens"], 30);
            assert_eq!(report["agents"][0]["history"]["cachedInputTokens"], 8);
            let sources = &report["agents"][0]["history"]["tokenSourceBreakdown"];
            assert_eq!(sources["explicitRecords"], 2);
            assert_eq!(sources["estimatedTotalTokens"], 0);
        }
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn later_transport_failure_preserves_completed_call_usage_without_fake_reply() {
        let dir = std::env::temp_dir().join(format!("lico-agent-usage-failure-{}", Uuid::new_v4()));
        ensure_private_dir(&dir).unwrap();
        std::fs::write(dir.join("input.txt"), "synthetic tool output").unwrap();
        let session_id = Uuid::new_v4().to_string();
        let path = dir.join(format!("{session_id}.jsonl"));
        let agent = fixture_agent(
            &dir,
            vec![
                Ok(fixture_response(fixture_tool_call())),
                Err(TransportError::Connect),
            ],
        );
        let mut output = Vec::new();
        run_prompt(
            &agent,
            &mut output,
            "synthetic prompt",
            &path,
            &session_id,
            &dir,
        )
        .unwrap();
        let records = std::fs::read_to_string(&path)
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str::<Value>(line).unwrap())
            .collect::<Vec<_>>();
        assert_eq!(records.len(), 2);
        assert_eq!(records[1]["type"], "usage.record");
        assert_eq!(records[1]["usage"]["totalTokens"], 15);
        assert!(
            Agent::load_persisted_history(&path, &session_id)
                .unwrap()
                .is_empty()
        );
        let events = String::from_utf8(output).unwrap();
        assert!(events.contains("prompt_failed"));
        assert!(!events.contains("agent_end"));
        let _ = std::fs::remove_dir_all(dir);
    }
}
