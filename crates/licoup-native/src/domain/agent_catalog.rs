//! Single Agent membership list. Hub, target scan, usage, skills, and
//! Flywheel pickers all read this catalog. It is seeded from the runtime
//! adapter registry plus local discovery definitions — never a second
//! hardcoded id list.

use crate::domain::cli_registration;
use crate::domain::native_roles;
use crate::domain::targets::{normalize_target, target_def, target_defs};
use crate::platform::runtime_adapters::{self, PACKAGED_RUNTIME_ADAPTER_IDS};
use serde_json::{Value, json};
use std::collections::BTreeSet;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AgentCatalogEntry {
    pub id: String,
    pub label: String,
    pub kind: String,
    pub summary: String,
    pub has_adapter: bool,
}

pub fn entries() -> Vec<AgentCatalogEntry> {
    membership(std::iter::empty::<&str>())
}

pub fn membership(extra_ids: impl IntoIterator<Item = impl AsRef<str>>) -> Vec<AgentCatalogEntry> {
    let mut seen = BTreeSet::new();
    let mut catalog = Vec::new();
    for id in PACKAGED_RUNTIME_ADAPTER_IDS {
        push_entry(&mut catalog, &mut seen, id);
    }
    for def in target_defs() {
        push_entry(&mut catalog, &mut seen, def.id);
    }
    for extra in extra_ids {
        let id = normalize_target(extra.as_ref());
        if !id.is_empty() {
            push_entry(&mut catalog, &mut seen, &id);
        }
    }
    for registration in cli_registration::registrations() {
        push_entry(&mut catalog, &mut seen, &registration.id);
    }
    catalog
}

/// Agents the Hub may show: dedicated runtime adapters or registered CLI lanes.
pub fn supported_membership(
    extra_ids: impl IntoIterator<Item = impl AsRef<str>>,
) -> Vec<AgentCatalogEntry> {
    membership(extra_ids)
        .into_iter()
        .filter(|entry| entry.has_adapter)
        .collect()
}

pub fn ids() -> Vec<String> {
    entries().into_iter().map(|entry| entry.id).collect()
}

pub fn contains(id: &str) -> bool {
    let normalized = normalize_target(id);
    if normalized.is_empty() {
        return false;
    }
    runtime_adapters::has_runtime_lane(&normalized) || target_def(&normalized).is_ok()
}

pub fn extra_ids_from_facts(facts: &[impl AsRef<str>]) -> Vec<String> {
    facts
        .iter()
        .map(|id| normalize_target(id.as_ref()))
        .filter(|id| !id.is_empty() && !contains(id))
        .collect()
}

pub fn extra_ids_from_params(params: &Value) -> Vec<String> {
    let Some(items) = params.get("discoveryCandidates").and_then(Value::as_array) else {
        return Vec::new();
    };
    extra_ids_from_facts(
        &items
            .iter()
            .filter_map(|item| {
                item.get("target")
                    .or_else(|| item.get("agentId"))
                    .and_then(Value::as_str)
            })
            .collect::<Vec<_>>(),
    )
}

pub fn projection() -> Value {
    let agents = entries()
        .into_iter()
        .map(|entry| {
            json!({
                "id": entry.id,
                "label": entry.label,
                "kind": entry.kind,
                "summary": entry.summary,
                "hasAdapter": entry.has_adapter
            })
        })
        .collect::<Vec<_>>();
    let profiles = native_roles::list()
        .into_iter()
        .map(|role| role.public_projection())
        .collect::<Vec<_>>();
    json!({
        "ok": true,
        "schemaVersion": "lico.agent-catalog.v1",
        "agents": agents,
        "profiles": profiles
    })
}

fn push_entry(catalog: &mut Vec<AgentCatalogEntry>, seen: &mut BTreeSet<String>, id: &str) {
    let normalized = normalize_target(id);
    if normalized.is_empty() || !seen.insert(normalized.clone()) {
        return;
    }
    let adapter = runtime_adapters::adapter_for_agent_public(&normalized);
    let registration = cli_registration::registration_for(&normalized);
    let def = target_def(&normalized).ok();
    catalog.push(AgentCatalogEntry {
        label: adapter
            .map(|item| item.label().to_string())
            .or_else(|| registration.as_ref().map(|item| item.label.clone()))
            .or_else(|| def.as_ref().map(|item| item.label.to_string()))
            .unwrap_or_else(|| normalized.clone()),
        kind: def
            .as_ref()
            .map(|item| item.kind.to_string())
            .unwrap_or_else(|| "cli".to_string()),
        summary: def
            .as_ref()
            .map(|item| item.config_hint.to_string())
            .unwrap_or_else(|| format!("{normalized} local discovery target")),
        has_adapter: adapter.is_some() || registration.is_some(),
        id: normalized,
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn membership_includes_adapters_and_discovery_defs() {
        let catalog = entries();
        let ids = catalog
            .iter()
            .map(|entry| entry.id.as_str())
            .collect::<Vec<_>>();
        for adapter in PACKAGED_RUNTIME_ADAPTER_IDS {
            assert!(ids.contains(adapter), "missing adapter {adapter}");
        }
        assert!(ids.contains(&"kimi-code"));
        assert!(ids.contains(&"grok"));
        assert!(ids.contains(&"command-code"));
        assert!(
            catalog
                .iter()
                .any(|entry| entry.id == "kimi-code" && entry.has_adapter)
        );
        assert!(
            catalog
                .iter()
                .any(|entry| entry.id == "grok" && entry.has_adapter)
        );
        assert!(
            catalog
                .iter()
                .any(|entry| entry.id == "command-code" && entry.has_adapter)
        );
        assert!(runtime_adapters::has_runtime_lane("grok"));
        assert!(runtime_adapters::has_runtime_lane("command-code"));
        assert!(runtime_adapters::adapter_for_agent_public("grok").is_none());
        assert!(runtime_adapters::adapter_for_agent_public("codex").is_some());
        assert!(runtime_adapters::has_runtime_lane("codex"));
        assert_eq!(ids.iter().collect::<BTreeSet<_>>().len(), ids.len());
    }

    #[test]
    fn extras_append_unknown_discovered_ids() {
        let catalog = membership(["custom-local-agent"]);
        assert!(catalog.iter().any(|entry| entry.id == "custom-local-agent"));
        assert!(contains("kimi-code"));
        assert!(contains("grok"));
        assert!(!contains(""));
        let supported = supported_membership(["custom-local-agent", "workbuddy"]);
        assert!(supported.iter().all(|entry| entry.has_adapter));
        assert!(supported.iter().any(|entry| entry.id == "kimi-code"));
        assert!(
            !supported
                .iter()
                .any(|entry| entry.id == "custom-local-agent")
        );
        assert!(!supported.iter().any(|entry| entry.id == "workbuddy"));
        assert!(!supported.iter().any(|entry| entry.id == "code"));
        assert_eq!(
            supported
                .iter()
                .find(|entry| entry.id == "kimi-code")
                .unwrap()
                .label,
            "Kimi Code CLI"
        );
        assert_eq!(
            supported
                .iter()
                .find(|entry| entry.id == "codex")
                .unwrap()
                .label,
            "Codex CLI"
        );
        assert_eq!(
            supported
                .iter()
                .find(|entry| entry.id == "cursor")
                .unwrap()
                .label,
            "Cursor CLI"
        );
    }

    #[test]
    fn projection_includes_allowlisted_profiles_without_prompts() {
        let _guard = native_roles::install_test_roles(vec![native_roles::NativeRole {
            id: "native-role:opencode/reviewer".to_owned(),
            host_agent_id: "opencode".to_owned(),
            slug: "reviewer".to_owned(),
            name: "Reviewer".to_owned(),
            instructions: "You review diffs in secret.".to_owned(),
            preferred_model: Some("anthropic/claude-sonnet-4".to_owned()),
            preferred_reasoning_effort: Some("high".to_owned()),
        }]);
        let projected = projection();
        assert_eq!(projected["schemaVersion"], "lico.agent-catalog.v1");
        let profiles = projected["profiles"].as_array().unwrap();
        assert!(
            profiles
                .iter()
                .any(|profile| profile["id"] == "native-role:opencode/reviewer"
                    && profile["hostAgentId"] == "opencode"
                    && profile["hasInstructions"] == true
                    && profile.get("instructions").is_none()
                    && profile.get("prompt").is_none())
        );
        let encoded = serde_json::to_string(&projected).unwrap();
        assert!(!encoded.contains("You review diffs in secret."));
    }
}
