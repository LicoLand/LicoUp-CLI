//! Closed, typed private IPC for an independent Agent runtime.
//! This is not a GUI command transport and not a Protocol Line.

use std::collections::VecDeque;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AgentIpcError {
    Closed,
    Capacity,
    KindRejected,
}

impl AgentIpcError {
    pub const fn code(self) -> &'static str {
        match self {
            Self::Closed => "agent_ipc_closed",
            Self::Capacity => "agent_ipc_capacity",
            Self::KindRejected => "agent_ipc_kind_rejected",
        }
    }
}

/// Closed message set. No dynamic map, command string, or UI bridge payload.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum AgentIpcMessage {
    BindSession {
        session_token: u64,
    },
    SubmitTurn {
        turn_id: u64,
        byte_len: u32,
    },
    Interrupt {
        turn_id: u64,
    },
    Progress {
        turn_id: u64,
        cursor: u64,
    },
    Complete {
        turn_id: u64,
    },
    Fail {
        turn_id: u64,
        code: AgentIpcFailureCode,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AgentIpcFailureCode {
    Unavailable,
    Cancelled,
    Capacity,
}

pub struct AgentPrivateIpc {
    inbound: VecDeque<AgentIpcMessage>,
    outbound: VecDeque<AgentIpcMessage>,
    capacity: usize,
    closed: bool,
}

impl AgentPrivateIpc {
    pub fn bounded(capacity: usize) -> Self {
        Self {
            inbound: VecDeque::new(),
            outbound: VecDeque::new(),
            capacity: capacity.max(1),
            closed: false,
        }
    }

    pub fn send_to_agent(&mut self, message: AgentIpcMessage) -> Result<(), AgentIpcError> {
        Self::push(self.closed, self.capacity, &mut self.inbound, message)
    }

    pub fn recv_from_host(&mut self) -> Result<Option<AgentIpcMessage>, AgentIpcError> {
        Self::pop(self.closed, &mut self.inbound)
    }

    pub fn send_to_host(&mut self, message: AgentIpcMessage) -> Result<(), AgentIpcError> {
        Self::push(self.closed, self.capacity, &mut self.outbound, message)
    }

    pub fn recv_from_agent(&mut self) -> Result<Option<AgentIpcMessage>, AgentIpcError> {
        Self::pop(self.closed, &mut self.outbound)
    }

    pub fn close(&mut self) {
        self.closed = true;
        self.inbound.clear();
        self.outbound.clear();
    }

    pub fn is_closed(&self) -> bool {
        self.closed
    }

    fn push(
        closed: bool,
        capacity: usize,
        queue: &mut VecDeque<AgentIpcMessage>,
        message: AgentIpcMessage,
    ) -> Result<(), AgentIpcError> {
        if closed {
            return Err(AgentIpcError::Closed);
        }
        if queue.len() >= capacity {
            return Err(AgentIpcError::Capacity);
        }
        queue.push_back(message);
        Ok(())
    }

    fn pop(
        closed: bool,
        queue: &mut VecDeque<AgentIpcMessage>,
    ) -> Result<Option<AgentIpcMessage>, AgentIpcError> {
        if closed {
            return Err(AgentIpcError::Closed);
        }
        Ok(queue.pop_front())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn closed_mailbox_rejects_further_traffic() {
        let mut ipc = AgentPrivateIpc::bounded(2);
        ipc.send_to_agent(AgentIpcMessage::BindSession { session_token: 7 })
            .expect("bind");
        assert!(matches!(
            ipc.recv_from_host().expect("recv"),
            Some(AgentIpcMessage::BindSession { session_token: 7 })
        ));
        ipc.close();
        assert_eq!(
            ipc.send_to_agent(AgentIpcMessage::Interrupt { turn_id: 1 })
                .expect_err("closed")
                .code(),
            "agent_ipc_closed"
        );
    }

    #[test]
    fn capacity_is_typed_and_does_not_drop_messages() {
        let mut ipc = AgentPrivateIpc::bounded(1);
        ipc.send_to_host(AgentIpcMessage::Complete { turn_id: 1 })
            .expect("first");
        assert_eq!(
            ipc.send_to_host(AgentIpcMessage::Complete { turn_id: 2 })
                .expect_err("full")
                .code(),
            "agent_ipc_capacity"
        );
        assert!(matches!(
            ipc.recv_from_agent().expect("drain"),
            Some(AgentIpcMessage::Complete { turn_id: 1 })
        ));
    }
}
