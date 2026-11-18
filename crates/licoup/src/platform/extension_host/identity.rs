//! Host incarnation: what makes a handle *this* host's handle.
//!
//! Every host-issued value — an invocation binding, a hook ticket, a prepared
//! extension — looks the same in a fresh host: the first instance is
//! `instance-1`, the first invocation is `invocation-1`, the generation is 1 and
//! the epoch is 1. Those facts are deliberately plain, so they cannot be the
//! thing a caller is trusted with: two hosts in one process hand out
//! indistinguishable values, and after a restart the counters begin again.
//!
//! Each host therefore mints a [`HostIncarnation`]: a random token, held behind
//! an `Arc`, with no public constructor and no serialization. Equality is
//! `Arc::ptr_eq`, so the only way to hold an equal incarnation is to have been
//! given one by the host that minted it. Every handle carries the incarnation of
//! the host that issued it, and every host checks that identity *first* — before
//! looking up an invocation id, a ticket id or an instance — so a handle from
//! another process, another run, or a second host in this process is refused
//! instead of being resolved against a coincidentally equal counter.
//!
//! The random token is not a credential and carries no user data; the display id
//! exists only for diagnostics and presentation arguments, and is never the
//! thing a check compares.

use std::sync::Arc;
use uuid::Uuid;

/// The private half of an incarnation: never exposed, never compared by value
/// outside this module, never serialized.
#[derive(Debug)]
struct IncarnationToken {
    value: Uuid,
}

/// The identity of one host run.
#[derive(Clone)]
pub struct HostIncarnation {
    display_id: u64,
    token: Arc<IncarnationToken>,
}

impl HostIncarnation {
    /// Mint a new, unforgeable incarnation.
    ///
    /// The random value is what makes a handle from a previous run
    /// distinguishable from one this run would have issued; the display id is
    /// only a label.
    pub(crate) fn mint(display_id: u64) -> Self {
        Self {
            display_id,
            token: Arc::new(IncarnationToken {
                value: Uuid::new_v4(),
            }),
        }
    }

    /// The label this incarnation was minted with, for diagnostics.
    ///
    /// Two different incarnations may share a display id (a counter that
    /// restarts); identity never rests on this value.
    pub fn display_id(&self) -> u64 {
        self.display_id
    }

    /// Whether this is the same host run as `other`.
    ///
    /// Clones share the allocation, which is the common case and the cheap
    /// one; a token that was minted separately compares by its random value,
    /// which is what makes the check independent of any counter.
    pub(crate) fn same_as(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.token, &other.token) || self.token.value == other.token.value
    }
}

impl PartialEq for HostIncarnation {
    fn eq(&self, other: &Self) -> bool {
        self.same_as(other)
    }
}

impl Eq for HostIncarnation {}

impl std::fmt::Debug for HostIncarnation {
    /// Prints the label only: the token is not diagnostics, and a log line is
    /// not a place to leak a value that identifies a run.
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "HostIncarnation({})", self.display_id)
    }
}
