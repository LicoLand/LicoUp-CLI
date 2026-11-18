//! This Agent's failure vocabulary.
//!
//! A closed code, a fixed sentence, the stage it was observed at and the
//! identity it was observed on. The codes are the RPC protocol's own; the
//! projection onto the host's normalized failure facts stays with the
//! composition, because that vocabulary is the host's.

/// One failure of a Lico Agent turn or probe.
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
    /// The payload the host projects onto its protocol-agnostic failure facts.
    pub fn into_payload(self) -> ProtocolFailurePayload {
        *self.0
    }

    pub(crate) fn new(code: &'static str, message: &'static str, stage: &'static str) -> Self {
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
}
