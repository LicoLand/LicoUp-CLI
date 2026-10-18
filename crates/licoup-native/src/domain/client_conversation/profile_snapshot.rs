//! Production authority-backed Membership Profile snapshots.
//!
//! The projection, hard filters and stable ranking live with the workflow
//! runtime that reads them (`licoup_workflow_runtime::ports`). This module
//! keeps the one implementation that reads this host's own owners, and
//! re-exports the moved vocabulary at its established path so the conversation
//! callers keep one name.

pub use licoup_workflow_runtime::ports::{
    CandidateFilters, PriceFacts, ProfileSnapshotAuthority, SharedSnapshotAuthority, TargetFacts,
    project_profile_snapshot, project_profile_snapshots, rank_candidates,
};

use std::sync::{Arc, Mutex};

/// Production authority backed by the existing named owners. Every read is
/// projected to allowlisted facts; raw paths and runtime values never leave
/// this boundary.
pub fn production_snapshot_authority() -> SharedSnapshotAuthority {
    Arc::new(Mutex::new(Box::new(ProductionSnapshotAuthority)))
}

struct ProductionSnapshotAuthority;

impl ProfileSnapshotAuthority for ProductionSnapshotAuthority {
    fn target_facts(&mut self, agent_id: &str) -> Option<TargetFacts> {
        let inspected = crate::domain::targets::inspect_target_read_only(
            &crate::target_port::agent_target_port(),
            agent_id,
        )
        .ok()?;
        let target = inspected.get("target")?;
        let status = target
            .get("status")
            .and_then(serde_json::Value::as_str)
            .filter(|value| !value.is_empty())
            .map(str::to_owned);
        let model = target
            .get("model")
            .and_then(serde_json::Value::as_str)
            .filter(|value| !value.is_empty())
            .map(str::to_owned)
            .or_else(|| {
                target
                    .pointer("/modelCatalog/defaultModel")
                    .and_then(serde_json::Value::as_str)
                    .filter(|value| !value.is_empty())
                    .map(str::to_owned)
            });
        let environment = target
            .get("location")
            .and_then(serde_json::Value::as_str)
            .filter(|value| !value.is_empty())
            .map(str::to_owned);
        let mut capabilities = ["conversationDriver", "conversationReadiness"]
            .into_iter()
            .filter_map(|pointer| {
                target
                    .pointer(&format!("/adapterCapabilities/{pointer}"))
                    .and_then(serde_json::Value::as_str)
                    .filter(|value| *value == "supported" || *value == "ready")
                    .map(|value| format!("{pointer}:{value}"))
            })
            .collect::<Vec<_>>();
        // Every capability fact is projected by its own owner; the channel
        // facts are the ones this list carried before the projection moved, and
        // the driver-inventory and readiness facts are published under their
        // own names above.
        capabilities.extend(
            crate::platform::runtime_adapters::native_capabilities_for_agent(agent_id)
                .into_iter()
                .filter(|fact| {
                    fact.source == licoup_agent_drivers::runtime_adapters::registry::SOURCE_AGENT_CLI_PRESENCE
                        || fact.source
                            == licoup_agent_drivers::runtime_adapters::registry::SOURCE_AGENT_DESKTOP_PRESENCE
                })
                .map(|fact| fact.name.to_owned()),
        );
        capabilities.sort();
        capabilities.dedup();
        let readiness = target
            .pointer("/adapterCapabilities/conversationReadiness")
            .and_then(serde_json::Value::as_str)
            .filter(|value| !value.is_empty())
            .map(str::to_owned);
        let reliability_class = target
            .pointer("/adapterCapabilities/conversationConsecutivePasses")
            .and_then(serde_json::Value::as_u64)
            .filter(|passes| *passes > 0)
            .map(|_| "verified".to_owned());
        Some(TargetFacts {
            status,
            model,
            environment,
            capabilities,
            readiness,
            reliability_class,
            latency_class: None,
        })
    }

    fn model_price_usd_per_million_tokens(&mut self, model: &str) -> Option<PriceFacts> {
        crate::domain::provider_model_pricing::model_price(model).map(|price| PriceFacts {
            input: price.input,
            output: price.output,
        })
    }

    fn coding_score(&mut self, agent_id: &str, model: &str) -> Option<i64> {
        crate::domain::agent_intelligence_catalog::merged_agent_model_score(agent_id, model)
            .or_else(|| {
                crate::domain::agent_intelligence_catalog::agent_model_max_intelligence(
                    agent_id, model,
                )
            })
    }

    fn skill_names(&mut self, agent_id: &str) -> Vec<String> {
        crate::domain::skill_hub::skill_list(&serde_json::json!({ "agent": agent_id }))
            .ok()
            .and_then(|value| {
                value
                    .get("skills")
                    .and_then(serde_json::Value::as_array)
                    .cloned()
            })
            .map(|skills| {
                skills
                    .iter()
                    .filter_map(|skill| skill.get("skillId"))
                    .filter_map(serde_json::Value::as_str)
                    .map(str::to_owned)
                    .collect()
            })
            .unwrap_or_default()
    }
}
