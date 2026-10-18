use crate::parser::valid_session_id;
use licoup_agent_adapter_sdk::adapters::driver_registry::{
    registry_get, registry_insert, registry_remove,
};
use std::path::{Path, PathBuf};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ControlDisposition {
    Accepted,
    NotPersisted,
    NoActiveTurn,
    SessionUnavailable,
    TransportUnavailable,
}

/// Process-local cancel claim. The pid must not outlive the supervised child:
/// `clear_active_turn` demotes an active pid to `Exited` so a late cancel can
/// still suppress the completion flush without signalling a recycled pid.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum TurnClaim {
    Active { pid: u32 },
    PendingCancel,
    Interrupted,
    Exited,
}

const REGISTRY_NAMESPACE: &str = "antigravity-active-turn";
const MAX_ACTIVE_TURNS: usize = 128;

pub(super) fn register_active_turn(session_id: &str, pid: u32) {
    if !valid_session_id(session_id) {
        return;
    }
    let pending = matches!(
        registry_get::<TurnClaim>(REGISTRY_NAMESPACE, session_id),
        Some(TurnClaim::PendingCancel)
    );
    let _ = registry_insert(
        REGISTRY_NAMESPACE,
        session_id,
        TurnClaim::Active { pid },
        MAX_ACTIVE_TURNS,
    );
    if pending {
        let _ = interrupt_active(session_id, pid);
    }
}

pub(super) fn clear_active_turn(session_id: &str) {
    match registry_get::<TurnClaim>(REGISTRY_NAMESPACE, session_id) {
        Some(TurnClaim::Active { .. }) => {
            let _ = registry_insert(
                REGISTRY_NAMESPACE,
                session_id,
                TurnClaim::Exited,
                MAX_ACTIVE_TURNS,
            );
        }
        Some(TurnClaim::Interrupted | TurnClaim::PendingCancel | TurnClaim::Exited) | None => {}
    }
}

pub fn cancel(session_id: &str) -> ControlDisposition {
    if !valid_session_id(session_id) {
        return ControlDisposition::SessionUnavailable;
    }
    match registry_get::<TurnClaim>(REGISTRY_NAMESPACE, session_id) {
        Some(TurnClaim::Active { pid }) => interrupt_active(session_id, pid),
        Some(TurnClaim::Interrupted) => ControlDisposition::Accepted,
        Some(TurnClaim::Exited) => {
            store_claim(session_id, TurnClaim::Interrupted);
            ControlDisposition::Accepted
        }
        Some(TurnClaim::PendingCancel) => ControlDisposition::NoActiveTurn,
        None => {
            store_claim(session_id, TurnClaim::PendingCancel);
            ControlDisposition::NoActiveTurn
        }
    }
}

/// Peek whether cancel was accepted or is pending. Does not consume the claim,
/// so the PTY loop can stop projecting new assistant text after interrupt.
pub(super) fn cancel_claimed(session_id: &str) -> bool {
    if session_id.is_empty() {
        return false;
    }
    matches!(
        registry_get::<TurnClaim>(REGISTRY_NAMESPACE, session_id),
        Some(TurnClaim::Interrupted | TurnClaim::PendingCancel)
    )
}

/// True when cancel was accepted and native interrupt was delivered, or when a
/// pending cancel survived until the turn finished. Consumes the claim.
pub(super) fn take_cancelled(session_id: &str) -> bool {
    if session_id.is_empty() {
        return false;
    }
    matches!(
        registry_remove::<TurnClaim>(REGISTRY_NAMESPACE, session_id),
        Some(TurnClaim::Interrupted | TurnClaim::PendingCancel)
    )
}

pub fn cleanup_session(session_id: &str) -> ControlDisposition {
    if !valid_session_id(session_id) {
        return ControlDisposition::SessionUnavailable;
    }
    match remove_antigravity_brain(session_id) {
        Ok(true) => ControlDisposition::Accepted,
        Ok(false) => ControlDisposition::NotPersisted,
        Err(_) => ControlDisposition::TransportUnavailable,
    }
}

fn interrupt_active(session_id: &str, pid: u32) -> ControlDisposition {
    // Claim first so the PTY loop stops projecting before SIGTERM can flush
    // remaining stdout (Stop-hook / finish word) into assistant text.
    store_claim(session_id, TurnClaim::Interrupted);
    if interrupt_pid(pid) {
        return ControlDisposition::Accepted;
    }
    store_claim(session_id, TurnClaim::Active { pid });
    ControlDisposition::TransportUnavailable
}

fn store_claim(session_id: &str, claim: TurnClaim) {
    let _ = registry_insert(REGISTRY_NAMESPACE, session_id, claim, MAX_ACTIVE_TURNS);
}

/// The turn runs in its own process group (`SupervisedChild::group_spawn`). A
/// negative pid signals the whole tree so a Stop-hook descendant cannot finish
/// the print turn after the leader is interrupted.
fn interrupt_pid(pid: u32) -> bool {
    #[cfg(unix)]
    {
        use nix::sys::signal::{Signal, kill};
        use nix::unistd::Pid;
        let Ok(raw) = i32::try_from(pid) else {
            return false;
        };
        if raw <= 0 {
            return false;
        }
        kill(Pid::from_raw(-raw), Signal::SIGTERM).is_ok()
    }
    #[cfg(not(unix))]
    {
        let _ = pid;
        false
    }
}

fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME").map(PathBuf::from)
}

fn remove_antigravity_brain(session_id: &str) -> Result<bool, ()> {
    let Some(home) = home_dir() else {
        return Err(());
    };
    let brain = home
        .join(".gemini")
        .join("antigravity-cli")
        .join("brain")
        .join(session_id);
    if !is_safe_brain_dir(&home, &brain, session_id) {
        return Err(());
    }
    if !brain.exists() {
        return Ok(false);
    }
    trash::delete(&brain).map_err(|_| ())?;
    Ok(true)
}

fn is_safe_brain_dir(home: &Path, brain: &Path, session_id: &str) -> bool {
    if !valid_session_id(session_id) {
        return false;
    }
    let root = home.join(".gemini").join("antigravity-cli").join("brain");
    brain.starts_with(&root) && brain.ends_with(session_id)
}
