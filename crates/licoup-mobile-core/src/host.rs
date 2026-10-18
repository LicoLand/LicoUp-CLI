//! The mobile entry's application port.
//!
//! The mobile core owns the *surface* — which operations a paired mobile
//! client exposes and how a request is admitted — and nothing else. Who
//! answers an operation stays where it already is: the endpoint pairing and
//! relay configuration owner, the endpoint identity/custody owner, the durable
//! delivery owner, and the Canonical Conversation authority.
//!
//! A port method therefore carries the canonical parameter object and returns
//! the canonical result object, exactly as the operation's own route already
//! does. No method here renames a field, invents a result shape, or decides a
//! protocol outcome: those are the answering owner's business. The host that
//! supplies this port is the composition that stands in front of those owners
//! — the desktop executor today, and the mobile platform host once the
//! platform builds bind this core.

use anyhow::Result;
use serde_json::Value;

/// The answering side of the mobile surface.
///
/// Every method is named after the canonical operation it answers, and every
/// one of them is on [`MOBILE_SURFACE`]. An implementation that cannot answer
/// an operation returns an error rather than an invented success; the entry
/// then fails the call closed.
///
/// [`MOBILE_SURFACE`]: crate::surface::MOBILE_SURFACE
pub trait MobileOperationHost {
    /// `mobile.relay.config.get` — the pairing/relay configuration the mobile
    /// settings surface reads.
    fn config_get(&self, params: &Value) -> Result<Value>;

    /// `mobile.relay.config.set` — the same configuration's write door.
    fn config_set(&self, params: &Value) -> Result<Value>;

    /// `mobile.relay.pairing.claim` — claim the pending pairing this client
    /// was invited to.
    fn pairing_claim(&self, params: &Value) -> Result<Value>;

    /// `mobile.relay.pairing.status` — the claim's current state.
    fn pairing_status(&self, params: &Value) -> Result<Value>;

    /// `mobile.relay.e2ee.status` — the negotiated end-to-end capability the
    /// durable delivery path currently has.
    fn e2ee_status(&self, params: &Value) -> Result<Value>;

    /// `mobile.relay.commands.createSecure` — dispatch one secure command
    /// through the authenticated endpoint application path.
    fn command_create_secure(&self, params: &Value) -> Result<Value>;

    /// `mobile.relay.commands.resultSecure` — settle a dispatched command with
    /// its protected result.
    fn command_result_secure(&self, params: &Value) -> Result<Value>;

    /// `mobile.relay.commands.resultReplayProof` — settle a dispatched command
    /// whose result is a replay proof.
    fn command_result_replay_proof(&self, params: &Value) -> Result<Value>;

    /// `conversation.create` — create a Canonical Conversation group.
    fn conversation_create(&self, params: &Value) -> Result<Value>;

    /// `conversation.list` — list Canonical Conversation summaries.
    fn conversation_list(&self, params: &Value) -> Result<Value>;

    /// `conversation.get` — read one Canonical Conversation aggregate.
    fn conversation_get(&self, params: &Value) -> Result<Value>;

    /// `conversation.events.page` — page one conversation's events.
    fn conversation_events_page(&self, params: &Value) -> Result<Value>;

    /// `conversation.message.post` — post one Message event.
    fn conversation_message_post(&self, params: &Value) -> Result<Value>;

    /// `conversation.membership.add` — add one membership.
    fn conversation_membership_add(&self, params: &Value) -> Result<Value>;

    /// `conversation.membership.leave` — retire one membership.
    fn conversation_membership_leave(&self, params: &Value) -> Result<Value>;
}
