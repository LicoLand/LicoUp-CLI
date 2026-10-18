use serde_json::Value;

#[derive(Clone, Debug, Default)]
pub struct EffectiveSettings {
    pub cwd: Option<String>,
    pub model: Option<String>,
    pub reasoning_effort: Option<String>,
    pub sandbox: Option<Value>,
    pub approval_policy: Option<Value>,
}

#[derive(Clone, Debug)]
pub struct ProtocolFailure(Box<ProtocolFailurePayload>);

#[derive(Clone, Debug)]
pub struct ProtocolFailurePayload {
    pub code: &'static str,
    pub message: &'static str,
    pub stage: &'static str,
    pub component: Option<&'static str>,
    pub retryable: Option<bool>,
    pub recovery: Option<&'static str>,
    pub user_interaction_required: bool,
    pub request_method: Option<String>,
    pub session_id: Option<String>,
    pub thread_id: Option<String>,
    pub turn_id: Option<String>,
    pub turn_status: Option<String>,
}

impl ProtocolFailure {
    pub fn into_payload(self) -> ProtocolFailurePayload {
        *self.0
    }

    pub(super) fn from_payload(payload: ProtocolFailurePayload) -> Self {
        Self(Box::new(payload))
    }
}

impl std::ops::Deref for ProtocolFailure {
    type Target = ProtocolFailurePayload;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl std::ops::DerefMut for ProtocolFailure {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

#[derive(Debug)]
pub struct RunResult {
    pub ok: bool,
    pub output: String,
    pub transitions: Vec<licoup_agent_adapter_sdk::Transition>,
    pub error: Option<ProtocolFailure>,
    pub session_id: String,
    pub thread_id: String,
    pub turn_id: String,
    pub turn_status: String,
    pub effective: EffectiveSettings,
    pub status_code: Option<i32>,
    pub stdout_truncated: bool,
    pub stderr_truncated: bool,
    pub started_at: String,
}

#[derive(Clone, Debug)]
pub struct ProtocolOutcome {
    pub output: String,
    pub session_id: String,
    pub thread_id: String,
    pub turn_id: String,
    pub turn_status: String,
    pub effective: EffectiveSettings,
}

#[derive(Debug)]
pub enum ProtocolEffect {
    Send(Value),
    Complete(Box<ProtocolOutcome>),
    Fail(ProtocolFailure),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProtocolPhase {
    AwaitInitialize,
    AwaitRateLimits,
    AwaitThread,
    AwaitThreadUnarchive,
    AwaitTurnStart,
    AwaitTurnCompleted,
    Finished,
}
