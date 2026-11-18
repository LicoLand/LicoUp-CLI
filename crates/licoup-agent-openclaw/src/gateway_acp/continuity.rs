use crate::gateway_acp::errors::ProtocolFailure;
use crate::gateway_acp::params::ProtocolConfig;
use licoup_foundation::core::acp::{self, AcpSessionMethod, AcpSessionOptions, AcpSessionUpdate};
use serde_json::Value;
use std::path::Path;

#[derive(Clone, Debug)]
pub struct SessionBinding {
    protocol_session_id: Option<String>,
    native_session_id: Option<String>,
}

impl SessionBinding {
    pub fn new(config: &ProtocolConfig) -> Self {
        Self {
            protocol_session_id: None,
            native_session_id: config.native_session_key.clone(),
        }
    }

    pub fn protocol_id(&self) -> Option<&str> {
        self.protocol_session_id.as_deref()
    }

    pub fn native_id(&self) -> Option<&str> {
        self.native_session_id.as_deref()
    }

    pub fn expected_protocol_id<'a>(&'a self, config: &'a ProtocolConfig) -> Option<&'a str> {
        self.protocol_id().or_else(|| {
            config
                .is_resume()
                .then_some(config.requested_session_id.as_str())
        })
    }

    pub fn capture_opening_update(&mut self, update: &AcpSessionUpdate) {
        if self.protocol_session_id.is_none() {
            self.protocol_session_id = Some(update.session_id.clone());
        }
        if let Some(key) = update
            .payload()
            .pointer("/_meta/sessionKey")
            .and_then(Value::as_str)
        {
            self.native_session_id = Some(key.to_string());
        }
    }

    pub fn reconcile_open_response(
        &mut self,
        config: &ProtocolConfig,
        returned_session_id: Option<String>,
        stage: &'static str,
    ) -> Result<(), ProtocolFailure> {
        let session_id = returned_session_id
            .filter(|value| !value.trim().is_empty())
            .ok_or_else(|| {
                ProtocolFailure::new(
                    "openclaw_acp_session_id_missing",
                    "OpenClaw ACP did not return a native conversation identifier.",
                    stage,
                )
            })?;
        if config.is_resume() && session_id != config.requested_session_id {
            return Err(ProtocolFailure::new(
                "openclaw_acp_session_mismatch",
                "OpenClaw ACP returned a different conversation than requested.",
                stage,
            ));
        }
        if self
            .protocol_session_id
            .as_deref()
            .is_some_and(|pending| pending != session_id)
        {
            return Err(ProtocolFailure::new(
                "openclaw_acp_session_mismatch",
                "OpenClaw ACP associated updates with a different conversation.",
                stage,
            ));
        }
        self.protocol_session_id = Some(session_id);
        if self.native_session_id.is_none() {
            return Err(ProtocolFailure::new(
                "openclaw_acp_native_session_id_missing",
                "OpenClaw ACP did not expose a resumable Gateway conversation identifier.",
                "session/open",
            ));
        }
        Ok(())
    }

    pub fn failure_session_id(&self, config: &ProtocolConfig) -> Option<String> {
        self.native_session_id.clone().or_else(|| {
            (!config.requested_session_id.is_empty()).then(|| config.requested_session_id.clone())
        })
    }
}

pub fn session_method(config: &ProtocolConfig) -> AcpSessionMethod<'_> {
    if config.is_resume() {
        AcpSessionMethod::Load(&config.requested_session_id)
    } else {
        AcpSessionMethod::New
    }
}

pub fn session_request(config: &ProtocolConfig) -> Result<Value, ProtocolFailure> {
    let method = session_method(config);
    let stage = method.method_name();
    let mut options = AcpSessionOptions::new(Path::new(&config.cwd));
    if let Some(meta) = config.session_meta() {
        options = options.meta(meta);
    }
    options = options.mcp_servers(&config.mcp_servers);
    acp::session_request(crate::parser::codec::SESSION_REQUEST_ID, method, options)
        .map_err(|error| ProtocolFailure::from_acp(error, stage))
}
