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
    pub thread_id: Option<String>,
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

    pub(super) fn new(code: &'static str, message: &'static str, stage: &'static str) -> Self {
        Self(Box::new(ProtocolFailurePayload {
            code,
            message,
            stage,
            user_interaction_required: false,
            request_method: None,
            session_id: None,
            thread_id: None,
            turn_id: None,
            turn_status: None,
        }))
    }

    pub(super) fn with_session(mut self, session_id: Option<&str>) -> Self {
        let Some(session_id) = session_id.map(str::trim).filter(|value| !value.is_empty()) else {
            return self;
        };
        self.session_id = Some(session_id.to_string());
        self.thread_id = Some(session_id.to_string());
        self
    }

    pub(super) fn with_user_interaction(mut self) -> Self {
        self.user_interaction_required = true;
        self
    }

    pub(super) fn with_turn_status(mut self, turn_status: &str) -> Self {
        self.turn_status = Some(turn_status.to_string());
        self
    }
}
