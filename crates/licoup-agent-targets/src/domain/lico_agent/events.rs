use serde_json::Value;

#[derive(Clone, Debug)]
pub enum AgentEvent {
    AgentStart,
    AgentEnd,
    TurnStart,
    TurnEnd,
    MessageStart {
        role: String,
    },
    MessageUpdate {
        role: String,
        delta: String,
    },
    MessageEnd {
        role: String,
        content: String,
    },
    Usage {
        model: Option<String>,
        usage: Value,
    },
    ToolExecutionStart {
        name: String,
        call_id: String,
    },
    ToolExecutionEnd {
        name: String,
        call_id: String,
        ok: bool,
        output: String,
    },
    Error {
        code: String,
        message: String,
    },
    Custom(Value),
}
