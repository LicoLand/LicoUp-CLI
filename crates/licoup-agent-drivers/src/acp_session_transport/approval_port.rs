//! The endpoint approval authority the ACP session transport's park-and-inbox
//! registration evaluates a parked interaction against.
//!
//! The authority is the host's own: it decides whether a parked interaction may
//! be raised, registers it with the endpoints that may answer, and fans the
//! request out. This crate declares the two evaluations it asks for and the
//! composition above this crate answers them — `licoup-native` today, whose
//! Secure Mesh approval authority lives inside the host — so the transport
//! names no authority and depends on no security crate.
//!
//! A build that composes nothing answers fail-closed: an evaluation that cannot
//! be made is a refusal, never a silent allow.

use std::sync::OnceLock;

use serde_json::Value;

/// One evaluation the transport asks the host's approval authority for.
pub type ApprovalEvaluate = fn(&Value) -> Result<Value, String>;

/// The host's approval authority, in the shape this transport reads it.
#[derive(Clone, Copy)]
pub struct ApprovalPort {
    /// Whether one parked interaction may be raised, and the request the
    /// authority registered for it.
    pub evaluate_request: ApprovalEvaluate,
    /// Whether one parked operation may be fanned out to the endpoints that
    /// may answer it.
    pub evaluate_fanout: ApprovalEvaluate,
}

static APPROVAL: OnceLock<ApprovalPort> = OnceLock::new();

/// Install the host's approval authority, once. The first installation wins, so
/// a running park cannot have the authority under it replaced.
pub fn install(port: ApprovalPort) -> bool {
    APPROVAL.set(port).is_ok()
}

fn evaluate(
    member: fn(ApprovalPort) -> ApprovalEvaluate,
    payload: &Value,
) -> Result<Value, String> {
    match APPROVAL.get().copied() {
        Some(port) => member(port)(payload),
        None => Err("approval_authority_unavailable".to_owned()),
    }
}

/// Whether one parked interaction may be raised.
pub fn evaluate_request(payload: &Value) -> Result<Value, String> {
    evaluate(|port| port.evaluate_request, payload)
}

/// Whether one parked operation may be fanned out.
pub fn evaluate_fanout(payload: &Value) -> Result<Value, String> {
    evaluate(|port| port.evaluate_fanout, payload)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_uncomposed_authority_refuses_rather_than_allows() {
        // The suite never installs a port: every evaluation must refuse, and
        // the transport's registration reads that refusal as a registration
        // failure rather than as an approval.
        assert!(evaluate_request(&serde_json::json!({})).is_err());
        assert!(evaluate_fanout(&serde_json::json!({})).is_err());
    }
}
