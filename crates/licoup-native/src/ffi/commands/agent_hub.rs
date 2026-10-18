use super::{AdmittedCommand, CliExecution, admitted_params};
use anyhow::Result;
use serde_json::{Map, Value};

fn hub_params(
    agent_id: Option<&str>,
    operation: Option<&str>,
    confirmation: Option<&str>,
    cancel: bool,
    private: Option<&Value>,
) -> Value {
    let mut params = admitted_params(
        &[
            ("agentId", agent_id),
            ("operation", operation),
            ("confirmation", confirmation),
        ],
        &[],
        &[("cancel", cancel)],
    );
    if let Some(private) = private {
        if let (Some(base), Some(extra)) = (params.as_object_mut(), private.as_object()) {
            for (key, value) in extra {
                if key == "agentId" && base.contains_key("agentId") {
                    continue;
                }
                if key == "confirmation" && base.contains_key("confirmation") {
                    continue;
                }
                if key == "operation" && base.contains_key("operation") {
                    continue;
                }
                base.insert(key.clone(), value.clone());
            }
        }
    }
    if params.as_object().is_none() {
        params = Value::Object(Map::new());
    }
    params
}

pub(super) fn handle_catalog(command: AdmittedCommand) -> Result<CliExecution> {
    Ok(CliExecution::Json(crate::domain::agent_hub::catalog(
        &hub_params(
            command.option_text("agent-id"),
            None,
            None,
            false,
            command.option_json("stdin-json"),
        ),
    )?))
}

pub(super) fn handle_plan(command: AdmittedCommand) -> Result<CliExecution> {
    Ok(CliExecution::Json(crate::domain::agent_hub::install_plan(
        &hub_params(
            command.option_text("agent-id"),
            command.option_text("operation"),
            None,
            false,
            command.option_json("stdin-json"),
        ),
    )?))
}

pub(super) fn handle_apply(command: AdmittedCommand) -> Result<CliExecution> {
    Ok(CliExecution::Json(crate::domain::agent_hub::install_apply(
        &hub_params(
            command.option_text("agent-id"),
            command.option_text("operation"),
            command.option_text("confirmation"),
            command.option_flag("cancel"),
            command.option_json("stdin-json"),
        ),
    )?))
}

#[cfg(test)]
mod tests {
    use super::super::{CliExecution, execute_cli};
    use serde_json::{Value, json};
    use std::env;
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_dir(name: &str) -> std::path::PathBuf {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default();
        let dir = env::temp_dir().join(format!(
            "lico-agent-hub-ffi-{}-{}-{}",
            name,
            now.as_secs(),
            now.subsec_nanos()
        ));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn snapshot_params(name: &str) -> Value {
        json!({
            "portableDir": temp_dir(name).to_string_lossy(),
            "platformCapabilities": {
                "os": "macos",
                "architecture": "aarch64",
                "managers": ["homebrew", "npm"],
                "scanGeneration": 4
            },
            "discoveryCandidates": []
        })
    }

    fn json_of(args: Vec<String>) -> Value {
        match execute_cli(args).expect("hub command must execute") {
            CliExecution::Json(value) => value,
            other => {
                assert!(false, "hub command must return json, got {other:?}");
                serde_json::Value::Null
            }
        }
    }

    #[test]
    fn catalog_projects_supported_cards_from_injected_snapshot() {
        let value = json_of(vec![
            "agent-hub".into(),
            "catalog".into(),
            "--stdin-json".into(),
            snapshot_params("catalog").to_string(),
        ]);
        assert_eq!(value["ok"], true);
        let cards = value["cards"].as_array().expect("cards");
        let ids = cards
            .iter()
            .map(|card| card["id"].as_str().unwrap())
            .collect::<Vec<_>>();
        for entry in crate::domain::agent_catalog::entries() {
            if entry.has_adapter {
                assert!(ids.contains(&entry.id.as_str()), "missing {}", entry.id);
            } else {
                assert!(
                    !ids.contains(&entry.id.as_str()),
                    "unsupported {}",
                    entry.id
                );
            }
        }
        assert!(ids.contains(&"kimi-code"));
        assert!(ids.contains(&"grok"));
        assert!(ids.contains(&"command-code"));
        assert!(!ids.contains(&"code"));
        assert!(!ids.contains(&"workbuddy"));
        let antigravity = cards
            .iter()
            .find(|card| card["id"] == "antigravity")
            .unwrap();
        assert_eq!(antigravity["adaptation"], "partial");
        let deepseek = cards
            .iter()
            .find(|card| card["id"] == "deepseek-harness")
            .unwrap();
        assert_eq!(deepseek["adaptation"], "pending-evaluation");
        assert_eq!(value["hostScope"], "desktop");
        let codex = cards.iter().find(|card| card["id"] == "codex").unwrap();
        assert_eq!(codex["homepage"], "https://developers.openai.com/codex");
        assert_eq!(codex["channelKind"], "");
    }

    #[test]
    fn catalog_with_agent_id_projects_one_card() {
        let mut params = snapshot_params("one-card");
        params["discoveryCandidates"] = json!([
            {
                "target": "codex",
                "present": true,
                "status": "detected"
            }
        ]);
        let value = json_of(vec![
            "agent-hub".into(),
            "catalog".into(),
            "--agent-id".into(),
            "codex".into(),
            "--stdin-json".into(),
            params.to_string(),
        ]);
        assert_eq!(value["ok"], true);
        let cards = value["cards"].as_array().expect("cards");
        assert_eq!(cards.len(), 1);
        assert_eq!(cards[0]["id"], "codex");
        assert_eq!(cards[0]["present"], true);
    }

    #[test]
    fn plan_selects_channel_from_injected_capabilities() {
        let value = json_of(vec![
            "agent-hub".into(),
            "plan".into(),
            "--agent-id".into(),
            "codex".into(),
            "--operation".into(),
            "install".into(),
            "--stdin-json".into(),
            snapshot_params("plan").to_string(),
        ]);
        assert_eq!(value["ok"], true);
        assert_eq!(value["status"], "planned");
        assert_eq!(value["selectedChannel"]["kind"], "homebrew");
        assert!(
            value["confirmation"]
                .as_str()
                .is_some_and(|token| token.starts_with("agent-hub:"))
        );
    }
}
