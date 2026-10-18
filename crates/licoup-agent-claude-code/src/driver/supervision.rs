use super::control::{ControlDisposition, ControlRequest};
use super::failure::{ProtocolFailure, supervisor_failure};
use super::model::{CompleteTranscript, TransportLifecycle};
use super::transport::PersistentTransport;
use crate::protocol::DriverConfig;
use crate::protocol::LaunchIdentity;
use serde_json::{Value, json};
use std::collections::HashMap;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, SyncSender};
use std::sync::{Arc, Mutex, OnceLock, Weak};
use std::time::Duration;

pub(super) const CONTROL_ACK_TIMEOUT: Duration = Duration::from_secs(1);
pub(super) const CONTROL_QUEUE_CAPACITY: usize = 4;
pub(super) const MAX_POOLED_TRANSPORTS: usize = 8;
pub(super) const MAX_TRACKED_SESSIONS: usize = 1024;

static NEXT_TRANSPORT_ID: AtomicU64 = AtomicU64::new(1);
static SUPERVISOR: OnceLock<Mutex<SupervisorRegistry>> = OnceLock::new();

#[derive(Debug, Default)]
struct SupervisorRegistry {
    transports: HashMap<u64, Arc<ManagedTransport>>,
    sessions: HashMap<String, Weak<ManagedTransport>>,
}

#[derive(Debug)]
pub(super) struct ManagedTransport {
    pub(super) id: u64,
    pub(super) identity: LaunchIdentity,
    pub(super) transport: Mutex<PersistentTransport>,
    pub(super) control_sender: SyncSender<ControlRequest>,
    pub(super) native_session_id: Mutex<Option<String>>,
    pub(super) active_session: Mutex<Option<String>>,
    pub(super) lifecycle: TransportLifecycle,
    transcript: Mutex<CompleteTranscript>,
}

fn supervisor() -> &'static Mutex<SupervisorRegistry> {
    SUPERVISOR.get_or_init(|| Mutex::new(SupervisorRegistry::default()))
}

pub(super) fn spawn_transport(
    executable: &str,
    config: &DriverConfig,
    cwd: Option<&Path>,
    max_stderr: usize,
) -> Result<Arc<ManagedTransport>, ProtocolFailure> {
    let mut registry = supervisor().lock().map_err(|_| supervisor_failure())?;
    if registry.transports.len() >= MAX_POOLED_TRANSPORTS {
        return Err(ProtocolFailure::new(
            "claude_code_transport_capacity",
            "Claude Code reached the bounded persistent transport capacity.",
            "process/supervisor",
        ));
    }
    let identity = LaunchIdentity::new(executable, config, cwd);
    let (control_sender, control_receiver) = mpsc::sync_channel(CONTROL_QUEUE_CAPACITY);
    let transport = PersistentTransport::spawn(&identity, control_receiver, max_stderr)?;
    let id = NEXT_TRANSPORT_ID.fetch_add(1, Ordering::Relaxed);
    let managed = Arc::new(ManagedTransport {
        id,
        identity,
        transport: Mutex::new(transport),
        control_sender,
        native_session_id: Mutex::new(None),
        active_session: Mutex::new(None),
        lifecycle: TransportLifecycle::default(),
        transcript: Mutex::new(CompleteTranscript::new()),
    });
    registry.transports.insert(id, Arc::clone(&managed));
    Ok(managed)
}

pub(super) fn bind_session(
    managed: &Arc<ManagedTransport>,
    session_id: &str,
) -> Result<(), ProtocolFailure> {
    if !valid_native_id(session_id) || !managed.lifecycle.is_live() {
        return Err(ProtocolFailure::new(
            "claude_code_session_id_invalid",
            "Claude Code returned an invalid native conversation identifier.",
            "session/open",
        ));
    }
    let mut registry = supervisor().lock().map_err(|_| supervisor_failure())?;
    registry
        .sessions
        .retain(|_, transport| transport.strong_count() > 0);
    if !registry.transports.contains_key(&managed.id)
        || registry.sessions.get(session_id).is_some_and(|existing| {
            existing
                .upgrade()
                .is_some_and(|owner| owner.id != managed.id)
        })
        || (registry.sessions.len() >= MAX_TRACKED_SESSIONS
            && !registry.sessions.contains_key(session_id))
    {
        return Err(ProtocolFailure::new(
            "claude_code_session_capacity",
            "Claude Code could not bind the native conversation safely.",
            "process/supervisor",
        ));
    }
    let mut native = managed
        .native_session_id
        .lock()
        .map_err(|_| supervisor_failure())?;
    if native
        .as_deref()
        .is_some_and(|existing| existing != session_id)
    {
        return Err(ProtocolFailure::new(
            "claude_code_session_mismatch",
            "Claude Code returned a different conversation than requested.",
            "session/open",
        ));
    }
    *native = Some(session_id.to_string());
    registry
        .sessions
        .insert(session_id.to_string(), Arc::downgrade(managed));
    Ok(())
}

fn valid_native_id(value: &str) -> bool {
    !value.trim().is_empty() && value.len() <= 512 && !value.chars().any(char::is_control)
}

fn detach_transport(registry: &mut SupervisorRegistry, transport_id: u64) {
    registry.transports.remove(&transport_id);
    registry.sessions.retain(|_, transport| {
        transport
            .upgrade()
            .is_some_and(|managed| managed.id != transport_id)
    });
}

pub(super) fn lookup_session_transport(session_id: &str) -> Option<Arc<ManagedTransport>> {
    if !valid_native_id(session_id) {
        return None;
    }
    let registry = supervisor().lock().ok()?;
    let managed = registry.sessions.get(session_id)?.upgrade()?;
    managed.lifecycle.is_live().then_some(managed)
}
#[cfg(test)]
pub(crate) fn has_live_session(session_id: &str) -> bool {
    lookup_session_transport(session_id).is_some()
}

pub(super) fn remove_transport(managed: &Arc<ManagedTransport>, cleanup: bool) {
    if let Ok(mut registry) = supervisor().lock() {
        detach_transport(&mut registry, managed.id);
    }
    if cleanup && managed.lifecycle.begin_closing() {
        if let Ok(mut transport) = managed.transport.lock() {
            if transport.shutdown().is_ok() {
                clear_transcript(managed);
                let _ = managed.lifecycle.mark_closed();
            }
        }
    }
}

pub(super) fn set_active_session(managed: &ManagedTransport, session_id: Option<String>) {
    if let Ok(mut active) = managed.active_session.lock() {
        *active = session_id;
    }
}

pub(super) fn record_success(
    managed: &ManagedTransport,
    turn_id: &str,
    prompt: &str,
    events: Vec<Value>,
    output: &str,
) {
    if managed.lifecycle.is_live()
        && let Ok(mut transcript) = managed.transcript.lock()
    {
        transcript.record_success(turn_id, prompt, events, output);
    }
}

pub fn history(session_id: &str, before: Option<usize>, limit: usize) -> Option<Value> {
    let managed = lookup_session_transport(session_id)?;
    let transcript = managed.transcript.lock().ok()?;
    let (turns, next_before) = transcript.project_backward_page(before, limit);
    Some(json!({
        "continuityScope": "process-local",
        "nativeSessionId": session_id,
        "turnCount": transcript.turn_count(),
        "byteCount": transcript.byte_count(),
        "turns": turns,
        "nextBefore": next_before,
        "hasMore": next_before.is_some(),
    }))
}

fn clear_transcript(managed: &ManagedTransport) {
    if let Ok(mut transcript) = managed.transcript.lock() {
        transcript.clear();
    }
}

pub fn cancel(session_id: &str) -> ControlDisposition {
    let Some(managed) = lookup_session_transport(session_id) else {
        return ControlDisposition::SessionUnavailable;
    };
    let active = managed
        .active_session
        .lock()
        .ok()
        .and_then(|value| value.clone());
    if active.as_deref() != Some(session_id) {
        return ControlDisposition::NoActiveTurn;
    }
    let (acknowledged, receiver) = mpsc::sync_channel(1);
    if managed
        .control_sender
        .try_send(ControlRequest::Cancel {
            session_id: session_id.to_string(),
            acknowledged,
        })
        .is_err()
    {
        return ControlDisposition::TransportUnavailable;
    }
    match receiver.recv_timeout(CONTROL_ACK_TIMEOUT) {
        Ok(true) => ControlDisposition::Accepted,
        Ok(false) => ControlDisposition::NoActiveTurn,
        Err(_) => ControlDisposition::TransportUnavailable,
    }
}

pub fn steer(session_id: &str, text: &str) -> ControlDisposition {
    if text.trim().is_empty() || text.len() > 1024 * 1024 {
        return ControlDisposition::TransportUnavailable;
    }
    let Some(managed) = lookup_session_transport(session_id) else {
        return ControlDisposition::SessionUnavailable;
    };
    let active = managed
        .active_session
        .lock()
        .ok()
        .and_then(|value| value.clone());
    if active.as_deref() != Some(session_id) {
        return ControlDisposition::NoActiveTurn;
    }
    let (acknowledged, receiver) = mpsc::sync_channel(1);
    if managed
        .control_sender
        .try_send(ControlRequest::Steer {
            session_id: session_id.to_owned(),
            text: text.to_owned(),
            acknowledged,
        })
        .is_err()
    {
        return ControlDisposition::TransportUnavailable;
    }
    match receiver.recv_timeout(CONTROL_ACK_TIMEOUT) {
        Ok(true) => ControlDisposition::Accepted,
        Ok(false) => ControlDisposition::NoActiveTurn,
        Err(_) => ControlDisposition::TransportUnavailable,
    }
}

pub fn cleanup_session(session_id: &str) -> ControlDisposition {
    let Some(managed) = lookup_session_transport(session_id) else {
        return ControlDisposition::SessionUnavailable;
    };
    if !managed.lifecycle.begin_closing() {
        return ControlDisposition::TransportUnavailable;
    }
    if let Ok(mut registry) = supervisor().lock() {
        detach_transport(&mut registry, managed.id);
    } else {
        return ControlDisposition::TransportUnavailable;
    }

    let active = managed
        .active_session
        .lock()
        .ok()
        .is_some_and(|value| value.is_some());
    if active {
        let (acknowledged, receiver) = mpsc::sync_channel(1);
        if managed
            .control_sender
            .try_send(ControlRequest::Cleanup { acknowledged })
            .is_err()
            || receiver.recv_timeout(CONTROL_ACK_TIMEOUT) != Ok(true)
        {
            return ControlDisposition::TransportUnavailable;
        }
    }
    let Ok(mut transport) = managed.transport.lock() else {
        return ControlDisposition::TransportUnavailable;
    };
    if transport.shutdown().is_err() {
        return ControlDisposition::TransportUnavailable;
    }
    clear_transcript(&managed);
    let _ = managed.lifecycle.mark_closed();
    ControlDisposition::Accepted
}

#[cfg(test)]
pub(crate) fn clear_all_for_test() -> ControlDisposition {
    let managed = {
        let Ok(mut registry) = supervisor().lock() else {
            return ControlDisposition::TransportUnavailable;
        };
        let transports = registry.transports.values().cloned().collect::<Vec<_>>();
        registry.transports.clear();
        registry.sessions.clear();
        transports
    };
    let mut failed = false;
    for transport in managed {
        if !transport.lifecycle.begin_closing() && transport.lifecycle.is_closed() {
            continue;
        }
        match transport.transport.lock() {
            Ok(mut inner) => {
                if inner.shutdown().is_ok() {
                    clear_transcript(&transport);
                    let _ = transport.lifecycle.mark_closed();
                } else {
                    failed = true;
                }
            }
            Err(_) => failed = true,
        }
    }
    if failed {
        ControlDisposition::TransportUnavailable
    } else {
        ControlDisposition::Accepted
    }
}
