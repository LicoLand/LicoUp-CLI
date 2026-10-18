//! Single ClientResourcePolicy for history, search, archive, spool, logs,
//! state quota, and maintenance. Callers cannot enlarge parallelism.

mod history;
mod policy;
mod search;

pub use history::{HistoryPage, HistoryPageSelector, IdentitySlot};
pub use policy::{
    CapacityFailure, ClientResourcePolicy, Reservation, ResourceBound, ResourceClass,
};
pub use search::{LocalSearchAuthority, SearchCursor, SearchError, SearchNamespace, SearchPage};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn policy_rejects_caller_raised_parallelism() {
        let policy = ClientResourcePolicy::default_bounded();
        assert!(
            policy
                .admit_workers(ResourceClass::ArchiveWorker, policy.archive_workers() + 1)
                .is_err()
        );
        assert!(
            policy
                .admit_workers(ResourceClass::ArchiveWorker, policy.archive_workers())
                .is_ok()
        );
    }
}
