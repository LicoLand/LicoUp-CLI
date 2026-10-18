//! Bounded transport-only values exposed to LicoUp domain code.

use std::fmt;

pub(super) const HTTP_TIMEOUT_SECONDS: u64 = 30;
pub(super) const MAX_REQUEST_BYTES: usize = 1_114_112;
pub(super) const MAX_SMALL_RESPONSE_BYTES: usize = 4 * 1024;
pub(super) const MAX_ERROR_RESPONSE_BYTES: usize = 4 * 1024;
pub(super) const MAX_RECEIVE_RESPONSE_BYTES: usize = 24 * 1024 * 1024;
pub(super) const MIN_OPAQUE_ID_CHARS: usize = 16;
pub(super) const MAX_OPAQUE_ID_CHARS: usize = 128;
pub(super) const MAX_LEASE_SECONDS: u64 = 86_400;
pub(super) const MIN_RECEIVE_ENVELOPES: u16 = 1;
pub(super) const MAX_RECEIVE_ENVELOPES: u16 = 16;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum BadTowerStationOperation {
    ConfigureTransport,
    LeaseMailbox,
    SendEnvelope,
    ReceiveEnvelopes,
    DeleteEnvelope,
}

impl BadTowerStationOperation {
    pub(crate) const fn key(self) -> &'static str {
        match self {
            Self::ConfigureTransport => "configure_transport",
            Self::LeaseMailbox => "lease_mailbox",
            Self::SendEnvelope => "send_envelope",
            Self::ReceiveEnvelopes => "receive_envelopes",
            Self::DeleteEnvelope => "delete_envelope",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum BadTowerStationErrorCategory {
    InvalidEndpoint,
    InvalidInput,
    RequestEncoding,
    RequestTooLarge,
    TransportOutcomeUnknown,
    ResponseOutcomeUnknown,
    ResponseTooLarge,
    ResponseProtocol,
    StationRejectedInput,
    LeaseRequired,
    TransportConflict,
    StationCapacity,
    StationFailure,
}

impl BadTowerStationErrorCategory {
    pub(crate) const fn key(self) -> &'static str {
        match self {
            Self::InvalidEndpoint => "invalid_endpoint",
            Self::InvalidInput => "invalid_input",
            Self::RequestEncoding => "request_encoding",
            Self::RequestTooLarge => "request_too_large",
            Self::TransportOutcomeUnknown => "transport_outcome_unknown",
            Self::ResponseOutcomeUnknown => "response_outcome_unknown",
            Self::ResponseTooLarge => "response_too_large",
            Self::ResponseProtocol => "response_protocol",
            Self::StationRejectedInput => "station_rejected_input",
            Self::LeaseRequired => "lease_required",
            Self::TransportConflict => "transport_conflict",
            Self::StationCapacity => "station_capacity",
            Self::StationFailure => "station_failure",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct BadTowerStationError {
    operation: BadTowerStationOperation,
    category: BadTowerStationErrorCategory,
    retryable: bool,
}

impl BadTowerStationError {
    pub(super) const fn new(
        operation: BadTowerStationOperation,
        category: BadTowerStationErrorCategory,
        retryable: bool,
    ) -> Self {
        Self {
            operation,
            category,
            retryable,
        }
    }

    #[cfg(test)]
    pub(crate) const fn operation(self) -> BadTowerStationOperation {
        self.operation
    }

    #[cfg(test)]
    pub(crate) const fn category(self) -> BadTowerStationErrorCategory {
        self.category
    }

    #[cfg(test)]
    pub(crate) const fn retryable(self) -> bool {
        self.retryable
    }
}

impl fmt::Display for BadTowerStationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "BadTower station {} failed ({})",
            self.operation.key(),
            self.category.key()
        )
    }
}

impl std::error::Error for BadTowerStationError {}

/// A station-reported lease result is only an untrusted transport hint.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct BadTowerLeaseTransportHint {
    station_reported_leased: bool,
}

impl BadTowerLeaseTransportHint {
    pub(super) const fn reported() -> Self {
        Self {
            station_reported_leased: true,
        }
    }

    pub(crate) const fn station_reported_leased(self) -> bool {
        self.station_reported_leased
    }
}

/// Station acceptance and duplicate flags never establish endpoint receipt.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct BadTowerDeliveryTransportHint {
    station_reported_accepted: bool,
    station_reported_duplicate: bool,
}

impl BadTowerDeliveryTransportHint {
    pub(super) const fn reported(accepted: bool, duplicate: bool) -> Self {
        Self {
            station_reported_accepted: accepted,
            station_reported_duplicate: duplicate,
        }
    }

    pub(crate) const fn station_reported_accepted(self) -> bool {
        self.station_reported_accepted
    }

    pub(crate) const fn station_reported_duplicate(self) -> bool {
        self.station_reported_duplicate
    }
}

/// A station-reported deletion result is only an untrusted transport hint.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct BadTowerDeletionTransportHint {
    station_reported_acknowledged: bool,
}

impl BadTowerDeletionTransportHint {
    pub(super) const fn reported(acknowledged: bool) -> Self {
        Self {
            station_reported_acknowledged: acknowledged,
        }
    }

    pub(crate) const fn station_reported_acknowledged(self) -> bool {
        self.station_reported_acknowledged
    }
}
