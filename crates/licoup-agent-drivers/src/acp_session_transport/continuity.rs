use super::capabilities::{
    AcpSessionDriverSpec, CONTROL_ACK_TIMEOUT, CONTROL_QUEUE_CAPACITY, MAX_POOLED_TRANSPORTS,
    MAX_TRACKED_SESSIONS,
};
use super::command::LaunchSpec;
use super::errors::{ProtocolFailure, supervisor_failure};
use super::io::write_cancel_notification;
use super::protocol::SessionProtocol;
use super::supervision::PersistentTransport;
use licoup_agent_targets::platform::virtual_machine::SshRuntimeConnection;
use licoup_foundation::core::acp;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, SyncSender, TryRecvError};
use std::sync::{Arc, Mutex, OnceLock, Weak};

static TRANSPORT_POOL: OnceLock<Mutex<HashMap<TransportKey, Arc<ManagedTransport>>>> =
    OnceLock::new();
static SESSION_TRANSPORTS: OnceLock<Mutex<HashMap<SessionKey, Weak<ManagedTransport>>>> =
    OnceLock::new();

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct TransportKey {
    driver_id: &'static str,
    executable: String,
    cwd: PathBuf,
    runtime_connection: Option<SshRuntimeConnection>,
}

impl TransportKey {
    #[cfg(test)]
    pub fn new(driver: AcpSessionDriverSpec, executable: &str, cwd: &Path) -> Self {
        Self::for_runtime(driver, executable, cwd, None)
    }

    pub fn for_runtime(
        driver: AcpSessionDriverSpec,
        executable: &str,
        cwd: &Path,
        runtime_connection: Option<&SshRuntimeConnection>,
    ) -> Self {
        Self {
            driver_id: driver.driver_id,
            executable: executable.to_string(),
            cwd: cwd.to_path_buf(),
            runtime_connection: runtime_connection.cloned(),
        }
    }
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct SessionKey {
    driver_id: &'static str,
    session_id: String,
}

impl SessionKey {
    pub fn new(driver: AcpSessionDriverSpec, session_id: &str) -> Self {
        Self {
            driver_id: driver.driver_id,
            session_id: session_id.to_string(),
        }
    }
}

#[derive(Debug)]
pub enum ControlRequest {
    Cancel {
        session_id: String,
        acknowledged: SyncSender<bool>,
    },
    Cleanup {
        acknowledged: SyncSender<bool>,
    },
}

#[derive(Debug)]
pub struct ManagedTransport {
    pub key: TransportKey,
    pub transport: Mutex<PersistentTransport>,
    pub control_sender: SyncSender<ControlRequest>,
    pub active_session: Mutex<Option<String>>,
}

pub fn acquire_transport(
    driver: AcpSessionDriverSpec,
    executable: &str,
    cwd: &Path,
    timeout_ms: u64,
    max_stdout: Option<usize>,
    max_stderr: usize,
    runtime_connection: Option<&SshRuntimeConnection>,
) -> Result<Arc<ManagedTransport>, ProtocolFailure> {
    let key = TransportKey::for_runtime(driver, executable, cwd, runtime_connection);
    if let Some(existing) = transport_pool()
        .lock()
        .map_err(|_| supervisor_failure())?
        .get(&key)
        .cloned()
    {
        return Ok(existing);
    }
    if transport_pool()
        .lock()
        .map_err(|_| supervisor_failure())?
        .len()
        >= MAX_POOLED_TRANSPORTS
    {
        return Err(ProtocolFailure::new(
            "hermes_acp_transport_capacity",
            "Hermes ACP reached the bounded persistent transport capacity.",
            "process/supervisor",
        ));
    }
    let (control_sender, control_receiver) = mpsc::sync_channel(CONTROL_QUEUE_CAPACITY);
    let launch = LaunchSpec::new(driver, executable, cwd)
        .with_runtime_connection(runtime_connection.cloned());
    let transport = PersistentTransport::spawn(
        &launch,
        control_receiver,
        timeout_ms,
        max_stdout,
        max_stderr,
    )?;
    let candidate = Arc::new(ManagedTransport {
        key: key.clone(),
        transport: Mutex::new(transport),
        control_sender,
        active_session: Mutex::new(None),
    });
    let mut pool = transport_pool().lock().map_err(|_| supervisor_failure())?;
    if let Some(existing) = pool.get(&key).cloned() {
        return Ok(existing);
    }
    if pool.len() >= MAX_POOLED_TRANSPORTS {
        return Err(ProtocolFailure::new(
            "hermes_acp_transport_capacity",
            "Hermes ACP reached the bounded persistent transport capacity.",
            "process/supervisor",
        ));
    }
    pool.insert(key, Arc::clone(&candidate));
    Ok(candidate)
}

fn transport_pool() -> &'static Mutex<HashMap<TransportKey, Arc<ManagedTransport>>> {
    TRANSPORT_POOL.get_or_init(|| Mutex::new(HashMap::new()))
}

fn session_transports() -> &'static Mutex<HashMap<SessionKey, Weak<ManagedTransport>>> {
    SESSION_TRANSPORTS.get_or_init(|| Mutex::new(HashMap::new()))
}

pub fn register_session(session_id: &str, managed: &Arc<ManagedTransport>) {
    if session_id.is_empty() {
        return;
    }
    if let Ok(mut sessions) = session_transports().lock() {
        sessions.retain(|_, transport| transport.strong_count() > 0);
        let key = SessionKey {
            driver_id: managed.key.driver_id,
            session_id: session_id.to_string(),
        };
        if sessions.len() >= MAX_TRACKED_SESSIONS && !sessions.contains_key(&key) {
            if let Some(oldest_available) = sessions.keys().next().cloned() {
                sessions.remove(&oldest_available);
            }
        }
        sessions.insert(key, Arc::downgrade(managed));
    }
}

pub fn set_active_session(managed: &ManagedTransport, session_id: Option<String>) {
    if let Ok(mut active) = managed.active_session.lock() {
        *active = session_id;
    }
}

pub fn remove_transport(managed: &Arc<ManagedTransport>, cleanup: bool) {
    if let Ok(mut pool) = transport_pool().lock() {
        if pool
            .get(&managed.key)
            .is_some_and(|current| Arc::ptr_eq(current, managed))
        {
            pool.remove(&managed.key);
        }
    }
    if let Ok(mut sessions) = session_transports().lock() {
        sessions.retain(|_, weak| {
            weak.upgrade()
                .is_some_and(|current| !Arc::ptr_eq(&current, managed))
        });
    }
    if cleanup && let Ok(mut transport) = managed.transport.lock() {
        let _ = transport.shutdown();
    }
}

pub fn handle_control_requests(
    transport: &mut PersistentTransport,
    protocol: &SessionProtocol,
) -> Option<ProtocolFailure> {
    loop {
        match transport.control_receiver.try_recv() {
            Ok(ControlRequest::Cancel {
                session_id,
                acknowledged,
            }) => {
                let matches = protocol.session_id.as_deref() == Some(session_id.as_str());
                let written =
                    matches && write_cancel_notification(&mut transport.stdin, &session_id).is_ok();
                let _ = acknowledged.send(written);
                if matches && !written {
                    return Some(protocol.failure_with_ids(
                        "hermes_acp_write_failed",
                        "Hermes ACP stopped accepting protocol messages.",
                        acp::SESSION_CANCEL_METHOD,
                    ));
                }
            }
            Ok(ControlRequest::Cleanup { acknowledged }) => {
                if let Some(session_id) = protocol.session_id.as_deref() {
                    let _ = write_cancel_notification(&mut transport.stdin, session_id);
                }
                let _ = acknowledged.send(true);
                return Some(protocol.failure_with_ids(
                    "hermes_acp_cleanup_requested",
                    "Hermes ACP transport cleanup was requested.",
                    "process/cleanup",
                ));
            }
            Err(TryRecvError::Empty) => return None,
            Err(TryRecvError::Disconnected) => {
                return Some(protocol.failure_with_ids(
                    "hermes_acp_supervisor_unavailable",
                    "Hermes ACP supervisor control channel is unavailable.",
                    "process/supervisor",
                ));
            }
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ControlDisposition {
    Accepted,
    NoActiveTurn,
    SessionUnavailable,
    TransportUnavailable,
}

pub fn cancel(driver: AcpSessionDriverSpec, session_id: &str) -> ControlDisposition {
    let managed = match lookup_session_transport(driver, session_id) {
        Some(managed) => managed,
        None => return ControlDisposition::SessionUnavailable,
    };
    let is_active = managed
        .active_session
        .lock()
        .ok()
        .and_then(|active| active.clone())
        .as_deref()
        == Some(session_id);
    if !is_active {
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

pub fn cleanup_session(driver: AcpSessionDriverSpec, session_id: &str) -> ControlDisposition {
    let managed = match lookup_session_transport(driver, session_id) {
        Some(managed) => managed,
        None => return ControlDisposition::SessionUnavailable,
    };
    if !request_cleanup_if_active(&managed) {
        return ControlDisposition::TransportUnavailable;
    }
    remove_transport(&managed, true);
    ControlDisposition::Accepted
}

pub fn request_cleanup_if_active(managed: &ManagedTransport) -> bool {
    let active = managed
        .active_session
        .lock()
        .ok()
        .is_some_and(|active| active.is_some());
    if !active {
        return true;
    }
    let (acknowledged, receiver) = mpsc::sync_channel(1);
    managed
        .control_sender
        .try_send(ControlRequest::Cleanup { acknowledged })
        .is_ok()
        && receiver.recv_timeout(CONTROL_ACK_TIMEOUT) == Ok(true)
}

pub fn lookup_session_transport(
    driver: AcpSessionDriverSpec,
    session_id: &str,
) -> Option<Arc<ManagedTransport>> {
    if session_id.trim().is_empty() {
        return None;
    }
    session_transports()
        .lock()
        .ok()?
        .get(&SessionKey::new(driver, session_id))
        .and_then(Weak::upgrade)
}
