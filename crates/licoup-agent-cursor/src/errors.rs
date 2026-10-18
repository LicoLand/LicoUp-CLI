//! Cursor's closed, privacy-safe failure vocabulary.
//!
//! The words here are the vendor protocol's: one static code, one static
//! message and one recovery hint per classified failure. Vendor prose is used
//! only to *select* one classification ([`CursorFailureKind::from_stderr`]) and
//! is never copied into the runtime response, the canonical Conversation, or
//! diagnostics.
//!
//! [`ProtocolFailure`] is the shape the client already reports for every
//! adapter, so a Cursor failure travels through the same bounded projection the
//! kernel reads and never a Cursor-only one.

/// One redacted protocol failure, in the shape the client reports.
#[derive(Clone, Debug)]
pub struct ProtocolFailure(Box<ProtocolFailurePayload>);

/// The failure's fields, as the client's bounded error projection reads them.
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
            component: None,
            retryable: None,
            recovery: None,
            user_interaction_required: false,
            request_method: None,
            session_id: None,
            thread_id: None,
            turn_id: None,
            turn_status: None,
        }))
    }

    pub fn with_session(mut self, session_id: Option<&str>) -> Self {
        if let Some(session_id) = session_id.filter(|value| !value.trim().is_empty()) {
            self.session_id = Some(session_id.to_string());
            self.thread_id = self.session_id.clone();
        }
        self
    }

    pub fn with_turn_status(mut self, turn_status: &str) -> Self {
        self.turn_status = Some(turn_status.to_string());
        self
    }

    pub fn with_resolution(
        mut self,
        component: &'static str,
        retryable: bool,
        recovery: &'static str,
    ) -> Self {
        self.component = Some(component);
        self.retryable = Some(retryable);
        self.recovery = Some(recovery);
        self
    }
}

/// Closed, privacy-safe Cursor failure vocabulary. Vendor prose is used only
/// inside the driver to select one static classification and is never copied
/// into the runtime response, canonical Conversation, or diagnostics.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CursorFailureKind {
    AuthenticationRequired,
    UsageLimitExceeded,
    RateLimited,
    ModelUnavailable,
    ExecutionFailed,
    TurnFailed,
}

impl CursorFailureKind {
    pub fn from_terminal_subtype(subtype: Option<&str>) -> Self {
        match subtype {
            Some("authentication_required" | "authentication_failed") => {
                Self::AuthenticationRequired
            }
            Some("usage_limit_exceeded" | "quota_exceeded") => Self::UsageLimitExceeded,
            Some("rate_limited") => Self::RateLimited,
            Some("model_unavailable" | "invalid_model") => Self::ModelUnavailable,
            Some("error_during_execution") => Self::ExecutionFailed,
            _ => Self::TurnFailed,
        }
    }

    pub fn from_stderr(stderr: &str) -> Option<Self> {
        let text = stderr.to_ascii_lowercase();
        if contains_any(
            &text,
            &[
                "authentication required",
                "not authenticated",
                "please log in",
                "please login",
                "unauthorized",
                "invalid api key",
            ],
        ) {
            return Some(Self::AuthenticationRequired);
        }
        if contains_any(&text, &["usage limit", "quota exceeded", "out of credits"]) {
            return Some(Self::UsageLimitExceeded);
        }
        if contains_any(&text, &["rate limit", "too many requests"]) {
            return Some(Self::RateLimited);
        }
        if contains_any(
            &text,
            &[
                "invalid model",
                "model unavailable",
                "model is not available",
                "unknown model",
            ],
        ) {
            return Some(Self::ModelUnavailable);
        }
        None
    }

    pub fn failure(self, session_id: Option<&str>) -> ProtocolFailure {
        let failure = match self {
            Self::AuthenticationRequired => {
                let mut failure = ProtocolFailure::new(
                    "cursor_cli_authentication_required",
                    "Cursor Agent CLI authentication is required before this turn can continue.",
                    "authentication/runtime",
                );
                failure.user_interaction_required = true;
                failure.request_method = Some("authenticate".to_owned());
                failure
            }
            Self::UsageLimitExceeded => ProtocolFailure::new(
                "cursor_cli_usage_limit_exceeded",
                "Cursor model usage limit exceeded.",
                "turn/completed",
            )
            .with_resolution(
                "native_cli",
                false,
                "select_available_model_or_wait_for_quota_reset",
            ),
            Self::RateLimited => ProtocolFailure::new(
                "cursor_cli_rate_limited",
                "Cursor temporarily rate-limited the requested turn.",
                "turn/completed",
            )
            .with_resolution("native_cli", true, "preserve_draft_and_retry"),
            Self::ModelUnavailable => ProtocolFailure::new(
                "cursor_cli_model_unavailable",
                "The selected Cursor model is unavailable.",
                "turn/prepare",
            )
            .with_resolution(
                "native_cli",
                false,
                "select_available_model_or_wait_for_quota_reset",
            ),
            Self::ExecutionFailed => ProtocolFailure::new(
                "cursor_cli_execution_failed",
                "Cursor Agent CLI could not complete the requested turn.",
                "turn/completed",
            )
            .with_resolution("native_cli", false, "review_terminal_result"),
            Self::TurnFailed => ProtocolFailure::new(
                "cursor_cli_turn_failed",
                "Cursor Agent CLI reported a failed turn result.",
                "turn/completed",
            ),
        };
        failure
            .with_session(session_id)
            .with_turn_status(self.turn_status())
    }

    fn turn_status(self) -> &'static str {
        match self {
            Self::AuthenticationRequired => "authentication_required",
            Self::UsageLimitExceeded => "usage_limit_exceeded",
            Self::RateLimited => "rate_limited",
            Self::ModelUnavailable => "model_unavailable",
            Self::ExecutionFailed => "error_during_execution",
            Self::TurnFailed => "failed",
        }
    }
}

fn contains_any(text: &str, needles: &[&str]) -> bool {
    needles.iter().any(|needle| text.contains(needle))
}
