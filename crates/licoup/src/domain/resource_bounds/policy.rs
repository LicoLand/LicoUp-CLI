//! Versioned resource bounds. Defaults use keyset cursors, fixed worker/byte
//! queues, pre-write reservations, and stable capacity failures.

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ResourceClass {
    HistoryPage,
    ParserBuffer,
    SearchResult,
    ArchiveWorker,
    Spool,
    LogSegment,
    StateQuota,
    MaintenanceBatch,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ResourceBound {
    pub class: ResourceClass,
    pub limit: u32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CapacityFailure {
    QueueSaturated { class: ResourceClass },
    ReservationDenied { class: ResourceClass },
    QuotaExceeded { class: ResourceClass },
    CursorInvalid,
}

impl CapacityFailure {
    pub const fn code(self) -> &'static str {
        match self {
            Self::QueueSaturated { .. } => "resource_queue_saturated",
            Self::ReservationDenied { .. } => "resource_reservation_denied",
            Self::QuotaExceeded { .. } => "resource_quota_exceeded",
            Self::CursorInvalid => "resource_cursor_invalid",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Reservation {
    class: ResourceClass,
    bytes: u32,
}

impl Reservation {
    pub const fn class(self) -> ResourceClass {
        self.class
    }

    pub const fn bytes(self) -> u32 {
        self.bytes
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ClientResourcePolicy {
    history_first_page: u32,
    history_second_page: u32,
    history_later_page: u32,
    parser_buffer_bytes: u32,
    search_page_size: u32,
    archive_workers: u32,
    archive_queue_bytes: u32,
    spool_queue_bytes: u32,
    log_segment_bytes: u32,
    state_quota_bytes: u32,
    maintenance_batch: u32,
}

impl ClientResourcePolicy {
    pub fn default_bounded() -> Self {
        let bounds = licoup_client_state::ClientResourcePolicy::standard().bounds();
        Self {
            history_first_page: bounds.history_page_size as u32,
            history_second_page: bounds.history_page_size as u32,
            history_later_page: 100,
            parser_buffer_bytes: bounds.parser_buffer_bytes as u32,
            search_page_size: bounds.search_result_limit as u32,
            archive_workers: bounds.archive_worker_count as u32,
            archive_queue_bytes: bounds.spool_queue_bytes as u32,
            spool_queue_bytes: bounds.spool_queue_bytes as u32,
            log_segment_bytes: bounds.log_segment_bytes as u32,
            state_quota_bytes: bounds.state_quota_bytes as u32,
            maintenance_batch: bounds.maintenance_batch_size as u32,
        }
    }

    pub const fn history_page_size(self, load_index: u32) -> u32 {
        match load_index {
            0 => self.history_first_page,
            1 => self.history_second_page,
            _ => self.history_later_page,
        }
    }

    pub const fn parser_buffer_bytes(self) -> u32 {
        self.parser_buffer_bytes
    }

    pub const fn search_page_size(self) -> u32 {
        self.search_page_size
    }

    pub const fn archive_workers(self) -> u32 {
        self.archive_workers
    }

    pub const fn maintenance_batch(self) -> u32 {
        self.maintenance_batch
    }

    pub const fn bound(self, class: ResourceClass) -> ResourceBound {
        let limit = match class {
            ResourceClass::HistoryPage => self.history_later_page,
            ResourceClass::ParserBuffer => self.parser_buffer_bytes,
            ResourceClass::SearchResult => self.search_page_size,
            ResourceClass::ArchiveWorker => self.archive_workers,
            ResourceClass::Spool => self.spool_queue_bytes,
            ResourceClass::LogSegment => self.log_segment_bytes,
            ResourceClass::StateQuota => self.state_quota_bytes,
            ResourceClass::MaintenanceBatch => self.maintenance_batch,
        };
        ResourceBound { class, limit }
    }

    pub fn admit_workers(
        self,
        class: ResourceClass,
        requested: u32,
    ) -> Result<u32, CapacityFailure> {
        let limit = self.bound(class).limit;
        if requested == 0 || requested > limit {
            return Err(CapacityFailure::QueueSaturated { class });
        }
        Ok(requested)
    }

    pub fn reserve(self, class: ResourceClass, bytes: u32) -> Result<Reservation, CapacityFailure> {
        let limit = self.bound(class).limit;
        if bytes == 0 || bytes > limit {
            return Err(CapacityFailure::ReservationDenied { class });
        }
        Ok(Reservation { class, bytes })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn history_pages_follow_fifty_then_fifty_then_hundred() {
        let policy = ClientResourcePolicy::default_bounded();
        assert_eq!(policy.history_page_size(0), 50);
        assert_eq!(policy.history_page_size(1), 50);
        assert_eq!(policy.history_page_size(2), 100);
        assert_eq!(policy.history_page_size(9), 100);
    }

    #[test]
    fn reservation_is_required_before_archive_write() {
        let policy = ClientResourcePolicy::default_bounded();
        let reserved = policy.reserve(ResourceClass::Spool, 1024).expect("reserve");
        assert_eq!(reserved.bytes(), 1024);
        assert!(
            policy
                .reserve(
                    ResourceClass::Spool,
                    policy.bound(ResourceClass::Spool).limit + 1
                )
                .is_err()
        );
    }
}
