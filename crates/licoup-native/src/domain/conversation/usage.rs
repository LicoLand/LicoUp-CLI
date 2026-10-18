//! Bounded normalization of explicit token-usage records from local agent
//! history. The implementation lives in `licoup-statistics`; this shim keeps
//! the conversation-internal path callers already name.

pub(crate) use licoup_statistics::normalize::{
    UsageFields, collect_token_usage, extract_token_usage, token_count_value,
};
