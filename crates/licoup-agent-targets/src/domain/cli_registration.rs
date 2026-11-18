//! Packaged and user-overlay CLI/PTY registrations reused by AgentCatalog.
//!
//! This is not a second agent registry. Catalog membership still comes from
//! AgentCatalog; these rows only supply a generic I/O lane for ids that have
//! no dedicated RuntimeAdapter.

use crate::domain::targets::normalize_target;
use licoup_foundation::platform::paths::portable_data_dir;
use serde::Deserialize;
use std::collections::BTreeMap;
use std::fs;
use std::sync::OnceLock;

const PACKAGED: &str = include_str!("../../resources/agent-hub/cli-registrations.toml");
const SCHEMA_VERSION: &str = "lico.agent-cli-registration.v1";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StreamMode {
    Pty,
    Stdio,
}

impl StreamMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pty => "pty",
            Self::Stdio => "stdio",
        }
    }

    fn parse(value: &str) -> Option<Self> {
        match value.trim() {
            "" | "pty" => Some(Self::Pty),
            "stdio" | "stdout" | "pipe" => Some(Self::Stdio),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CliRegistration {
    pub id: String,
    pub label: String,
    pub command: String,
    pub args: Vec<String>,
    pub stream_mode: StreamMode,
}

#[derive(Debug, Deserialize)]
struct Document {
    schema_version: String,
    #[serde(default)]
    agents: Vec<RawAgent>,
}

#[derive(Debug, Deserialize)]
struct RawAgent {
    id: String,
    #[serde(default)]
    label: String,
    command: String,
    #[serde(default)]
    args: Vec<String>,
    #[serde(default)]
    stream_mode: String,
}

static REGISTRATIONS: OnceLock<Vec<CliRegistration>> = OnceLock::new();

pub fn registrations() -> &'static [CliRegistration] {
    REGISTRATIONS.get_or_init(load_all)
}

pub fn registration_for(agent_id: &str) -> Option<CliRegistration> {
    let normalized = normalize_target(agent_id);
    if normalized.is_empty() {
        return None;
    }
    registrations()
        .iter()
        .find(|item| item.id == normalized)
        .cloned()
}

pub fn parse_document(raw: &str) -> Result<Vec<CliRegistration>, String> {
    let document: Document =
        toml::from_str(raw).map_err(|error| format!("cli_registration_invalid: {error}"))?;
    if document.schema_version != SCHEMA_VERSION {
        return Err("cli_registration_schema".to_owned());
    }
    let mut parsed = Vec::new();
    for agent in document.agents {
        let id = normalize_target(&agent.id);
        if id.is_empty() || agent.command.trim().is_empty() {
            continue;
        }
        let Some(stream_mode) = StreamMode::parse(&agent.stream_mode) else {
            continue;
        };
        let label = agent.label.trim();
        parsed.push(CliRegistration {
            label: if label.is_empty() {
                id.clone()
            } else {
                label.to_owned()
            },
            command: agent.command.trim().to_owned(),
            args: agent.args,
            stream_mode,
            id,
        });
    }
    Ok(parsed)
}

fn load_all() -> Vec<CliRegistration> {
    let mut by_id = BTreeMap::new();
    if let Ok(packaged) = parse_document(PACKAGED) {
        for registration in packaged {
            by_id.insert(registration.id.clone(), registration);
        }
    }
    if let Some(overlay) = load_user_overlay() {
        for registration in overlay {
            by_id.insert(registration.id.clone(), registration);
        }
    }
    by_id.into_values().collect()
}

fn load_user_overlay() -> Option<Vec<CliRegistration>> {
    let root = portable_data_dir().ok()?;
    let path = root
        .join("client-state")
        .join("agent-hub")
        .join("cli-registrations.toml");
    let metadata = fs::symlink_metadata(&path).ok()?;
    if !metadata.file_type().is_file() || metadata.file_type().is_symlink() {
        return None;
    }
    let raw = fs::read_to_string(&path).ok()?;
    parse_document(&raw).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn packaged_registers_grok_and_command_code() {
        let packaged = parse_document(PACKAGED).expect("packaged registrations");
        let ids = packaged
            .iter()
            .map(|item| item.id.as_str())
            .collect::<Vec<_>>();
        assert!(ids.contains(&"grok"));
        assert!(ids.contains(&"command-code"));
        let grok = packaged.iter().find(|item| item.id == "grok").unwrap();
        assert_eq!(grok.command, "grok");
        assert_eq!(grok.stream_mode, StreamMode::Pty);
        assert!(registration_for("grok").is_some());
        assert!(registration_for("command-code").is_some());
        assert!(registration_for("cmdc").is_some());
        assert!(registration_for("codex").is_none());
    }

    #[test]
    fn overlay_replaces_the_same_id_and_appends_extras() {
        let overlay = parse_document(
            r#"
schema_version = "lico.agent-cli-registration.v1"
[[agents]]
id = "grok"
label = "Grok overlay"
command = "overlay-grok"
args = ["--prompt", "{prompt}"]
stream_mode = "stdio"
[[agents]]
id = "custom-cli-agent"
command = "custom-cli"
args = []
stream_mode = "pty"
"#,
        )
        .unwrap();
        assert_eq!(overlay.len(), 2);
        let grok = overlay.iter().find(|item| item.id == "grok").unwrap();
        assert_eq!(grok.command, "overlay-grok");
        assert_eq!(grok.stream_mode, StreamMode::Stdio);
        assert_eq!(
            grok.args,
            vec!["--prompt".to_owned(), "{prompt}".to_owned()]
        );
        assert!(
            overlay
                .iter()
                .any(|item| item.id == "custom-cli-agent" && item.command == "custom-cli")
        );
    }

    #[test]
    fn rejects_unknown_schema_and_empty_command() {
        assert!(
            parse_document("schema_version = \"other\"\n[[agents]]\nid = \"x\"\ncommand = \"x\"\n")
                .is_err()
        );
        let parsed = parse_document(
            "schema_version = \"lico.agent-cli-registration.v1\"\n[[agents]]\nid = \"x\"\ncommand = \"\"\n",
        )
        .unwrap();
        assert!(parsed.is_empty());
    }
}
