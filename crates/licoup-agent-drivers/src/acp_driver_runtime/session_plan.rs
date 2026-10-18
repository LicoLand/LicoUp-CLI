use super::errors::ProtocolFailure;
use super::model::CapabilityProbe;
use super::params::ProtocolConfig;
use licoup_foundation::core::acp::AcpSessionMethod;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AcpSessionPlan {
    New,
    Load,
    Resume,
}

impl AcpSessionPlan {
    pub fn method<'a>(self, requested_session_id: &'a str) -> AcpSessionMethod<'a> {
        match self {
            Self::New => AcpSessionMethod::New,
            Self::Load => AcpSessionMethod::Load(requested_session_id),
            Self::Resume => AcpSessionMethod::Resume(requested_session_id),
        }
    }
}

pub fn select_acp_session_plan(
    config: &ProtocolConfig,
    capabilities: &CapabilityProbe,
) -> Result<AcpSessionPlan, ProtocolFailure> {
    if !config.is_resume() {
        return Ok(AcpSessionPlan::New);
    }
    if capabilities.load_session {
        return Ok(AcpSessionPlan::Load);
    }
    if capabilities.resume_session {
        return Ok(AcpSessionPlan::Resume);
    }
    Err(ProtocolFailure::new(
        "acp_resume_unsupported",
        "The ACP agent cannot resume an existing native conversation.",
        "session/resume",
    )
    .with_session(Some(&config.requested_session_id)))
}

pub fn reconcile_acp_session_id(
    config: &ProtocolConfig,
    plan: AcpSessionPlan,
    returned_session_id: Option<String>,
) -> Result<String, ProtocolFailure> {
    let returned_session_id = returned_session_id
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| {
            ProtocolFailure::new(
                "acp_session_id_missing",
                "The ACP agent did not return a native conversation identifier.",
                plan.method(&config.requested_session_id).method_name(),
            )
            .with_session(
                plan.ne(&AcpSessionPlan::New)
                    .then_some(&config.requested_session_id),
            )
        })?;
    if plan != AcpSessionPlan::New && returned_session_id != config.requested_session_id {
        return Err(ProtocolFailure::new(
            "acp_session_id_mismatch",
            "The ACP agent returned a different conversation than the one requested.",
            plan.method(&config.requested_session_id).method_name(),
        )
        .with_session(Some(&config.requested_session_id)));
    }
    Ok(returned_session_id)
}
