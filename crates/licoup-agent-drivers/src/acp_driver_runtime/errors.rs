use super::model::AcpDriverSpec;
use licoup_foundation::core::acp;
use serde_json::Value;

#[derive(Clone, Debug)]
pub struct ProtocolFailure(Box<ProtocolFailurePayload>);

#[derive(Clone, Debug)]
pub struct ProtocolFailurePayload {
    pub code: String,
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

    pub fn new(code: &'static str, message: &'static str, stage: &'static str) -> Self {
        Self(Box::new(ProtocolFailurePayload {
            code: code.to_string(),
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

    pub fn with_session(mut self, session_id: Option<&str>) -> Self {
        self.session_id = session_id.map(str::to_string);
        self.thread_id = session_id.map(str::to_string);
        self
    }

    pub fn user_interaction(method: &str, session_id: Option<&str>) -> Self {
        Self(Box::new(ProtocolFailurePayload {
            code: "acp_user_interaction_required".to_string(),
            message: "The agent requires explicit user interaction before this turn can continue.",
            stage: "session/request_permission",
            user_interaction_required: true,
            request_method: Some(method.to_string()),
            session_id: session_id.map(str::to_string),
            thread_id: session_id.map(str::to_string),
            turn_id: None,
            turn_status: Some("cancelled".to_string()),
        }))
    }

    pub fn namespaced(mut self, driver: AcpDriverSpec) -> Self {
        if driver.error_prefix != "acp"
            && let Some(suffix) = self.code.strip_prefix("acp_")
        {
            self.code = format!("{}_{}", driver.error_prefix, suffix);
        }
        self
    }

    pub fn from_acp(error: acp::AcpError, stage: &'static str) -> Self {
        Self::new(
            error.code(),
            "The ACP protocol message could not be processed safely.",
            stage,
        )
    }
}

pub fn failure_from_response(
    message: &Value,
    fallback_code: &'static str,
    fallback_message: &'static str,
    stage: &'static str,
    session_id: Option<&str>,
) -> ProtocolFailure {
    match message
        .get("error")
        .and_then(|error| error.get("code"))
        .and_then(Value::as_i64)
    {
        Some(-32000) => {
            let mut failure = ProtocolFailure::new(
                "acp_authentication_required",
                "The ACP agent requires native authentication before this request can continue.",
                stage,
            )
            .with_session(session_id);
            failure.user_interaction_required = true;
            failure.request_method = Some("authenticate".to_string());
            failure
        }
        Some(-32002) => ProtocolFailure::new(
            "acp_native_session_not_found",
            "The requested native conversation does not exist in the ACP agent.",
            stage,
        )
        .with_session(session_id),
        Some(-32800) => {
            let mut failure = ProtocolFailure::new(
                "acp_request_cancelled",
                "The ACP agent cancelled the request.",
                stage,
            )
            .with_session(session_id);
            failure.turn_status = Some("cancelled".to_string());
            failure
        }
        _ => ProtocolFailure::new(fallback_code, fallback_message, stage).with_session(session_id),
    }
}
