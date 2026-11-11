//! Stable composition root for session merge, projection, paging, and ordering.

mod codex_lineage;
mod composition;
mod dedupe_paging;
mod delegated_merge;
mod model_names;
mod session_index;
mod stable_order;

pub(crate) use composition::finalize_history_sessions;
pub(crate) use dedupe_paging::{
    collapse_sessions_by_native_identity, dedupe_history_sessions, paged_history_sessions,
};
pub(super) use model_names::collect_history_model_names;
pub(crate) use session_index::apply_codex_session_index_titles;
pub(crate) use stable_order::sort_sessions_by_updated_at;

#[cfg(test)]
mod tests;
