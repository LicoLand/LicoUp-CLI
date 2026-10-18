use std::sync::mpsc::SyncSender;

#[derive(Debug)]
pub(super) enum ControlRequest {
    Cancel {
        session_id: String,
        acknowledged: SyncSender<bool>,
    },
    Steer {
        session_id: String,
        text: String,
        acknowledged: SyncSender<bool>,
    },
    Cleanup {
        acknowledged: SyncSender<bool>,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ControlDisposition {
    Accepted,
    NoActiveTurn,
    SessionUnavailable,
    TransportUnavailable,
}
