//! Adapter transport: the one call/response shape every reach into an Agent
//! speaks, and the transports that answer it. The production host driver is
//! one; the unavailable and counting transports are the seam's own.

use std::sync::atomic::{AtomicU64, Ordering};

use licoup_agent_runtime::work_context::NativeCapabilitySnapshot;
use serde_json::{Value, json};

#[derive(Clone, Debug)]
pub struct AdapterCall {
    pub method: &'static str,
    pub params: Value,
}

#[derive(Clone, Debug)]
pub struct AdapterResponse {
    pub ok: bool,
    pub result: Value,
    pub error_message: Option<String>,
}

impl AdapterResponse {
    pub fn ok(result: Value) -> Self {
        Self {
            ok: true,
            result,
            error_message: None,
        }
    }

    pub fn err(message: impl Into<String>) -> Self {
        Self {
            ok: false,
            result: json!({}),
            error_message: Some(message.into()),
        }
    }
}

pub trait AdapterTransport: Send + Sync {
    fn invoke(&self, call: &AdapterCall) -> AdapterResponse;
    fn invocation_count(&self) -> u64;
    fn negotiated_capabilities(&self) -> Option<NativeCapabilitySnapshot> {
        None
    }
}

/// Production default. Never reports applied success.
#[derive(Default)]
pub struct UnavailableAdapterTransport {
    invocations: AtomicU64,
}

impl AdapterTransport for UnavailableAdapterTransport {
    fn invoke(&self, _call: &AdapterCall) -> AdapterResponse {
        self.invocations.fetch_add(1, Ordering::SeqCst);
        AdapterResponse::err("adapter-unavailable")
    }

    fn invocation_count(&self) -> u64 {
        self.invocations.load(Ordering::SeqCst)
    }
}

/// Test transport with an invocation counter. Not a production adapter.
pub struct CountingTransport {
    invocations: AtomicU64,
    handler: Box<dyn Fn(&AdapterCall) -> AdapterResponse + Send + Sync>,
}

impl CountingTransport {
    pub fn new(handler: impl Fn(&AdapterCall) -> AdapterResponse + Send + Sync + 'static) -> Self {
        Self {
            invocations: AtomicU64::new(0),
            handler: Box::new(handler),
        }
    }
}

impl AdapterTransport for CountingTransport {
    fn invoke(&self, call: &AdapterCall) -> AdapterResponse {
        self.invocations.fetch_add(1, Ordering::SeqCst);
        (self.handler)(call)
    }

    fn invocation_count(&self) -> u64 {
        self.invocations.load(Ordering::SeqCst)
    }
}
