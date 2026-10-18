//! This Agent's protocol failure: one reported failure, in a stable shape.
//!
//! It is the parser's and the process half's shared statement that a Claude Code
//! turn could not complete, and it names only facts the client may show: a
//! closed code, a static redacted message, the lifecycle stage it was observed
//! at, and the identities the CLI had already reported. Provider text, prompts
//! and raw frames never enter it.

/// One Agent protocol's reported failure.
#[derive(Clone, Debug)]
pub struct ProtocolFailure(Box<ProtocolFailurePayload>);

/// The facts one reported failure may carry.
#[derive(Clone, Debug)]
pub struct ProtocolFailurePayload {
    /// The failure's stable code.
    pub code: &'static str,
    /// The failure's static, redacted message.
    pub message: &'static str,
    /// The lifecycle stage the failure was observed at.
    pub stage: &'static str,
    /// Whether the client must ask the user before the turn can continue.
    pub user_interaction_required: bool,
    /// The vendor request method the turn is waiting on, when it reports one.
    pub request_method: Option<String>,
    /// The native conversation identity, when it is already known.
    pub session_id: Option<String>,
    /// The same identity as a durable thread, when this Agent reports one.
    pub thread_id: Option<String>,
    /// The turn the failure belongs to, when it is already known.
    pub turn_id: Option<String>,
    /// The vendor turn status, when the CLI reported one.
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
    /// The payload, owned by the caller.
    ///
    /// A crate outside this one cannot move a field out of the boxed payload, so
    /// this is the one way a caller takes the facts rather than borrowing them,
    /// and the box stays an implementation detail of this type.
    pub fn into_payload(self) -> ProtocolFailurePayload {
        *self.0
    }

    /// A failure with no identity bound yet.
    pub fn new(code: &'static str, message: &'static str, stage: &'static str) -> Self {
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

    /// Bind the native conversation identity this failure reports.
    pub fn with_session(mut self, session_id: Option<&str>) -> Self {
        self.session_id = session_id
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string);
        self.thread_id = self.session_id.clone();
        self
    }

    /// Bind the turn this failure belongs to.
    pub fn with_turn(mut self, turn_id: &str) -> Self {
        self.turn_id = (!turn_id.is_empty()).then(|| turn_id.to_string());
        self
    }
}
