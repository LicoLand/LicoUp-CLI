//! Length-safe constant-time equality for mailbox token candidates.

use subtle::ConstantTimeEq;

pub(in crate::licoarc_relay) fn constant_time_equal(left: &[u8], right: &[u8]) -> bool {
    left.len() == right.len() && bool::from(left.ct_eq(right))
}
