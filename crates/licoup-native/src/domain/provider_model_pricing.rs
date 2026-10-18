//! The native paths the model pricing facts' callers already use.
//!
//! The pricing catalog and its readers moved to `licoup-model-catalog`, which
//! owns the billing facts and the planning inputs built on them. Every former
//! path stays reachable through this re-export for the consumers that still
//! live in `licoup-native`: the qualification owners and the conversation
//! profile snapshot.
//!
//! A missing route has no price. Absence is reported as absence and is never
//! read as a zero price.

pub use licoup_model_catalog::pricing::{
    ModelTokenPrice, RefreshSummary, agent_model_price, model_price, refresh_official_sources,
};
