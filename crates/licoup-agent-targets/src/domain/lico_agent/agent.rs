use super::events::AgentEvent;
use super::loop_::run_turn;
use super::profiles::{base_system_prompt, plan_system_prompt};
use super::tools::{ToolRegistry, read_tool, write_plan_tool};
use super::transport::LlmTransport;
use crate::port::AgentTargetPort;
use serde_json::{Value, json};
use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AgentProfileKind {
    Base,
    Plan,
}

impl AgentProfileKind {
    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "base" | "agent" => Some(Self::Base),
            "plan" => Some(Self::Plan),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Base => "base",
            Self::Plan => "plan",
        }
    }
}

pub struct AgentConfig {
    pub profile: AgentProfileKind,
    pub model: String,
    pub workspace: PathBuf,
    pub plan_path: Option<PathBuf>,
}

pub struct Agent {
    config: AgentConfig,
    transport: Arc<dyn LlmTransport>,
    tools: ToolRegistry,
    history: Vec<Value>,
    abort: AtomicBool,
}

impl Agent {
    /// Load only the complete user/assistant history owned by one persisted
    /// Lico Agent session. The transcript header, not its filename, owns
    /// identity.
    pub fn load_persisted_history(
        path: &Path,
        expected_session_id: &str,
    ) -> Result<Vec<Value>, &'static str> {
        let file = File::open(path).map_err(|_| "lico_agent_transcript_missing")?;
        let mut header_seen = false;
        let mut history = Vec::new();
        for line in BufReader::new(file).lines() {
            let line = line.map_err(|_| "lico_agent_transcript_invalid")?;
            if line.trim().is_empty() {
                continue;
            }
            let value: Value =
                serde_json::from_str(&line).map_err(|_| "lico_agent_transcript_invalid")?;
            match value.get("type").and_then(Value::as_str) {
                Some("session") if !header_seen && history.is_empty() => {
                    let identity = value
                        .get("id")
                        .and_then(Value::as_str)
                        .map(str::trim)
                        .filter(|identity| !identity.is_empty())
                        .ok_or("lico_agent_transcript_invalid")?;
                    if identity != expected_session_id {
                        return Err("lico_agent_transcript_identity_mismatch");
                    }
                    header_seen = true;
                }
                Some("message") if header_seen => {
                    let role = value
                        .get("role")
                        .and_then(Value::as_str)
                        .filter(|role| matches!(*role, "user" | "assistant"))
                        .ok_or("lico_agent_transcript_invalid")?;
                    let content = value
                        .get("text")
                        .and_then(Value::as_str)
                        .ok_or("lico_agent_transcript_invalid")?;
                    history.push(json!({"role": role, "content": content}));
                }
                Some("usage.record")
                    if header_seen && value.get("usage").is_some_and(Value::is_object) => {}
                _ => return Err("lico_agent_transcript_invalid"),
            }
        }
        if !header_seen {
            return Err("lico_agent_transcript_invalid");
        }
        Ok(history)
    }

    pub fn new(config: AgentConfig, transport: Arc<dyn LlmTransport>) -> Result<Self, String> {
        let mut tools = ToolRegistry::new();
        tools.register(read_tool(config.workspace.clone()));
        if config.profile == AgentProfileKind::Plan {
            let plan = config
                .plan_path
                .clone()
                .ok_or_else(|| "plan_path_required".to_string())?;
            tools.register(write_plan_tool(plan));
        }
        Ok(Self {
            config,
            transport,
            tools,
            history: Vec::new(),
            abort: AtomicBool::new(false),
        })
    }

    pub fn inject_history(&mut self, messages: Vec<Value>) {
        self.history = messages;
    }

    pub fn history(&self) -> &[Value] {
        &self.history
    }

    pub fn profile(&self) -> AgentProfileKind {
        self.config.profile
    }

    pub fn abort(&self) {
        self.abort.store(true, Ordering::SeqCst);
    }

    pub fn prompt(
        &mut self,
        port: &AgentTargetPort,
        user_text: &str,
        mut on_event: impl FnMut(AgentEvent),
    ) -> Result<(), String> {
        self.abort.store(false, Ordering::SeqCst);
        on_event(AgentEvent::AgentStart);
        let user = json!({"role": "user", "content": user_text});
        on_event(AgentEvent::MessageStart {
            role: "user".into(),
        });
        on_event(AgentEvent::MessageEnd {
            role: "user".into(),
            content: user_text.to_string(),
        });
        self.history.push(user);
        let system = match self.config.profile {
            AgentProfileKind::Base => base_system_prompt(),
            AgentProfileKind::Plan => plan_system_prompt(),
        };
        let result = run_turn(
            port,
            self.transport.as_ref(),
            &self.config.model,
            system,
            &mut self.history,
            &self.tools,
            &self.abort,
            &mut on_event,
        );
        on_event(AgentEvent::AgentEnd);
        result
    }
}

#[cfg(test)]
mod tests {
    use super::Agent;
    use std::fs;

    #[test]
    fn persisted_history_requires_matching_header_and_keeps_every_turn() {
        let dir = std::env::temp_dir().join(format!("lico-agent-history-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&dir).unwrap();
        let session_id = uuid::Uuid::new_v4().to_string();
        let path = dir.join("session.jsonl");
        fs::write(
            &path,
            format!(
                "{{\"type\":\"session\",\"id\":\"{session_id}\"}}\n{{\"type\":\"message\",\"role\":\"user\",\"text\":\"first\"}}\n{{\"type\":\"message\",\"role\":\"assistant\",\"text\":\"answer\"}}\n{{\"type\":\"message\",\"role\":\"user\",\"text\":\"second\"}}\n"
            ),
        )
        .unwrap();

        let history = Agent::load_persisted_history(&path, &session_id).unwrap();
        assert_eq!(history.len(), 3);
        assert_eq!(history[0]["content"], "first");
        assert_eq!(history[1]["content"], "answer");
        assert_eq!(history[2]["content"], "second");
        assert_eq!(
            Agent::load_persisted_history(&path, &uuid::Uuid::new_v4().to_string()),
            Err("lico_agent_transcript_identity_mismatch")
        );

        fs::write(&path, "not-json\n").unwrap();
        assert_eq!(
            Agent::load_persisted_history(&path, &session_id),
            Err("lico_agent_transcript_invalid")
        );
        let _ = fs::remove_dir_all(dir);
    }
}
