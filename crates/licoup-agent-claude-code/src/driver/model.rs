use crate::protocol::{EffectiveSettings, ProtocolFailure};
use serde_json::Value;
use serde_json::json;
use std::sync::atomic::{AtomicU8, Ordering};
#[cfg(test)]
use std::sync::{Condvar, Mutex};
use std::time::Duration;

/// Official Claude Code streaming-input lane. Prompt and process-local
/// conversation identity never use the command line.
/// The runtime protocol this Agent's CLI lane reports, named by the package.
pub const RUNTIME_PROTOCOL: &str = crate::protocol::RUNTIME_PROTOCOL;
pub(super) const PROCESS_POLL_INTERVAL: Duration = Duration::from_millis(50);

#[repr(u8)]
enum TransportState {
    Live = 0,
    Closing = 1,
    Closed = 2,
}

#[derive(Debug)]
pub(crate) struct TransportLifecycle {
    state: AtomicU8,
    #[cfg(test)]
    changed: Mutex<()>,
    #[cfg(test)]
    notification: Condvar,
}

impl Default for TransportLifecycle {
    fn default() -> Self {
        Self {
            state: AtomicU8::new(TransportState::Live as u8),
            #[cfg(test)]
            changed: Mutex::new(()),
            #[cfg(test)]
            notification: Condvar::new(),
        }
    }
}

impl TransportLifecycle {
    pub(crate) fn is_live(&self) -> bool {
        self.state.load(Ordering::Acquire) == TransportState::Live as u8
    }

    #[cfg(test)]
    pub(crate) fn is_closing(&self) -> bool {
        self.state.load(Ordering::Acquire) == TransportState::Closing as u8
    }

    #[cfg(test)]
    pub(crate) fn is_closed(&self) -> bool {
        self.state.load(Ordering::Acquire) == TransportState::Closed as u8
    }

    pub(crate) fn begin_closing(&self) -> bool {
        let claimed = self
            .state
            .compare_exchange(
                TransportState::Live as u8,
                TransportState::Closing as u8,
                Ordering::AcqRel,
                Ordering::Acquire,
            )
            .is_ok();
        if claimed {
            #[cfg(test)]
            self.notification.notify_all();
        }
        claimed
    }

    pub(crate) fn mark_closed(&self) -> bool {
        let closed = self
            .state
            .compare_exchange(
                TransportState::Closing as u8,
                TransportState::Closed as u8,
                Ordering::AcqRel,
                Ordering::Acquire,
            )
            .is_ok();
        if closed {
            #[cfg(test)]
            self.notification.notify_all();
        }
        closed
    }

    #[cfg(test)]
    pub(crate) fn wait_until_closing(&self, timeout: Duration) -> bool {
        if !self.is_live() {
            return true;
        }
        let Ok(guard) = self.changed.lock() else {
            return false;
        };
        let _ = self
            .notification
            .wait_timeout_while(guard, timeout, |_| self.is_live());
        !self.is_live()
    }

    #[cfg(test)]
    pub(crate) fn wait_until_closed(&self, timeout: Duration) -> bool {
        if self.is_closed() {
            return true;
        }
        let Ok(guard) = self.changed.lock() else {
            return false;
        };
        let _ = self
            .notification
            .wait_timeout_while(guard, timeout, |_| !self.is_closed());
        self.is_closed()
    }
}

#[derive(Clone, Debug)]
struct TranscriptTurn {
    turn_id: String,
    prompt: String,
    events: Vec<Value>,
    output: String,
}

#[derive(Debug)]
pub(crate) struct CompleteTranscript {
    turns: Vec<TranscriptTurn>,
    byte_count: usize,
}

impl CompleteTranscript {
    pub(crate) fn new() -> Self {
        Self {
            turns: Vec::new(),
            byte_count: 0,
        }
    }

    pub(crate) fn record_success(
        &mut self,
        turn_id: &str,
        prompt: &str,
        events: Vec<Value>,
        output: &str,
    ) {
        let output_bytes = output.len();
        if turn_id.trim().is_empty() || output.is_empty() {
            return;
        }
        self.turns.push(TranscriptTurn {
            turn_id: turn_id.to_string(),
            prompt: prompt.to_string(),
            events,
            output: output.to_string(),
        });
        self.byte_count = self.byte_count.saturating_add(output_bytes);
    }

    pub(crate) fn project_backward_page(
        &self,
        before: Option<usize>,
        limit: usize,
    ) -> (Vec<Value>, Option<usize>) {
        let end = before.unwrap_or(self.turns.len()).min(self.turns.len());
        let start = end.saturating_sub(limit);
        let turns = self.turns[start..end]
            .iter()
            .map(|turn| {
                json!({
                    "turnId": turn.turn_id,
                    "prompt": turn.prompt,
                    "events": turn.events,
                    "output": turn.output
                })
            })
            .collect();
        (turns, (start > 0).then_some(start))
    }

    pub(crate) fn turn_count(&self) -> usize {
        self.turns.len()
    }

    pub(crate) fn byte_count(&self) -> usize {
        self.byte_count
    }

    pub(crate) fn clear(&mut self) {
        self.turns.clear();
        self.byte_count = 0;
    }
}

#[derive(Debug)]
pub struct RunResult {
    pub ok: bool,
    pub output: String,
    pub transitions: Vec<licoup_agent_adapter_sdk::Transition>,
    pub error: Option<ProtocolFailure>,
    pub session_id: String,
    pub thread_id: String,
    pub turn_id: String,
    pub turn_status: String,
    pub effective: EffectiveSettings,
    pub status_code: Option<i32>,
    pub stdout_truncated: bool,
    pub stderr_truncated: bool,
    pub started_at: String,
}

impl RunResult {
    pub(super) fn failed(
        failure: ProtocolFailure,
        started_at: String,
        stdout_truncated: bool,
        stderr_truncated: bool,
    ) -> Self {
        let session_id = failure.session_id.clone().unwrap_or_default();
        let transitions = crate::protocol::parser::failure_transitions(
            failure.code,
            failure.stage,
            failure.message,
        );
        Self {
            ok: false,
            output: String::new(),
            transitions,
            thread_id: failure
                .thread_id
                .clone()
                .unwrap_or_else(|| session_id.clone()),
            session_id,
            turn_id: failure.turn_id.clone().unwrap_or_default(),
            turn_status: failure.turn_status.clone().unwrap_or_default(),
            effective: EffectiveSettings::default(),
            error: Some(failure),
            status_code: None,
            stdout_truncated,
            stderr_truncated,
            started_at,
        }
    }
}
