//! Crate-private HTTP adapter for an untrusted BadTower transport station.

mod contract;
mod http_io;
mod transport;
mod wire;

pub(crate) use contract::{
    BadTowerDeletionTransportHint, BadTowerDeliveryTransportHint, BadTowerLeaseTransportHint,
};
pub(crate) use transport::BadTowerStationTransport;

#[cfg(test)]
pub(crate) use contract::{
    BadTowerStationError, BadTowerStationErrorCategory, BadTowerStationOperation,
};

#[cfg(test)]
mod tests;
