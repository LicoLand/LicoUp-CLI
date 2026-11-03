//! Bounded normalization of explicit token-usage records from local agent
//! history. The implementation lives in `agent-statistics`; this shim keeps
//! the conversation-internal path callers already name.

pub(crate) use agent_statistics::normalize::extract_token_usage;
