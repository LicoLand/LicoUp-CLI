//! Zeroizing endpoint delivery secret and channel-binding primitives.

use std::fmt;

use rand_core::{OsRng, RngCore};
use zeroize::Zeroizing;

use super::constants::{CHANNEL_BINDING_BYTES, DELIVERY_SECRET_BYTES};

pub struct SecureMeshDeliverySecret {
    bytes: Zeroizing<[u8; DELIVERY_SECRET_BYTES]>,
}

impl SecureMeshDeliverySecret {
    pub fn generate() -> Self {
        let mut bytes = Zeroizing::new([0u8; DELIVERY_SECRET_BYTES]);
        OsRng.fill_bytes(bytes.as_mut());
        Self { bytes }
    }

    pub fn from_bytes(bytes: [u8; DELIVERY_SECRET_BYTES]) -> Self {
        Self {
            bytes: Zeroizing::new(bytes),
        }
    }

    pub(in crate::licoarc_relay) fn as_bytes(&self) -> &[u8; DELIVERY_SECRET_BYTES] {
        &self.bytes
    }
}

impl fmt::Debug for SecureMeshDeliverySecret {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("SecureMeshDeliverySecret([redacted])")
    }
}

#[derive(Clone, Copy, Eq, Hash, PartialEq)]
pub struct SecureMeshRelayChannelBinding([u8; CHANNEL_BINDING_BYTES]);

impl SecureMeshRelayChannelBinding {
    pub fn from_bytes(bytes: [u8; CHANNEL_BINDING_BYTES]) -> Self {
        Self(bytes)
    }

    pub(in crate::licoarc_relay) fn as_bytes(&self) -> &[u8; CHANNEL_BINDING_BYTES] {
        &self.0
    }
}

impl fmt::Debug for SecureMeshRelayChannelBinding {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("SecureMeshRelayChannelBinding([redacted])")
    }
}
