use licoup_foundation::core::acp;

#[derive(Clone, Debug)]
pub struct ProtocolFailure(Box<ProtocolFailurePayload>);

#[derive(Clone, Debug)]
pub struct ProtocolFailurePayload {
    pub code: &'static str,
    pub message: &'static str,
    pub stage: &'static str,
    pub user_interaction_required: bool,
    pub request_method: Option<String>,
    pub session_id: Option<String>,
    pub turn_id: Option<String>,
    pub turn_status: Option<String>,
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

impl ProtocolFailure {
    pub fn into_payload(self) -> ProtocolFailurePayload {
        *self.0
    }

    pub fn new(code: &'static str, message: &'static str, stage: &'static str) -> Self {
        Self(Box::new(ProtocolFailurePayload {
            code,
            message,
            stage,
            user_interaction_required: false,
            request_method: None,
            session_id: None,
            turn_id: None,
            turn_status: None,
        }))
    }

    pub fn user_interaction(method: &str, session_id: Option<&str>, turn_id: Option<&str>) -> Self {
        Self(Box::new(ProtocolFailurePayload {
            code: "openclaw_user_interaction_required",
            message: "OpenClaw requires explicit user interaction before this turn can continue.",
            stage: "server/request",
            user_interaction_required: true,
            request_method: Some(method.to_string()),
            session_id: session_id.map(str::to_string),
            turn_id: turn_id.map(str::to_string),
            turn_status: None,
        }))
    }

    pub fn from_acp(error: acp::AcpError, stage: &'static str) -> Self {
        Self::new(
            error.code(),
            "The OpenClaw ACP protocol message could not be processed safely.",
            stage,
        )
    }

    pub fn with_ids(mut self, session_id: Option<String>, turn_id: &str) -> Self {
        self.session_id = session_id.filter(|value| !value.is_empty());
        self.turn_id = (!turn_id.is_empty()).then(|| turn_id.to_string());
        self
    }
}
