//! Process-local control registry for the official Codex App Server turn API.

use serde_json::Value;
use std::{
    collections::HashMap,
    sync::{Mutex, OnceLock, mpsc},
    time::Duration,
};

const MAX_ACTIVE_TURNS: usize = 32;
const CONTROL_ACK_TIMEOUT: Duration = Duration::from_secs(1);

#[derive(Debug)]
pub(super) enum ControlRequestKind {
    Steer(String),
    Interrupt,
}

#[derive(Debug)]
pub(super) struct ControlRequest {
    pub(super) kind: ControlRequestKind,
    acknowledged: mpsc::SyncSender<bool>,
}

#[derive(Clone)]
struct ActiveTurn {
    turn_id: String,
    token: String,
    sender: mpsc::SyncSender<ControlRequest>,
}

static ACTIVE: OnceLock<Mutex<HashMap<String, ActiveTurn>>> = OnceLock::new();

fn active() -> &'static Mutex<HashMap<String, ActiveTurn>> {
    ACTIVE.get_or_init(|| Mutex::new(HashMap::new()))
}

pub(super) struct ActiveTurnGuard {
    thread_id: String,
    token: String,
}

impl Drop for ActiveTurnGuard {
    fn drop(&mut self) {
        if let Ok(mut registry) = active().lock()
            && registry
                .get(&self.thread_id)
                .is_some_and(|turn| turn.token == self.token)
        {
            registry.remove(&self.thread_id);
        }
    }
}

pub(super) fn bind(
    thread_id: &str,
    turn_id: &str,
    sender: mpsc::SyncSender<ControlRequest>,
) -> Option<ActiveTurnGuard> {
    if !valid_id(thread_id) || !valid_id(turn_id) {
        return None;
    }
    let mut registry = active().lock().ok()?;
    if registry.len() >= MAX_ACTIVE_TURNS && !registry.contains_key(thread_id) {
        return None;
    }
    let token = uuid::Uuid::new_v4().simple().to_string();
    registry.insert(
        thread_id.to_owned(),
        ActiveTurn {
            turn_id: turn_id.to_owned(),
            token: token.clone(),
            sender,
        },
    );
    Some(ActiveTurnGuard {
        thread_id: thread_id.to_owned(),
        token,
    })
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ControlDisposition {
    Accepted,
    NoActiveTurn,
    SessionUnavailable,
    TransportUnavailable,
}

pub fn steer(thread_id: &str, expected_turn_id: &str, text: &str) -> ControlDisposition {
    if text.trim().is_empty() || text.len() > 1024 * 1024 {
        return ControlDisposition::TransportUnavailable;
    }
    let Some(turn) = active()
        .lock()
        .ok()
        .and_then(|registry| registry.get(thread_id).cloned())
    else {
        return ControlDisposition::SessionUnavailable;
    };
    if turn.turn_id != expected_turn_id {
        return ControlDisposition::NoActiveTurn;
    }
    let (acknowledged, receiver) = mpsc::sync_channel(1);
    if turn
        .sender
        .try_send(ControlRequest {
            kind: ControlRequestKind::Steer(text.to_owned()),
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

pub fn interrupt(thread_id: &str) -> ControlDisposition {
    let Some(turn) = active()
        .lock()
        .ok()
        .and_then(|registry| registry.get(thread_id).cloned())
    else {
        return ControlDisposition::SessionUnavailable;
    };
    let (acknowledged, receiver) = mpsc::sync_channel(1);
    if turn
        .sender
        .try_send(ControlRequest {
            kind: ControlRequestKind::Interrupt,
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

impl ControlRequest {
    pub(super) fn into_protocol(
        self,
        thread_id: &str,
        turn_id: &str,
    ) -> (String, Value, mpsc::SyncSender<bool>) {
        let (request_id, message) = match self.kind {
            ControlRequestKind::Steer(text) => {
                crate::parser::steer_request(thread_id, turn_id, &text)
            }
            ControlRequestKind::Interrupt => crate::parser::interrupt_request(thread_id, turn_id),
        };
        (request_id, message, self.acknowledged)
    }
}

fn valid_id(value: &str) -> bool {
    !value.is_empty() && value.len() <= 512 && !value.chars().any(char::is_control)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exact_turn_binding_accepts_and_mismatch_rejects() {
        let (sender, receiver) = mpsc::sync_channel(2);
        let _guard = bind("thread-1", "turn-1", sender).unwrap();
        assert_eq!(
            steer("thread-1", "turn-other", "guidance"),
            ControlDisposition::NoActiveTurn
        );
        let worker = std::thread::spawn(move || {
            let request = receiver.recv().unwrap();
            let (request_id, message, acknowledged) = request.into_protocol("thread-1", "turn-1");
            assert_eq!(message["id"], request_id);
            assert_eq!(message["method"], "turn/steer");
            acknowledged.send(true).unwrap();
        });
        assert_eq!(
            steer("thread-1", "turn-1", "guidance"),
            ControlDisposition::Accepted
        );
        worker.join().unwrap();
    }

    #[test]
    fn active_interrupt_targets_the_exact_bound_turn() {
        let (sender, receiver) = mpsc::sync_channel(1);
        let _guard = bind("thread-interrupt", "turn-interrupt", sender).unwrap();
        let worker = std::thread::spawn(move || {
            let request = receiver.recv().unwrap();
            let (_, message, acknowledged) =
                request.into_protocol("thread-interrupt", "turn-interrupt");
            assert_eq!(message["method"], "turn/interrupt");
            assert_eq!(message["params"]["threadId"], "thread-interrupt");
            assert_eq!(message["params"]["turnId"], "turn-interrupt");
            acknowledged.send(true).unwrap();
        });
        assert_eq!(interrupt("thread-interrupt"), ControlDisposition::Accepted);
        worker.join().unwrap();
    }
}
