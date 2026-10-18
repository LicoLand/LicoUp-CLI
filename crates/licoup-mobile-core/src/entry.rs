//! The mobile entry: admission and routing for the bounded mobile surface.
//!
//! One request arrives as `{"action": …, "params": …}` — the same envelope the
//! native bridge, the generated contracts and the Dart caller already use.
//! The entry admits it against [`MOBILE_SURFACE`], routes it to the answering
//! owner through [`MobileOperationHost`], and answers an off-surface or
//! malformed request with the same bounded refusal shape the bridge's own
//! unsupported response carries.
//!
//! The entry decides *admission*, never *behaviour*. It does not open the
//! conversation store, hold custody, or settle a delivery itself; those are
//! the answering owners' operations and the port is where they are reached. A
//! host that cannot answer returns an error, and the error propagates to the
//! platform boundary that redacts it, exactly as the existing native bridge
//! lets an owner's error propagate.
//!
//! [`MOBILE_SURFACE`]: crate::surface::MOBILE_SURFACE

use anyhow::{Result, anyhow, ensure};
use serde_json::Value;

use crate::host::MobileOperationHost;
use crate::read_model::{ChatList, ThreadView, project_chat_list, project_thread};
use crate::settings::MobileSettings;
use crate::surface::{
    MOBILE_SURFACE, invalid_request_response, is_mobile_operation, unsupported_operation_response,
};

/// Largest request the mobile entry admits, in bytes. It is the same bound the
/// native bridge applies before it parses a request, restated here so the core
/// is safe to call directly.
pub const MAX_MOBILE_REQUEST_BYTES: usize = 2 * 1024 * 1024;

/// Largest action name the mobile entry admits. A name longer than this is not
/// a surface operation under any later widening.
pub const MAX_MOBILE_ACTION_BYTES: usize = 128;

/// The mobile entry over one answering host.
#[derive(Clone, Copy, Debug)]
pub struct MobileEntry<H> {
    host: H,
    settings: MobileSettings,
}

impl<H: MobileOperationHost> MobileEntry<H> {
    /// The entry over the answering host, with the standard bounded policy.
    pub fn new(host: H) -> Self {
        Self {
            host,
            settings: MobileSettings::standard(),
        }
    }

    /// The same entry over a caller-supplied resource policy.
    #[must_use]
    pub fn with_settings(host: H, settings: MobileSettings) -> Self {
        Self { host, settings }
    }

    #[must_use]
    pub fn host(&self) -> &H {
        &self.host
    }

    #[must_use]
    pub fn settings(&self) -> MobileSettings {
        self.settings
    }

    /// The exact operations this entry admits.
    #[must_use]
    pub fn surface(&self) -> &'static [&'static str] {
        MOBILE_SURFACE
    }

    /// Whether one operation is on the surface.
    #[must_use]
    pub fn supports(&self, operation: &str) -> bool {
        is_mobile_operation(operation)
    }

    /// Admit and route one request.
    ///
    /// Every admission refusal — a request that is not an `{action, params}`
    /// object, an action name outside the surface, params that are not an
    /// object — is answered as a bounded value, so a caller reads one shape
    /// whether the bridge or this core refused it. Only the answering owner's
    /// failure is an error: only the owner knows whether it is retryable, and
    /// only the platform boundary may decide what of it is safe to say.
    pub fn dispatch(&self, request: &Value) -> Result<Value> {
        let Some(object) = request.as_object() else {
            return Ok(invalid_request_response());
        };
        let Some(action) = object.get("action").and_then(Value::as_str) else {
            return Ok(invalid_request_response());
        };
        if action.is_empty() || action.len() > MAX_MOBILE_ACTION_BYTES {
            return Ok(invalid_request_response());
        }
        if !self.supports(action) {
            return Ok(unsupported_operation_response(action));
        }
        let params = object
            .get("params")
            .cloned()
            .unwrap_or_else(|| Value::Object(serde_json::Map::new()));
        if !params.is_object() {
            return Ok(invalid_request_response());
        }
        self.route(action, &params)
    }

    /// Admit and route one request carried as JSON text, bounded before it is
    /// parsed.
    pub fn dispatch_json(&self, request_json: &str) -> Result<Value> {
        ensure!(
            request_json.len() <= MAX_MOBILE_REQUEST_BYTES,
            "mobile_surface_request_too_large"
        );
        let request: Value = serde_json::from_str(request_json)
            .map_err(|_| anyhow!("mobile_surface_invalid_json"))?;
        self.dispatch(&request)
    }

    /// Project the canonical summaries onto the bounded mobile chat list.
    #[must_use]
    pub fn chat_list(&self, summaries: &[licoup_conversation::ConversationSummary]) -> ChatList {
        project_chat_list(self.settings.policy(), summaries)
    }

    /// Project one canonical aggregate and event page onto the mobile thread.
    #[must_use]
    pub fn thread(
        &self,
        conversation: &licoup_conversation::Conversation,
        page: &licoup_conversation::EventPage,
    ) -> ThreadView {
        project_thread(self.settings.policy(), conversation, page)
    }

    fn route(&self, action: &str, params: &Value) -> Result<Value> {
        match action {
            "mobile.relay.config.get" => self.host.config_get(params),
            "mobile.relay.config.set" => self.host.config_set(params),
            "mobile.relay.pairing.claim" => self.host.pairing_claim(params),
            "mobile.relay.pairing.status" => self.host.pairing_status(params),
            "mobile.relay.e2ee.status" => self.host.e2ee_status(params),
            "mobile.relay.commands.createSecure" => self.host.command_create_secure(params),
            "mobile.relay.commands.resultSecure" => self.host.command_result_secure(params),
            "mobile.relay.commands.resultReplayProof" => {
                self.host.command_result_replay_proof(params)
            }
            "conversation.create" => self.host.conversation_create(params),
            "conversation.list" => self.host.conversation_list(params),
            "conversation.get" => self.host.conversation_get(params),
            "conversation.events.page" => self.host.conversation_events_page(params),
            "conversation.message.post" => self.host.conversation_message_post(params),
            "conversation.membership.add" => self.host.conversation_membership_add(params),
            "conversation.membership.leave" => self.host.conversation_membership_leave(params),
            // `dispatch` admits only surface operations, so this arm is
            // unreachable; it stays so the routing match is total and a later
            // surface entry cannot silently fall through.
            _ => Ok(unsupported_operation_response(action)),
        }
    }
}
