use crate::parser::safe_session_id;
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ControlDisposition {
    Accepted,
    NotPersisted,
    NoActiveTurn,
    SessionUnavailable,
    TransportUnavailable,
}

static ACTIVE_TURNS: OnceLock<Mutex<HashMap<String, u32>>> = OnceLock::new();

fn active_turns() -> &'static Mutex<HashMap<String, u32>> {
    ACTIVE_TURNS.get_or_init(|| Mutex::new(HashMap::new()))
}

pub(super) fn register_active_turn(session_id: &str, pid: u32) {
    if !safe_session_id(session_id) {
        return;
    }
    if let Ok(mut registry) = active_turns().lock() {
        registry.insert(session_id.to_string(), pid);
    }
}

pub(super) fn clear_active_turn(session_id: &str) {
    if let Ok(mut registry) = active_turns().lock() {
        registry.remove(session_id);
    }
}

pub fn cancel(session_id: &str) -> ControlDisposition {
    if !safe_session_id(session_id) {
        return ControlDisposition::SessionUnavailable;
    }
    #[cfg(unix)]
    {
        use nix::sys::signal::{Signal, kill};
        use nix::unistd::Pid;
        cancel_registered(session_id, |pid| {
            // The turn runs as its own process group (command_group group_spawn),
            // so a negative pid signals the whole process tree. Descendants that
            // hold the pty open would otherwise keep the turn alive after the
            // root exits.
            kill(Pid::from_raw(-(pid as i32)), Signal::SIGTERM).is_ok()
        })
    }
    #[cfg(not(unix))]
    {
        ControlDisposition::TransportUnavailable
    }
}

#[cfg(unix)]
fn cancel_registered(
    session_id: &str,
    signal_group: impl FnOnce(u32) -> bool,
) -> ControlDisposition {
    let Ok(mut registry) = active_turns().lock() else {
        return ControlDisposition::TransportUnavailable;
    };
    let Some(pid) = registry.get(session_id).copied() else {
        return ControlDisposition::NoActiveTurn;
    };
    // Keep the registry lock through the group signal and removal. Natural
    // exit cleanup first observes the leader without reaping it, then removes
    // this entry before reaping; a cancellation racing that handoff can only
    // signal the still-reserved process group, never a recycled group id.
    if signal_group(pid) {
        registry.remove(session_id);
        ControlDisposition::Accepted
    } else {
        ControlDisposition::TransportUnavailable
    }
}

pub fn cleanup_session(session_id: &str) -> ControlDisposition {
    if !safe_session_id(session_id) {
        return ControlDisposition::SessionUnavailable;
    }
    let removed = remove_cursor_chat_storage(session_id);
    match removed {
        Ok(true) => ControlDisposition::Accepted,
        Ok(false) => ControlDisposition::NotPersisted,
        Err(_) => ControlDisposition::TransportUnavailable,
    }
}

fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME").map(PathBuf::from)
}

fn remove_cursor_chat_storage(session_id: &str) -> Result<bool, ()> {
    let Some(home) = home_dir() else {
        return Err(());
    };
    let mut removed_any = false;
    let chats_root = home.join(".cursor").join("chats");
    if chats_root.is_dir() {
        removed_any |= remove_matching_chat_leaves(&chats_root, session_id)?;
    }
    let projects_root = home.join(".cursor").join("projects");
    if projects_root.is_dir() {
        removed_any |= remove_matching_transcript_dirs(&projects_root, session_id)?;
    }
    Ok(removed_any)
}

fn remove_matching_chat_leaves(chats_root: &Path, session_id: &str) -> Result<bool, ()> {
    let mut removed = false;
    let entries = fs::read_dir(chats_root).map_err(|_| ())?;
    for entry in entries {
        let entry = entry.map_err(|_| ())?;
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let leaf = path.join(session_id);
        if is_safe_chat_leaf(&home_dir().unwrap_or_default(), &leaf, session_id) && leaf.is_dir() {
            trash::delete(&leaf).map_err(|_| ())?;
            removed = true;
        }
    }
    Ok(removed)
}

fn remove_matching_transcript_dirs(root: &Path, session_id: &str) -> Result<bool, ()> {
    let mut removed = false;
    removed |= remove_transcript_leaf(&root.join("agent-transcripts"), session_id)?;
    let entries = fs::read_dir(root).map_err(|_| ())?;
    for entry in entries {
        let entry = entry.map_err(|_| ())?;
        let path = entry.path();
        if path.is_dir()
            && path.file_name().and_then(|name| name.to_str()) != Some("agent-transcripts")
        {
            removed |= remove_transcript_leaf(&path.join("agent-transcripts"), session_id)?;
        }
    }
    Ok(removed)
}

fn remove_transcript_leaf(transcripts: &Path, session_id: &str) -> Result<bool, ()> {
    let target = transcripts.join(session_id);
    if !target.is_dir() {
        return Ok(false);
    }
    if !is_safe_transcript_dir(&home_dir().unwrap_or_default(), &target, session_id) {
        return Err(());
    }
    trash::delete(&target).map_err(|_| ())?;
    Ok(true)
}

fn is_safe_chat_leaf(home: &Path, leaf: &Path, session_id: &str) -> bool {
    if !safe_session_id(session_id) {
        return false;
    }
    let chats_root = home.join(".cursor").join("chats");
    if !leaf.starts_with(&chats_root) {
        return false;
    }
    let Ok(relative) = leaf.strip_prefix(&chats_root) else {
        return false;
    };
    let mut parts = relative.components();
    matches!(
        (parts.next(), parts.next(), parts.next()),
        (Some(_), Some(_), None)
    ) && leaf.ends_with(session_id)
}

fn is_safe_transcript_dir(home: &Path, target: &Path, session_id: &str) -> bool {
    if !safe_session_id(session_id) {
        return false;
    }
    let projects_root = home.join(".cursor").join("projects");
    target.starts_with(&projects_root)
        && target.ends_with(session_id)
        && target
            .parent()
            .and_then(|parent| parent.file_name())
            .and_then(|name| name.to_str())
            == Some("agent-transcripts")
}

#[cfg(all(test, unix))]
mod tests {
    use super::{
        ControlDisposition, active_turns, cancel_registered, clear_active_turn,
        register_active_turn,
    };
    use std::sync::mpsc;
    use std::thread;
    use std::time::Duration;

    #[test]
    fn cancellation_holds_registration_until_signal_finishes() {
        let session_id = "cursor-cancel-race-00000001";
        register_active_turn(session_id, 12345);
        let (signal_started, signal_started_rx) = mpsc::channel();
        let (allow_signal, allow_signal_rx) = mpsc::channel();
        let cancel_session = session_id.to_owned();
        let cancel_thread = thread::spawn(move || {
            cancel_registered(&cancel_session, |pid| {
                assert_eq!(pid, 12345);
                signal_started.send(()).unwrap();
                allow_signal_rx.recv().unwrap();
                true
            })
        });

        signal_started_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("cancellation did not reach the process-group signal");
        assert!(active_turns().try_lock().is_err());

        let clear_session = session_id.to_owned();
        let clear_thread = thread::spawn(move || clear_active_turn(&clear_session));
        allow_signal.send(()).unwrap();
        assert_eq!(cancel_thread.join().unwrap(), ControlDisposition::Accepted);
        clear_thread.join().unwrap();
        assert!(!active_turns().lock().unwrap().contains_key(session_id));
    }
}
