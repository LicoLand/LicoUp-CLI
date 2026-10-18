//! The composition point for the Agent inventory port.
//!
//! `licoup-agent-targets` owns the declarations and declares
//! [`AgentTargetPort`] for every fact it reads from a module composed above it.
//! This module is where this host answers that port: each member names the
//! owner that actually holds the fact, so the inventory crate never refers to
//! the driver engines, the model facts, the conversation state or the local
//! model gateway, and no reference points upward.
//!
//! It sits at the crate root, above the domain and platform layers, exactly
//! like the environment-port and product-version bindings: answering the port
//! is composition, not a domain concern, and a domain module that reached into
//! `crate::platform` would be the coupling the layering exists to prevent.
//!
//! `agreement_with_the_declarations` below is the other half of that split.
//! The inventory's thirteen declarations and the engines' packaged adapters are
//! one set, and the check that they stay one-to-one needs both halves in view,
//! which is exactly what composition is. It used to live inside the inventory
//! as a direct call into the driver registry; it cannot live there any more
//! without the reference pointing upward, and it is a claim about the
//! composition rather than about either half alone.

use licoup_agent_targets::port::{AgentTargetPort, CanonicalModelFacts, RuntimeDriverFacts};

/// The port this host composes: every fact answered by the crate that owns it.
pub fn agent_target_port() -> AgentTargetPort {
    AgentTargetPort {
        probe_runtime_driver: crate::platform::runtime_adapters::probe_runtime_driver,
        runtime_driver_profile: runtime_driver_facts,
        append_activity_event: append_activity_event,
        default_local_agent_workspace:
            crate::platform::agent_workspace::default_local_agent_workspace,
        codex_app_server_model_catalog: codex_app_server_model_catalog,
        resolve_canonical_model: resolve_canonical_model,
        model_registry_revision: || {
            crate::domain::model_registry::refresh_cached_snapshot()
                .revision()
                .to_string()
        },
        attach_allowlisted_model_fields:
            crate::domain::agent_intelligence_catalog::attach_allowlisted_model_fields,
        agent_model_max_intelligence:
            crate::domain::agent_intelligence_catalog::agent_model_max_intelligence,
        conversation_model_catalog: crate::domain::conversations::model_catalog,
        extract_token_usage: |value| crate::domain::conversation::usage::extract_token_usage(value),
        autostart_status: crate::platform::llm_gateway_autostart::autostart_status,
        autostart_enable: crate::platform::llm_gateway_autostart::autostart_enable,
        autostart_disable: crate::platform::llm_gateway_autostart::autostart_disable,
        autostart_refresh: crate::platform::llm_gateway_autostart::refresh_after_data_home_recovery,
    }
}

/// The local target scan in the shape `licoup-agent-runtime` declares for it.
///
/// The relay's authorization context takes the scan as
/// `fn(&Value) -> Result<Value>` and runs it only on the branch that needs it,
/// so this is a function pointer and never a computed value: computing it
/// eagerly would probe processes, binaries and virtual machines and write the
/// discovery cache on every call.
pub fn target_scan(params: &serde_json::Value) -> anyhow::Result<serde_json::Value> {
    licoup_agent_targets::domain::targets::scan_targets_with_params(&agent_target_port(), params)
}

/// Append one activity event to the journal of the store the record was
/// written through. The journal is a client-state owner above the inventory.
fn append_activity_event(
    store: &licoup_client_state::ClientStateStore,
    event_type: &str,
    payload: serde_json::Value,
) -> anyhow::Result<serde_json::Value> {
    store.activity_log().append(event_type, payload)
}

/// Read the Codex App Server model catalog, naming its failure in this host's
/// error type so the port reports one error type to the inventory.
fn codex_app_server_model_catalog(binary: &std::path::Path) -> anyhow::Result<serde_json::Value> {
    licoup_agent_codex::app_server::driver::list_models(binary)
        .map_err(|()| anyhow::anyhow!("codex_app_server_model_catalog_unavailable"))
}

/// Project the driver registry's profile into the declaration shape the
/// inventory declares. The registry keeps the readiness hot-swap state and the
/// engine dispatch enum it reads, so it stays in the engines half.
fn runtime_driver_facts(agent_id: &str) -> Option<RuntimeDriverFacts> {
    let profile = crate::platform::runtime_adapters::runtime_driver_profile(agent_id)?;
    Some(RuntimeDriverFacts {
        driver_status: profile.driver_status,
        readiness: profile.readiness,
        protocol: profile.protocol,
        blocker: profile.blocker,
        capability_matrix: profile.capability_matrix,
        summary_codes: profile.summary_codes,
        consecutive_passes: profile.consecutive_passes,
        evidence_age_class: profile.evidence_age_class,
    })
}

/// Resolve one model name through the canonical registry snapshot.
fn resolve_canonical_model(
    raw: &str,
    provider_id: Option<&str>,
    source_agent_id: Option<&str>,
) -> Option<CanonicalModelFacts> {
    crate::domain::model_registry::refresh_cached_snapshot()
        .resolve_with_provider(raw, provider_id, source_agent_id)
        .map(|model| CanonicalModelFacts {
            id: model.id.clone(),
            lab_id: model.lab_id.clone(),
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    /// The iron rule this delivery is built on: one Agent, one declaration, one
    /// packaged adapter. The inventory's target ids and the engines' packaged
    /// adapter ids are the same set, and each id has exactly one driver profile.
    ///
    /// This assertion used to live in `domain/targets/tests/discovery.rs`, where
    /// it could read the driver registry directly because both trees were in one
    /// crate. It is a claim about the composition of the two halves, so it moved
    /// here with the boundary rather than being dropped.
    #[test]
    fn agreement_with_the_declarations() {
        let port = agent_target_port();
        let declared = licoup_agent_targets::domain::targets::target_defs()
            .into_iter()
            .map(|def| def.id)
            .collect::<BTreeSet<_>>();
        let packaged = crate::platform::runtime_adapters::PACKAGED_RUNTIME_ADAPTER_IDS
            .iter()
            .copied()
            .collect::<BTreeSet<_>>();
        let projected = declared
            .iter()
            .filter(|id| (port.runtime_driver_profile)(id).is_some())
            .copied()
            .collect::<BTreeSet<_>>();
        assert_eq!(
            projected, packaged,
            "every packaged runtime adapter must be a declared target with one driver profile"
        );
    }

    /// The projection the scan reports is the driver declaration, not a second
    /// derivation: a scan of one target carries the profile's own protocol,
    /// readiness and driver status.
    #[test]
    fn scan_projection_reports_the_driver_declaration() {
        let port = agent_target_port();
        let scan = licoup_agent_targets::domain::targets::scan_targets_with_params(
            &port,
            &serde_json::json!({ "targetIds": ["opencode"] }),
        )
        .expect("scan");
        let candidate = scan["results"][0]["candidate"].clone();
        let profile = (port.runtime_driver_profile)("opencode").expect("opencode driver");
        assert_eq!(
            candidate["adapterCapabilities"]["conversationProtocol"],
            serde_json::json!(profile.protocol)
        );
        assert_eq!(
            candidate["adapterCapabilities"]["conversationReadiness"],
            serde_json::json!(profile.readiness)
        );
    }
}
