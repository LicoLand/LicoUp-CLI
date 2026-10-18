//! Bound types for client resource policy. Not an execution engine.

/// Fixed numeric bounds for one resource class family.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ClientResourceBounds {
    pub history_page_size: usize,
    pub parser_buffer_bytes: usize,
    pub search_result_limit: usize,
    pub archive_worker_count: usize,
    pub spool_queue_bytes: usize,
    pub spool_queue_events: usize,
    pub log_segment_bytes: usize,
    pub state_quota_bytes: usize,
    pub maintenance_batch_size: usize,
}

/// Versioned resource policy. Callers cannot enlarge parallelism or create
/// unbounded collections through this type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ClientResourcePolicy {
    bounds: ClientResourceBounds,
}

impl ClientResourcePolicy {
    pub const VERSION: &'static str = "licoup.client-resource-policy.v1";

    pub fn standard() -> Self {
        Self {
            bounds: ClientResourceBounds {
                history_page_size: 50,
                parser_buffer_bytes: 1_048_576,
                search_result_limit: 50,
                archive_worker_count: 1,
                spool_queue_bytes: 4_194_304,
                spool_queue_events: 1_024,
                log_segment_bytes: 1_048_576,
                state_quota_bytes: 67_108_864,
                maintenance_batch_size: 128,
            },
        }
    }

    pub fn bounds(&self) -> ClientResourceBounds {
        self.bounds
    }

    pub fn allows_unbounded_collections(&self) -> bool {
        false
    }
}

impl Default for ClientResourcePolicy {
    fn default() -> Self {
        Self::standard()
    }
}
