//! Closed configuration plans for connecting supported coding agents to the
//! loopback LicoUp Gateway. Plans never contain upstream provider credentials.
//!
//! OpenCode and Pi embed the live catalog returned by the running Gateway.
//! Codex and Claude Code point at the loopback Gateway without embedding a
//! model catalog.

use anyhow::{Result, anyhow, ensure};
use serde::Serialize;
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

pub const GATEWAY_AGENT_CONFIG_SCHEMA: &str = "licoup.llm-gateway-agent-config.v1";
pub const LOCAL_CLIENT_TOKEN_PLACEHOLDER: &str = "__LICOUP_GATEWAY_CLIENT_TOKEN__";

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum GatewayAgentTarget {
    Codex,
    ClaudeCode,
    OpenCode,
    Pi,
}

impl GatewayAgentTarget {
    pub fn parse(value: &str) -> Result<Self> {
        match value {
            "codex" => Ok(Self::Codex),
            "claude-code" | "claude" => Ok(Self::ClaudeCode),
            "opencode" | "open-code" | "open_code" => Ok(Self::OpenCode),
            "pi" | "pi-agent" | "pi_agent" | "pi-coding-agent" | "pi_coding_agent" => Ok(Self::Pi),
            _ => Err(anyhow!("llm_gateway_agent_adapter_unsupported")),
        }
    }
    pub fn agent_id(self) -> &'static str {
        match self {
            Self::Codex => "codex",
            Self::ClaudeCode => "claude-code",
            Self::OpenCode => "opencode",
            Self::Pi => "pi",
        }
    }

    pub fn embeds_model_catalog(self) -> bool {
        matches!(self, Self::OpenCode | Self::Pi)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GatewayAgentModel {
    pub id: String,
    pub name: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GatewayAgentConfigPlan {
    pub schema_version: &'static str,
    pub agent_id: &'static str,
    pub destination: PathBuf,
    pub content: String,
    pub contains_upstream_secret: bool,
    pub confirmation_digest: String,
}

pub fn plan_agent_config(
    target: GatewayAgentTarget,
    agent_config_root: &Path,
    gateway_port: u16,
    local_token_helper: &Path,
    models: &[GatewayAgentModel],
) -> Result<GatewayAgentConfigPlan> {
    ensure!(gateway_port > 0, "llm_gateway_port_invalid");
    ensure!(
        agent_config_root.is_absolute() && local_token_helper.is_absolute(),
        "llm_gateway_agent_config_path_invalid"
    );
    let base_url = format!("http://127.0.0.1:{gateway_port}");
    let (destination, content) = match target {
        GatewayAgentTarget::Codex => {
            let helper = toml_string(local_token_helper.to_string_lossy().as_ref())?;
            let base = toml_string(&base_url)?;
            (
                agent_config_root.join("licoup-gateway.config.toml"),
                format!(
                    "model_provider = \"licoup-gateway\"\n\n[model_providers.licoup-gateway]\nname = \"LicoUp Gateway\"\nbase_url = {base}\nwire_api = \"responses\"\n[model_providers.licoup-gateway.auth]\ncommand = {helper}\nargs = [\"gateway\", \"client-token\", \"--agent\", \"codex\"]\ntimeout_ms = 5000\nrefresh_interval_ms = 300000\n"
                ),
            )
        }
        GatewayAgentTarget::ClaudeCode => {
            let destination = agent_config_root.join("settings.licoup-gateway.json");
            let document = serde_json::json!({
                "env": { "ANTHROPIC_BASE_URL": base_url },
                "apiKeyHelper": format!("{} gateway client-token --agent claude-code", local_token_helper.display())
            });
            (
                destination,
                format!("{}\n", serde_json::to_string_pretty(&document)?),
            )
        }
        GatewayAgentTarget::OpenCode => {
            // Sidecar only: never rewrite the user's opencode.json / opencode.jsonc.
            // Load via OPENCODE_CONFIG so OpenCode merges it with the existing global config.
            let destination = agent_config_root.join("opencode.licoup-gateway.json");
            let chat_base = format!("{base_url}/v1");
            let mut model_map = Map::new();
            for model in models {
                model_map.insert(model.id.to_owned(), json!({ "name": model.name }));
            }
            let document = json!({
                "$schema": "https://opencode.ai/config.json",
                "provider": {
                    "licoup-gateway": {
                        "npm": "@ai-sdk/openai-compatible",
                        "name": "LicoUp Gateway",
                        "options": {
                            "baseURL": chat_base,
                            "apiKey": LOCAL_CLIENT_TOKEN_PLACEHOLDER
                        },
                        "models": Value::Object(model_map)
                    }
                }
            });
            let _ = local_token_helper;
            (
                destination,
                format!("{}\n", serde_json::to_string_pretty(&document)?),
            )
        }
        GatewayAgentTarget::Pi => {
            // Sidecar only: never rewrite the user's models.json wholesale.
            // Merge providers.licoup-gateway into ~/.pi/agent/models.json via the helper script.
            let destination = agent_config_root.join("models.licoup-gateway.json");
            let chat_base = format!("{base_url}/v1");
            let model_list: Vec<Value> = models
                .iter()
                .map(|model| {
                    json!({
                        "id": model.id,
                        "name": model.name
                    })
                })
                .collect();
            let document = json!({
                "providers": {
                    "licoup-gateway": {
                        "baseUrl": chat_base,
                        "api": "openai-completions",
                        "apiKey": LOCAL_CLIENT_TOKEN_PLACEHOLDER,
                        "authHeader": true,
                        "compat": {
                            "supportsDeveloperRole": false,
                            "supportsReasoningEffort": false
                        },
                        "models": model_list
                    }
                }
            });
            let _ = local_token_helper;
            (
                destination,
                format!("{}\n", serde_json::to_string_pretty(&document)?),
            )
        }
    };
    let digest = Sha256::digest(
        [
            target.agent_id().as_bytes(),
            destination.to_string_lossy().as_bytes(),
            content.as_bytes(),
        ]
        .concat(),
    );
    let confirmation_digest = digest.iter().map(|byte| format!("{byte:02x}")).collect();
    Ok(GatewayAgentConfigPlan {
        schema_version: GATEWAY_AGENT_CONFIG_SCHEMA,
        agent_id: target.agent_id(),
        destination,
        content,
        contains_upstream_secret: false,
        confirmation_digest,
    })
}

fn toml_string(value: &str) -> Result<String> {
    ensure!(
        !value.chars().any(char::is_control),
        "llm_gateway_agent_config_path_invalid"
    );
    Ok(format!(
        "\"{}\"",
        value.replace('\\', "\\\\").replace('"', "\\\"")
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn live_models() -> Vec<GatewayAgentModel> {
        [
            ("kimi:kimi-k3", "Kimi K3"),
            ("deepseek:deepseek-v4-flash", "DeepSeek V4 Flash"),
            ("deepseek:deepseek-v4-pro", "DeepSeek V4 Pro"),
            ("kilo:kilo-auto/frontier", "Kilo Auto Frontier"),
            ("kilo:kilo-auto/balanced", "Kilo Auto Balanced"),
            ("kilo:kilo-auto/free", "Kilo Auto Free"),
            ("kilo:anthropic/claude-opus-5", "Claude Opus 5"),
            ("kilo:anthropic/claude-sonnet-5", "Claude Sonnet 5"),
        ]
        .into_iter()
        .map(|(id, name)| GatewayAgentModel {
            id: id.into(),
            name: name.into(),
        })
        .collect()
    }

    #[test]
    fn codex_and_claude_plans_use_loopback_and_never_embed_upstream_keys() {
        for target in [
            GatewayAgentTarget::Codex,
            GatewayAgentTarget::ClaudeCode,
            GatewayAgentTarget::OpenCode,
            GatewayAgentTarget::Pi,
        ] {
            let plan = plan_agent_config(
                target,
                Path::new("/synthetic/config"),
                15722,
                Path::new("/synthetic/lico-native"),
                &live_models(),
            )
            .unwrap();
            assert!(plan.content.contains("127.0.0.1:15722"));
            assert!(!plan.contains_upstream_secret);
            assert!(!plan.content.contains("credentialProvider"));
            assert!(!plan.content.contains("sk-"));
        }
    }

    #[test]
    fn opencode_plan_writes_sidecar_openai_compatible_provider() {
        let plan = plan_agent_config(
            GatewayAgentTarget::OpenCode,
            Path::new("/synthetic/opencode"),
            15722,
            Path::new("/synthetic/lico-native"),
            &live_models(),
        )
        .unwrap();
        assert_eq!(
            plan.destination,
            PathBuf::from("/synthetic/opencode/opencode.licoup-gateway.json")
        );
        assert!(plan.content.contains("@ai-sdk/openai-compatible"));
        assert!(plan.content.contains("http://127.0.0.1:15722/v1"));
        assert!(plan.content.contains(LOCAL_CLIENT_TOKEN_PLACEHOLDER));
        assert!(!plan.content.contains("licoup-local"));
        assert!(plan.content.contains("kimi:kimi-k3"));
        assert!(plan.content.contains("deepseek:deepseek-v4-pro"));
        assert!(plan.content.contains("kilo:kilo-auto/frontier"));
        assert!(plan.content.contains("kilo:anthropic/claude-opus-5"));
        assert!(!plan.content.contains("anthropic/claude-sonnet-4.6"));
        assert!(!plan.content.contains("kimi-k2-0905-preview"));
        assert!(!plan.content.contains("opencode.jsonc"));
    }

    #[test]
    fn opencode_and_pi_plans_project_only_live_gateway_models() {
        let only_kilo = vec![GatewayAgentModel {
            id: "kilo:kilo-auto/free".into(),
            name: "Kilo Auto Free".into(),
        }];
        for target in [GatewayAgentTarget::OpenCode, GatewayAgentTarget::Pi] {
            let plan = plan_agent_config(
                target,
                Path::new("/synthetic/config"),
                15722,
                Path::new("/synthetic/lico-native"),
                &only_kilo,
            )
            .unwrap();
            assert!(plan.content.contains("kilo:kilo-auto/free"));
            assert!(!plan.content.contains("kimi:k3"));
            assert!(!plan.content.contains("deepseek:deepseek-v4"));
        }

        let empty = plan_agent_config(
            GatewayAgentTarget::OpenCode,
            Path::new("/synthetic/config"),
            15722,
            Path::new("/synthetic/lico-native"),
            &[],
        )
        .unwrap();
        assert!(empty.content.contains("\"models\": {}"));
        assert!(!empty.content.contains("kimi:"));
        assert!(!empty.content.contains("kilo:"));
    }

    #[test]
    fn pi_plan_writes_sidecar_openai_completions_provider() {
        let plan = plan_agent_config(
            GatewayAgentTarget::Pi,
            Path::new("/synthetic/pi/agent"),
            15722,
            Path::new("/synthetic/lico-native"),
            &live_models(),
        )
        .unwrap();
        assert_eq!(
            plan.destination,
            PathBuf::from("/synthetic/pi/agent/models.licoup-gateway.json")
        );
        assert!(plan.content.contains("openai-completions"));
        assert!(plan.content.contains("http://127.0.0.1:15722/v1"));
        assert!(plan.content.contains(LOCAL_CLIENT_TOKEN_PLACEHOLDER));
        assert!(!plan.content.contains("licoup-local"));
        assert!(plan.content.contains("kimi:kimi-k3"));
        assert!(plan.content.contains("deepseek:deepseek-v4-flash"));
        assert!(plan.content.contains("kilo:kilo-auto/balanced"));
        assert!(plan.content.contains("kilo:anthropic/claude-sonnet-5"));
        assert!(
            !plan
                .content
                .contains("\"id\": \"anthropic/claude-sonnet-4.6\"")
        );
        assert!(!plan.content.contains("settings.json"));
    }

    #[test]
    fn unknown_agents_fail_closed() {
        assert!(GatewayAgentTarget::parse("unknown").is_err());
    }
}
