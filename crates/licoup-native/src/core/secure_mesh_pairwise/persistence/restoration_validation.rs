use std::collections::HashSet;

use super::super::{key_ratchet::SecureMeshPairwiseSession, support::decode_secret_32};
use super::{
    public_snapshot::PersistedSkippedMessageKeyPublic, store_model::SecureMeshPairwiseDurableRecord,
};

pub(super) fn replay_window_preserved(
    previous_ids: &[String],
    current_ids: &[String],
    received_advance: u64,
) -> bool {
    let received_advance = usize::try_from(received_advance).unwrap_or(usize::MAX);
    let retained_count = previous_ids.len().saturating_sub(received_advance);
    if retained_count == 0 {
        return true;
    }
    // Both replay collections are bounded by the pairwise replay-window limit.
    let current_ids = current_ids
        .iter()
        .map(String::as_str)
        .collect::<HashSet<_>>();
    previous_ids[previous_ids.len() - retained_count..]
        .iter()
        .all(|id| current_ids.contains(id.as_str()))
}

pub(super) fn skipped_keys_not_reintroduced(
    previous_skipped: &[PersistedSkippedMessageKeyPublic],
    session: &SecureMeshPairwiseSession,
    previous: &SecureMeshPairwiseDurableRecord,
) -> bool {
    if session.dh_epoch > previous.dh_epoch
        || session.receiving_chain_index > previous.received_count
    {
        return true;
    }
    // The set remains bounded by the skipped-key retention limit and avoids
    // repeatedly decoding and rescanning persisted sender keys.
    let previous_keys = previous_skipped
        .iter()
        .filter_map(|previous| {
            decode_secret_32(&previous.sender_ratchet_public_key)
                .ok()
                .map(|sender_key| (previous.dh_epoch, previous.chain_index, sender_key))
        })
        .collect::<HashSet<_>>();
    session.skipped_keys.iter().all(|skipped| {
        previous_keys.contains(&(
            skipped.dh_epoch,
            skipped.chain_index,
            skipped.sender_ratchet_public_key,
        ))
    })
}
