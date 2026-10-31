//! Process-local, adapter-neutral interaction routing.
//!
//! A native transport parks exactly one response sender under an opaque token.
//! The trusted client resolves that token once. There is deliberately no
//! elapsed-time expiry: observer lifetime and user response lifetime are not
//! native turn lifetime.

use serde_json::{Value, json};
use std::collections::{HashMap, HashSet, VecDeque};
#[cfg(any(test, feature = "test-support"))]
use std::sync::Condvar;
use std::sync::mpsc::{Receiver, SyncSender, sync_channel};
use std::sync::{Mutex, OnceLock};
#[cfg(any(test, feature = "test-support"))]
use std::time::{Duration, Instant};
use uuid::Uuid;

static ROUTES: OnceLock<Mutex<RouteRegistry>> = OnceLock::new();
#[cfg(any(test, feature = "test-support"))]
static ROUTE_PARKED: OnceLock<Condvar> = OnceLock::new();
const MAX_PENDING_ROUTES: usize = 256;
const MAX_CONSUMED_ROUTE_IDS: usize = 1024;

fn routes() -> &'static Mutex<RouteRegistry> {
    ROUTES.get_or_init(|| Mutex::new(RouteRegistry::default()))
}

#[cfg(any(test, feature = "test-support"))]
fn route_parked() -> &'static Condvar {
    ROUTE_PARKED.get_or_init(Condvar::new)
}

#[derive(Default)]
struct RouteRegistry {
    pending: HashMap<String, PendingRoute>,
    consumed: HashSet<String>,
    consumed_order: VecDeque<String>,
}

impl RouteRegistry {
    fn remember_consumed(&mut self, token: String) {
        self.consumed.insert(token.clone());
        self.consumed_order.push_back(token);
        if self.consumed_order.len() > MAX_CONSUMED_ROUTE_IDS
            && let Some(expired) = self.consumed_order.pop_front()
        {
            self.consumed.remove(&expired);
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ResponseShape {
    Approval,
    Select,
    Confirm,
    Input,
    Editor,
}

#[derive(Clone, Debug)]
pub struct InteractionRequest {
    pub adapter_id: String,
    pub session_id: String,
    pub turn_id: String,
    pub request_id: Value,
    pub method: String,
    pub summary: String,
    pub options: Vec<String>,
    pub response_shape: ResponseShape,
}

#[derive(Debug)]
struct PendingRoute {
    request: InteractionRequest,
    response_tx: SyncSender<Value>,
}

pub struct ParkedInteraction {
    pub token: String,
    pub response_rx: Receiver<Value>,
}

pub fn park(request: InteractionRequest) -> Result<ParkedInteraction, &'static str> {
    park_with_token(Uuid::new_v4().to_string(), request)
}

pub fn park_with_token(
    token: String,
    request: InteractionRequest,
) -> Result<ParkedInteraction, &'static str> {
    if request.adapter_id.trim().is_empty()
        || request.session_id.trim().is_empty()
        || request.turn_id.trim().is_empty()
        || request.method.trim().is_empty()
        || request.request_id.is_null()
    {
        return Err("native_interaction_identity_invalid");
    }
    if token.trim().is_empty() {
        return Err("native_interaction_token_invalid");
    }
    let (response_tx, response_rx) = sync_channel(1);
    let mut guard = routes()
        .lock()
        .map_err(|_| "native_interaction_registry_unavailable")?;
    if guard.pending.contains_key(&token) || guard.consumed.contains(&token) {
        return Err("native_interaction_route_duplicate");
    }
    if guard.pending.len() >= MAX_PENDING_ROUTES {
        return Err("native_interaction_capacity_exhausted");
    }
    guard.pending.insert(
        token.clone(),
        PendingRoute {
            request,
            response_tx,
        },
    );
    #[cfg(any(test, feature = "test-support"))]
    route_parked().notify_all();
    Ok(ParkedInteraction { token, response_rx })
}

/// Resolve an opaque process-local route once. The native adapter owns the
/// final response encoding, so the route carries structured values only.
pub fn resolve(token: &str, response: Value) -> Result<Value, &'static str> {
    resolve_scoped(token, None, None, response)
}

/// Resolve through a trusted live-turn RPC. When scope is supplied, both
/// native identities must match the parked transport generation before the
/// one-shot route is consumed.
pub fn resolve_scoped(
    token: &str,
    session_id: Option<&str>,
    turn_id: Option<&str>,
    response: Value,
) -> Result<Value, &'static str> {
    let token = token.trim();
    if token.is_empty() {
        return Err("native_interaction_token_invalid");
    }
    let mut guard = routes()
        .lock()
        .map_err(|_| "native_interaction_registry_unavailable")?;
    let route = match guard.pending.get(token) {
        Some(route) => route,
        None if guard.consumed.contains(token) => {
            return Err("native_interaction_route_consumed");
        }
        None => return Err("native_interaction_route_missing"),
    };
    if session_id.is_some_and(|value| value.trim() != route.request.session_id)
        || turn_id.is_some_and(|value| value.trim() != route.request.turn_id)
    {
        return Err("native_interaction_scope_mismatch");
    }
    let valid = match route.request.response_shape {
        ResponseShape::Approval | ResponseShape::Confirm => response
            .get("allow")
            .or_else(|| response.get("confirmed"))
            .and_then(Value::as_bool)
            .is_some(),
        ResponseShape::Select => response
            .get("selected")
            .and_then(Value::as_str)
            .is_some_and(|selected| {
                route
                    .request
                    .options
                    .iter()
                    .any(|option| option == selected)
            }),
        ResponseShape::Input | ResponseShape::Editor => {
            response.get("text").and_then(Value::as_str).is_some()
        }
    };
    if !valid {
        return Err("native_interaction_response_invalid");
    }
    let route = guard
        .pending
        .remove(token)
        .ok_or("native_interaction_route_missing")?;
    guard.remember_consumed(token.to_owned());
    drop(guard);
    let _native_request_identity = &route.request.request_id;
    let _display_safe_summary = &route.request.summary;
    route
        .response_tx
        .send(response)
        .map_err(|_| "native_interaction_transport_closed")?;
    Ok(json!({
        "ok": true,
        "adapterId": route.request.adapter_id,
        "sessionId": route.request.session_id,
        "turnId": route.request.turn_id,
        "method": route.request.method,
        "signal": "in-process-one-shot",
    }))
}

pub fn abandon(token: &str) {
    if let Ok(mut guard) = routes().lock() {
        guard.pending.remove(token.trim());
    }
}

#[cfg(any(test, feature = "test-support"))]
pub fn pending_token(adapter_id: &str, summary_fragment: &str) -> Option<String> {
    routes().lock().ok().and_then(|guard| {
        guard.pending.iter().find_map(|(token, route)| {
            (route.request.adapter_id == adapter_id
                && route.request.summary.contains(summary_fragment))
            .then(|| token.clone())
        })
    })
}

#[cfg(any(test, feature = "test-support"))]
#[derive(Debug, Eq, PartialEq)]
pub struct PendingRouteSnapshot {
    pub token: String,
    pub adapter_id: String,
    pub session_id: String,
    pub turn_id: String,
    pub summary: String,
}

#[cfg(any(test, feature = "test-support"))]
pub fn wait_for_pending_route(
    adapter_id: &str,
    session_id: &str,
    summary: &str,
    timeout: Duration,
) -> Result<PendingRouteSnapshot, &'static str> {
    let deadline = Instant::now()
        .checked_add(timeout)
        .ok_or("native_interaction_route_wait_invalid")?;
    let mut guard = routes()
        .lock()
        .map_err(|_| "native_interaction_registry_unavailable")?;
    loop {
        if let Some((token, route)) = guard.pending.iter().find(|(_, route)| {
            route.request.adapter_id == adapter_id
                && route.request.session_id == session_id
                && route.request.summary == summary
        }) {
            return Ok(PendingRouteSnapshot {
                token: token.clone(),
                adapter_id: route.request.adapter_id.clone(),
                session_id: route.request.session_id.clone(),
                turn_id: route.request.turn_id.clone(),
                summary: route.request.summary.clone(),
            });
        }

        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err("native_interaction_route_wait_timeout");
        }
        let (next_guard, _) = route_parked()
            .wait_timeout(guard, remaining)
            .map_err(|_| "native_interaction_registry_unavailable")?;
        guard = next_guard;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_agent_interaction_is_single_use_and_has_no_expiry() {
        let parked = park(InteractionRequest {
            adapter_id: "pi".into(),
            session_id: "synthetic-session".into(),
            turn_id: "synthetic-turn".into(),
            request_id: json!(7),
            method: "select".into(),
            summary: "Choose an option".into(),
            options: vec!["one".into(), "two".into()],
            response_shape: ResponseShape::Select,
        })
        .unwrap();
        assert_eq!(
            resolve(&parked.token, json!({"selected": "one"})).unwrap()["ok"],
            true
        );
        assert_eq!(parked.response_rx.recv().unwrap()["selected"], "one");
        assert_eq!(
            resolve(&parked.token, json!({"selected": "two"})),
            Err("native_interaction_route_consumed")
        );
        assert_eq!(
            resolve("unknown-route", json!({"selected": "two"})),
            Err("native_interaction_route_missing")
        );
    }

    #[test]
    fn invalid_or_wrong_scope_response_does_not_consume_live_route() {
        let parked = park(InteractionRequest {
            adapter_id: "pi".into(),
            session_id: "session".into(),
            turn_id: "turn".into(),
            request_id: json!(9),
            method: "input".into(),
            summary: "Input".into(),
            options: Vec::new(),
            response_shape: ResponseShape::Input,
        })
        .unwrap();
        assert_eq!(
            resolve_scoped(
                &parked.token,
                Some("other"),
                Some("turn"),
                json!({"text": "answer"}),
            ),
            Err("native_interaction_scope_mismatch")
        );
        assert_eq!(
            resolve_scoped(
                &parked.token,
                Some("session"),
                Some("turn"),
                json!({"allow": true}),
            ),
            Err("native_interaction_response_invalid")
        );
        resolve_scoped(
            &parked.token,
            Some("session"),
            Some("turn"),
            json!({"text": "answer"}),
        )
        .unwrap();
        assert_eq!(parked.response_rx.recv().unwrap()["text"], "answer");
    }
}
