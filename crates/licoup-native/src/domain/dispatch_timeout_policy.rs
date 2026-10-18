//! Writable per-agent (and optional task) dispatch timeout policy.
//!
//! `timeoutMs` 0 or a missing value means "use this policy". The default
//! policy has no execution deadline (0 ms). Observation windows are not kill
//! timers; turns run unbounded unless an explicit deadline or policy is configured.
//! Finite non-zero values stay inside the 1s–30min clamp.

use crate::platform::client_state::ClientStateStore;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use std::collections::BTreeMap;

pub const MIN_DISPATCH_TIMEOUT_MS: u64 = 1_000;
pub const MAX_DISPATCH_TIMEOUT_MS: u64 = 30 * 60 * 1_000;
pub const SETTINGS_KEY: &str = "dispatchTimeoutPolicy";
const SETTINGS_COLLECTION: &str = "settings";

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AgentTimeoutPolicy {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub tasks: BTreeMap<String, u64>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DispatchTimeoutPolicy {
    pub default_timeout_ms: u64,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub agents: BTreeMap<String, AgentTimeoutPolicy>,
}

impl Default for DispatchTimeoutPolicy {
    fn default() -> Self {
        Self {
            default_timeout_ms: 0,
            agents: BTreeMap::new(),
        }
    }
}

impl DispatchTimeoutPolicy {
    pub fn resolve(&self, agent_id: &str, task: Option<&str>) -> u64 {
        let agent = self.agents.get(agent_id);
        let suggested = task
            .and_then(|task| agent.and_then(|policy| policy.tasks.get(task)).copied())
            .or_else(|| agent.and_then(|policy| policy.timeout_ms))
            .unwrap_or(self.default_timeout_ms);
        clamp_timeout_ms(suggested)
    }
}

pub fn clamp_timeout_ms(value: u64) -> u64 {
    if value == 0 {
        0
    } else {
        value.clamp(MIN_DISPATCH_TIMEOUT_MS, MAX_DISPATCH_TIMEOUT_MS)
    }
}

pub fn timeout_unbounded(params: &Value) -> bool {
    ["timeoutUnbounded", "unboundedTimeout"]
        .iter()
        .any(|key| params.get(*key).and_then(Value::as_bool) == Some(true))
}

/// Resolve the turn deadline. `timeoutMs` 0 or omission uses the writable
/// policy unless the caller set the unbounded override.
pub fn resolve_dispatch_timeout(params: &Value) -> Result<u64, ()> {
    if timeout_unbounded(params) {
        return Ok(0);
    }
    let requested = match params.get("timeoutMs") {
        None | Some(Value::Null) => None,
        Some(value) => Some(value.as_u64().ok_or(())?),
    };
    match requested {
        Some(0) | None => {
            let agent = text_param(params, &["agentId", "agent"]).unwrap_or_default();
            let task = text_param(params, &["taskType", "task"]);
            Ok(policy_from_params(params)
                .unwrap_or_else(load_or_default)
                .resolve(&agent, task.as_deref()))
        }
        Some(value) if (MIN_DISPATCH_TIMEOUT_MS..=MAX_DISPATCH_TIMEOUT_MS).contains(&value) => {
            Ok(value)
        }
        Some(_) => Err(()),
    }
}

pub fn load_or_default() -> DispatchTimeoutPolicy {
    load().unwrap_or_default()
}

pub fn load() -> Option<DispatchTimeoutPolicy> {
    let store = ClientStateStore::portable_read_only().ok()?;
    load_from_store(&store)
}

pub fn load_from_store(store: &ClientStateStore) -> Option<DispatchTimeoutPolicy> {
    let settings = store.read_collection_read_only(SETTINGS_COLLECTION).ok()?;
    policy_from_value(settings.get(SETTINGS_KEY)?)
}

pub fn store(policy: &DispatchTimeoutPolicy) -> Result<DispatchTimeoutPolicy, String> {
    let store =
        ClientStateStore::portable().map_err(|error| redacted_store_error(&error.to_string()))?;
    store_in(store, policy)
}

pub fn store_in(
    store: ClientStateStore,
    policy: &DispatchTimeoutPolicy,
) -> Result<DispatchTimeoutPolicy, String> {
    let normalized = DispatchTimeoutPolicy {
        default_timeout_ms: clamp_timeout_ms(policy.default_timeout_ms),
        agents: policy
            .agents
            .iter()
            .map(|(agent, entry)| {
                (
                    agent.clone(),
                    AgentTimeoutPolicy {
                        timeout_ms: entry.timeout_ms.map(clamp_timeout_ms),
                        tasks: entry
                            .tasks
                            .iter()
                            .map(|(task, value)| (task.clone(), clamp_timeout_ms(*value)))
                            .collect(),
                    },
                )
            })
            .collect(),
    };
    let mut settings = store
        .read_collection(SETTINGS_COLLECTION)
        .map_err(|error| redacted_store_error(&error.to_string()))?
        .as_object()
        .cloned()
        .unwrap_or_else(Map::new);
    settings.insert(
        SETTINGS_KEY.to_owned(),
        serde_json::to_value(&normalized)
            .map_err(|error| redacted_store_error(&error.to_string()))?,
    );
    store
        .write_collection(SETTINGS_COLLECTION, Value::Object(settings))
        .map_err(|error| redacted_store_error(&error.to_string()))?;
    Ok(normalized)
}

pub fn policy_to_json(policy: &DispatchTimeoutPolicy) -> Value {
    json!({
        "defaultTimeoutMs": policy.default_timeout_ms,
        "agents": policy.agents,
    })
}

pub fn policy_envelope(policy: &DispatchTimeoutPolicy) -> Value {
    json!({
        "policy": policy_to_json(policy),
        "minTimeoutMs": MIN_DISPATCH_TIMEOUT_MS,
        "maxTimeoutMs": MAX_DISPATCH_TIMEOUT_MS,
    })
}

fn policy_from_params(params: &Value) -> Option<DispatchTimeoutPolicy> {
    policy_from_value(params.get("dispatchTimeoutPolicy")?)
}

fn policy_from_value(value: &Value) -> Option<DispatchTimeoutPolicy> {
    serde_json::from_value(value.clone()).ok()
}

fn text_param(params: &Value, keys: &[&str]) -> Option<String> {
    keys.iter()
        .find_map(|key| params.get(*key).and_then(Value::as_str))
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
}

fn redacted_store_error(error: &str) -> String {
    if error.contains("unsupported") || error.contains("mismatch") {
        "timeout_policy_unavailable".to_owned()
    } else {
        "timeout_policy_unavailable".to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use licoup_foundation::platform::paths::set_portable_data_dir_override;

    #[test]
    fn zero_and_missing_timeouts_use_the_writable_policy() {
        let policy = json!({
            "defaultTimeoutMs": 120_000,
            "agents": {
                "claude-code": {
                    "timeoutMs": 600_000,
                    "tasks": { "frontend": 90_000 }
                }
            }
        });
        assert_eq!(
            resolve_dispatch_timeout(&json!({
                "agent": "claude-code",
                "dispatchTimeoutPolicy": policy,
            }))
            .unwrap(),
            600_000
        );
        assert_eq!(
            resolve_dispatch_timeout(&json!({
                "agentId": "claude-code",
                "taskType": "frontend",
                "timeoutMs": 0,
                "dispatchTimeoutPolicy": policy,
            }))
            .unwrap(),
            90_000
        );
        assert_eq!(
            resolve_dispatch_timeout(&json!({
                "agent": "cursor",
                "dispatchTimeoutPolicy": policy,
            }))
            .unwrap(),
            120_000
        );
        assert_eq!(
            resolve_dispatch_timeout(&json!({
                "timeoutMs": 5_000,
                "dispatchTimeoutPolicy": policy,
            }))
            .unwrap(),
            5_000
        );
        assert_eq!(
            resolve_dispatch_timeout(&json!({
                "timeoutMs": 0,
                "timeoutUnbounded": true,
                "dispatchTimeoutPolicy": policy,
            }))
            .unwrap(),
            0
        );
        assert!(
            resolve_dispatch_timeout(&json!({
                "timeoutMs": 500,
                "dispatchTimeoutPolicy": policy,
            }))
            .is_err()
        );
    }

    #[test]
    fn stored_policy_can_be_rewritten_without_a_binary_change() {
        let root =
            std::env::temp_dir().join(format!("licoup-timeout-policy-{}", uuid::Uuid::new_v4()));
        let previous = set_portable_data_dir_override(Some(root.clone()));
        let stored = store(&DispatchTimeoutPolicy {
            default_timeout_ms: 45_000,
            agents: BTreeMap::from([(
                "cursor".to_owned(),
                AgentTimeoutPolicy {
                    timeout_ms: Some(20_000),
                    tasks: BTreeMap::from([("retrieval".to_owned(), 12_000)]),
                },
            )]),
        })
        .unwrap();
        assert_eq!(stored.default_timeout_ms, 45_000);
        assert_eq!(
            resolve_dispatch_timeout(&json!({"agent": "cursor", "task": "retrieval"})).unwrap(),
            12_000
        );
        store(&DispatchTimeoutPolicy {
            default_timeout_ms: 30_000,
            agents: BTreeMap::new(),
        })
        .unwrap();
        assert_eq!(
            resolve_dispatch_timeout(&json!({"agent": "cursor"})).unwrap(),
            30_000
        );
        set_portable_data_dir_override(previous);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn default_timeout_is_unbounded_zero() {
        assert_eq!(DispatchTimeoutPolicy::default().default_timeout_ms, 0);
        assert_eq!(clamp_timeout_ms(0), 0);
        assert_eq!(clamp_timeout_ms(500), MIN_DISPATCH_TIMEOUT_MS);
        assert_eq!(clamp_timeout_ms(50_000_000), MAX_DISPATCH_TIMEOUT_MS);

        // Without policy in params or store, resolution returns 0 (unbounded)
        let resolved = resolve_dispatch_timeout(&json!({
            "agent": "codex"
        }))
        .unwrap();
        assert_eq!(resolved, 0);

        let resolved_zero = resolve_dispatch_timeout(&json!({
            "agent": "codex",
            "timeoutMs": 0
        }))
        .unwrap();
        assert_eq!(resolved_zero, 0);
    }
}
